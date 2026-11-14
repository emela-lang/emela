/* Host functions for SystemArgs, SystemStdin, SystemStdout and SystemStderr on C (`@external(c, "./process.c", ...)` in
 * process.emel), the counterpart of process.mjs.
 *
 * Args: the runtime keeps `argv[1]` and after from `main` (`em_start`). The runtime replaces
 * invalid UTF-8 in a returned string with U+FFFD (`[ext.boundary]`), which is what
 * `[api.std.process]` asks of the arguments.
 *
 * Stdin: the bytes read from file descriptor 0 are kept in `buffer[start..end]` until an
 * operation takes them. A waiting operation registers with `em_on_readable` and reads once per
 * wake-up, so other fibers run while it waits; a regular file is always ready for poll(2), so it
 * is read the same way. Nothing is registered while no operation waits, so the input does not
 * keep the program running. Interrupting the waiting fiber completes its task without taking
 * anything, so what was read stays for the next operation. */
#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <poll.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "emela.h"

/* In the runtime (emela_runtime.c), not in emela.h. */
int32_t em_arg_count(void);
const char *em_arg_at(int32_t i);
void em_host_string_length(const char *s, size_t n);
void em_write_stdout(const char *bytes, size_t n, bool newline);
void em_write_stderr(const char *bytes, size_t n, bool newline);
void em_flush_stdout_before_wait(void);

static void *emela_process_alloc(size_t n) {
  void *p = malloc(n == 0 ? 1 : n);
  if (p == NULL) {
    fputs("emela: out of memory\n", stderr);
    abort();
  }
  return p;
}

/* A `malloc`ed copy of `n` bytes, NUL-terminated, whose length the runtime reads as `n`. */
static char *emela_process_string(const char *bytes, size_t n) {
  char *copy = emela_process_alloc(n + 1);
  if (n > 0) memcpy(copy, bytes, n);
  copy[n] = '\0';
  em_host_string_length(copy, n);
  return copy;
}

/* ---- Args ---- */

int32_t emela_process_arg_count(void) { return em_arg_count(); }

char *emela_process_arg_at(int32_t index) {
  const char *arg = em_arg_at(index);
  size_t n = strlen(arg);
  char *copy = emela_process_alloc(n + 1);
  memcpy(copy, arg, n + 1);
  return copy;
}

/* ---- Stdin ---- */

static unsigned char *emela_stdin_buffer = NULL;
static size_t emela_stdin_capacity = 0;
static size_t emela_stdin_start = 0;
static size_t emela_stdin_end = 0;
static bool emela_stdin_ended = false;
/* The message of a read error, until an operation reports it (`malloc`ed). */
static char *emela_stdin_failure = NULL;
/* The text or the message of the last take, and its length. */
static char *emela_stdin_last = NULL;
static size_t emela_stdin_last_length = 0;

enum { EMELA_STDIN_TAKEN, EMELA_STDIN_ENDED, EMELA_STDIN_INVALID, EMELA_STDIN_FAILED, EMELA_STDIN_NOT_YET };

/* What a waiting operation needs: a whole line, or the end of the input. */
enum { EMELA_STDIN_LINE = 1, EMELA_STDIN_END = 2 };

static bool emela_stdin_has_line(void) {
  return memchr(emela_stdin_buffer + emela_stdin_start, '\n', emela_stdin_end - emela_stdin_start) != NULL;
}

static bool emela_stdin_ready(int need) {
  if (emela_stdin_ended || emela_stdin_failure != NULL) return true;
  return need == EMELA_STDIN_LINE && emela_stdin_end > emela_stdin_start && emela_stdin_has_line();
}

