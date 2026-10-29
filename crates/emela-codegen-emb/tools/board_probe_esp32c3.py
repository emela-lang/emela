#!/usr/bin/env python3
"""Verify an ESP32-C3 flash image from a real ROM reset in Espressif QEMU."""
import argparse
import json
import os
from pathlib import Path
import re
import selectors
import struct
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
from board_probe_common import ALLOCATOR, UART_EXPECTED, atomic_json, claim_output, digest, elf_symbols, kill_group, run, verify_manifest
QEMU_VERSION = "esp_develop_9.2.2_20260417"
METRICS = ["board_app_completed", "board_app_result", "board_failures", "board_ticks",
           "board_notifications", "board_rejected_notifications", "board_uart_bytes", "board_stack_used", "board_stack_guard_ok",
           "board_trap_cause", "board_trap_pc"]


class Monitor:
    def __init__(self, process, trace, timeout):
        self.process = process
        self.trace = trace
        self.timeout = timeout
        self.selector = selectors.DefaultSelector()
        self.selector.register(process.stdout, selectors.EVENT_READ)
        self.buffer = b""
        self.read()
        self.command("qmp_capabilities")

    def read(self):
        deadline = time.monotonic() + self.timeout
        while b"\n" not in self.buffer:
            if not self.selector.select(max(0, deadline - time.monotonic())):
                raise TimeoutError("QEMU monitor did not respond")
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise ValueError("QEMU exited before completing the flash scenario")
            self.buffer += chunk
        line, self.buffer = self.buffer.split(b"\n", 1)
        value = json.loads(line)
        self.trace.append({"response": value})
        return value

    def command(self, name, arguments=None):
        request = {"execute": name}
        if arguments is not None:
            request["arguments"] = arguments
        self.trace.append({"request": request})
        self.process.stdin.write(json.dumps(request).encode() + b"\n")
        self.process.stdin.flush()
        while True:
            response = self.read()
            if "error" in response:
                raise ValueError(f"QEMU monitor: {response['error']}")
            if "return" in response:
                return response["return"]

    def word(self, address):
        value = self.command("human-monitor-command", {"command-line": f"xp /1wx 0x{address:x}"})
        match = re.search(r":\s*(0x[0-9a-f]+)", value)
        if not match:
            raise ValueError(f"invalid QEMU memory observation: {value!r}")
        return int(match[1], 16)


