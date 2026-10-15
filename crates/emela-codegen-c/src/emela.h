/* emela.h: the API for C host functions of `suspend fn` external functions (spec 12.6).
 *
 * The compiler writes this header to the output directory, and a `.c` file that
 * `@external(c, "./file.c", "name")` points to includes it:
 *
 *     #include "emela.h"
 *
 *     static void on_timer(em_task *task, void *ctx) { em_complete_unit(task); }
 *
 *     // @external(c, "./clock.c", "host_sleep")
 *     // suspend fn host_sleep(ms: Int) -> ()
 *     void host_sleep(int32_t ms, em_task *task) { em_after(ms, task, on_timer, NULL); }
 *
 * The C function of a `suspend fn` external function takes the arguments of the table of spec
 * 12.4, then `em_task *task`, and returns `void`. The task stands for the fiber waiting for the
 * result. Complete it exactly once with the function for the return type: right away inside the
 * C function, or later (from a callback, or from another thread). A second completion is a
 * defect. The completion functions may be called from any thread; every other `em_` function
 * only on the runtime's thread (inside the external function and inside the callbacks the
 * runtime calls).
 *
 * The callbacks registered with `em_after`, `em_on_readable` and `em_on_writable` run on the
 * runtime's thread, and usually complete the task. Completing a task removes what is still
 * registered for it (its callbacks are not called any more).
 *
 * When the waiting fiber is interrupted (spec 11.4), the callback registered with `em_on_cancel`
 * is called. The task must still be completed exactly once afterwards (the value is discarded),
 * which frees it. When fibers only wait for tasks and nothing is registered with the event loop
 * (no timer, no file descriptor), the run has stalled: the waiting continuations are abandoned
 * (spec 14.5) and the tasks are cancelled. A host function that hands its task to another thread
 * keeps the event loop waiting with a registration of its own (`em_after` with a long time),
 * which completing the task removes.
 *
 * When a task pointer stays valid: an `em_task *` is a handle, not the address of memory the host
 * may read (never dereference it). From the call of the external function until the task is
 * completed, it may be passed to every function here. After the first completion, it may only be
 * passed to a completion function again, and that call is the defect of a second completion: the
 * runtime recognizes the handle even after it has freed the task (which it does once it is back
 * in its event loop), so the second completion never touches freed memory, from the runtime's
 * thread or from another one. `em_after`, `em_on_readable`, `em_on_writable` and `em_on_cancel`
 * with a completed task do nothing. A slot is only given the same handle again after 2^32 later
 * tasks have used it (2^12 where pointers have 32 bits), so a completion that late could hit
 * another task. Passing a pointer that never came from the runtime is a bug of the host (the
 * program usually aborts). The shape of these types and functions may change between versions. */
#ifndef EMELA_H
#define EMELA_H

#include <stdbool.h>
#include <stdint.h>

/* A fiber waiting for a `suspend fn` external function. */
typedef struct em_task em_task;

/* What `em_after`, `em_on_readable`, `em_on_writable` and `em_on_cancel` call back. */
typedef void (*em_callback)(em_task *task, void *ctx);

/* Completes the task with the value (the return type of the external function). */
void em_complete_int(em_task *task, int32_t value);
void em_complete_int64(em_task *task, int64_t value);
void em_complete_float(em_task *task, double value);
void em_complete_bool(em_task *task, bool value);
/* `value` is a NUL-terminated UTF-8 string allocated with `malloc`; the runtime copies and frees
 * it (invalid UTF-8 becomes U+FFFD; NULL is a defect). */
void em_complete_string(em_task *task, char *value);
void em_complete_unit(em_task *task);
/* For an `@external type` result. */
void em_complete_handle(em_task *task, void *value);
/* An error of the host: the waiting fiber gets a defect with the message (copied). */
void em_complete_defect(em_task *task, const char *message);

/* Calls `callback` on the runtime's thread after `ms` milliseconds (a negative `ms` is 0). */
void em_after(int64_t ms, em_task *task, em_callback callback, void *ctx);
/* Calls `callback` on the runtime's thread once `fd` is ready for reading (or writing). Each
 * registration is called at most once; register again to wait again. */
void em_on_readable(int fd, em_task *task, em_callback callback, void *ctx);
void em_on_writable(int fd, em_task *task, em_callback callback, void *ctx);
/* Calls `callback` on the runtime's thread when the waiting fiber is interrupted. A later call
 * replaces the callback. */
void em_on_cancel(em_task *task, em_callback callback, void *ctx);

#endif
