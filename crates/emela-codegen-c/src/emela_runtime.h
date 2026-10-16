/* The Emela C runtime (target c, spec 16, 16.5).
 *
 * The generated `main.c` includes only this header, so that the prototypes it writes for host
 * functions (`@external(c, ...)`, spec 12.4) do not clash with the C library headers.
 *
 * Values (see crates/emela-codegen-c/NOTES.md):
 *
 * - Int and Bool are `int32_t`, Int64 is `int64_t`, Float is `double`, and the tag of an enum
 *   whose constructors take no arguments is an `int32_t`.
 * - Everything else is a reference (`em_ref`) to a heap object, or NULL for the empty list.
 *
 * Every heap object starts with a 16-byte header:
 *
 *     offset 0: uint32_t rc     reference count; 0 marks static data, which is never counted
 *     offset 4: uint32_t aux    unused (padding)
 *     offset 8: em_desc *desc   what the object is: its kind, constructor tag, slot count and
 *                               which slots hold references
 *
 * followed by 8-byte slots (`em_word`). While an object is being freed, its first 8 bytes hold
 * the link of the free stack, so freeing never recurses (long lists and deep enums).
 *
 * Strings are UTF-8 with their length in slot 0 and the bytes (with a terminating NUL that is
 * not part of the string) from slot 1. Lists are cons cells (slot 0 the head, slot 1 the tail).
 * Closures are `{function pointer, captures...}`. Handlers are `{retired flag, operations...}`.
 *
 * Single-threaded (only the completions of `emela.h` may come from other threads, through a locked
 * inbox): no atomics. Defects and failures are passed back through the pending slots
 * `em_defect` and `em_failure`, which callers test after each call that may set them. They hold a
 * value only while a call returns (never at a suspension point), so one pair serves every fiber.
 * An interruption is pending as a defect of its own kind (spec 11.4). */
#ifndef EMELA_RUNTIME_H
#define EMELA_RUNTIME_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "emela.h"

typedef struct em_obj em_obj;
typedef em_obj *em_ref;
typedef void (*em_fnptr)(void);

typedef union em_word {
  int32_t i;
  int64_t l;
  double f;
  em_ref p;
  void *h;
  em_fnptr fn;
} em_word;

enum {
  /* Fixed slots; `refs` says which hold references. Enums, tuples, records, cons cells,
   * closures, handlers, cells and boxes. */
  EM_KIND_FIELDS = 0,
  /* Slot 0 the byte length, then the UTF-8 bytes and a NUL. */
  EM_KIND_STRING = 1,
  /* An evidence table (spec 16.5): slot 0 the binding count n, then n pairs of an effect index
   * and a handler. */
  EM_KIND_EVIDENCE = 2,
  /* A host value of an `@external type` (spec 12.3) in slot 0. Never freed by Emela. */
  EM_KIND_HOST = 3,
  /* An object of the runtime (a fiber, a scope, a Deferred, a resume): `fin` releases what it
   * holds before it is freed. */
  EM_KIND_RUNTIME = 4,
};

typedef struct em_desc {
  uint32_t kind;
  uint32_t nfields;
  /* The constructor index of an enum value. */
  uint32_t tag;
  /* The enum index of an error value (`is_error`); -1 otherwise. */
  int32_t enum_id;
  /* The constructor or handler name. */
  const char *name;
  /* For an error value: shows it (consumes the argument). Used for `error: ...` at exit. */
  em_ref (*show)(em_ref);
  /* Per slot, 1 if it holds a reference. */
  const uint8_t *refs;
  /* For EM_KIND_RUNTIME: releases what the object holds. */
  void (*fin)(em_ref);
} em_desc;

typedef struct em_hdr {
  union {
    struct {
      uint32_t rc;
      uint32_t aux;
    } h;
    em_obj *next;
  } u;
  const em_desc *desc;
} em_hdr;

struct em_obj {
  em_hdr hdr;
  em_word slots[];
};

/* The static header of a constant object (string literals, constructors without fields,
 * closures without captures). */
#define EM_STATIC_HDR(desc) {{{0, 0}}, (desc)}

/* ---- Reference counting ---- */

void em_free(em_ref o);

