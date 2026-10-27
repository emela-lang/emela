#!/usr/bin/env python3
"""Shared tool invocation and artifact validation for STM32 board packages."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(path)


def resolve_tool(name, toolchain=None):
    if toolchain:
        path = Path(toolchain).expanduser().resolve() / "bin" / ("arm-none-eabi-" + name)
        if not path.is_file():
            raise ValueError(f"ARM toolchain executable not found: {path}")
        return str(path)
    path = shutil.which("arm-none-eabi-" + name)
    if not path:
        raise ValueError("arm-none-eabi toolchain not found; set ARM_TOOLCHAIN_PATH or PATH")
    return path


def run(command, cwd, environment, log):
    result = subprocess.run([str(item) for item in command], cwd=cwd, env=environment,
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log.write(json.dumps([str(item) for item in command]) + "\n" + result.stdout)
    log.flush()
    if result.returncode:
        raise ValueError(f"command failed ({result.returncode}): {command[0]}\n{result.stdout}")
    return result.stdout


def tcl_word(value):
    replacements = {"\\": "\\\\", '"': '\\"', "$": "\\$", "[": "\\[", "]": "\\]",
                    "\n": "\\n", "\r": "\\r", "\t": "\\t"}
    return '"' + "".join(replacements.get(char, char) for char in str(value)) + '"'


def flash(output, board, target, port=None, dry_run=False):
    manifest = json.loads((output / "firmware.json").read_text())
    if manifest.get("schema_version") != 1 or manifest.get("board") != board:
        raise ValueError(f"firmware manifest does not match {board}")
    artifacts = manifest.get("artifacts", [])
    if not isinstance(artifacts, list) or not {"firmware.elf", "firmware.bin", "firmware.map", "resources.json"}.issubset(artifacts):
        raise ValueError("firmware manifest is missing required artifacts")
    for name in artifacts:
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or not (output / path).is_file():
            raise ValueError(f"invalid firmware artifact: {name}")
        if digest(output / name) != manifest.get("sha256", {}).get(name):
            raise ValueError(f"firmware artifact is stale or modified: {name}")
    command = [os.environ.get("OPENOCD", "openocd"), "-f", "interface/stlink.cfg", "-f", "target/" + target]
    if port:
        command += ["-c", "adapter serial " + tcl_word(port)]
    command += ["-c", "program " + tcl_word(output / "firmware.elf") + " verify reset exit"]
    if dry_run:
        print(json.dumps({"board": board, "dry_run": True, "argv": command}, ensure_ascii=False))
        return
    subprocess.run(command, check=True)
