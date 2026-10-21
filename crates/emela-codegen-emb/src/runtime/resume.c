/* A resume value owns this record, never a raw pointer to a recycled frame. */
struct emela_resume {
  EmelaFiber caller;
  EmelaFiber root;
  void *result;
  const char *duplicate_message;
  uint16_t duplicate_length;
  int initial;
  int used;
#ifdef EMELA_FAILURE_RUNTIME
  EmelaError *error_out;
  EmelaError error;
#endif
};
static int emela_resume_unavailable(emela_resume *resume) {
  emela_fiber *root = emela_resume_live(resume->root);
  emela_fiber *caller = emela_resume_live(resume->caller);
  return root == NULL || root->state == EMELA_FIBER_DONE || caller == NULL || caller->finishing ||
    ((root->cancelled || root->finishing || caller->cancelled
#ifdef EMELA_FAILURE_RUNTIME
      || caller->failure_stop != NULL
#endif
    ) && !emela_interrupt_masked(caller));
}
static void emela_resume_wake(emela_resume *resume) {
  if (emela_resume_unavailable(resume)) return;
  emela_fiber *caller = emela_resume_live(resume->caller);
  if (caller == NULL || caller->state != EMELA_FIBER_WAITING) emela_scheduler_invalid();
  emela_ready_push(caller);
}
static void emela_resume_boundary(emela_fiber *fiber) {
  /* Propagate a real suspension through the initial phases of enclosing operations. */
  while (fiber != NULL && fiber->perform != NULL && fiber->perform->initial) {
    emela_resume *resume = fiber->perform;
    resume->initial = 0;
    fiber = emela_resume_live(resume->caller);
    if (resume->used) emela_resume_wake(resume);
  }
}
static inline emela_fiber *emela_operation_begin(emela_fiber *caller, void *result,
                                        const char *message, uint16_t length
#ifdef EMELA_FAILURE_RUNTIME
    , EmelaError *error_out
#endif
) {
  EmelaFiber handle = emela_fiber_create();
  emela_fiber *implementation = emela_fiber_get(handle);
  implementation->root = caller->root;
  implementation->cancelled = caller->cancelled;
  emela_resume *resume = (emela_resume *)emela_heap_allocate(sizeof(emela_resume), EMELA_RESUME_KIND);
  *resume = (emela_resume){
    .caller = {(uint32_t)(caller - emela_fibers), caller->generation},
    .root = caller->root, .result = result,
    .duplicate_message = message, .duplicate_length = length, .initial = 1
  };
#ifdef EMELA_FAILURE_RUNTIME
  resume->error_out = error_out;
#endif
  implementation->perform = resume;
  caller->state = EMELA_FIBER_WAITING;
  return implementation;
}
static inline emela_resume *emela_resume_prepare(void *environment) {
  emela_resume *resume = (emela_resume *)environment;
  if (resume == NULL) emela_scheduler_invalid();
  /* Checking the root first also makes callbacks from previous invocations harmless. */
  if (emela_resume_unavailable(resume)) return NULL;
  if (resume->used)
    emela_board_defect(EMELA_DEFECT_PANIC, resume->duplicate_message, resume->duplicate_length);
  resume->used = 1;
  return resume;
}
static void emela_resume_after_step(emela_fiber *fiber) {
#ifdef EMELA_SCOPE_RUNTIME
  if (fiber->state == EMELA_FIBER_DONE && fiber->child != NULL) {
    emela_child_after_step(fiber);
    return;
  }
#endif
  if (fiber->state != EMELA_FIBER_DONE || fiber->perform == NULL) return;
  emela_resume *resume = fiber->perform;
#ifdef EMELA_FAILURE_RUNTIME
  if (resume->error.tag != 0) {
    if (resume->used) emela_operation_failed_after_resume(resume->duplicate_message, resume->duplicate_length, resume->error);
    if (!emela_resume_unavailable(resume)) {
      if (resume->error_out == NULL) emela_scheduler_invalid();
      *resume->error_out = resume->error;
      emela_error_retain(*resume->error_out);
      resume->used = 1;
      if (!resume->initial) emela_resume_wake(resume);
    }
  }
#endif
  if (resume->initial) {
    resume->initial = 0;
    if (resume->used) emela_resume_wake(resume);
    else emela_resume_boundary(emela_resume_live(resume->caller));
  }
  fiber->perform = NULL;
#ifdef EMELA_FAILURE_RUNTIME
  emela_error_clear(&resume->error);
#endif
  emela_heap_release(resume);
  emela_fiber_release((EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation});
}
static inline void emela_resume_commit(emela_resume *resume) {
  if (resume->initial) return;
  emela_fiber *caller = emela_resume_live(resume->caller);
  if (caller == NULL || caller->state != EMELA_FIBER_WAITING) emela_scheduler_invalid();
  if (emela_current_fiber == NULL || emela_same_fiber(emela_current_fiber->root, resume->root))
    emela_resume_drive(caller);
  else emela_ready_push(caller);
}
