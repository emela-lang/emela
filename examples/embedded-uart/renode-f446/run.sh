#!/usr/bin/env bash
# Builds the UART example for NUCLEO-F446RE and checks the text written to USART2 in Renode.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SAMPLE_DIR="$ROOT/examples/embedded-uart"
SHARED="$ROOT/examples/embedded-gpio/renode-f446"
OUT="$SAMPLE_DIR/target/f446-renode"

STM32_RENODE_ROOT="${STM32_RENODE_ROOT:-/home/aoi_nemui/bareruby/stm32_renode}"
ARM_TOOLCHAIN_PATH="${ARM_TOOLCHAIN_PATH:-$STM32_RENODE_ROOT/.tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1}"
STM32CUBE_F4="${STM32CUBE_F4:-$STM32_RENODE_ROOT/.tools/stm32cube/STM32CubeF4-1.28.3}"
RENODE="${RENODE:-$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/renode}"

CC="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-gcc"
SIZE="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-size"
HAL="$STM32CUBE_F4/Drivers/STM32F4xx_HAL_Driver"
CMSIS_DEVICE="$STM32CUBE_F4/Drivers/CMSIS/Device/ST/STM32F4xx"
CMSIS_CORE="$STM32CUBE_F4/Drivers/CMSIS/Include"
GENERATED="$SAMPLE_DIR/target/emela/c-embedded"
ELF="$OUT/firmware.elf"

for path in "$CC" "$SIZE" "$RENODE" "$HAL/Src" "$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s"; do
    if [[ ! -e "$path" ]]; then
        printf 'Missing required tool or file: %s\n' "$path" >&2
        exit 1
    fi
done

mkdir -p "$OUT/objects" "$OUT/tmp" "$OUT/renode-config"

cd "$ROOT"
cargo run --locked -p emela-cli -- build examples/embedded-uart

ARCH_FLAGS=(-mcpu=cortex-m4 -mthumb -mfpu=fpv4-sp-d16 -mfloat-abi=hard)
COMMON_FLAGS=("${ARCH_FLAGS[@]}" -O2 -g3 -ffunction-sections -fdata-sections
    -DSTM32F446xx -DUSE_HAL_DRIVER
    -I"$SCRIPT_DIR" -I"$GENERATED" -I"$HAL/Inc"
    -I"$CMSIS_DEVICE/Include" -I"$CMSIS_CORE")
OBJECTS=()
OBJECT_INDEX=0

compile() {
    local source="$1"
    shift
    local object="$OUT/objects/$OBJECT_INDEX.o"
    "$CC" "${COMMON_FLAGS[@]}" "$@" -c "$source" -o "$object" \
        >"$OUT/compile-$OBJECT_INDEX.log" 2>&1 || {
            cat "$OUT/compile-$OBJECT_INDEX.log" >&2
            return 1
        }
    OBJECTS+=("$object")
    OBJECT_INDEX=$((OBJECT_INDEX + 1))
}

compile "$SCRIPT_DIR/board.c" -std=gnu11
compile "$GENERATED/main.c" -std=gnu11
compile "$GENERATED/emela_board_hal.c" -std=gnu11
compile "$CMSIS_DEVICE/Source/Templates/system_stm32f4xx.c" -std=gnu11
compile "$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s" -x assembler-with-cpp
for module in hal hal_cortex hal_rcc hal_rcc_ex hal_gpio hal_pwr hal_pwr_ex hal_flash \
    hal_flash_ex hal_dma hal_uart; do
    compile "$HAL/Src/stm32f4xx_${module}.c" -std=gnu11
done

"$CC" "${ARCH_FLAGS[@]}" -nostartfiles --specs=nano.specs --specs=nosys.specs \
    -T"$SHARED/firmware.ld" -Wl,--gc-sections,-Map="$OUT/firmware.map" \
    "${OBJECTS[@]}" -o "$ELF" >"$OUT/link.log" 2>&1 || {
        cat "$OUT/link.log" >&2
        exit 1
    }
"$SIZE" "$ELF" | tee "$OUT/size.txt"

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
    echo "emulation RunFor \"0:00:00.500000\""
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

EXPECTED=$'Hello\ncount 0\ncount 1\ncount 2\n'
ACTUAL="$(cat "$OUT/usart2.txt" 2>/dev/null || true)"
if [[ "$ACTUAL"$'\n' == "$EXPECTED" ]]; then
    printf 'PASS USART2: %q\n' "$ACTUAL"
else
    printf 'FAIL USART2: expected %q, got %q\n' "$EXPECTED" "$ACTUAL" >&2
    exit 1
fi
