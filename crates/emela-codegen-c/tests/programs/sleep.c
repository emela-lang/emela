/* The host side of `sleep` in these programs (`@external(c, "./sleep.c", "emela_test_sleep")`):
 * completes with `ms` after `ms` milliseconds, and at once (value discarded) when the sleeping
 * fiber is interrupted, which removes the timer. */
#include <stdint.h>

#include "emela.h"

static void emela_test_wake(em_task *task, void *ctx) {
  em_complete_int(task, (int32_t)(intptr_t)ctx);
}

void emela_test_sleep(int32_t ms, em_task *task) {
  em_after(ms, task, emela_test_wake, (void *)(intptr_t)ms);
  em_on_cancel(task, emela_test_wake, (void *)(intptr_t)ms);
}
