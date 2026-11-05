/* One 128 KiB SRAM1 slice; disjoint instruction and data physical aliases. */
OUTPUT_ARCH(riscv)
ENTRY(_start)
MEMORY
{
    IRAM (rx) : ORIGIN = 0x40380000, LENGTH = 64K
    DRAM (rw) : ORIGIN = 0x3fc90000, LENGTH = 48K
    STACK (rw) : ORIGIN = 0x3fc9c000, LENGTH = 16K
}
SECTIONS
{
    .text : ALIGN(256)
    {
        KEEP(*(.text.startup))
        KEEP(*(.text.vectors))
        KEEP(*(.text.trap))
        *(.text .text.*)
        *(.rodata .rodata.*)
        . = ALIGN(4);
    } > IRAM
    .data : ALIGN(4)
    {
        _data_start = .;
        *(.data .data.* .sdata .sdata.*)
        . = ALIGN(4);
        _data_end = .;
    } > DRAM
    __global_pointer$ = _data_start + 0x800;
    .bss (NOLOAD) : ALIGN(4)
    {
        _bss_start = .;
        *(.bss .bss.* .sbss .sbss.*)
        *(COMMON)
        . = ALIGN(4);
        _bss_end = .;
    } > DRAM
    .stack (NOLOAD) : ALIGN(16)
    {
        _stack_guard_start = .;
        . += 256;
        _stack_bottom = .;
        . = ORIGIN(STACK) + LENGTH(STACK);
        _stack_top = .;
    } > STACK
    /DISCARD/ : { *(.comment .note.* .eh_frame*) }
}
ASSERT(_bss_end <= ORIGIN(DRAM) + LENGTH(DRAM), "board data exceeds SRAM slice")
ASSERT(_stack_bottom < _stack_top, "board stack reservation is empty")
ASSERT(ORIGIN(IRAM) + LENGTH(IRAM) - 0x700000 <= ORIGIN(DRAM), "IRAM/DRAM aliases overlap")
ASSERT(_stack_top < 0x3fcde710 - 0x2000, "board overlaps ROM loader stack")
