/* Host functions for Clock on C (`@external(c, "./clock.c", ...)` in clock.emel). `sleep` waits,
 * so it has no C function until the C target can suspend (spec 18.1 #37). */
#include <stdint.h>
#include <time.h>

/* Milliseconds since the Unix epoch, like JS `Date.now()`. */
int64_t emela_clock_now_milliseconds(void) {
  struct timespec ts;
  timespec_get(&ts, TIME_UTC);
  return (int64_t)ts.tv_sec * 1000 + (int64_t)(ts.tv_nsec / 1000000);
}
