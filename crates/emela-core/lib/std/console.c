/* Host functions for Console on C (`@external(c, "./console.c", ...)` in console.emel). */
#include <stdio.h>

/* Writes the line and a newline to standard output. */
void emela_console_print_line(const char *text) {
  fputs(text, stdout);
  fputc('\n', stdout);
}

/* Writes the line and a newline to standard error, after what standard output holds so far. */
void emela_console_print_error_line(const char *text) {
  fflush(stdout);
  fputs(text, stderr);
  fputc('\n', stderr);
}
