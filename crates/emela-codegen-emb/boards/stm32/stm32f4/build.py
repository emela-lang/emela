#!/usr/bin/env python3
"""Build and program a selected STM32F4 profile from generated Emela C."""
import argparse
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import subprocess

HERE = Path(__file__).resolve().parent
if not (HERE / "artifact_manifest.py").is_file():
    sys.path.insert(0, str(HERE.parents[1]))
from artifact_manifest import include_directories, load_manifest

if not (HERE / "stm32_common.py").is_file() and (HERE.parent / "common").is_dir():
    sys.path.insert(0, str(HERE.parent / "common"))
from stm32_common import atomic_json, digest, flash, resolve_tool, run


def selected_profile(path=None):
    profile_path = Path(path) if path else HERE / "profile.json"
    if not profile_path.is_file() and path is None:
        profile_path = HERE / "boards/stm32f446re.json"
    profile = json.loads(profile_path.read_text())
    chip_path = HERE / "chip.json"
    if not chip_path.is_file():
        chip_path = HERE / "chips" / (profile["chip"] + ".json")
    chip = json.loads(chip_path.read_text())
    if profile.get("schema_version") != 1 or profile["chip"] != chip["id"]:
        raise ValueError("invalid STM32F4 board/chip profile")
    if not re.fullmatch(r"[a-z0-9-]+", profile["board"]):
        raise ValueError("invalid STM32F4 board identifier")
    if profile["cpu"] != "cortex-m4" or profile["fpu"] != "fpv4-sp-d16" or profile["float_abi"] != "hard":
        raise ValueError("unsupported STM32F4 CPU/FPU profile")
    if profile["flash_origin"] != 0x08000000 or profile["ram_origin"] != 0x20000000:
        raise ValueError("unsupported STM32F4 Flash/SRAM origins")
    if not 0 < chip["flash_bytes"] <= 2 * 1024 * 1024 or not 0 < chip["ram_bytes"] <= 256 * 1024:
        raise ValueError("invalid STM32F4 Flash/SRAM capacities")
    masks = chip["gpio_pin_masks"]
    if not 1 <= len(masks) <= 11 or any(type(mask) is not int or not 0 <= mask <= 0xffff for mask in masks):
        raise ValueError("invalid STM32F4 bonded GPIO masks")
    vectors = chip["external_vectors"]
    if not 1 <= len(vectors) <= 128 or any(name is not None and not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*_IRQHandler", name) for name in vectors):
        raise ValueError("invalid STM32F4 external interrupt table")
    uart = profile["console_uart"]
    expected_uart = {1: (0x40011000, 2, 4, 7), 2: (0x40004400, 1, 17, 7),
                     3: (0x40004800, 1, 18, 7), 6: (0x40011400, 2, 5, 8)}
    if (uart["base"], uart["apb"], uart["rcc_bit"], uart["af"]) != expected_uart.get(uart["id"]):
        raise ValueError("unsupported STM32F4 console UART registers")
    for pin in (uart["tx_pin"], uart["rx_pin"]):
        if type(pin) is not int or pin < 0 or pin // 16 >= len(masks) or not masks[pin // 16] & (1 << (pin % 16)):
            raise ValueError("console pin is not bonded in the selected MCU package")
    if uart["tx_pin"] == uart["rx_pin"]:
        raise ValueError("console TX and RX must use distinct pins")
    stack, guard = profile["stack_bytes"], profile["guard_bytes"]
    if stack <= 0 or stack % 8 or guard < 32 or guard % 8 or stack + guard >= chip["ram_bytes"]:
        raise ValueError("invalid STM32F4 stack/guard reservation")
    if profile["openocd_target"] != "stm32f4x.cfg":
        raise ValueError("unsupported STM32F4 OpenOCD target")
    return profile, chip


def configure(output, profile, chip):
    uart = profile["console_uart"]
    values = {"GPIO_PORT_COUNT": len(chip["gpio_pin_masks"]),
              "GPIO_PIN_MASKS": "{" + ", ".join(hex(mask) for mask in chip["gpio_pin_masks"]) + "}",
              "CONSOLE_UART_ID": uart["id"], "CONSOLE_UART_BASE": hex(uart["base"]),
              "CONSOLE_UART_APB": uart["apb"], "CONSOLE_UART_RCC_BIT": uart["rcc_bit"],
              "CONSOLE_TX_PIN": uart["tx_pin"], "CONSOLE_RX_PIN": uart["rx_pin"],
              "CONSOLE_UART_AF": uart["af"], "EXTERNAL_IRQ_COUNT": len(chip["external_vectors"])}
    header = ["/* Generated from the selected board and MCU profiles. */", "#ifndef EMELA_STM32F4_BOARD_CONFIG_H",
              "#define EMELA_STM32F4_BOARD_CONFIG_H"]
    header += [f"#define EMELA_{name} {value}" for name, value in values.items()]
    header += ["#endif", ""]
    (output / "board_config.h").write_text("\n".join(header))
    vectors = ["/* External IRQ slots from the selected MCU's official CMSIS table. */"]
    vectors += ["    vector " + name if name else "    .word 0" for name in chip["external_vectors"]]
    (output / "board_vectors.inc").write_text("\n".join(vectors) + "\n")
    linker = (HERE / "firmware.ld.in").read_text()
    substitutions = {"FLASH_ORIGIN": profile["flash_origin"], "FLASH_BYTES": chip["flash_bytes"],
                     "RAM_ORIGIN": profile["ram_origin"], "RAM_BYTES": chip["ram_bytes"],
                     "STACK_BYTES": profile["stack_bytes"], "GUARD_BYTES": profile["guard_bytes"]}
    for name, value in substitutions.items():
        linker = linker.replace("@" + name + "@", hex(value))
    if re.search(r"@[A-Z_]+@", linker):
        raise ValueError("unresolved STM32F4 linker template placeholder")
    (output / "firmware.ld").write_text(linker)


def build(args, profile, chip):
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = output / "firmware.json"
    manifest.unlink(missing_ok=True)
    configure(output, profile, chip)
    generated = args.generated_dir.resolve()
    project = args.project_dir.resolve()
    inputs = load_manifest(generated, require_aliases=True)
    sources = [HERE / "runtime.c", HERE / "validation_stack.c", HERE / "startup.S", *inputs["source"]]
    sources += [source.resolve() for source in args.source]
    for source in sources:
        if not source.is_file():
            raise ValueError(f"build input not found: {source}")
    toolchain = args.toolchain or os.environ.get("ARM_TOOLCHAIN_PATH")
    cc, nm, objcopy, size = [resolve_tool(name, toolchain) for name in ["gcc", "nm", "objcopy", "size"]]
    arch = ["-mcpu=" + profile["cpu"], "-mthumb", "-mfpu=" + profile["fpu"], "-mfloat-abi=" + profile["float_abi"]]
    flags = arch + ["-" + args.optimization, "-g3", "-std=c11", "-ffunction-sections",
                    "-fdata-sections", "-Wall", "-Wextra", "-Werror", "-fstack-usage"]
    includes = {HERE, generated, project, *[source.parent for source in sources], *include_directories(inputs)}
    flags += ["-I" + str(output)] + ["-I" + str(path) for path in sorted(includes - {output})]
    object_dir = output / "objects"
    object_dir.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="compiler-tmp-", dir=output) as temporary:
        environment = dict(os.environ, TMPDIR=temporary)
        with (output / "build.log").open("w") as log:
            compiler = run([cc, "--version"], project, environment, log).splitlines()[0]
            objects = []
            for index, source in enumerate(sources):
                obj = object_dir / f"{index}-{source.name}.o"
                extra = ["-x", "assembler-with-cpp"] if source.suffix.lower() == ".s" else []
                run([cc, *flags, *extra, "-c", source, "-o", obj], project, environment, log)
                objects.append(obj)
            elf = output / "firmware.elf"
            run([cc, *arch, "-nostartfiles", "--specs=nano.specs", "--specs=nosys.specs",
                 "-T" + str(output / "firmware.ld"), "-Wl,--gc-sections", "-Wl,--wrap=main",
                 "-Wl,-Map," + str(output / "firmware.map"), *objects, "-lm", "-o", elf],
                project, environment, log)
            symbols_text = run([nm, "-S", "--defined-only", elf], project, environment, log)
            symbols = {}
            for line in symbols_text.splitlines():
                fields = line.split()
                if len(fields) in [3, 4]:
                    symbols[fields[-1]] = int(fields[0], 16)
            forbidden = re.compile(r"^(?:malloc|calloc|realloc|free|sbrk|_sbrk|_sbrk_r|_malloc_r|_calloc_r|_realloc_r|_free_r)$")
            if any(forbidden.fullmatch(name) for name in symbols):
                raise ValueError("firmware unexpectedly depends on a system allocator")
            raw = output / "firmware.bin"
            run([objcopy, "-O", "binary", elf, raw], project, environment, log)
            size_text = run([size, elf], project, environment, log)
            (output / "size.txt").write_text(size_text)
    image = raw.read_bytes()
    flash_origin, ram_origin = profile["flash_origin"], profile["ram_origin"]
    if len(image) < 8 or len(image) > chip["flash_bytes"]:
        raise ValueError("invalid STM32F4 Flash image size")
    initial_sp, reset = struct.unpack_from("<II", image)
    if initial_sp != ram_origin + chip["ram_bytes"] or not (reset & 1) or not flash_origin <= (reset & ~1) < flash_origin + len(image):
        raise ValueError("Flash image has an invalid initial SP/reset vector")
    if symbols["g_pfnVectors"] != flash_origin or symbols["__emela_vector_table_end__"] - flash_origin != (16 + len(chip["external_vectors"])) * 4:
        raise ValueError("Flash image has an invalid MCU vector table length/address")
    header = elf.read_bytes()[:52]
    if len(header) != 52 or header[:7] != b"\x7fELF\x01\x01\x01" or struct.unpack_from("<H", header, 18)[0] != 40:
        raise ValueError("firmware must be a little-endian 32-bit ARM ELF")
    static_ram = symbols["__emela_static_ram_end__"] - ram_origin
    stack = symbols["__emela_stack_top__"] - symbols["__emela_stack_bottom__"]
    guard = symbols["__emela_stack_guard_end__"] - symbols["__emela_stack_guard_start__"]
    resources = {"flash_bytes": len(image), "flash_capacity": chip["flash_bytes"],
                 "static_ram_bytes": static_ram, "stack_reserved_bytes": stack,
                 "stack_guard_bytes": guard, "ram_capacity": chip["ram_bytes"],
                 "ram_reserved_bytes": static_ram + stack + guard,
                 "external_irq_count": len(chip["external_vectors"]), "system_allocator": False}
    atomic_json(output / "resources.json", resources)
    artifacts = ["firmware.elf", "firmware.bin", "firmware.map", "resources.json", "firmware.ld", "board_config.h", "board_vectors.inc"]
    atomic_json(manifest, {"schema_version": 1, "board": profile["board"],
                          "mcu": chip["mcu"], "physical_board": profile["physical_board"],
                          "boot": "internal-flash-reset-vector", "flash_address": flash_origin,
                          "entry_address": reset, "initial_sp": initial_sp, "compiler": compiler,
                          "optimization": args.optimization, "artifacts": artifacts,
                          "sha256": {name: digest(output / name) for name in artifacts},
                          "resources": resources})
    print(json.dumps({"board": profile["board"], "manifest": str(manifest), "resources": resources}))


def main(profile_path=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--generated-dir", required=True, type=Path)
    build_parser.add_argument("--output-dir", required=True, type=Path)
    build_parser.add_argument("--project-dir", required=True, type=Path)
    build_parser.add_argument("--optimization", choices=["O0", "O2"], default="O2")
    build_parser.add_argument("--source", action="append", type=Path, default=[])
    build_parser.add_argument("--toolchain")
    flash_parser = commands.add_parser("flash")
    flash_parser.add_argument("--output-dir", required=True, type=Path)
    flash_parser.add_argument("--port", help="ST-Link adapter serial number")
    flash_parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "build":
            args.output_dir.resolve().mkdir(parents=True, exist_ok=True)
            (args.output_dir.resolve() / "firmware.json").unlink(missing_ok=True)
        profile, chip = selected_profile(profile_path)
        if args.command == "build":
            build(args, profile, chip)
        else:
            flash(args.output_dir.resolve(), profile["board"], profile["openocd_target"], args.port, args.dry_run)
        return 0
    except (OSError, ValueError, KeyError, TypeError, struct.error, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
