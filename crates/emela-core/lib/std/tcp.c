/* Host functions for SystemTcp on C (`@external(c, "./tcp.c", ...)` in tcp.emel), the counterpart
 * of tcp.mjs.
 *
 * Only the registry is here so far. The init and the release of SystemTcp are reached by every
 * `with Process` (`[ext.decl]`), so they need a C binding even before the operations have one; the
 * operations have no `c` binding yet, and calling them on native is E0412. A registry therefore
 * never holds a connection, and its release only frees it. */
#include <stdio.h>
#include <stdlib.h>

typedef struct emela_tcp_registry {
  /* Reserved for the connections the registry opened. */
  int released;
} emela_tcp_registry;

/* A new registry, for the init of a SystemTcp. */
void *emela_tcp_open_registry(void) {
  emela_tcp_registry *r = calloc(1, sizeof(emela_tcp_registry));
  if (r == NULL) {
    fputs("emela: out of memory\n", stderr);
    abort();
  }
  return r;
}

/* Frees the registry, for the release of its SystemTcp. Emela does not free host values
 * (`[ext.host-type]`), and the registry is not used after the release. */
void emela_tcp_release_registry(void *registry) { free(registry); }
