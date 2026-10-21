/* A context is reclaimed only after its operation contexts and cleanup frames have ended. */
static int emela_interrupt_masked(emela_fiber *fiber) {
  for (uint32_t depth = 0; depth < EMELA_FIBER_CAPACITY; depth++) {
    if (fiber->holds != 0) return 1;
#ifdef EMELA_RESUME_RUNTIME
    if (fiber->perform == NULL) return 0;
    fiber = emela_resume_live(fiber->perform->caller);
    if (fiber == NULL) emela_scheduler_invalid();
#else
    return 0;
#endif
  }
  emela_scheduler_invalid();
}
static int emela_should_unwind(emela_fiber *fiber) {
#ifdef EMELA_FAILURE_RUNTIME
  if (fiber->forced) return 1;
  /* Deliver an observed failure before an external interruption discards its frame. */
  if (fiber->top != NULL && fiber->top->has_failure != NULL &&
      fiber->top->has_failure(fiber, fiber->top)) return 0;
#endif
  return (fiber->cancelled
#ifdef EMELA_FAILURE_RUNTIME
    || fiber->failure_stop != NULL
#endif
  ) && !emela_interrupt_masked(fiber);
}
typedef struct {
  uint32_t cleanup;
  uint32_t next;
  uint32_t keep;
  int running;
#ifdef EMELA_FAILURE_RUNTIME
  unsigned holds;
  int forced;
#endif
} emela_protection;
static inline void emela_interrupt_hold(emela_fiber *fiber) {
  if (fiber->holds == UINT32_MAX) emela_scheduler_exhausted();
  fiber->holds++;
}
static inline void emela_interrupt_unhold(emela_fiber *fiber) {
  if (fiber->holds == 0) emela_scheduler_invalid();
  fiber->holds--;
}
static inline void emela_protection_leave(emela_fiber *fiber, emela_protection *guards,
                                         uint32_t depth, uint32_t keep, uint32_t next, uint32_t *pc) {
  if (keep > depth) emela_scheduler_invalid();
  if (keep == depth) { *pc = next; return; }
  emela_protection *guard = &guards[depth - 1];
  if (guard->running) emela_scheduler_invalid();
  guard->running = 1;
  guard->next = next;
  guard->keep = keep;
#ifdef EMELA_FAILURE_RUNTIME
  guard->holds = fiber->holds;
  guard->forced = fiber->forced;
  fiber->forced = 0;
#endif
  emela_interrupt_hold(fiber);
  *pc = guard->cleanup;
}
static inline int emela_protection_finish(emela_fiber *fiber, emela_protection *guards,
                                         uint32_t *depth, uint32_t *pc) {
  if (*depth == 0 || !guards[*depth - 1].running) emela_scheduler_invalid();
  emela_protection guard = guards[--*depth];
  emela_interrupt_unhold(fiber);
#ifdef EMELA_FAILURE_RUNTIME
  fiber->forced |= guard.forced;
#endif
  if (emela_should_unwind(fiber)) return 0;
  emela_protection_leave(fiber, guards, *depth, guard.keep, guard.next, pc);
  return 1;
}
static int emela_unwind_has_operations(EmelaFiber owner) {
#ifdef EMELA_RESUME_RUNTIME
  for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
    emela_fiber *candidate = &emela_fibers[slot];
    if (candidate->state != EMELA_FIBER_FREE && candidate->perform != NULL &&
        emela_same_fiber(candidate->perform->caller, owner)) return 1;
  }
#else
  (void)owner;