static inline void em_dup(em_ref o) {
  if (o != NULL && o->hdr.u.h.rc != 0) o->hdr.u.h.rc++;
}

static inline void em_drop(em_ref o) {
  if (o != NULL && o->hdr.u.h.rc != 0 && --o->hdr.u.h.rc == 0) em_free(o);
}

em_ref em_alloc(const em_desc *desc, size_t nslots);

/* ---- Pending defect and failure ---- */

/* The pending defect (a String), or NULL. */
extern em_ref em_defect;
/* The pending failure (an error value), or NULL. */
extern em_ref em_failure;

/* RuntimeOp::Defect: makes the message (borrowed) the pending defect. */
void em_raise(em_ref msg);
void em_raise_cstr(const char *msg);
/* RuntimeOp::DefectTake and DefectResume. */
em_ref em_defect_take(void);
void em_defect_resume(em_ref d);
/* RuntimeOp::Retire. */
void em_retire(em_ref handler, em_ref name);
void em_handler_used_after_release(em_ref h);
/* Whether the handler may be used; raises `handler X was used after release` otherwise. */
static inline bool em_handler_ok(em_ref h) {
  if (h->slots[0].i == 0) return true;
  em_handler_used_after_release(h);
  return false;
}

_Noreturn void em_unreachable(void);

/* ---- Constants (spec 5.6) ---- */

typedef struct em_const {
  /* 0 not evaluated, 1 being evaluated, 2 has the value, 3 has the defect. */
  int state;
  em_word value;
  bool ref;
  em_ref defect;
  const char *name;
} em_const;

/* Returns true if the caller must evaluate the const (state was 0, now 1). Otherwise the value
 * (dup'ed) or the defect is ready: `*out` has the value, or a defect is pending. */
bool em_const_enter(em_const *c, em_word *out);
/* After the evaluation: remembers the value, or the pending defect (annotated with the name). */
void em_const_leave(em_const *c, em_word value);
void em_const_release(em_const *c);

/* ---- Objects ---- */

static inline uint32_t em_tag(em_ref o) { return o->hdr.desc->tag; }

static inline int32_t em_is_error(em_ref o, int32_t enum_id) {
  return o->hdr.desc->enum_id == enum_id;
}

static inline em_fnptr em_closure_fn(em_ref c) { return c->slots[0].fn; }

extern const em_desc em_desc_string;
extern const em_desc em_desc_cons_val;
extern const em_desc em_desc_cons_ref;
extern const em_desc em_desc_box_val;
extern const em_desc em_desc_cell_val;
extern const em_desc em_desc_cell_ref;

static inline em_ref em_cons(em_word head, bool ref, em_ref tail) {
  em_ref c = em_alloc(ref ? &em_desc_cons_ref : &em_desc_cons_val, 2);
  c->slots[0] = head;
  c->slots[1].p = tail;
  return c;
}

/* Evidence tables. `em_ev_new` consumes `base` (or NULL) and leaves `n` bindings to set. */
em_ref em_ev_new(em_ref base, int64_t n);
/* Sets binding `i` of the `n` that `em_ev_new` left (consumes `h`). */
static inline void em_ev_set(em_ref ev, int64_t n, int64_t i, int32_t effect, em_ref h) {
  int64_t k = ev->slots[0].l - n + i;
  ev->slots[1 + 2 * k].i = effect;
  ev->slots[2 + 2 * k].p = h;
}
em_ref em_ev_lookup(em_ref ev, int32_t effect);

/* Boxes for `Any` and for host values. */
em_ref em_box(em_word w);
em_ref em_host_box(void *h);
static inline void *em_host_get(em_ref o) { return o->slots[0].h; }

/* ---- Integers (spec 16.1): wrapping, without undefined behavior ---- */

static inline int32_t em_iadd(int32_t a, int32_t b) { return (int32_t)((uint32_t)a + (uint32_t)b); }
static inline int32_t em_isub(int32_t a, int32_t b) { return (int32_t)((uint32_t)a - (uint32_t)b); }
static inline int32_t em_imul(int32_t a, int32_t b) { return (int32_t)((uint32_t)a * (uint32_t)b); }
static inline int32_t em_ineg(int32_t a) { return (int32_t)(0u - (uint32_t)a); }
/* The divisor is not 0 (the Heap IR tests it first). INT_MIN / -1 wraps. */
static inline int32_t em_idiv(int32_t a, int32_t b) { return b == -1 ? em_ineg(a) : a / b; }
static inline int32_t em_irem(int32_t a, int32_t b) { return b == -1 ? 0 : a % b; }