/* Room for `n` more bytes at the end of the buffer. */
static void emela_stdin_reserve(size_t n) {
  if (emela_stdin_end + n <= emela_stdin_capacity) return;
  size_t kept = emela_stdin_end - emela_stdin_start;
  if (kept + n <= emela_stdin_capacity / 2) {
    memmove(emela_stdin_buffer, emela_stdin_buffer + emela_stdin_start, kept);
  } else {
    size_t capacity = emela_stdin_capacity < 4096 ? 4096 : emela_stdin_capacity;
    while (kept + n > capacity) capacity *= 2;
    unsigned char *grown = emela_process_alloc(capacity);
    if (kept > 0) memcpy(grown, emela_stdin_buffer + emela_stdin_start, kept);
    free(emela_stdin_buffer);
    emela_stdin_buffer = grown;
    emela_stdin_capacity = capacity;
  }
  emela_stdin_start = 0;
  emela_stdin_end = kept;
}

/* Reads what file descriptor 0 has once. Returns false when nothing was ready (wait again). */
static bool emela_stdin_read(void) {
  /* Several operations may wait at once, and each wake-up reads: look first, so that a read
   * never blocks the runtime. poll(2) cannot look at some devices (POLLNVAL, a terminal on
   * macOS); those are read without looking. */
  struct pollfd pfd = {0, POLLIN, 0};
  int ready;
  do {
    ready = poll(&pfd, 1, 0);
  } while (ready < 0 && errno == EINTR);
  if (ready == 0) return false;
  emela_stdin_reserve(65536);
  ssize_t n;
  do {
    n = read(0, emela_stdin_buffer + emela_stdin_end, 65536);
  } while (n < 0 && errno == EINTR);
  if (n > 0) {
    emela_stdin_end += (size_t)n;
  } else if (n == 0) {
    emela_stdin_ended = true;
  } else if (errno == EAGAIN || errno == EWOULDBLOCK) {
    return false;
  } else {
    const char *message = strerror(errno);
    size_t length = strlen(message);
    emela_stdin_failure = emela_process_alloc(length + 1);
    memcpy(emela_stdin_failure, message, length + 1);
  }
  return true;
}

static void emela_stdin_on_readable(em_task *task, void *ctx) {
  int need = (int)(intptr_t)ctx;
  while (!emela_stdin_ready(need)) {
    if (!emela_stdin_read()) {
      em_on_readable(0, task, emela_stdin_on_readable, ctx);
      return;
    }
  }
  em_complete_unit(task);
}

/* The waiting fiber was interrupted: complete without taking anything. */
static void emela_stdin_on_cancel(em_task *task, void *ctx) {
  (void)ctx;
  em_complete_unit(task);
}

static void emela_stdin_wait(int need, em_task *task) {
  if (emela_stdin_ready(need)) {
    em_complete_unit(task);
    return;
  }
  /* A prompt written before the read reaches the host before the wait (`[api.std.process]`). */
  em_flush_stdout_before_wait();
  em_on_readable(0, task, emela_stdin_on_readable, (void *)(intptr_t)need);
  em_on_cancel(task, emela_stdin_on_cancel, NULL);
}

void emela_stdin_wait_line(em_task *task) { emela_stdin_wait(EMELA_STDIN_LINE, task); }

void emela_stdin_wait_end(em_task *task) { emela_stdin_wait(EMELA_STDIN_END, task); }

/* The length of the valid UTF-8 sequence at `p[i..n]`, or 0 (overlong forms, surrogates and code
 * points above U+10FFFF are invalid). */
static size_t emela_stdin_utf8_at(const unsigned char *p, size_t n, size_t i) {
  unsigned char c = p[i];
  if (c < 0x80) return 1;
  size_t len;
  unsigned char lo = 0x80, hi = 0xBF;
  if (c >= 0xC2 && c <= 0xDF) {
    len = 2;
  } else if (c >= 0xE0 && c <= 0xEF) {
    len = 3;
    if (c == 0xE0) lo = 0xA0;
    if (c == 0xED) hi = 0x9F;
  } else if (c >= 0xF0 && c <= 0xF4) {
    len = 4;
    if (c == 0xF0) lo = 0x90;
    if (c == 0xF4) hi = 0x8F;
  } else {
    return 0;
  }
  if (n - i < len) return 0;
  if (p[i + 1] < lo || p[i + 1] > hi) return 0;
  for (size_t k = 2; k < len; k++) {
    if (p[i + k] < 0x80 || p[i + k] > 0xBF) return 0;
  }
  return len;
}

