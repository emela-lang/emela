/* The host side of `print` in these programs (`@external(c, "./print.c", "emela_test_print")`). */
#include <stdio.h>

void emela_test_print(const char *s) { puts(s); }
