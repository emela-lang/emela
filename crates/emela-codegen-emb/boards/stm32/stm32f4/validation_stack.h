#ifndef EMELA_VALIDATION_STACK_H
#define EMELA_VALIDATION_STACK_H

#include <stdint.h>

/* Validation only. Sample from thread mode after workload completion. */
void emela_validation_stack_sample(void);

/* Naked leaf; safe in an ISR. Records MSP without heap, output or scheduler work. */
void emela_validation_stack_record_sp(void);

extern volatile uint32_t emela_validation_stack_used;
extern volatile uint32_t emela_validation_stack_capacity;
extern volatile uint32_t emela_validation_stack_painted;
extern volatile uint32_t emela_validation_stack_guard_ok;
extern volatile uint32_t emela_validation_stack_samples;
extern volatile uint32_t emela_validation_stack_initial_sp;
extern volatile uint32_t emela_validation_stack_min_sp;

#endif
