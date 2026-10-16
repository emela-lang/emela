/* Host functions for Random on C (`@external(c, "./random.c", ...)` in random.emel). The bytes
 * come from the system's cryptographically secure source: arc4random_buf on macOS, getrandom on
 * Linux. */
#if defined(__linux__)
#define _DEFAULT_SOURCE
#endif

#include <stddef.h>
#include <stdint.h>
#include <string.h>

#if defined(__APPLE__)
#include <stdlib.h>

static void emela_random_fill(void *buf, size_t n) { arc4random_buf(buf, n); }
#else
#include <sys/random.h>
#include <sys/types.h>

static void emela_random_fill(void *buf, size_t n) {
  unsigned char *p = buf;
  while (n > 0) {
    ssize_t got = getrandom(p, n, 0);
    if (got > 0) {
      p += got;
      n -= (size_t)got;
    }
  }
}
#endif

/* Uniform over the whole Int (32-bit) range. */
int32_t emela_random_int(void) {
  uint32_t u;
  int32_t i;
  emela_random_fill(&u, sizeof u);
  memcpy(&i, &u, sizeof i);
  return i;
}

/* Uniform in [0, 1): 53 random bits times 2^-53, as random.mjs does. */
double emela_random_float(void) {
  uint32_t w[2];
  emela_random_fill(w, sizeof w);
  return ((double)(w[0] >> 5) * 67108864.0 + (double)(w[1] >> 6)) / 9007199254740992.0;
}