#endif
  return 0;
}
static int emela_unwind_context(emela_fiber *fiber) {
  if (fiber->state == EMELA_FIBER_READY) emela_ready_remove(fiber);
#ifdef EMELA_TASK_CAPACITY
  emela_task_abandon((EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation});
#endif
  fiber->state = EMELA_FIBER_RUNNING;
  while (fiber->top != NULL) {
#ifdef EMELA_FAILURE_RUNTIME
    if (fiber->top == fiber->failure_stop) {
      fiber->failure_stop = NULL;
      fiber->forced = 0;
      emela_ready_push(fiber);
      return 1;
    }
    if (!emela_should_unwind(fiber)) {
      emela_ready_push(fiber);
      return 1;
    }
#endif
    if (fiber->top->unwind != NULL) {
      int action = fiber->top->unwind(fiber, fiber->top);
      if (action != 0) {
        if (action == 1) emela_ready_push(fiber);
        return action != 3;
      }
    }
    emela_frame_return(fiber);
  }
  fiber->finishing = 1;
  fiber->state = EMELA_FIBER_WAITING;
  return 1;
}
static void emela_unwind_sweep(void) {
  int progress;
  do {
    progress = 0;
    for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
      emela_fiber *fiber = &emela_fibers[slot];
      if (fiber->state == EMELA_FIBER_FREE || fiber->state == EMELA_FIBER_DONE) continue;
#ifdef EMELA_RESUME_RUNTIME
      if (fiber->perform != NULL) {
        emela_fiber *root = emela_resume_live(fiber->root);
        emela_fiber *caller = emela_resume_live(fiber->perform->caller);
        if (root == NULL) emela_scheduler_invalid();
        if (caller == NULL) emela_scheduler_invalid();
        if (root->finishing || root->cancelled || caller->finishing
#ifdef EMELA_FAILURE_RUNTIME
            || caller->failure_stop != NULL
#endif
        ) fiber->cancelled = 1;
      }
#endif
      if (fiber->active != 0) continue;
      EmelaFiber owner = {slot, fiber->generation};
      if (emela_unwind_has_operations(owner)) continue;
      if (emela_should_unwind(fiber) && !fiber->finishing)
        progress |= emela_unwind_context(fiber);
      if (!fiber->finishing) continue;
      fiber->state = EMELA_FIBER_DONE;
#ifdef EMELA_RESUME_RUNTIME
      emela_resume_after_step(fiber);
#endif
      progress = 1;
    }
  } while (progress);
}
static void emela_resume_drive(emela_fiber *target) {
  emela_fiber *saved = emela_current_fiber, *work = target;
  target->state = EMELA_FIBER_RUNNING;
  for (;;) {
    if (emela_should_unwind(work)) { emela_unwind_sweep(); break; }
    work->active++;
    emela_current_fiber = work;
#ifdef EMELA_RESUME_RUNTIME
    EmelaFiber parent = work->perform == NULL ? (EmelaFiber){EMELA_SLOT_NONE, 0} : work->perform->caller;
#endif
    work->top->step(work, work->top);
    work->active--;
    emela_unwind_sweep();
    int ended = work->state == EMELA_FIBER_DONE || work->state == EMELA_FIBER_FREE || work->finishing;
    if (work == target && ended) break;
    if (ended) {
#ifdef EMELA_RESUME_RUNTIME
      work = emela_resume_live(parent);
      if (work == NULL || work->state != EMELA_FIBER_READY) break;
      emela_ready_remove(work);
      work->state = EMELA_FIBER_RUNNING;
#else
      break;
#endif
    }
    if (work->state == EMELA_FIBER_RUNNING) continue;
#ifdef EMELA_RESUME_RUNTIME
    emela_fiber *child = NULL;
    for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
      emela_fiber *candidate = &emela_fibers[slot];
      if (candidate->state == EMELA_FIBER_READY && candidate->perform != NULL &&
          candidate->perform->initial && candidate->perform->caller.slot == (uint32_t)(work - emela_fibers) &&
          candidate->perform->caller.generation == work->generation) { child = candidate; break; }
    }
    if (child == NULL) break;
    emela_ready_remove(child);
    child->state = EMELA_FIBER_RUNNING;
    work = child;
#else
    break;
#endif
  }
  emela_current_fiber = saved;
}

#ifdef EMELA_FAILURE_RUNTIME
static inline void emela_force_interrupt(emela_fiber *fiber) {
  for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
    emela_fiber *context = &emela_fibers[slot];
    if (context->state != EMELA_FIBER_FREE && emela_same_fiber(context->root, fiber->root)) {
      context->cancelled = context->forced = 1;
    }
  }
}
#endif
