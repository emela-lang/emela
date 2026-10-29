#!/usr/bin/env python3
"""Observe the Rust board probe from raw STM32F4 Flash reset in Renode."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True

from board_probe_common import atomic_json, claim_output, digest, elf_symbols, metric, run_captured, verify_manifest, UART_EXPECTED

CRATE = Path(__file__).resolve().parents[1]
PLATFORMS = {
    "cpu-f407": ("stm32f407vg", "cpu", ["cpus/stm32f4.repl"]),
    "cpu-f412": ("stm32f412zg", "cpu", ["cpus/stm32f412.repl"]),
    "cpu-f429": ("stm32f429zi", "cpu", ["cpus/stm32f429.repl"]),
    "discovery": ("stm32f4-discovery", "board", ["boards/stm32f4_discovery.repl"]),
    "discovery-additional-gpios": ("stm32f4-discovery", "board-with-fragment", ["boards/stm32f4_discovery.repl", "boards/stm32f4_discovery-additional_gpios.repl"]),
    "discovery-bb": ("stm32f4-discovery", "board-with-fragment", ["boards/stm32f4_discovery.repl", "boards/stm32f4_discovery-bb.repl"]),
    "discovery-kit": ("stm32f4-discovery", "composed-board", ["boards/stm32f4_discovery-kit.repl"]),
    "cpu-f446re": ("stm32f446re", "cpu-profile-on-generic-model", ["cpus/stm32f4.repl"]),
    "cpu-f446-compat": ("stm32f446", "compatibility-profile-on-generic-model", ["cpus/stm32f4.repl"]),
    "nucleo-f446re": ("nucleo-f446re", "board-profile-on-generic-model", ["cpus/stm32f4.repl"]),
}
EXPECTED_FILES = {
    "cpus/stm32f4.repl", "cpus/stm32f412.repl", "cpus/stm32f429.repl",
    "boards/stm32f4_discovery.repl", "boards/stm32f4_discovery-kit.repl",
    "boards/stm32f4_discovery-additional_gpios.repl", "boards/stm32f4_discovery-bb.repl",
}
def inventory(renode):
    root = renode.resolve().parent / "platforms"
    found = {str(path.relative_to(root)) for path in root.rglob("*stm32f4*.repl")}
    if found != EXPECTED_FILES:
        raise ValueError(f"Renode F4 inventory changed: missing={sorted(EXPECTED_FILES - found)}, extra={sorted(found - EXPECTED_FILES)}")
    return {name: digest(root / name) for name in sorted(found)}


def copy_platforms(renode, output, monitor):
    root = renode.parent / "platforms"
    for name in sorted(EXPECTED_FILES):
        text = (root / name).read_text()
        text = "\n".join(line for line in text.splitlines() if not re.match(r"\s*ApplySVD\s", line)) + "\n"
        # F429's metadata-only init block becomes empty after removing ApplySVD.
        text = re.sub(r"\nsysbus:\n    init:\n\s*\Z", "\n", text)
        if name == "cpus/stm32f4.repl":
            registration = "flash: Memory.MappedMemory @ sysbus 0x08000000"
            if text.count(registration) != 1:
                raise ValueError("unexpected generic STM32F4 Flash registration")
            text = text.replace(registration, "flash: Memory.MappedMemory @ {\n        sysbus 0x08000000;\n        sysbus 0x00000000\n    }")
        def replace_using(match):
            dependency = match[1]
            relative = Path(name).parent / dependency if dependency.startswith(".") else Path(dependency).relative_to("platforms")
            if str(relative) not in EXPECTED_FILES:
                raise ValueError(f"unexpected platform dependency: {dependency}")
            return 'using "' + str(monitor / "platforms" / relative) + '"'
        text = re.sub(r'using "([^"]+)"', replace_using, text)
        target = output / "platforms" / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)


def wire_pin(pin):
    return "gpioPort" + chr(ord("A") + pin // 16), pin % 16


def observe(args):
    output = args.output.resolve()
    lease = claim_output(output, "stm32f4")
    report = {"schema_version": 1, "status": "running", "platform": args.platform,
              "boot": "raw-flash-reset", "loads_elf": False, "sets_pc_sp": False,
              "sets_vtor": False, "model_transformations": ["remove network-only ApplySVD metadata", "map BOOT0=0 Flash alias", "restrict Flash/SRAM to selected real MCU", "set SysTick frequency to reset HSI 16MHz", "attach external LED and transition observer endpoints"]}
    atomic_json(output / "report.json", report)
    monitor_directory = None
    try:
        if not 0 < args.timeout <= 3600:
            raise ValueError("timeout must be within (0, 3600]")
        for name in ["uart.log", "renode.log", "model.log", "run.resc"]:
            (output / name).unlink(missing_ok=True)
        firmware = args.firmware_dir.resolve()
        manifest = verify_manifest(firmware)
        board, kind, platforms = PLATFORMS[args.platform]
        report.update({"board": board, "platform_kind": kind, "platform_files": platforms})
        if manifest.get("board") != board or manifest.get("schema_version") != 1:
            raise ValueError(f"expected a {board} firmware manifest")
        renode = args.renode.expanduser().resolve()
        report["platform_source_sha256"] = inventory(renode)
        profile_path = firmware.parent / "bsp/profile.json"
        if not profile_path.is_file():
            raise ValueError("expected the Rust probe's bundled bsp/profile.json")
        profile = json.loads(profile_path.read_text())
        chip = json.loads((profile_path.parent / "chip.json").read_text())
        if profile["board"] != board:
            raise ValueError("BSP profile does not match firmware")
        report["profile"] = profile
        report["chip"] = chip
        entries = elf_symbols(firmware / "firmware.elf", 40)
        symbols = {name: entry[0] for name, entry in entries.items()}
        pools = {name: entry[1] for name, entry in entries.items()
                 if name.startswith(("emela_heap_storage", "emela_fibers", "emela_frames", "emela_tasks", "emela_events", "emela_deferred_slots", "emela_resume_transfer"))}
        if pools.get("emela_heap_storage") != 2048:
            raise ValueError("the board probe must retain its fixed 2048-byte Emela heap")
        if any(re.fullmatch(r"(?:malloc|calloc|realloc|free|sbrk|_sbrk|_sbrk_r|_malloc_r|_calloc_r|_realloc_r|_free_r)", name) for name in symbols):
            raise ValueError("firmware unexpectedly links a system allocator")
        shutil.copyfile(firmware / "firmware.bin", output / "firmware.bin")
        # Keep monitor command paths free of spaces even when the caller's TMPDIR has them.
        monitor_directory = Path(tempfile.mkdtemp(prefix="emela-f4-monitor-", dir="/tmp"))
        monitor = monitor_directory / "artifacts"
        monitor.symlink_to(output)
        copy_platforms(renode, output, monitor)
        shutil.copyfile(CRATE / "tools/models/STM32F4ActuatorObserver.cs", output / "STM32F4ActuatorObserver.cs")
        probe_pin = 5 if board in {"stm32f446", "stm32f446re", "nucleo-f446re"} else 60
        port, number = wire_pin(probe_pin)
        machine = "".join(f'using "{monitor}/platforms/{name}"\n' for name in platforms)
        machine += f'nvic:\n    systickFrequency: 16000000\nflash:\n    size: {chip["flash_bytes"]}\nsram:\n    size: {chip["ram_bytes"]}\n'
        machine += 'probeLed: Miscellaneous.LED @ sysbus\nprobeObserver: Miscellaneous.STM32F4ActuatorObserver @ sysbus\n'
        machine += f'{port}:\n    {number} -> ' + ('UserLED@0 | ' if args.platform.startswith("discovery") else '') + 'probeLed@0 | probeObserver@0\n'
        (output / "machine.repl").write_text(machine)
        uart_path = output / "uart.log"
        uart = "usart" + str(profile["console_uart"]["id"])
        script = f'logFile @{monitor}/model.log\nlogLevel 1 file\nlogLevel 3 console\ninclude @{monitor}/STM32F4ActuatorObserver.cs\nmach create\nmachine LoadPlatformDescription @{monitor}/machine.repl\nsysbus LoadBinary @{monitor}/firmware.bin 0x08000000\nsysbus.cpu PerformanceInMips 100\nsysbus.{uart} CreateFileBackend @{monitor}/uart.log true\n'
        for name in ["board_boot_data", "board_boot_bss"]:
            script += f'sysbus WriteDoubleWord 0x{symbols[name]:x} 0xdeadbeef\n'
        # Logger messages are asynchronous; keep observation labels on the monitor stream.
        script += 'sysbus.cpu Reset\n'
        script += 'emulation RunFor "0:00:00.200000"\n'
        names = ["board_app_completed", "board_app_result", "board_app_failures", "board_ticks",
                 "board_notifications", "board_rejected_notifications", "board_uart_bytes", "board_boot_data", "board_boot_bss",
                 "board_boot_initialized", "emela_validation_stack_used", "emela_validation_stack_capacity",
                 "emela_validation_stack_guard_ok", "emela_validation_stack_samples"]
        for name in names:
            script += f'echo "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{symbols[name]:x}\n'
        console = profile["console_uart"]
        register_addresses = {"PROBE_ODR": 0x40020014 + (probe_pin // 16) * 0x400,
                              "PROBE_MODER": 0x40020000 + (probe_pin // 16) * 0x400,
                              "RCC_AHB1ENR": 0x40023830,
                              "RCC_CONSOLE_APBENR": 0x40023840 if console["apb"] == 1 else 0x40023844}
        for direction in ["tx", "rx"]:
            pin = console[direction + "_pin"]
            base = 0x40020000 + (pin // 16) * 0x400
            register_addresses[direction + "_MODER"] = base
            register_addresses[direction + "_AFR"] = base + 0x20 + 4 * ((pin % 16) // 8)
        for name, address in register_addresses.items():
            script += f'echo "EMELA_REGISTER_{name}"\nsysbus ReadDoubleWord 0x{address:x}\n'
        for key, command in [("STATE", "probeObserver State"), ("TRANSITIONS", "probeObserver TransitionCount"), ("LED", "probeLed State")]:
            script += f'echo "EMELA_EXTERNAL_{key}"\nsysbus.{command}\n'
        (output / "run.resc").write_text(script + 'quit\n')
        with tempfile.TemporaryDirectory(prefix="renode-tmp-", dir=output) as temporary:
            environment = dict(os.environ, TMPDIR=temporary, XDG_CONFIG_HOME=str(output / "renode-config"))
            code, log, timed_out = run_captured([str(renode), "--disable-xwt", "--console", "--plain", "-e", "include @" + str(monitor / "run.resc")], cwd=CRATE, env=environment, timeout=args.timeout)
        (output / "renode.log").write_text(log)
        model_log_path = output / "model.log"
        model_log = model_log_path.read_text(encoding="utf-8-sig") if model_log_path.is_file() else ""
        report["renode_process"] = {"exit_code": code, "timed_out": timed_out}
        if code != 0 or timed_out or re.search(r"\[(?:ERROR|WARNING)\]|There was an error executing command|Unhandled Exception", log + "\n" + model_log):
            raise ValueError(f"Renode failed or reported a warning (exit={code}, timeout={timed_out})")
        if not model_log_path.is_file():
            raise ValueError("missing separate Renode model log")
        report["model_log_sha256"] = digest(model_log_path)
        version = re.search(r"Renode, version (\S+)", log)
        if not version or version[1] != "1.16.1":
            raise ValueError("this platform matrix is pinned to Renode 1.16.1")
        report["renode_version"] = version[1]
        values = {name: metric(log, "EMELA_METRIC_" + name) for name in names}
        registers = {name: metric(log, "EMELA_REGISTER_" + name) for name in register_addresses}
        if not registers["RCC_CONSOLE_APBENR"] & (1 << console["rcc_bit"]):
            raise ValueError("console UART peripheral clock is disabled")
        for direction in ["tx", "rx"]:
            pin = console[direction + "_pin"]
            if ((registers[direction + "_MODER"] >> (2 * (pin % 16))) & 3) != 2:
                raise ValueError(f"console {direction} pin is not configured as alternate function")
            if ((registers[direction + "_AFR"] >> (4 * (pin % 8))) & 15) != console["af"]:
                raise ValueError(f"console {direction} pin alternate function differs from profile")
            if not registers["RCC_AHB1ENR"] & (1 << (pin // 16)):
                raise ValueError(f"console {direction} GPIO peripheral clock is disabled")
        expected = {"board_app_completed": 1, "board_app_result": 42, "board_app_failures": 0,
                    "board_notifications": 1, "board_rejected_notifications": 0, "board_boot_data": 0x454d4244,
                    "board_boot_bss": 0x4253534f, "board_boot_initialized": 1, "emela_validation_stack_guard_ok": 1}
        for name, value in expected.items():
            if values[name] != value:
                raise ValueError(f"{name}: {values[name]} != {value}")
        if values["board_ticks"] != 10 or not 0 < values["emela_validation_stack_used"] <= values["emela_validation_stack_capacity"] or values["emela_validation_stack_samples"] < 2:
            raise ValueError("timer did not advance exactly 10 ticks or stack observation is invalid")
        uart_bytes = uart_path.read_bytes()
        if uart_bytes != UART_EXPECTED or values["board_uart_bytes"] != len(UART_EXPECTED):
            raise ValueError(f"unexpected board probe UART bytes: {uart_bytes!r}")
        if registers["PROBE_ODR"] & (1 << number) or ((registers["PROBE_MODER"] >> (2 * number)) & 3) != 1:
            raise ValueError("the probe pin is not configured as an output with a final low state")
        transitions = [(0, 1), (0, 0)]
        actual_transitions = [(int(pin), int(level)) for pin, level in re.findall(r"EMELA_F4_GPIO pin=(\d+) level=(\d+)", model_log)]
        if actual_transitions != transitions:
            raise ValueError(f"unexpected external GPIO endpoint transitions: {actual_transitions!r}")
        def property_value(key):
            if key in {"STATE", "TRANSITIONS"}:
                return metric(log, "EMELA_EXTERNAL_" + key)
            match = re.search(r"EMELA_EXTERNAL_" + key + r"[^\n]*\n\s*(True|False)\s*\n", log)
            if not match:
                raise ValueError(f"missing external endpoint property {key}")
            return int(match[1] == "True")
        if property_value("STATE") != 0 or property_value("TRANSITIONS") != len(transitions) or property_value("LED") != 0:
            raise ValueError("external endpoint final state/count did not match")
        report.update({"status": "passed", "metrics": values, "uart_sha256": digest(uart_path), "gpio_pin": probe_pin,
                       "external_gpio_transitions": actual_transitions, "external_gpio_transition_count": len(transitions),
                       "resources": manifest["resources"], "console_registers": registers, "fixed_pool_symbol_bytes": pools,
                       "flash_sha256": manifest["sha256"]["firmware.bin"]})
        print(f"PASSED {args.platform}/{manifest['optimization']}: result 42, 10 IRQ ticks, {len(transitions)} GPIO transitions")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, TimeoutError) as error:
        report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        atomic_json(output / "report.json", report)
        if monitor_directory:
            shutil.rmtree(monitor_directory)
        lease.close()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--renode", type=Path, default=os.environ.get("RENODE"))
    parser.add_argument("--toolchain", type=Path, default=os.environ.get("ARM_TOOLCHAIN_PATH"))
    parser.add_argument("--platform", choices=PLATFORMS, default="cpu-f446re")
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    if not args.renode:
        parser.error("set RENODE or pass --renode")
    try:
        report = observe(args)
        return 0 if report["status"] == "passed" else 1
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