static bool emela_stdin_utf8(const unsigned char *p, size_t n) {
  for (size_t i = 0; i < n;) {
    size_t len = emela_stdin_utf8_at(p, n, i);
    if (len == 0) return false;
    i += len;
  }
  return true;
}

static void emela_stdin_set_last(const char *bytes, size_t n) {
  free(emela_stdin_last);
  emela_stdin_last = emela_process_alloc(n + 1);
  if (n > 0) memcpy(emela_stdin_last, bytes, n);
  emela_stdin_last[n] = '\0';
  emela_stdin_last_length = n;
}

/* Takes `buffer[from..to]` as the text of the take: TAKEN, or INVALID when it is not UTF-8. */
static int32_t emela_stdin_decode(size_t from, size_t to) {
  if (from == to) {
    emela_stdin_set_last("", 0);
    return EMELA_STDIN_TAKEN;
  }
  const unsigned char *p = emela_stdin_buffer + from;
  if (!emela_stdin_utf8(p, to - from)) {
    emela_stdin_set_last("", 0);
    return EMELA_STDIN_INVALID;
  }
  emela_stdin_set_last((const char *)p, to - from);
  return EMELA_STDIN_TAKEN;
}

/* Reports the read error once; the input ends after it. */
static int32_t emela_stdin_failed(void) {
  emela_stdin_set_last(emela_stdin_failure, strlen(emela_stdin_failure));
  free(emela_stdin_failure);
  emela_stdin_failure = NULL;
  emela_stdin_ended = true;
  emela_stdin_start = emela_stdin_end;
  return EMELA_STDIN_FAILED;
}

int32_t emela_stdin_take_line(void) {
  size_t from = emela_stdin_start;
  const unsigned char *nl =
      from < emela_stdin_end ? memchr(emela_stdin_buffer + from, '\n', emela_stdin_end - from) : NULL;
  if (nl != NULL) {
    size_t to = (size_t)(nl - emela_stdin_buffer);
    emela_stdin_start = to + 1;
    if (to > from && emela_stdin_buffer[to - 1] == '\r') to--;
    return emela_stdin_decode(from, to);
  }
  if (emela_stdin_failure != NULL) return emela_stdin_failed();
  if (!emela_stdin_ended) return EMELA_STDIN_NOT_YET;
  if (from == emela_stdin_end) {
    emela_stdin_set_last("", 0);
    return EMELA_STDIN_ENDED;
  }
  emela_stdin_start = emela_stdin_end;
  return emela_stdin_decode(from, emela_stdin_end);
}

int32_t emela_stdin_take_text(void) {
  if (emela_stdin_failure != NULL) return emela_stdin_failed();
  if (!emela_stdin_ended) return EMELA_STDIN_NOT_YET;
  size_t from = emela_stdin_start;
  emela_stdin_start = emela_stdin_end;
  return emela_stdin_decode(from, emela_stdin_end);
}

char *emela_stdin_taken(void) {
  return emela_process_string(emela_stdin_last != NULL ? emela_stdin_last : "", emela_stdin_last_length);
}

/* ---- Stdout and Stderr ----
 *
 * The runtime writes, through the same `FILE *` as SystemConsole and `dbg`, and handles a closed
 * stream (`em_write_stdout` in the runtime). A write to standard error flushes standard output
 * first. */

void emela_process_write_stdout(const char *text) { em_write_stdout(text, strlen(text), false); }

void emela_process_write_stderr(const char *text) { em_write_stderr(text, strlen(text), false); }

/* ---- ExitCode ---- */

/* In the runtime (emela_runtime.c), not in emela.h: the code `em_exit` returns when the run did
 * not end with an error, a defect or an interruption. */
void em_set_exit_code(int32_t code);

void emela_process_set_exit_code(int32_t code) { em_set_exit_code(code); }
