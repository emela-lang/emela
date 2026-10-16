/* Host functions for Clock on C (`@external(c, "./clock.c", ...)` in clock.emel). */
#include <stdint.h>
#include <time.h>

#include "emela.h"

/* Milliseconds since the Unix epoch, like JS `Date.now()`. */
int64_t emela_clock_now_milliseconds(void) {
  struct timespec ts;
  timespec_get(&ts, TIME_UTC);
  return (int64_t)ts.tv_sec * 1000 + (int64_t)(ts.tv_nsec / 1000000);
}

static void emela_clock_wake(em_task *task, void *ctx) {
  (void)ctx;
  em_complete_unit(task);
}

/* Completes after `ms` milliseconds (a negative `ms` is 0, spec 8.8). Interrupting the sleeping
 * fiber completes it at once, which removes the timer (JS: the AbortSignal clears the timeout). */
void emela_clock_sleep(int32_t ms, em_task *task) {
  em_after(ms < 0 ? 0 : ms, task, emela_clock_wake, NULL);
  em_on_cancel(task, emela_clock_wake, NULL);
}

/* The runtime's TestClock hook: completes when the run stalls and `at` is the earliest time
 * waited for (spec 8.8). */
void em_when_stalled(int64_t at, em_task *task);

void emela_clock_when_stalled(int64_t at, em_task *task) { em_when_stalled(at, task); }