def verify(args, report, output):
    firmware = args.firmware_dir.resolve()
    manifest = verify_manifest(firmware)
    if manifest.get("board") != "esp32c3" or manifest.get("flash_address") != 0:
        raise ValueError("expected an ESP32-C3 image at flash offset 0x0")
    symbols = elf_symbols(firmware / "firmware.elf", 243)
    if any(ALLOCATOR.fullmatch(name) for name in symbols):
        raise ValueError("firmware unexpectedly depends on a system allocator")
    if symbols.get("emela_heap_storage", (0, 0))[1] != 2048:
        raise ValueError("the probe must retain its 2048-byte fixed heap")
    for name in METRICS:
        if name not in symbols or symbols[name][1:] != (4, 1):
            raise ValueError(f"missing 32-bit board metric: {name}")
    version = run([args.qemu, "--version"], output, "qemu-version", cwd=output, timeout=args.timeout).strip()
    if QEMU_VERSION not in version:
        raise ValueError(f"expected the fixed Espressif QEMU version {QEMU_VERSION}; found {version!r}")
    image = (firmware / "firmware.bin").read_bytes()
    if len(image) > 4 * 1024 * 1024:
        raise ValueError("firmware image exceeds 4 MB flash")
    (output / "flash.bin").write_bytes(image + b"\xff" * (4 * 1024 * 1024 - len(image)))
    uart = output / "uart.log"
    uart.unlink(missing_ok=True)
    command = [str(args.qemu.resolve()), "-machine", "esp32c3", "-display", "none", "-monitor", "none",
               "-serial", "file:" + str(uart), "-qmp", "stdio", "-drive",
               "file=" + str(output / "flash.bin").replace(",", ",,") + ",if=mtd,format=raw", "-no-reboot"]
    report.update({"qemu": version, "argv": command, "firmware": str(firmware),
                   "firmware_image_sha256": digest(firmware / "firmware.bin"),
                   "boot": "built-in ROM reset -> flash offset 0x0 -> RAM segments -> firmware entry",
                   "resources": manifest["resources"], "monitor_trace": []})
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="emela-c3-qemu-", dir=output) as temporary, (output / "qemu.log").open("w") as log:
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
                                   env={**os.environ, "TMPDIR": temporary}, start_new_session=True)
        try:
            monitor = Monitor(process, report["monitor_trace"], min(10, args.timeout))
            while monitor.word(symbols["board_app_completed"][0]) == 0:
                if monitor.word(symbols["board_failures"][0]):
                    raise ValueError("the application reported a board defect")
                if time.monotonic() - started > args.timeout:
                    raise TimeoutError("the application did not complete before the timeout")
                time.sleep(0.1)
            monitor.command("stop")
            metrics = {name: monitor.word(symbols[name][0]) for name in METRICS}
            metrics["board_app_result"] = struct.unpack("<i", struct.pack("<I", metrics["board_app_result"]))[0]
            registers = monitor.command("human-monitor-command", {"command-line": "info registers"})
            report["metrics"] = metrics
            report["registers"] = registers
            checks = {"board_app_completed": 1, "board_app_result": 42,
                      "board_failures": 0, "board_stack_guard_ok": 1, "board_rejected_notifications": 0,
                      "board_ticks": 10, "board_notifications": 1, "board_uart_bytes": len(UART_EXPECTED)}
            for name, expected in checks.items():
                if metrics[name] != expected:
                    raise ValueError(f"{name}: observed {metrics[name]}, expected {expected}")
            if not 0 < metrics["board_stack_used"] <= 16384 - 256:
                raise ValueError("observed stack use is outside its reservation")
            vector = re.search(r"\bmtvec\s+([0-9a-f]+)", registers)
            if not vector or int(vector[1], 16) != (symbols["_vectors"][0] | 1):
                raise ValueError("the CPU did not install the hardware-mode-1 vector table")
            if metrics["board_notifications"] and metrics["board_trap_cause"] != 0x80000001:
                raise ValueError("clock completion did not pass through the real ESP32-C3 interrupt")
            monitor.command("quit")
            process.wait(timeout=5)
            if process.returncode != 0:
                raise ValueError(f"QEMU exited with status {process.returncode}")
        finally:
            if process.poll() is None:
                kill_group(process)
                process.wait(timeout=5)
    text = uart.read_text(errors="replace")
    if "SPI_FAST_FLASH_BOOT" not in text or "load:0x40380000" not in text or "entry 0x40380000" not in text:
        raise ValueError("the UART log does not contain the ROM flash load sequence")
    if text.count("POWERON") != 1 or text.count("entry 0x40380000") != 1:
        raise ValueError("the application reset unexpectedly")
    application = [line for line in text.splitlines() if line.startswith("board probe:")]
    if application != UART_EXPECTED.decode().splitlines():
        raise ValueError(f"unexpected application UART lines: {application!r}")
    report["gpio_verification"] = {
        "status": "unavailable",
        "reason": "The pinned QEMU GPIO model ignores output writes and returns zero except for the strap register",
        "source": "https://github.com/espressif/qemu/blob/esp-develop-9.2.2-20260417/hw/gpio/esp32_gpio.c",
        "verified_transitions": [],
    }
    report["uart_sha256"] = digest(uart)
    report["resources"]["observed_stack_bytes"] = report["metrics"]["board_stack_used"]
    report["wall_seconds"] = round(time.monotonic() - started, 3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware-dir", type=Path, required=True)
    parser.add_argument("--qemu", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    output = args.output.resolve()
    try:
        lease = claim_output(output, "esp32c3")
    except (OSError, ValueError) as error:
        parser.error(str(error))
    report = {"schema_version": 1, "status": "running", "board": "esp32c3",
              "loads_elf": False, "sets_pc_sp": False}
    atomic_json(output / "report.json", report)
    try:
        if not 0 < args.timeout <= 3600:
            raise ValueError("timeout must be within (0, 3600]")
        verify(args, report, output)
        report["status"] = "passed"
    except (OSError, ValueError, KeyError, struct.error, subprocess.SubprocessError, TimeoutError) as error:
        report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        atomic_json(output / "report.json", report)
        lease.close()
    print(f"{report['status'].upper()}: ESP32-C3 ROM/Flash probe, {output / 'report.json'}")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
