/* Context callbacks run in main, outside the event lock. Managed board-held values need
 * their own retained reference; borrowed bytes must remain pinned until release.
 * Callbacks may retain/release heap values and enqueue notifications. Do not reenter the
 * scheduler or generated Emela entry points, including start/cancel wrappers.
 */
typedef void (*EmelaTaskContextRelease)(void *context);
