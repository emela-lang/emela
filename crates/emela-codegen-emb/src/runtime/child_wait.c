/* Join and Await share a FIFO. Each waiter uses one slot of the existing frame pool. */
enum { EMELA_WAIT_JOIN, EMELA_WAIT_AWAIT, EMELA_WAIT_INTERRUPT };
struct emela_child_waiter {
  emela_frame base;
  emela_child *child;
  EmelaFiber owner;
  emela_child_waiter *previous;
  emela_child_waiter *next;
  void *result;
  void (*deliver)(emela_child *, void *);
  int linked;
  int verb;
};
_Static_assert(sizeof(emela_child_waiter) <= EMELA_FRAME_BYTES, "increase frame_bytes for Fiber.join/await");
static void emela_child_check(emela_child *child, int verb) {
  if (child == NULL) emela_scheduler_invalid();
  if (child->scope->ended) {
    if (verb == EMELA_WAIT_AWAIT)
      emela_board_defect(EMELA_DEFECT_PANIC, "fiber was awaited after its scope ended", UINT16_C(38));
    else if (verb == EMELA_WAIT_JOIN)
      emela_board_defect(EMELA_DEFECT_PANIC, "fiber was joined after its scope ended", UINT16_C(37));
    else
      emela_board_defect(EMELA_DEFECT_PANIC, "fiber was interrupted after its scope ended", UINT16_C(42));
  }
}
static void emela_child_publish(emela_child *child) {
  for (emela_child_waiter *waiter = child->head; waiter != NULL;) {
    emela_child_waiter *next = waiter->next;
    emela_fiber *fiber = emela_fiber_get(waiter->owner);
    /* Closing scopes only wake masked cleanup; abandoned tails are swept first. */
    if (child->scope->closed && !child->scope->ended && !emela_interrupt_masked(fiber)) {
      waiter = next;
      continue;
    }
    if (waiter->previous != NULL) waiter->previous->next = next;
    else child->head = next;
    if (next != NULL) next->previous = waiter->previous;
    else child->tail = waiter->previous;
    waiter->previous = waiter->next = NULL;
    waiter->linked = 0;
    if (!fiber->cancelled || emela_interrupt_masked(fiber)) {
      if (fiber->state != EMELA_FIBER_WAITING) emela_scheduler_invalid();
      emela_ready_push(fiber);
    }
    waiter = next;
  }
}
static void emela_child_wait_drop(emela_frame *base) {
  emela_child_waiter *waiter = (emela_child_waiter *)base;
  if (waiter->linked) {
    emela_child *child = waiter->child;
    if (waiter->previous != NULL) waiter->previous->next = waiter->next;
    else child->head = waiter->next;
    if (waiter->next != NULL) waiter->next->previous = waiter->previous;
    else child->tail = waiter->previous;
  }
  emela_heap_release(waiter->child);
}
static void emela_child_wait_step(emela_fiber *fiber, emela_frame *base) {
  emela_child_waiter *waiter = (emela_child_waiter *)base;
  emela_child *child = waiter->child;
  emela_child_check(child, waiter->verb);
  if (child->status == 0 || waiter->linked) emela_scheduler_invalid();
  if (waiter->verb == EMELA_WAIT_JOIN && child->status == 2)
    emela_board_defect(EMELA_DEFECT_PANIC, "fiber was interrupted", UINT16_C(21));
#ifdef EMELA_FAILURE_RUNTIME
  if (waiter->verb == EMELA_WAIT_JOIN && child->status == 3) {
    /* The scope's original failure remains the boundary result. */
    for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
      emela_fiber *context = &emela_fibers[slot];
      if (context->state != EMELA_FIBER_FREE && emela_same_fiber(context->root, fiber->root))
        context->forced = 1;
    }
    return;
  }
#endif
  /* Even a discarded result must observe termination and interruption. */
  if (waiter->deliver != NULL && waiter->result != NULL) waiter->deliver(child, waiter->result);
  emela_frame_return(fiber);
}
static inline EMELA_UNUSED void emela_child_wait(emela_fiber *fiber, emela_child *child, int verb,
                                   void *result, void (*deliver)(emela_child *, void *)) {
  emela_child_check(child, verb);
  emela_child_waiter *waiter = (emela_child_waiter *)emela_frame_push(fiber, sizeof(*waiter), emela_child_wait_step);
  waiter->base.drop = emela_child_wait_drop;
  waiter->child = child;
  waiter->owner = (EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation};
  waiter->result = result;
  waiter->deliver = deliver;
  waiter->previous = waiter->next = NULL;
  waiter->linked = 0;
  waiter->verb = verb;
  emela_heap_retain(child);
  if (child->status == 0) {
    waiter->previous = child->tail;
    if (child->tail != NULL) child->tail->next = waiter;
    else child->head = waiter;
    child->tail = waiter;
    waiter->linked = 1;
    fiber->state = EMELA_FIBER_WAITING;
    emela_resume_boundary(fiber);
  }
}
static inline EMELA_UNUSED void emela_child_interrupt(emela_fiber *fiber, emela_child *child) {
  emela_child_check(child, EMELA_WAIT_INTERRUPT);
  if (child->status != 0) return;
  for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
    emela_fiber *context = &emela_fibers[slot];
    if (context->state != EMELA_FIBER_FREE && emela_same_fiber(context->root, child->execution))
      context->cancelled = 1;
  }
  /* Self interruption is delivered after the current mask, without waiting on itself. */
  if (emela_same_fiber(fiber->root, child->execution)) return;
  emela_child_wait(fiber, child, EMELA_WAIT_INTERRUPT, NULL, NULL);
}
