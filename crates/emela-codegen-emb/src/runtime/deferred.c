static emela_deferred_slot *emela_deferred_slot_get(emela_deferred *deferred) {
  if (deferred->slot >= EMELA_DEFERRED_CAPACITY) emela_scheduler_invalid();
  emela_deferred_slot *slot = &emela_deferred_slots[deferred->slot];
  if (slot->generation != deferred->generation) emela_scheduler_invalid();
  return slot;
}
static void emela_deferred_unlink(emela_deferred *deferred) {
  if (deferred->previous != NULL) deferred->previous->next = deferred->next;
  else deferred->scope->deferreds = deferred->next;
  if (deferred->next != NULL) deferred->next->previous = deferred->previous;
  deferred->previous = deferred->next = NULL;
}
/* Detach before releasing a value: its captures may own this or other Deferreds. */
static EmelaAny emela_deferred_recycle(emela_deferred *deferred) {
  emela_deferred_slot *slot = emela_deferred_slot_get(deferred);
  if (slot->head != NULL) emela_scheduler_invalid();
  emela_deferred_unlink(deferred);
  EmelaAny value = slot->value;
  slot->value = (EmelaAny){0};
  slot->full = 0;
  slot->next = emela_deferred_free;
  emela_deferred_free = deferred->slot;
  deferred->slot = EMELA_SLOT_NONE;
  return value;
}
static void emela_deferred_wake_waiters(emela_deferred_slot *slot) {
  while (slot->head != NULL) {
    emela_deferred_waiter *waiter = slot->head;
    slot->head = waiter->next;
    if (slot->head != NULL) slot->head->previous = NULL;
    else slot->tail = NULL;
    waiter->previous = waiter->next = NULL;
    waiter->linked = 0;
    emela_fiber *fiber = emela_fiber_get(waiter->owner);
    if (!fiber->cancelled || emela_interrupt_masked(fiber)) {
      if (fiber->state != EMELA_FIBER_WAITING) emela_scheduler_invalid();
      emela_ready_push(fiber);
    }
  }
}
static void emela_deferred_scope_end(emela_scope *scope) {
  while (scope->deferreds != NULL) {
    emela_deferred *deferred = scope->deferreds;
    emela_heap_retain(deferred);
    /* Remaining operation continuations check the ended scope before reading a reused slot. */
    emela_deferred_wake_waiters(emela_deferred_slot_get(deferred));
    EmelaAny value = emela_deferred_recycle(deferred);
    emela_any_release(value);
    emela_heap_release(deferred);
  }
}
static inline EmelaAny emela_deferred_allocate(emela_scope *scope) {
  if (!emela_deferred_initialized) {
    for (uint32_t i = 0; i < EMELA_DEFERRED_CAPACITY; i++)
      emela_deferred_slots[i].next = i + 1 < EMELA_DEFERRED_CAPACITY ? i + 1 : EMELA_SLOT_NONE;
    emela_deferred_free = 0;
    emela_deferred_initialized = 1;
  }
  if (emela_deferred_free == EMELA_SLOT_NONE) emela_scheduler_exhausted();
  uint32_t index = emela_deferred_free;
  emela_deferred_slot *slot = &emela_deferred_slots[index];
  if (slot->generation == UINT32_MAX) emela_scheduler_exhausted();
  emela_deferred_free = slot->next;
  slot->generation++;
  slot->value = (EmelaAny){0};
  slot->head = slot->tail = NULL;
  slot->full = 0;
  emela_deferred *deferred = (emela_deferred *)emela_heap_allocate(sizeof(*deferred), EMELA_DEFERRED_KIND);
  *deferred = (emela_deferred){.scope = scope, .next = scope->deferreds,
    .slot = index, .generation = slot->generation};
  if (scope->deferreds != NULL) scope->deferreds->previous = deferred;
  scope->deferreds = deferred;
  emela_heap_retain(scope);
  return (EmelaAny){.payload.any_box = deferred};
}

static inline emela_deferred_slot *emela_deferred_prepare_complete(emela_deferred *deferred) {
  if (deferred == NULL) emela_scheduler_invalid();
  if (deferred->scope->ended)
    emela_board_defect(EMELA_DEFECT_PANIC, "deferred was completed after its scope ended", UINT16_C(44));
  emela_deferred_slot *slot = emela_deferred_slot_get(deferred);
  return slot->full ? NULL : slot;
}
static inline void emela_deferred_publish(emela_deferred_slot *slot) {
  emela_deferred_wake_waiters(slot);
}
static inline emela_deferred_slot *emela_deferred_get_await(emela_deferred *deferred) {
  if (deferred == NULL) emela_scheduler_invalid();
  if (deferred->scope->ended)
    emela_board_defect(EMELA_DEFECT_PANIC, "deferred was awaited after its scope ended", UINT16_C(42));
  return emela_deferred_slot_get(deferred);
}
static inline emela_deferred_slot *emela_deferred_get_poll(emela_deferred *deferred) {
  if (deferred == NULL) emela_scheduler_invalid();
  if (deferred->scope->ended)
    emela_board_defect(EMELA_DEFECT_PANIC, "deferred was polled after its scope ended", UINT16_C(41));
  return emela_deferred_slot_get(deferred);
}
