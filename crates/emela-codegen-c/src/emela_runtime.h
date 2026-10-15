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
 * Single-threaded: no atomics. Defects and failures are passed back through the pending slots
 * `em_defect` and `em_failure` (one fiber for now), which callers test after each call that may
 * set them. */
#ifndef EMELA_RUNTIME_H
#define EMELA_RUNTIME_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

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
_Noreturn void em_unsupported(const char *what);

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

/* ---- The program ---- */

void em_start(int argc, char **argv);
/* Reports how the test function that just returned ended, as a line of the test runner protocol
 * on stdout, and clears a pending defect or failure. */
void em_test_result(const char *name);
void em_test_done(void);
/* Ends the run: reports a pending defect (`defect: ...`, 101) or failure (`error: ...`, 1),
 * releases what the runtime holds and, with EMELA_LEAK_CHECK set, reports live allocations.
 * Returns the exit code. `cleanup` releases the program's const values. */
int em_exit(void (*cleanup)(void));

#endif