static inline int64_t em_ladd(int64_t a, int64_t b) { return (int64_t)((uint64_t)a + (uint64_t)b); }
static inline int64_t em_lsub(int64_t a, int64_t b) { return (int64_t)((uint64_t)a - (uint64_t)b); }
static inline int64_t em_lmul(int64_t a, int64_t b) { return (int64_t)((uint64_t)a * (uint64_t)b); }
static inline int64_t em_lneg(int64_t a) { return (int64_t)(0u - (uint64_t)a); }
static inline int64_t em_ldiv(int64_t a, int64_t b) { return b == -1 ? em_lneg(a) : a / b; }
static inline int64_t em_lrem(int64_t a, int64_t b) { return b == -1 ? 0 : a % b; }

double em_fmod(double a, double b);

/* Shifts: a negative amount is a defect; an amount at or above the width shifts every bit out. */
int32_t em_ishl(int32_t a, int32_t by);
int32_t em_ishr(int32_t a, int32_t by);
int32_t em_iushr(int32_t a, int32_t by);
int64_t em_lshl(int64_t a, int32_t by);
int64_t em_lshr(int64_t a, int32_t by);
int64_t em_lushr(int64_t a, int32_t by);

/* ---- Builtins (arguments borrowed, results owned) ---- */

int32_t em_str_eq(em_ref a, em_ref b);
/* Code point order (the same as UTF-8 byte order): negative, 0 or positive. */
int32_t em_str_cmp(em_ref a, em_ref b);
int32_t em_string_byte_size(em_ref s);
em_ref em_string_append(em_ref a, em_ref b);
int32_t em_string_contains(em_ref s, em_ref sub);
int32_t em_string_starts_with(em_ref s, em_ref prefix);
int32_t em_string_ends_with(em_ref s, em_ref suffix);
em_ref em_string_split(em_ref s, em_ref sep);
em_ref em_string_join(em_ref parts, em_ref sep);
em_ref em_string_trim(em_ref s);
em_ref em_string_code_points(em_ref s);
em_ref em_string_from_code_point(int32_t n);
em_ref em_int_to_string(int32_t n);
em_ref em_int_parse(em_ref s);
em_ref em_int_checked_add(int32_t a, int32_t b);
em_ref em_int64_to_string(int64_t n);
em_ref em_float_to_string(double x);
em_ref em_float_parse(em_ref s);
int32_t em_float_floor(double x);
int32_t em_float_ceil(double x);
int32_t em_float_round(double x);
int32_t em_float_truncate(double x);
em_ref em_show_string(em_ref s);
void em_debug_print(em_ref s);

/* ---- The host boundary (spec 12.4, 12.5) ---- */

/* The bytes of a String for C, or NULL (with a pending defect) if it contains U+0000. */
const char *em_cstr(em_ref s);
/* A String from a host string (`malloc`ed, freed here); invalid UTF-8 becomes U+FFFD. NULL is a
 * defect naming the external function. */
em_ref em_from_host_string(char *s, const char *function);

/* ---- `suspend fn` external functions (spec 12.6) ----
 *
 * The wrapper of such a function makes a task (`em_task_new`, with the kind of host value it
 * returns and the function's name), calls the host function with it and then calls
 * `em_task_end`: if the task was completed in place, that returns the value (or leaves a defect
 * pending); otherwise it starts a suspension that waits for the completion. */

enum {
  EM_HOST_UNIT = 0,
  EM_HOST_INT = 1,
  EM_HOST_INT64 = 2,
  EM_HOST_FLOAT = 3,
  EM_HOST_BOOL = 4,
  EM_HOST_STRING = 5,
  EM_HOST_HANDLE = 6,
};

em_task *em_task_new(int32_t kind, const char *function);
em_word em_task_end(em_task *task);

