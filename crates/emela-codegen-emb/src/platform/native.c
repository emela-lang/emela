/* ABI 2 host services. Completions run only on the calling thread. */
#define _POSIX_C_SOURCE 200809L
#include "emela_board.h"
#include "emela_embedded.h"
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <errno.h>

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    fprintf(stderr, "Emela defect %d: ", (int)kind);
    if (length) (void)fwrite(message, 1, length, stderr);
    fputc('\n', stderr); abort();
}
void emela_board_debug(const char *bytes, uint16_t length) {
    if (length) (void)fwrite(bytes, 1, length, stdout);
}
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t previous) { (void)previous; }
#ifdef EMELA_REQUIRE_SERVICE_CONSOLE
#include "emela_console.h"
void emela_board_console_print_line(const char *bytes, uint16_t length) {
    if (length) (void)fwrite(bytes, 1, length, stdout);
    fputc('\n',stdout);
}
void emela_board_console_print_error_line(const char *bytes, uint16_t length) {
    if (length) (void)fwrite(bytes, 1, length, stderr);
    fputc('\n',stderr);
}
#endif
#ifdef EMELA_REQUIRE_SERVICE_CLOCK
#include "emela_clock.h"
static int64_t milliseconds(clockid_t clock) {
    struct timespec time;
    if (clock_gettime(clock,&time)!=0) abort();
    return (int64_t)time.tv_sec*INT64_C(1000)+(int64_t)time.tv_nsec/INT64_C(1000000);
}
int64_t emela_board_clock_now(void) { return milliseconds(CLOCK_REALTIME); }
#ifdef EMELA_HAS_TASK_RUNTIME
typedef struct { EmelaTask task; int64_t deadline; int active; } NativeTimer;
static NativeTimer timers[EMELA_PLATFORM_TASK_CAPACITY];
void emela_board_clock_sleep(EmelaTask task, int32_t duration) {
    /* A recycled scheduler slot replaces a canceled timer. No borrowed values persist. */
    unsigned slot=EMELA_PLATFORM_TASK_CAPACITY;
    for (unsigned i=0;i<EMELA_PLATFORM_TASK_CAPACITY;++i) if (timers[i].task.slot==task.slot) { slot=i; break; }
    if (slot==EMELA_PLATFORM_TASK_CAPACITY) for (unsigned i=0;i<EMELA_PLATFORM_TASK_CAPACITY;++i) if (!timers[i].active) { slot=i;break; }
    if (slot!=EMELA_PLATFORM_TASK_CAPACITY) {
        timers[slot]=(NativeTimer){task,milliseconds(CLOCK_MONOTONIC)+(duration>0?duration:0),1};return;
    }
    emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED,"native timer capacity",21);
}
static void poll_timers(void) {
    int64_t now=milliseconds(CLOCK_MONOTONIC);
    for (unsigned i=0;i<EMELA_PLATFORM_TASK_CAPACITY;++i) if (timers[i].active && timers[i].deadline<=now) {
        EmelaTask task=timers[i].task;timers[i].active=0;
        (void)emela_task_complete_unit(task);
    }
}
#endif
#endif
static void (*native_poll)(void);
void emela_native_set_poll(void (*poll)(void)) { native_poll = poll; }
void emela_board_idle(void) {
    if (native_poll != NULL) native_poll();
#if defined(EMELA_REQUIRE_SERVICE_CLOCK) && defined(EMELA_HAS_TASK_RUNTIME)
    poll_timers();
#endif
    struct timespec interval={0,1000000};
    while (nanosleep(&interval,&interval)!=0 && errno==EINTR) {}
}
void emela_platform_start(void (*program)(void)) { program(); }
