typedef struct emela_child emela_child;
typedef struct emela_scope emela_scope;
typedef struct emela_deferred emela_deferred;
#ifdef EMELA_CHILD_WAIT_RUNTIME
typedef struct emela_child_waiter emela_child_waiter;
#endif
typedef struct emela_scope_frame emela_scope_frame;
struct emela_scope {
  emela_child *children;
  EmelaFiber closer;
  uint32_t running;
  int closed;
  int ended;
#ifdef EMELA_FAILURE_RUNTIME
  EmelaFiber owner;
  emela_scope_frame *frame;
  EmelaError first_error;
#endif
#ifdef EMELA_DEFERRED_CAPACITY
  emela_deferred *deferreds; /* Non-owning: a live Deferred retains its scope. */
#endif
};
struct emela_child {
  emela_scope *scope;
  emela_child *previous;
  emela_child *next; /* Non-owning: executions, handles, and waiters own children. */
  EmelaFiber execution;
  EmelaAny result;
  int status;
#ifdef EMELA_FAILURE_RUNTIME
  EmelaError error;
#endif
#ifdef EMELA_CHILD_WAIT_RUNTIME
  emela_child_waiter *head;
  emela_child_waiter *tail;
#endif
};
static void emela_child_unlink(emela_child *child) {
  if (child->previous != NULL) child->previous->next = child->next;
  else if (child->scope->children == child) child->scope->children = child->next;
  else return;
  if (child->next != NULL) child->next->previous = child->previous;
  child->previous = child->next = NULL;
}
/* The constructor is part of Scope's generated handler, even if never called. */
struct emela_deferred {
  emela_scope *scope;
#ifdef EMELA_DEFERRED_CAPACITY
  emela_deferred *previous;
  emela_deferred *next;
  uint32_t slot;
  uint32_t generation;
#endif
};
