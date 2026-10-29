#!/usr/bin/env python3
"""Record and verify firmware inputs without executing a shell."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inside(path, parent):
    return path == parent or parent in path.parents


def snapshot(spec, artifacts=True):
    output = Path(spec["output"]).resolve()
    paths = set()

    def add(path):
        # Preserve module names as well as symlink targets in the input inventory.
        paths.update((path.absolute(), path.resolve()))

    root = Path(spec["source_root"]).resolve()
    # Include the module inventory so newly added conflicting modules invalidate a build.
    if spec["single_file"]:
        candidates = root.iterdir()
    else:
        candidates = walk_inputs(root, output)
    for path in candidates:
        if path.is_file() and path.suffix in (".emel", ".c", ".h", ".S", ".s", ".toml"):
            add(path)
    for dependency in spec.get("dependencies", []):
        source = Path(dependency["directory"]).resolve() / "src"
        for path in walk_inputs(source, output):
            if path.is_file() and path.suffix in (".emel", ".c", ".h", ".S", ".s", ".toml"):
                add(path)
    for name in spec["files"]:
        path = Path(name).absolute()
        if not path.is_file():
            raise ValueError(f"build input is missing: {path}; run emela build")
        add(path)
        if path.suffix in (".c", ".S", ".s"):
            for header in walk_inputs(path.parent, output):
                if header.suffix == ".h":
                    add(header)
    if artifacts:
        for name in spec["bsp_files"]:
            add(output / "bsp" / name)
        for path in (output / "generated").iterdir():
            if path.is_file() and path.suffix in (".c", ".h"):
                add(path)
        metadata = firmware(spec)
        add(output / "firmware/firmware.json")
        for name in metadata["artifacts"]:
            add(output / "firmware" / name)
    return {str(path): digest(path) for path in sorted(paths)}


def walk_inputs(root, output):
    def fail(error):
        raise error

    for directory, children, files in os.walk(Path(root).resolve(), followlinks=True, onerror=fail):
        path = Path(directory)
        if path.resolve() in {parent.resolve() for parent in path.parents}:
            raise ValueError(f"cyclic input directory: {path}")
        # Source discovery skips hidden directories, but names such as target are modules.
        children[:] = [name for name in children if not name.startswith(".") and not inside((path / name).resolve(), output)]
        for name in files:
            yield path / name


def firmware(spec):
    output = Path(spec["output"]) / "firmware"
    metadata = json.loads((output / "firmware.json").read_text())
    if metadata.get("board") != spec["board"]:
        raise ValueError("firmware board differs from the selected board; run emela build")
    if metadata.get("schema_version") != 1:
        raise ValueError("unsupported board firmware metadata version")
    names = metadata.get("artifacts")
    if not isinstance(names, list) or not {"firmware.elf", "firmware.bin", "firmware.map"}.issubset(names):
        raise ValueError("firmware metadata is missing required artifacts")
    for name in names:
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or not (output / path).is_file():
            raise ValueError(f"invalid firmware artifact: {name}")
    return metadata


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("prepare", "seal", "verify"))
    parser.add_argument("--spec")
    parser.add_argument("--expected")
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--prepared")
    args = parser.parse_args()
    manifest = Path(args.manifest)
    if args.action == "prepare":
        spec = json.loads(args.expected)
        manifest.write_text(json.dumps({"spec": spec, "sha256": snapshot(spec, artifacts=False)}, indent=2) + "\n")
    elif args.action == "seal":
        spec = json.loads(Path(args.spec).read_text())
        prepared = json.loads(Path(args.prepared).read_text())
        inputs = snapshot(spec, artifacts=False)
        if prepared != {"spec": spec, "sha256": inputs}:
            raise ValueError("project inputs changed during the build; run emela build again")
        hashes = snapshot(spec)
        # Keep the input hashes captured with the artifacts tied to the prepared build inputs.
        if inputs != snapshot(spec, artifacts=False) or any(hashes.get(name) != value for name, value in inputs.items()):
            raise ValueError("project inputs changed while recording firmware; run emela build again")
        state = {"schema_version": 1, "spec": spec, "sha256": hashes}
        manifest.write_text(json.dumps(state, indent=2) + "\n")
    else:
        state = json.loads(manifest.read_text())
        expected = json.loads(args.expected)
        if state.get("schema_version") != 1 or state.get("spec") != expected:
            raise ValueError("selected project or board settings differ from the successful build; run emela build")
        firmware(expected)
        if state["sha256"] != snapshot(expected):
            raise ValueError("firmware, BSP, generated C, or project inputs changed; run emela build")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
