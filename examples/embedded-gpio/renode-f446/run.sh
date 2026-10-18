#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SAMPLE_DIR="$ROOT/examples/embedded-gpio"
OUT="$SAMPLE_DIR/target/f446-renode"

STM32_RENODE_ROOT="${STM32_RENODE_ROOT:-/home/aoi_nemui/bareruby/stm32_renode}"
ARM_TOOLCHAIN_PATH="${ARM_TOOLCHAIN_PATH:-$STM32_RENODE_ROOT/.tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1}"
STM32CUBE_F4="${STM32CUBE_F4:-$STM32_RENODE_ROOT/.tools/stm32cube/STM32CubeF4-1.28.3}"
RENODE="${RENODE:-$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/renode}"

CC="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-gcc"
SIZE="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-size"
NM="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-nm"
HAL="$STM32CUBE_F4/Drivers/STM32F4xx_HAL_Driver"
CMSIS_DEVICE="$STM32CUBE_F4/Drivers/CMSIS/Device/ST/STM32F4xx"
CMSIS_CORE="$STM32CUBE_F4/Drivers/CMSIS/Include"
GENERATED="$SAMPLE_DIR/target/emela/c-embedded"
ELF="$OUT/firmware.elf"

for path in "$CC" "$SIZE" "$NM" "$RENODE" "$HAL/Src" "$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s"; do
    if [[ ! -e "$path" ]]; then
        printf 'Missing required tool or file: %s\n' "$path" >&2
        exit 1
    fi
done

mkdir -p "$OUT/objects" "$OUT/tmp" "$OUT/renode-config"

cd "$ROOT"
cargo run --locked -p emela-cli -- build examples/embedded-gpio

ARCH_FLAGS=(-mcpu=cortex-m4 -mthumb -mfpu=fpv4-sp-d16 -mfloat-abi=hard)
COMMON_FLAGS=("${ARCH_FLAGS[@]}" -O2 -g3 -ffunction-sections -fdata-sections
    -DSTM32F446xx -DUSE_HAL_DRIVER
    -I"$SCRIPT_DIR" -I"$GENERATED" -I"$HAL/Inc"
    -I"$CMSIS_DEVICE/Include" -I"$CMSIS_CORE")
OBJECTS=()
OBJECT_INDEX=0

compile_c() {
    local source="$1"
    local object="$OUT/objects/$OBJECT_INDEX.o"
    "$CC" "${COMMON_FLAGS[@]}" -std=gnu11 -c "$source" -o "$object" \
        >"$OUT/compile-$OBJECT_INDEX.log" 2>&1 || {
            cat "$OUT/compile-$OBJECT_INDEX.log" >&2
            return 1
        }
    OBJECTS+=("$object")
    OBJECT_INDEX=$((OBJECT_INDEX + 1))
}

compile_asm() {
    local source="$1"
    local object="$OUT/objects/$OBJECT_INDEX.o"
    "$CC" "${COMMON_FLAGS[@]}" -x assembler-with-cpp -c "$source" -o "$object" \
        >"$OUT/compile-$OBJECT_INDEX.log" 2>&1 || {
            cat "$OUT/compile-$OBJECT_INDEX.log" >&2
            return 1
        }
    OBJECTS+=("$object")
    OBJECT_INDEX=$((OBJECT_INDEX + 1))
}

compile_c "$SCRIPT_DIR/board.c"
compile_c "$GENERATED/main.c"
compile_c "$GENERATED/emela_board_hal.c"
compile_c "$CMSIS_DEVICE/Source/Templates/system_stm32f4xx.c"
compile_asm "$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s"
for module in hal hal_cortex hal_rcc hal_rcc_ex hal_gpio hal_pwr hal_pwr_ex hal_flash hal_flash_ex; do
    compile_c "$HAL/Src/stm32f4xx_${module}.c"
done

"$CC" "${ARCH_FLAGS[@]}" -nostartfiles --specs=nano.specs --specs=nosys.specs \
    -T"$SCRIPT_DIR/firmware.ld" -Wl,--gc-sections,-Map="$OUT/firmware.map" \
    "${OBJECTS[@]}" -o "$ELF" >"$OUT/link.log" 2>&1 || {
        cat "$OUT/link.log" >&2
        exit 1
    }
"$SIZE" "$ELF" | tee "$OUT/size.txt"
"$NM" "$ELF" | grep -E 'emela_export_button_changed|EXTI15_10_IRQHandler|HAL_GPIO_EXTI_Callback' \
    | tee "$OUT/interrupt-symbols.txt"

PLATFORM="$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/platforms/cpus/stm32f4.repl"
sed '/^[[:space:]]*ApplySVD[[:space:]]/d' "$PLATFORM" > "$OUT/stm32f4.repl"
cat > "$OUT/machine.repl" <<EOF
using "$OUT/stm32f4.repl"
dwt: Miscellaneous.DWT @ sysbus 0xE0001000
    frequency: 84000000
nvic:
    systickFrequency: 84000000
syscfg: Miscellaneous.STM32_SYSCFG @ sysbus 0x40013800
flash:
    size: 524288
sram:
    size: 131072
led: Miscellaneous.LED @ gpioPortA 5
gpioPortA:
    5 -> led@0
button: Miscellaneous.Button @ gpioPortC 13
    -> gpioPortC@13
    invert: true
EOF

cat > "$OUT/run.resc" <<EOF
mach create
machine LoadPlatformDescription @$OUT/machine.repl
sysbus LoadELF @$ELF
sysbus.cpu VectorTableOffset 0x08000000
sysbus.cpu PerformanceInMips 1
emulation RunFor "0:00:00.100000"
sysbus InfoLog "EMELA_GPIO_INITIAL"
sysbus ReadDoubleWord 0x40020014
sysbus InfoLog "EMELA_BUTTON_INITIAL_INPUT"
sysbus ReadDoubleWord 0x40020810
gpioPortC.button Press
emulation RunFor "0:00:00.010000"
sysbus InfoLog "EMELA_GPIO_PRESSED"
sysbus ReadDoubleWord 0x40020014
sysbus InfoLog "EMELA_BUTTON_PRESSED_INPUT"
sysbus ReadDoubleWord 0x40020810
gpioPortC.button Release
emulation RunFor "0:00:00.010000"
sysbus InfoLog "EMELA_GPIO_RELEASED"
sysbus ReadDoubleWord 0x40020014
sysbus InfoLog "EMELA_BUTTON_RELEASED_INPUT"
sysbus ReadDoubleWord 0x40020810
quit
EOF

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

python3 - "$OUT/renode.log" <<'PY'
import re
import sys

log = open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines()
checks = [
    ("EMELA_GPIO_INITIAL", 0),
    ("EMELA_GPIO_PRESSED", 1 << 5),
    ("EMELA_GPIO_RELEASED", 0),
]
observed = {}
for marker, expected in checks:
    try:
        index = next(i for i, line in enumerate(log) if marker in line)
        value_line = next(line for line in log[index + 1:] if re.fullmatch(r"\s*0x[0-9a-fA-F]+\s*", line))
    except StopIteration:
        raise SystemExit(f"FAIL {marker}: could not read the register value from the log")
    value = int(value_line, 16)
    observed[marker] = value
    if value != expected:
        raise SystemExit(f"FAIL {marker}: expected 0x{expected:08x}, got 0x{value:08x}")

for marker, value in observed.items():
    print(f"PASS {marker}: GPIOA_ODR=0x{value:08x}")
PY
