#!/usr/bin/env bash
# Runner for `emela test --device`. Builds the C of the test firmware Emela wrote (the directory
# given as the argument) for NUCLEO-F446RE, runs it in Renode, and copies USART2 output to stdout.
# Build logs go to stderr (stdout is used to read the test results).
set -euo pipefail

GENERATED="$(cd "$1" && pwd)"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$SCRIPT_DIR/../target/f446-test"

STM32_RENODE_ROOT="${STM32_RENODE_ROOT:-/home/aoi_nemui/bareruby/stm32_renode}"
ARM_TOOLCHAIN_PATH="${ARM_TOOLCHAIN_PATH:-$STM32_RENODE_ROOT/.tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1}"
STM32CUBE_F4="${STM32CUBE_F4:-$STM32_RENODE_ROOT/.tools/stm32cube/STM32CubeF4-1.28.3}"
RENODE="${RENODE:-$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/renode}"
# Emulated time long enough for all tests to finish.
RUN_FOR="${EMELA_TEST_RUN_FOR:-0:00:02}"

CC="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-gcc"
HAL="$STM32CUBE_F4/Drivers/STM32F4xx_HAL_Driver"
CMSIS_DEVICE="$STM32CUBE_F4/Drivers/CMSIS/Device/ST/STM32F4xx"
CMSIS_CORE="$STM32CUBE_F4/Drivers/CMSIS/Include"
ELF="$OUT/firmware.elf"

for path in "$CC" "$RENODE" "$HAL/Src" "$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s"; do
    if [[ ! -e "$path" ]]; then
        printf 'Missing required tool or file: %s\n' "$path" >&2
        exit 1
    fi
done

mkdir -p "$OUT/hal" "$OUT/objects" "$OUT/tmp" "$OUT/renode-config"

ARCH_FLAGS=(-mcpu=cortex-m4 -mthumb -mfpu=fpv4-sp-d16 -mfloat-abi=hard)
COMMON_FLAGS=("${ARCH_FLAGS[@]}" -O2 -g3 -ffunction-sections -fdata-sections
    -DSTM32F446xx -DUSE_HAL_DRIVER
    -I"$SCRIPT_DIR" -I"$GENERATED" -I"$HAL/Inc"
    -I"$CMSIS_DEVICE/Include" -I"$CMSIS_CORE")

compile() {
    local source="$1" object="$2"
    shift 2
    "$CC" "${COMMON_FLAGS[@]}" "$@" -c "$source" -o "$object" >&2
}

# Build the HAL and startup code only once (not again each time the tests restart).
HAL_OBJECTS=()
for module in hal hal_cortex hal_rcc hal_rcc_ex hal_gpio hal_pwr hal_pwr_ex hal_flash \
    hal_flash_ex hal_dma hal_uart; do
    object="$OUT/hal/$module.o"
    [[ -e "$object" ]] || compile "$HAL/Src/stm32f4xx_${module}.c" "$object" -std=gnu11
    HAL_OBJECTS+=("$object")
done
[[ -e "$OUT/hal/system.o" ]] ||
    compile "$CMSIS_DEVICE/Source/Templates/system_stm32f4xx.c" "$OUT/hal/system.o" -std=gnu11
[[ -e "$OUT/hal/startup.o" ]] ||
    compile "$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s" "$OUT/hal/startup.o" \
        -x assembler-with-cpp
HAL_OBJECTS+=("$OUT/hal/system.o" "$OUT/hal/startup.o")

rm -f "$OUT"/objects/*.o
OBJECTS=()
compile "$SCRIPT_DIR/test_board.c" "$OUT/objects/test_board.o" -std=gnu11
OBJECTS+=("$OUT/objects/test_board.o")
for source in "$GENERATED"/*.c; do
    object="$OUT/objects/$(basename "$source" .c).o"
    compile "$source" "$object" -std=gnu11
    OBJECTS+=("$object")
done

"$CC" "${ARCH_FLAGS[@]}" -nostartfiles --specs=nano.specs --specs=nosys.specs \
    -T"$SCRIPT_DIR/firmware.ld" -Wl,--gc-sections \
    "${OBJECTS[@]}" "${HAL_OBJECTS[@]}" -o "$ELF" >&2

PLATFORM="$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/platforms/cpus/stm32f4.repl"
sed '/^[[:space:]]*ApplySVD[[:space:]]/d' "$PLATFORM" > "$OUT/stm32f4.repl"
{
    echo "using \"$OUT/stm32f4.repl\""
    echo "nvic:"
    echo "    systickFrequency: 84000000"
    echo "flash:"
    echo "    size: 524288"
    echo "sram:"
    echo "    size: 131072"
} > "$OUT/machine.repl"

rm -f "$OUT/usart2.txt"
{
    echo "mach create"
    echo "machine LoadPlatformDescription @$OUT/machine.repl"
    echo "sysbus LoadELF @$ELF"
    echo "sysbus.cpu VectorTableOffset 0x08000000"
    echo "sysbus.cpu PerformanceInMips 10"
    echo "sysbus.usart2 CreateFileBackend @$OUT/usart2.txt true"
    echo "emulation RunFor \"$RUN_FOR\""
    echo "quit"
} > "$OUT/run.resc"

XDG_CONFIG_HOME="$OUT/renode-config" TMPDIR="$OUT/tmp" \
    "$RENODE" --disable-xwt --console --plain -e "include @$OUT/run.resc" \
    >"$OUT/renode.log" 2>&1 || {
        cat "$OUT/renode.log" >&2
        exit 1
    }
if grep -Eq "There was an error executing command|Unhandled Exception|\[ERROR\]" "$OUT/renode.log"; then
    cat "$OUT/renode.log" >&2
    exit 1
fi

cat "$OUT/usart2.txt" 2>/dev/null || true
