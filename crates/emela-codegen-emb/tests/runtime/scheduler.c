#include <assert.h>
#include <setjmp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#define EMELA_UNUSED __attribute__((unused))
#define EMELA_FIBER_CAPACITY 2
#define EMELA_FRAME_CAPACITY 64
#define EMELA_FRAME_BYTES 64
typedef enum { EMELA_DEFECT_ASSERT, EMELA_DEFECT_HEAP_EXHAUSTED } EmelaDefectKind;
static jmp_buf defect_target;
static EmelaDefectKind expected_defect;
static _Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)message; (void)length;
  assert(kind == expected_defect);
  longjmp(defect_target, 1);
}
#include "scheduler.h"
#include <scheduler.c>

typedef struct { emela_frame base; uint32_t left; int *out; } Counter;
static unsigned calls;
static unsigned order[128];
static void count(emela_fiber *fiber, emela_frame *base) {
  Counter *frame = (Counter *)base;
  order[calls++] = (unsigned)(fiber - emela_fibers);
  if (frame->left == 0) { (*frame->out)++; emela_frame_return(fiber); return; }
  uint32_t left = frame->left--;
  Counter *child = (Counter *)emela_frame_push(fiber, sizeof(Counter), count);
  child->left = left - 1;
  child->out = frame->out;
  frame->left = 0;
}
static EmelaFiber start(uint32_t depth, int *out) {
  EmelaFiber handle = emela_fiber_create();
  emela_fiber *fiber = emela_fiber_get(handle);
  Counter *frame = (Counter *)emela_frame_push(fiber, sizeof(Counter), count);
  frame->left = depth;
  frame->out = out;
  emela_ready_push(fiber);
  return handle;
}
int main(void) {
  static int a, b;
  static EmelaFiber first, second;
  assert(!emela_scheduler_ready());
  first = start(20, &a); second = start(3, &b);
  assert(emela_scheduler_ready() && calls == 0);
  assert(emela_scheduler_run(0) == 0 && calls == 0);
  assert(emela_scheduler_run(4) == 4 && calls == 4);
  assert(order[0] == first.slot && order[1] == second.slot);
  assert(order[2] == first.slot && order[3] == second.slot);
  assert(!emela_fiber_done(first) && !emela_fiber_done(second));
  assert(emela_scheduler_run(100) == 44);
  assert(a == 21 && b == 4 && emela_fiber_done(first) && emela_fiber_done(second));
  emela_fiber_release(first); emela_fiber_release(second);
  for (unsigned i = 0; i < 1000; i++) {
    calls = 0;
    EmelaFiber next = start(30, &a);
    assert(emela_scheduler_run(100) == 61);
    emela_fiber_release(next);
  }
  expected_defect = EMELA_DEFECT_ASSERT;
  if (setjmp(defect_target) == 0) { (void)emela_fiber_done(first); abort(); }
  first = start(1, &a); second = start(1, &b);
  expected_defect = EMELA_DEFECT_HEAP_EXHAUSTED;
  if (setjmp(defect_target) == 0) { (void)emela_fiber_create(); abort(); }
  assert(emela_scheduler_run(100) == 6);
  emela_fiber_release(first); emela_fiber_release(second);
  first = start(64, &a);
  calls = 0;
  if (setjmp(defect_target) == 0) { (void)emela_scheduler_run(100); abort(); }
  return 0;
}
