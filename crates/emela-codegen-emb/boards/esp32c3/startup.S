/* The ROM loader initializes .data from the ESP image's RAM segments. */
.section .text.startup, "ax", @progbits
.balign 64
.global _start
.type _start, @function
_start:
    csrci mstatus, 8
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop
    la sp, _stack_top
    la t0, _vectors
    ori t0, t0, 1
    csrw mtvec, t0
    la t0, _bss_start
    la t1, _bss_end
1:
    bgeu t0, t1, 2f
    sw zero, 0(t0)
    addi t0, t0, 4
    j 1b
2:
    la t0, _stack_guard_start
    la t1, _stack_bottom
    li t2, 0xdeadc0de
3:
    bgeu t0, t1, 4f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 3b
4:
    la t1, _stack_top
    li t2, 0xa5a5a5a5
5:
    bgeu t0, t1, 6f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 5b
6:
    call board_main
7:
    wfi
    j 7b
.size _start, .-_start

/* ESP32-C3 uses 32 fixed-width entries in RISC-V vectored mode 1. */
.section .text.vectors, "ax", @progbits
.balign 256
.global _vectors
_vectors:
    .option push
    .option norvc
    .rept 32
    j trap_entry
    .endr
    .option pop

.section .text.trap, "ax", @progbits
.balign 64
.global trap_entry
.type trap_entry, @function
trap_entry:
    addi sp, sp, -128
    sw ra, 0(sp)
    sw t0, 4(sp)
    sw t1, 8(sp)
    sw t2, 12(sp)
    sw a0, 16(sp)
    sw a1, 20(sp)
    sw a2, 24(sp)
    sw a3, 28(sp)
    sw a4, 32(sp)
    sw a5, 36(sp)
    sw a6, 40(sp)
    sw a7, 44(sp)
    sw t3, 48(sp)
    sw t4, 52(sp)
    sw t5, 56(sp)
    sw t6, 60(sp)
    csrr a0, mcause
    csrr a1, mepc
    mv a2, sp
    call board_trap
    lw ra, 0(sp)
    lw t0, 4(sp)
    lw t1, 8(sp)
    lw t2, 12(sp)
    lw a0, 16(sp)
    lw a1, 20(sp)
    lw a2, 24(sp)
    lw a3, 28(sp)
    lw a4, 32(sp)
    lw a5, 36(sp)
    lw a6, 40(sp)
    lw a7, 44(sp)
    lw t3, 48(sp)
    lw t4, 52(sp)
    lw t5, 56(sp)
    lw t6, 60(sp)
    addi sp, sp, 128
    mret
.size trap_entry, .-trap_entry
