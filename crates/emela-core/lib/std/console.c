/* Host functions for Console on C (`@external(c, "./console.c", ...)` in console.emel). They
 * write through the runtime, the same path as SystemStdout, SystemStderr and `dbg`, so that writes
 * mixed between them keep their order and a closed stream is handled as `[api.std.process]` says. */
#include <stdbool.h>
#include <stddef.h>
#include <string.h>

/* In the runtime (emela_runtime.c), not in emela.h. */
void em_write_stdout(const char *bytes, size_t n, bool newline);
void em_write_stderr(const char *bytes, size_t n, bool newline);

/* Writes the line and a newline to standard output. */
void emela_console_print_line(const char *text) { em_write_stdout(text, strlen(text), true); }

/* Writes the line and a newline to standard error, after what standard output holds so far. */
void emela_console_print_error_line(const char *text) { em_write_stderr(text, strlen(text), true); }
