/* Host functions for Env on C (`@external(c, "./env.c", ...)` in env.emel). Reads the process
 * environment with getenv. */
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

/* Whether the environment variable named `name` is set. */
bool emela_env_has(const char *name) { return getenv(name) != NULL; }

/* A copy of the value of the environment variable named `name` (`malloc`ed; the runtime frees it).
 * Call only when `emela_env_has` is true. */
char *emela_env_get(const char *name) {
  const char *value = getenv(name);
  if (value == NULL) value = "";
  size_t n = strlen(value);
  char *copy = malloc(n + 1);
  if (copy != NULL) memcpy(copy, value, n + 1);
  return copy;
}
