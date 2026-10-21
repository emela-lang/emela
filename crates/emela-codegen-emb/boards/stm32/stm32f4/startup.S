/* SPDX-License-Identifier: Apache-2.0 */
.syntax unified
.cpu cortex-m4
.fpu fpv4-sp-d16
.thumb

/* Weak handlers can be replaced by application C or assembly definitions. */
.macro vector name
    .word \name
    .weak \name
    .thumb_set \name, Default_Handler
.endm

.section .isr_vector,"a",%progbits
.global g_pfnVectors
.type g_pfnVectors, %object
g_pfnVectors:
    .word _estack
    .word Reset_Handler
    vector NMI_Handler
    vector HardFault_Handler
    vector MemManage_Handler
    vector BusFault_Handler
    vector UsageFault_Handler
    .word 0
    .word 0
    .word 0
    .word 0
    vector SVC_Handler
    vector DebugMon_Handler
    .word 0
    vector PendSV_Handler
    vector SysTick_Handler
    /* Slot numbers and weak names come from the selected MCU metadata. */
#include "board_vectors.inc"
.global __emela_vector_table_end__
__emela_vector_table_end__:
.size g_pfnVectors, .-g_pfnVectors

.section .text.Reset_Handler,"ax",%progbits
.global Reset_Handler
.type Reset_Handler, %function
.thumb_func
Reset_Handler:
    /* Enable the FPU before hard-float code; reset clock remains HSI at 16 MHz. */
    ldr r0, =0xe000ed88
    ldr r1, [r0]
    ldr r2, =0x00f00000
    orrs r1, r2
    str r1, [r0]
    ldr r0, =0xe000ed08
    ldr r1, =g_pfnVectors
    str r1, [r0]
    dsb
    isb
    ldr r0, =_sidata
    ldr r1, =_sdata
    ldr r2, =_edata
1:
    cmp r1, r2
    bhs 2f
    ldr r3, [r0], #4
    str r3, [r1], #4
    b 1b
2:
    ldr r1, =_sbss
    ldr r2, =_ebss
    movs r3, #0
3:
    cmp r1, r2
    bhs 4f
    str r3, [r1], #4
    b 3b
4:
    bl __libc_init_array
    bl main
5:
    wfi
    b 5b
.size Reset_Handler, .-Reset_Handler

.section .text.Default_Handler,"ax",%progbits
.global Default_Handler
.type Default_Handler, %function
.thumb_func
Default_Handler:
    cpsid i
6:
    wfi
    b 6b
.size Default_Handler, .-Default_Handler
.section .note.GNU-stack,"",%progbits
