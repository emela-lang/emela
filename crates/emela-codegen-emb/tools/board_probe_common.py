"""Shared bounded process, manifest, and output handling for board probe runners."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import struct
import subprocess
import tempfile

UART_EXPECTED = b"board probe: high\nboard probe: low\n"
ALLOCATOR = re.compile(r"(?:malloc|calloc|realloc|free|sbrk|_sbrk|_sbrk_r|_malloc_r|_calloc_r|_realloc_r|_free_r)")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(path)


def claim_output(output, kind):
    output.mkdir(parents=True, exist_ok=True)
    marker = output / ".emela-board-probe-owner"
    owner = f"emela-codegen-emb board probe {kind} v1\n"
    if any(output.iterdir()) and not (marker.is_file() and marker.read_text() == owner):
        raise ValueError(f"refusing nonempty or unrecognized output directory: {output}")
    lease = (output / ".emela-board-probe-lock").open("a")
    try:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        lease.close()
        raise ValueError(f"output directory is already in use: {output}")
    marker.write_text(owner)
    return lease


def metric(log, label):
    lines = log.splitlines()
    positions = [index for index, line in enumerate(lines)
                 if re.search(re.escape(label) + r"(?![A-Za-z0-9_])", line)]
    if len(positions) != 1:
        raise ValueError(f"expected one {label} label, found {len(positions)}")
    for line in lines[positions[0] + 1:]:
        if "EMELA_" in line:
            break
        if re.fullmatch(r"\s*0x[0-9a-fA-F]+\s*", line):
            return int(line, 16)
    raise ValueError(f"missing value after {label}")


def verify_manifest(firmware):
    manifest = json.loads((firmware / "firmware.json").read_text())
    if not isinstance(manifest, dict) or manifest.get("schema_version") != 1:
        raise ValueError("unsupported firmware manifest schema")
    artifacts = manifest.get("artifacts", [])
    required = {"firmware.elf", "firmware.bin", "firmware.map", "resources.json"}
    if not isinstance(artifacts, list) or not isinstance(manifest.get("sha256"), dict):
        raise ValueError("invalid firmware artifact list or hashes")
    if any(not isinstance(name, str) or Path(name).name != name or name in {".", ".."}
           for name in artifacts):
        raise ValueError("invalid artifact filename in firmware manifest")
    if not required.issubset(artifacts):
        raise ValueError("firmware manifest is missing required artifacts")
    for name in artifacts:
        if digest(firmware / name) != manifest.get("sha256", {}).get(name):
            raise ValueError(f"firmware artifact hash mismatch: {name}")
    resources = json.loads((firmware / "resources.json").read_text())
    if resources != manifest.get("resources"):
        raise ValueError("firmware resource report differs from its manifest")
    return manifest


def elf_symbols(path, machine):
    data = path.read_bytes()
    if len(data) < 52 or data[:7] != b"\x7fELF\x01\x01\x01" or struct.unpack_from("<H", data, 18)[0] != machine:
        raise ValueError(f"expected little-endian ELF32 machine {machine}")
    offset = struct.unpack_from("<I", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 46)
    if size != 40 or offset + count * size > len(data):
        raise ValueError("invalid ELF section table")
    sections = [struct.unpack_from("<10I", data, offset + index * size) for index in range(count)]
    result = {}
    for section in sections:
        if section[1] != 2:
            continue
        if (section[6] >= len(sections) or section[9] != 16 or section[5] % 16
                or section[4] + section[5] > len(data)):
            raise ValueError("invalid ELF symbol table")
        strings = sections[section[6]]
        if strings[4] + strings[5] > len(data):
            raise ValueError("invalid ELF string table")
        names = data[strings[4]:strings[4] + strings[5]]
        for at in range(section[4], section[4] + section[5], section[9]):
            name, value, length, info, _, _ = struct.unpack_from("<IIIBBH", data, at)
            end = names.find(b"\0", name)
            if end >= 0:
                result[names[name:end].decode()] = (value, length, info & 15)
    return result


def descendant_groups(pid):
    table = []
    proc = Path("/proc")
    for entry in proc.iterdir() if proc.is_dir() else []:
        if not entry.name.isdecimal():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            table.append((int(entry.name), int(fields[1]), int(fields[2])))
        except (OSError, ValueError, IndexError):
            continue
    descendants = {pid}
    while True:
        found = {child for child, parent, _ in table if parent in descendants}
        if found <= descendants:
            break
        descendants.update(found)
    return ({pid} | {group for child, _, group in table if child in descendants}) - {os.getpgrp()}


def kill_group(process):
    for group in descendant_groups(process.pid):
        try:
            os.killpg(group, signal.SIGKILL)
        except ProcessLookupError:
            pass


def run_captured(command, *, cwd, env, timeout):
    process = subprocess.Popen(list(map(str, command)), cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, errors="replace", start_new_session=True)
    try:
        output, _ = process.communicate(timeout=timeout)
        return process.returncode, output, False
    except subprocess.TimeoutExpired:
        kill_group(process)
        output, _ = process.communicate()
        return None, output, True


def run(command, output, name, *, cwd, environment=None, timeout=120):
    with tempfile.TemporaryDirectory(prefix=name + "-", dir=output) as temporary:
        env = dict(os.environ if environment is None else environment, TMPDIR=temporary)
        code, text, timed_out = run_captured(command, cwd=cwd, env=env, timeout=timeout)
    (output / (name + ".log")).write_text(text)
    if code != 0 or timed_out:
        raise ValueError(f"{name} failed (exit={code}, timeout={timed_out}): {text[-4000:]}")
    return text
