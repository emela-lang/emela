/* Both callbacks run only in the main scheduler context, outside the event lock.
 * The producer supplies a borrowed, registered, nonempty EmelaError. Its referenced values
 * must remain valid until release: the runtime copies inline Strings and retains heap edges.
 * List nodes, recursive boxes, and other heap containers must be valid Emela-owned values.
 * Callbacks may retain/release Emela heap values and enqueue other task notifications.
 * Do not call the scheduler or generated Emela entry points (including start/cancel wrappers)
 * from either callback. Scheduler reentry defects; other entry-point reentry is not supported.
 * A board-held managed value must have its own retained reference throughout the lease.
 */
typedef void (*EmelaTaskFailureProducer)(void *context, EmelaError *error);

/* ISR-safe: only queues pointers in fixed storage; performs no allocation or callbacks.
 * Returning one transfers a context lease until exactly one release callback (if non-NULL).
 * The board must pin a notification-time snapshot, including all referenced data, until then.
 * A stale, duplicate, nonfailing task, or NULL producer returns zero without taking the lease.
 * A full queue defects after unlocking, without taking the lease.
 * Cancellation skips the producer and releases the lease when the scheduler drains the event.
 * The board must keep pumping the scheduler after cancellation, including zero-ready-fiber cases.
 */
int emela_task_fail(EmelaTask task, EmelaTaskFailureProducer producer,
                    void *context, EmelaTaskContextRelease release);
