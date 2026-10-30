#!/usr/bin/env python3
"""Build and flash the RAM-loaded ESP32-C3 board package."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
if not (HERE / "artifact_manifest.py").is_file():
    sys.path.insert(0, str(HERE.parent))
from artifact_manifest import include_directories, load_manifest

BOARD = "esp32c3"
ESPTOOL_VERSION = "4.8.1"
ARTIFACTS = ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def atomic_json(path, data):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(path)


def run(argv, output, label):
    argv = list(map(str, argv))
    with tempfile.TemporaryDirectory(prefix="emela-c3-", dir=output) as temporary:
        result = subprocess.run(argv, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                env={**os.environ, "TMPDIR": temporary}, check=False)
    (output / (label + ".log")).write_text(result.stdout)
    if result.returncode:
        raise ValueError(f"{label} failed ({result.returncode}); see {output / (label + '.log')}")
    return result.stdout


def esptool(output):
    python = os.environ.get("EMELA_ESPTOOL_PYTHON", sys.executable)
    version = run([python, "-c", "import importlib.metadata; print(importlib.metadata.version('esptool'))"],
                  output, "esptool-version").strip()
    if version != ESPTOOL_VERSION:
        raise ValueError(f"esptool {ESPTOOL_VERSION} is required; found {version!r}; "
                         f"install with {python} -m pip install esptool=={ESPTOOL_VERSION}")
    return [python, "-m", "esptool", "--chip", BOARD]


def toolchain():
    root = os.environ.get("EMELA_ESP32C3_TOOLCHAIN")
    gcc = Path(root).expanduser() / "bin/riscv32-esp-elf-gcc" if root else shutil.which("riscv32-esp-elf-gcc")
    if not gcc or not Path(gcc).is_file():
        raise ValueError("riscv32-esp-elf-gcc was not found; set EMELA_ESP32C3_TOOLCHAIN to the toolchain directory")
    binary = Path(gcc).resolve().parent
    tools = {name: binary / ("riscv32-esp-elf-" + name) for name in ["gcc", "nm", "size", "objdump"]}
    if any(not path.is_file() for path in tools.values()):
        raise ValueError(f"incomplete ESP32-C3 toolchain: {binary}")
    return tools


def image_segments(image, expected_entry):
    data = image.read_bytes()
    if len(data) < 24 or data[0] != 0xe9 or data[1] == 0 or data[1] > 16:
        raise ValueError("invalid ESP32-C3 image header")
    if struct.unpack_from("<H", data, 12)[0] != 5:
        raise ValueError("image is not for ESP32-C3 (chip ID 5)")
    entry = struct.unpack_from("<I", data, 4)[0]
    if entry != expected_entry or not 0x40380000 <= entry < 0x40390000:
        raise ValueError("image entry does not match the SRAM ELF entry")
    cursor = 24
    checksum = 0xef
    segments = []
    for _ in range(data[1]):
        if cursor + 8 > len(data):
            raise ValueError("truncated image segment header")
        address, length = struct.unpack_from("<II", data, cursor)
        cursor += 8
        if cursor + length > len(data):
            raise ValueError("truncated image segment")
        if not ((0x40380000 <= address <= address + length <= 0x40390000)
                or (0x3fc90000 <= address <= address + length <= 0x3fc9c000)):
            raise ValueError(f"image segment is outside the board's loadable RAM: {address:#x}/{length:#x}")
        for byte in data[cursor:cursor + length]:
            checksum ^= byte
        segments.append({"address": address, "bytes": length, "file_offset": cursor})
        cursor += length
    checksum_at = cursor | 15
    if checksum_at >= len(data) or data[checksum_at] != checksum:
        raise ValueError("image checksum is invalid")
    end = checksum_at + 1
    if data[23] != 1 or data[end:end + 32] != hashlib.sha256(data[:end]).digest() or len(data) != end + 32:
        raise ValueError("image SHA-256 footer is invalid")
    for index, segment in enumerate(segments):
        for previous in segments[:index]:
            if max(segment["address"], previous["address"]) < min(segment["address"] + segment["bytes"],
                                                                  previous["address"] + previous["bytes"]):
                raise ValueError("image load segments overlap")
    return segments


def build(args):
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = output / "firmware.json"
    manifest.unlink(missing_ok=True)
    for name in ARTIFACTS:
        (output / name).unlink(missing_ok=True)
    generated = args.generated_dir.resolve()
    project = args.project_dir.resolve()
    inputs = load_manifest(generated, require_aliases=True)
    tools = toolchain()
    esp = esptool(output)
    flags = ["-march=rv32imc_zicsr_zifencei", "-mabi=ilp32", "-" + args.optimization, "-g3", "-std=c11",
             "-ffreestanding", "-fno-builtin", "-fno-pic", "-msmall-data-limit=0", "-ffunction-sections",
             "-fdata-sections", "-Wall", "-Wextra", "-Werror", "-fstack-usage",
             "-I", str(generated), "-I", str(project)]
    flags += ["-I" + str(path) for path in sorted(include_directories(inputs) - {generated, project})]
    sources = [*inputs["source"], HERE / "board.c", HERE / "startup.S"]
    for path in args.source:
        source = path.resolve()
        if not source.is_file() or source.suffix not in {".c", ".S", ".s"}:
            raise ValueError(f"source is not a C/assembly file: {source}")
        sources.append(source)
    objects = []
    objdir = output / "objects"
    objdir.mkdir(exist_ok=True)
    for index, source in enumerate(sources):
        obj = objdir / (str(index) + "-" + source.name + ".o")
        run([tools["gcc"], *flags, "-c", source, "-o", obj], output, "compile-" + str(index))
        objects.append(obj)
    elf = output / "firmware.elf"
    run([tools["gcc"], *flags, "-nostartfiles", "--specs=nosys.specs", "-T", HERE / "firmware.ld",
         "-Wl,--gc-sections", "-Wl,--no-relax", "-Wl,-Map," + str(output / "firmware.map"),
         *objects, "-lm", "-lgcc", "-o", elf], output, "link")
    symbols = {}
    for line in run([tools["nm"], "-S", "--defined-only", elf], output, "symbols").splitlines():
        fields = line.split()
        if len(fields) == 4:
            symbols[fields[3]] = {"address": int(fields[0], 16), "bytes": int(fields[1], 16)}
        elif len(fields) == 3:
            symbols[fields[2]] = {"address": int(fields[0], 16), "bytes": 0}
    forbidden = {"malloc", "_malloc_r", "calloc", "_calloc_r", "realloc", "_realloc_r", "free", "_free_r", "sbrk", "_sbrk", "_sbrk_r"}
    if forbidden.intersection(symbols):
        raise ValueError("the linked firmware unexpectedly uses a libc heap allocator")
    elf_data = elf.read_bytes()
    if elf_data[:7] != b"\x7fELF\x01\x01\x01" or struct.unpack_from("<H", elf_data, 18)[0] != 243:
        raise ValueError("expected a little-endian RISC-V ELF32")
    entry = struct.unpack_from("<I", elf_data, 24)[0]
    sections = run([tools["size"], elf], output, "size").splitlines()[1].split()
    resources = {name: int(value) for name, value in zip(["text", "data", "bss_including_reserved_stack"], sections[:3])}
    resources.update({"execution": "ROM-loads-RAM-segments-from-flash", "physical_sram_limit_bytes": 131072,
                      "stack_reserved_bytes": 16384, "stack_guard_bytes": 256,
                      "static_data_bytes": symbols["_bss_end"]["address"] - symbols["_data_start"]["address"],
                      "runtime_pool_symbols": {name: value for name, value in symbols.items()
                                               if 0x3fc90000 <= value["address"] < 0x3fc9c000
                                               and name.startswith(("emela_heap", "emela_fibers", "emela_frames", "emela_tasks", "emela_events", "emela_deferred", "emela_resume"))},
                      "observed_stack_bytes": None})
    run([tools["objdump"], "-h", elf], output, "sections")
    binary = output / "firmware.bin"
    run([*esp, "elf2image", "--flash_mode", "dio", "--flash_freq", "40m", "--flash_size", "4MB",
         "--output", binary, elf], output, "elf2image")
    segments = image_segments(binary, entry)
    run([*esp, "image_info", "--version", "2", binary], output, "image-info")
    resources["flash_image_bytes"] = binary.stat().st_size
    resources["code_and_rodata_sram_bytes"] = int(sections[0])
    resources["total_reserved_sram_bytes"] = int(sections[0]) + resources["static_data_bytes"] + 16384
    atomic_json(output / "resources.json", resources)
    atomic_json(manifest, {"schema_version": 1, "board": BOARD, "artifacts": ARTIFACTS,
                          "sha256": {name: digest(output / name) for name in ARTIFACTS},
                          "optimization": args.optimization, "entry_address": entry, "flash_address": 0,
                          "flash_mode": "dio", "flash_frequency": "40m", "flash_size": "4MB",
                          "esptool_version": ESPTOOL_VERSION, "segments": segments, "resources": resources})
    print(f"ESP32-C3 firmware: {binary} ({binary.stat().st_size} bytes, flash offset 0x0)")


def flash(args):
    output = args.output_dir.resolve()
    manifest = json.loads((output / "firmware.json").read_text())
    if manifest.get("schema_version") != 1 or manifest.get("board") != BOARD or manifest.get("flash_address") != 0:
        raise ValueError("firmware manifest does not identify an ESP32-C3 image at flash offset 0x0")
    if manifest.get("artifacts") != ARTIFACTS:
        raise ValueError("firmware artifact list is invalid")
    for name in ARTIFACTS:
        if digest(output / name) != manifest["sha256"].get(name):
            raise ValueError(f"firmware artifact hash mismatch: {name}")
    image_segments(output / "firmware.bin", manifest["entry_address"])
    if not args.dry_run and not args.port:
        raise ValueError("ESP32-C3 flashing requires --port (for example /dev/ttyUSB0)")
    python = os.environ.get("EMELA_ESPTOOL_PYTHON", sys.executable)
    command = [python, "-m", "esptool", "--chip", BOARD]
    if args.port:
        command += ["--port", args.port]
    command += ["--before", "default_reset", "--after", "hard_reset", "write_flash",
                "--flash_mode", "dio", "--flash_freq", "40m", "--flash_size", "4MB", "0x0", str(output / "firmware.bin")]
    if args.dry_run:
        print(json.dumps({"dry_run": True, "board": BOARD, "argv": command}, ensure_ascii=False, indent=2))
        return
    esptool(output)
    run(command, output, "flash")
    print("ESP32-C3 firmware written and reset")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    builder = commands.add_parser("build")
    builder.add_argument("--generated-dir", type=Path, required=True)
    builder.add_argument("--output-dir", type=Path, required=True)
    builder.add_argument("--project-dir", type=Path, required=True)
    builder.add_argument("--optimization", choices=["O0", "O2"], default="O2")
    builder.add_argument("--source", type=Path, action="append", default=[])
    flasher = commands.add_parser("flash")
    flasher.add_argument("--output-dir", type=Path, required=True)
    flasher.add_argument("--port")
    flasher.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        (build if args.command == "build" else flash)(args)
    except (OSError, ValueError, KeyError, IndexError) as error:
        print(f"ESP32-C3: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
