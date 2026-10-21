/* Structured lifetime and failure boundaries. All work runs in the main context. */
struct emela_scope_frame {
  emela_frame base;
  emela_scope *scope;
#ifdef EMELA_FAILURE_RUNTIME
  EmelaError body_error;
  EmelaError *error_out;
#endif
};
#ifdef EMELA_FAILURE_RUNTIME
static int emela_scope_has_failure(emela_fiber *fiber, emela_frame *base) {
  (void)fiber;
  emela_scope_frame *frame = (emela_scope_frame *)base;
  return frame->body_error.tag != 0 || frame->scope->first_error.tag != 0;
}
static inline int emela_child_has_failure(emela_fiber *fiber, emela_frame *base) {
  (void)base;
  return fiber->child != NULL && fiber->child->error.tag != 0;
}
#endif
static void emela_scope_close(emela_scope *scope, EmelaFiber closer);
#ifdef EMELA_FAILURE_RUNTIME
static void emela_scope_failed(emela_scope *scope, EmelaError *error) {
  if (scope->first_error.tag != 0) {
    emela_error_add_note(&scope->first_error, *error);
    return;
  }
  emela_error_ensure_notes(error);
  scope->first_error = *error;
  emela_error_retain(*error);
  emela_fiber *owner = emela_resume_live(scope->owner);
  if (owner == NULL || scope->ended) emela_scheduler_invalid();
  emela_frame *target = &scope->frame->base;
  /* An outer failing scope must retain priority over an inner one. */
  for (emela_frame *at = owner->top; at != NULL; at = at->parent) {
    if (at == target) { owner->failure_stop = target; break; }
    if (at == owner->failure_stop) break;
  }
  emela_scope_close(scope, scope->owner);
}
#endif
static void emela_scope_finish(emela_scope *scope) {
  if (scope->ended || !scope->closed || scope->running != 0) return;
  scope->ended = 1;
#ifdef EMELA_DEFERRED_CAPACITY
  emela_deferred_scope_end(scope);
#endif
  while (scope->children != NULL) {
    emela_child *child = scope->children;
    emela_heap_retain(child);
    /* Releasing a result may destroy this child or another child in the weak list. */
    emela_child_unlink(child);
#ifdef EMELA_CHILD_WAIT_RUNTIME
    emela_child_publish(child);
#endif
    /* Ended handles keep the scope tombstone, but must not keep result cycles alive. */
    EmelaAny result = child->result;
    child->result = (EmelaAny){0};
    emela_any_release(result);
#ifdef EMELA_FAILURE_RUNTIME
    emela_error_clear(&child->error);
#endif
    emela_heap_release(child);
  }
}
static void emela_scope_close(emela_scope *scope, EmelaFiber closer) {
  if (!scope->closed) {
    scope->closed = 1;
    scope->closer = closer;
    for (emela_child *child = scope->children; child != NULL; child = child->next) {
      if (child->status != 0) continue;
      for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
        emela_fiber *fiber = &emela_fibers[slot];
        if (fiber->state != EMELA_FIBER_FREE && emela_same_fiber(fiber->root, child->execution))
          fiber->cancelled = 1;
      }
    }
  } else if (closer.slot == EMELA_SLOT_NONE) {
    scope->closer = closer;
  }
}
static void emela_child_after_step(emela_fiber *fiber) {
  if (fiber->state != EMELA_FIBER_DONE || fiber->child == NULL) return;
  emela_child *child = fiber->child;
  emela_scope *scope = child->scope;
  emela_heap_retain(scope);
  child->status = fiber->cancelled ? 2 : 1;
#ifdef EMELA_FAILURE_RUNTIME
  if (child->error.tag != 0) {
    child->status = 3;
    emela_scope_failed(scope, &child->error);
  }
#endif
#ifdef EMELA_CHILD_WAIT_RUNTIME
  /* Closing the scope settles its remaining continuations after all children are reclaimed. */
  emela_child_publish(child);
#endif
  if (scope->running == 0) emela_scheduler_invalid();
  scope->running--;
  fiber->child = NULL;
  emela_fiber_release(child->execution);
  emela_heap_release(child);
  if (scope->closed && scope->running == 0) {
    emela_fiber *closer = emela_resume_live(scope->closer);
    if (closer != NULL && (!closer->cancelled || emela_interrupt_masked(closer)
#ifdef EMELA_FAILURE_RUNTIME
        || scope->first_error.tag != 0
#endif
      ) && closer->state == EMELA_FIBER_WAITING)
      emela_ready_push(closer);
    if (scope->closer.slot == EMELA_SLOT_NONE) emela_scope_finish(scope);
  }
  emela_heap_release(scope);
}
_Static_assert(sizeof(emela_scope_frame) <= EMELA_FRAME_BYTES, "increase frame_bytes for Scope");
static void emela_scope_drop(emela_frame *base) {
  emela_scope *scope = ((emela_scope_frame *)base)->scope;
  emela_scope_close(scope, (EmelaFiber){EMELA_SLOT_NONE, 0});
  emela_scope_finish(scope);
#ifdef EMELA_FAILURE_RUNTIME
  emela_error_clear(&((emela_scope_frame *)base)->body_error);
  /* Escaping errors own a separate copy; scope-local payloads may capture child handles. */
  emela_error_clear(&scope->first_error);
#endif
  emela_heap_release(scope);
}
static void emela_scope_step(emela_fiber *fiber, emela_frame *base) {
  emela_scope_frame *frame = (emela_scope_frame *)base;
  emela_scope *scope = frame->scope;
#ifdef EMELA_FAILURE_RUNTIME
  if (frame->body_error.tag != 0) {
    emela_scope_failed(scope, &frame->body_error);
    emela_error_clear(&frame->body_error);
  }
  if (fiber->failure_stop == base) fiber->failure_stop = NULL;
#endif
  emela_scope_close(scope, (EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation});
  if (scope->running != 0) {
    fiber->state = EMELA_FIBER_WAITING;
    emela_resume_boundary(fiber);
    return;
  }
  emela_scope_finish(scope);
#ifdef EMELA_FAILURE_RUNTIME
  if (scope->first_error.tag != 0) {
    if (frame->error_out == NULL) emela_board_defect(EMELA_DEFECT_UNHANDLED_ERROR, "unhandled error", UINT16_C(15));
    *frame->error_out = scope->first_error;
    emela_error_retain(*frame->error_out);
  }
#endif
  emela_frame_return(fiber);
}
static int emela_scope_unwind(emela_fiber *fiber, emela_frame *base) {
  emela_scope *scope = ((emela_scope_frame *)base)->scope;
  int newly_closed = !scope->closed;
  emela_scope_close(scope, (EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation});
  if (scope->running != 0) {
    fiber->state = EMELA_FIBER_WAITING;
    return newly_closed ? 2 : 3;
  }
  emela_scope_finish(scope);
  return 0;
}
static inline emela_scope *emela_scope_begin(emela_fiber *fiber
#ifdef EMELA_FAILURE_RUNTIME
    , EmelaError *error_out
#endif
) {
  emela_scope *scope = (emela_scope *)emela_heap_allocate(sizeof(emela_scope), EMELA_SCOPE_KIND);
  *scope = (emela_scope){.closer = {EMELA_SLOT_NONE, 0}};
  emela_scope_frame *frame = (emela_scope_frame *)emela_frame_push(fiber, sizeof(*frame), emela_scope_step);
  frame->base.drop = emela_scope_drop;
  frame->base.unwind = emela_scope_unwind;
  frame->scope = scope;
#ifdef EMELA_FAILURE_RUNTIME
  scope->owner = (EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation};
  scope->frame = frame;
  frame->body_error = (EmelaError){0};
  frame->error_out = error_out;
  frame->base.has_failure = emela_scope_has_failure;
#endif
  return scope;
}
static void emela_scope_check(emela_scope *scope) {
  if (scope == NULL || scope->closed)
    emela_board_defect(EMELA_DEFECT_PANIC, "fiber scope was used after it ended", UINT16_C(34));
}
static inline emela_fiber *emela_child_begin(emela_scope *scope) {
  emela_scope_check(scope);
  EmelaFiber handle = emela_fiber_create();
  emela_fiber *fiber = emela_fiber_get(handle);
  emela_child *child = (emela_child *)emela_heap_allocate(sizeof(emela_child), EMELA_CHILD_KIND);
  *child = (emela_child){.scope = scope, .next = scope->children, .execution = handle};
  emela_heap_retain(scope);
  /* The initial reference belongs to the execution context. The scope list is weak. */
  if (scope->children != NULL) scope->children->previous = child;
  scope->children = child;
  scope->running++;
  fiber->child = child;
  return fiber;
}
static inline EmelaAny emela_deferred_create(emela_scope *scope) {
  emela_scope_check(scope);
#ifdef EMELA_DEFERRED_CAPACITY
  return emela_deferred_allocate(scope);
#else
  emela_scheduler_invalid(); /* No reachable operation can invoke this generated constructor. */
#endif
}
