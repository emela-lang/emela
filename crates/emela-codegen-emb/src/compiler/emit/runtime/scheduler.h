/* All scheduler APIs are called from the main execution context, never from an ISR. */
typedef struct {
  uint32_t slot;
  uint32_t generation;
} EmelaFiber;

/* Runs at most budget work units (a top-level dispatch or a completion event).
 * A dispatch includes synchronous same-fiber resume work up to its next suspension.
 * Returns the number actually processed. A zero budget performs no work. */
uint32_t emela_scheduler_run(uint32_t budget);
/* Main-context observation only; does not dispatch. A platform may mask interrupts
 * around this check and its architecture-specific wait instruction. */
int emela_scheduler_ready(void);
int emela_fiber_done(EmelaFiber fiber);
/* Releases a completed handle. Result storage belongs to the caller. */
void emela_fiber_release(EmelaFiber fiber);