/* ---- Suspension (spec 14.5; the Heap IR's state machines) ----
 *
 * A suspending function returns as usual, or comes back suspended: then `em_suspending` is set
 * and each suspending function on the way out pushes a frame of its own (`em_frame_push`) onto
 * the continuation being captured, innermost first, and returns. The runtime's driver loop takes
 * the continuation and later resumes it frame by frame: it pops a frame and calls its resume
 * function with the input (a value; a failure or defect in the pending slots), so resuming never
 * nests on the C stack.
 *
 * A frame is an object of `3 + n` slots: slot 0 its resume function, slot 1 the state (the
 * resume point), slot 2 the next (outer) frame, then the n saved variables. */

/* Whether a suspension is bubbling up. */
extern bool em_suspending;

/* The value representations, for the runtime functions that move values of any type. */
enum { EM_REPR_VOID = 0, EM_REPR_I32 = 1, EM_REPR_I64 = 2, EM_REPR_F64 = 3, EM_REPR_REF = 4 };

typedef em_word (*em_resume_fn)(em_ref frame, em_word in);

em_ref em_frame_new(const em_desc *desc, em_resume_fn resume, int32_t state, size_t nsaved);
/* Pushes the frame onto the continuation being captured (as the outermost so far). */
void em_frame_push(em_ref frame);
/* Frees a frame whose saved variables were moved out. */
void em_frame_free(em_ref frame);

/* A suspending call in a non-suspending function (spec 8.7). If it comes back suspended, the
 * continuation will never be resumed: it is unwound at once as interrupted (spec 14.5), and the
 * interruption is the pending defect. */
void em_sync_enter(void);
void em_sync_leave(void);

/* RuntimeOp::Yield and YieldPerform: the fairness yield before a suspending call (spec 11.1). */
void em_yield(bool perform);
/* RuntimeOp::Hold and Unhold: a release that may suspend is not interrupted (spec 11.4). */
void em_hold(void);
void em_unhold(void);

/* A call of a `suspend fn` operation: `em_resume_new` makes the resume function value passed as
 * the implementation's last argument (the caller keeps a second reference), and after the call
 * `em_perform_end` returns the value to continue with (or a pending failure or defect), or
 * starts a suspension. Consumes the caller's reference. */
em_ref em_resume_new(em_ref handler, const char *op, int32_t repr);
em_word em_perform_end(em_ref resume);

/* ---- Fibers (spec chapter 11) ----
 *
 * Arguments are borrowed. `repr` is the representation of the value type `A`. */

em_word em_fiber_scope(em_ref body, int32_t repr);
em_ref em_fiber_fork(em_ref scope, em_ref f);
em_ref em_fiber_deferred(em_ref scope);
em_word em_fiber_join(em_ref fiber, int32_t repr);
em_ref em_fiber_await(em_ref fiber, int32_t repr);
void em_fiber_interrupt(em_ref fiber);
em_ref em_fiber_attempt(em_ref f, int32_t repr);
int32_t em_deferred_complete(em_ref d, em_word value, int32_t repr);
em_word em_deferred_await(em_ref d, int32_t repr);
em_ref em_deferred_poll(em_ref d, int32_t repr);
int32_t em_cause_kind(em_ref c);
em_ref em_cause_to_string(em_ref c);
void em_cause_raise(em_ref c);

/* Runs `entry` (main or a test function, returning a value of representation `repr`) as the
 * root fiber of a run until the run ends (spec 14.5). Afterwards a failure or defect that ended
 * it is pending (an interruption is a defect of its own kind). */
void em_run(em_word (*entry)(void), int32_t repr);

/* ---- The program ---- */

void em_start(int argc, char **argv);
/* Reports how the test function that just returned ended, as a line of the test runner protocol
 * on stdout, and clears a pending defect or failure. */
void em_test_result(const char *name);
void em_test_done(void);
/* Ends the run: reports a pending defect (`defect: ...`, 101) or failure (`error: ...`, 1),
 * releases what the runtime holds and, with the environment variable EMELA_LEAK_CHECK set (or
 * the runtime compiled with -DEMELA_LEAK_CHECK), reports live allocations (exit code 99).
 * Returns the exit code. `cleanup` releases the program's const values. */
int em_exit(void (*cleanup)(void));

#endif
