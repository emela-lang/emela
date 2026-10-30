#!/usr/bin/env python3
"""Check explicit Flow IR/test links against source and compiled Rust test registries."""
import argparse
import json
from pathlib import Path
import subprocess
import sys


def cargo(crate, target, *arguments):
    selection = ["--lib"] if target == "lib" else ["--test", target]
    result = subprocess.run(
        ["cargo", "test", "-p", "emela-codegen-emb", "--manifest-path", str(crate / "Cargo.toml"),
         "--locked", "--offline", *selection, *arguments],
        cwd=crate, text=True, capture_output=True,
    )
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return result.stdout


def listed(output):
    return {line.removesuffix(": test") for line in output.splitlines() if line.endswith(": test")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-built", action="store_true", help="also verify cfg, modules, includes and ignored status against compiled test binaries")
    options = parser.parse_args()
    crate = Path(__file__).resolve().parent.parent
    result = subprocess.run(
        ["bash", str(crate / "tools/inventory.sh"), "tests"],
        cwd=crate, text=True, capture_output=True,
    )
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    print(result.stdout, end="")
    if options.check_built:
        associations = json.loads((crate / "inventory/tests.json").read_text())
        targets = sorted({test["target"] for tests in associations.values() for test in tests})
        present, ignored = {}, {}
        for target in targets:
            present[target] = listed(cargo(crate, target, "--", "--list"))
            ignored[target] = listed(cargo(crate, target, "--", "--ignored", "--list"))
        for variant, tests in associations.items():
            enabled = False
            for test in tests:
                target, name = test["target"], test["name"]
                if name not in present[target]:
                    raise RuntimeError(f"test is not registered in compiled target: {variant}: {target}::{name}")
                enabled |= name not in ignored[target]
            if not enabled:
                raise RuntimeError(f"all compiled associated tests are ignored: {variant}")
        distinct = {(test["target"], test["name"]) for tests in associations.values() for test in tests}
        print(f"Verified {len(associations)} variants and {len(distinct)} distinct test references in {len(targets)} compiled targets.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"test-inventory: {exc}", file=sys.stderr)
        sys.exit(1)
