/* Waiters live in the existing frame pool and own their Deferred until delivery or cancellation. */
_Static_assert(sizeof(emela_deferred_waiter) <= EMELA_FRAME_BYTES, "increase frame_bytes for Deferred.await");
static void emela_deferred_wait_drop(emela_frame *base) {
  emela_deferred_waiter *waiter = (emela_deferred_waiter *)base;
  if (waiter->linked) {
    emela_deferred_slot *slot = emela_deferred_slot_get(waiter->deferred);
    if (waiter->previous != NULL) waiter->previous->next = waiter->next;
    else slot->head = waiter->next;
    if (waiter->next != NULL) waiter->next->previous = waiter->previous;
    else slot->tail = waiter->previous;
    waiter->linked = 0;
  }
  emela_heap_release(waiter->deferred);
}
static void emela_deferred_wait_step(emela_fiber *fiber, emela_frame *base) {
  emela_deferred_waiter *waiter = (emela_deferred_waiter *)base;
  emela_deferred_slot *slot = emela_deferred_get_await(waiter->deferred);
  if (!slot->full || waiter->linked) emela_scheduler_invalid();
  if (waiter->deliver != NULL && waiter->result != NULL) waiter->deliver(slot->value, waiter->result);
  emela_frame_return(fiber);
}
static inline void emela_deferred_wait(emela_fiber *fiber, emela_deferred *deferred,
                                      void *result, void (*deliver)(EmelaAny, void *)) {
  emela_deferred_slot *slot = emela_deferred_get_await(deferred);
  emela_deferred_waiter *waiter = (emela_deferred_waiter *)emela_frame_push(fiber, sizeof(*waiter), emela_deferred_wait_step);
  waiter->base.drop = emela_deferred_wait_drop;
  waiter->deferred = deferred;
  waiter->owner = (EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation};
  waiter->result = result;
  waiter->deliver = deliver;
  waiter->previous = waiter->next = NULL;
  waiter->linked = 0;
  emela_heap_retain(deferred);
  if (!slot->full) {
    waiter->previous = slot->tail;
    if (slot->tail != NULL) slot->tail->next = waiter;
    else slot->head = waiter;
    slot->tail = waiter;
    waiter->linked = 1;
    fiber->state = EMELA_FIBER_WAITING;
    emela_resume_boundary(fiber);
  }
}
