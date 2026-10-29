#include <stddef.h>
#include <stdint.h>

#include "validation_stack.h"

#if !defined(__arm__) || !defined(__GNUC__)
#error "MSP validation requires ARM GCC naked assembly"
#endif

#define EMELA_STACK_PATTERN UINT32_C(0xa55a3cc3)
#define EMELA_GUARD_PATTERN UINT32_C(0xd15ea5ed)

extern uint32_t __emela_stack_bottom__[];
extern uint32_t __emela_stack_top__[];
extern uint32_t __emela_stack_guard_start__[];
extern uint32_t __emela_stack_guard_end__[];

volatile uint32_t emela_validation_stack_used;
volatile uint32_t emela_validation_stack_capacity;
volatile uint32_t emela_validation_stack_painted;
volatile uint32_t emela_validation_stack_guard_ok;
volatile uint32_t emela_validation_stack_samples;
volatile uint32_t emela_validation_stack_initial_sp;
volatile uint32_t emela_validation_stack_min_sp;

/* No prologue, calls or callee-saved registers: never paint the active caller frame. */
__attribute__((naked, noinline)) static void emela_validation_stack_paint(void) {
    __asm volatile(
        "mrs r3, primask\n"
        "cpsid i\n"
        "ldr r0, =__emela_stack_guard_start__\n"
        "ldr r1, =__emela_stack_guard_end__\n"
        "ldr r2, =0xd15ea5ed\n"
        "1: cmp r0, r1\n"
        "bhs 2f\n"
        "str r2, [r0], #4\n"
        "b 1b\n"
        "2: mov r1, sp\n"
        "ldr r2, =0xa55a3cc3\n"
        "3: cmp r0, r1\n"
        "bhs 4f\n"
        "str r2, [r0], #4\n"
        "b 3b\n"
        "4: ldr r0, =emela_validation_stack_initial_sp\n"
        "str r1, [r0]\n"
        "ldr r0, =emela_validation_stack_min_sp\n"
        "str r1, [r0]\n"
        "ldr r0, =emela_validation_stack_painted\n"
        "ldr r2, =__emela_stack_bottom__\n"
        "subs r2, r1, r2\n"
        "str r2, [r0]\n"
        "msr primask, r3\n"
        "bx lr\n"
    );
}

__attribute__((naked, noinline)) void emela_validation_stack_record_sp(void) {
    __asm volatile(
        "mrs r3, primask\n"
        "cpsid i\n"
        "mrs r0, msp\n"
        "ldr r1, =emela_validation_stack_min_sp\n"
        "ldr r2, [r1]\n"
        "cmp r0, r2\n"
        "bhs 1f\n"
        "str r0, [r1]\n"
        "1: msr primask, r3\n"
        "bx lr\n"
    );
}

void emela_validation_stack_sample(void) {
    emela_validation_stack_record_sp();
    const uintptr_t bottom = (uintptr_t)__emela_stack_bottom__;
    const uintptr_t top = (uintptr_t)__emela_stack_top__;
    const uintptr_t paint_end = emela_validation_stack_initial_sp;
    uintptr_t lowest = bottom;
    uint32_t guard_ok = 1;
    for (volatile uint32_t *p = __emela_stack_guard_start__; p < __emela_stack_guard_end__; ++p) {
        if (*p != EMELA_GUARD_PATTERN) guard_ok = 0;
    }
    while (lowest < paint_end && *(volatile uint32_t *)lowest == EMELA_STACK_PATTERN) {
        lowest += sizeof(uint32_t);
    }
    if (emela_validation_stack_min_sp < lowest) lowest = emela_validation_stack_min_sp;
    emela_validation_stack_capacity = (uint32_t)(top - bottom);
    emela_validation_stack_used = lowest <= top ? (uint32_t)(top - lowest) : UINT32_MAX;
    emela_validation_stack_guard_ok = guard_ok;
    emela_validation_stack_samples++;
}

int __real_main(void);

int __wrap_main(void) {
    emela_validation_stack_paint();
    emela_validation_stack_sample();
    return __real_main();
}
