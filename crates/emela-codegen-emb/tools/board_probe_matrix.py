#!/usr/bin/env python3
"""Build the Rust board probe and observe the pinned STM32F4 Renode matrix."""
import argparse
import concurrent.futures
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

sys.dont_write_bytecode = True

from board_probe_common import atomic_json, claim_output, digest, run, verify_manifest
from board_probe_stm32f4 import CRATE, PLATFORMS, inventory

ROOT = CRATE.parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", required=True, type=Path, help="compiled Rust board_probe example")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--renode", type=Path, default=os.environ.get("RENODE"))
    parser.add_argument("--toolchain", type=Path, default=os.environ.get("ARM_TOOLCHAIN_PATH"))
    parser.add_argument("--platform", choices=PLATFORMS, action="append")
    parser.add_argument("--optimization", choices=["O0", "O2"], action="append")
    parser.add_argument("--jobs", type=int, choices=[1, 2], default=2)
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    output = args.output.resolve()
    try:
        lease = claim_output(output, "matrix")
    except (OSError, ValueError) as error:
        parser.error(str(error))
    selected = list(dict.fromkeys(args.platform or PLATFORMS))
    optimizations = list(dict.fromkeys(args.optimization or ["O0", "O2"]))
    report = {"schema_version": 1, "status": "running", "platforms": selected,
              "optimizations": optimizations, "builds": [], "cases": [],
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    atomic_json(output / "report.json", report)
    try:
        if not 0 < args.timeout <= 3600 or not args.renode:
            raise ValueError("set RENODE/--renode and a timeout within (0, 3600]")
        probe = args.probe.expanduser().resolve()
        if not probe.is_file() or not os.access(probe, os.X_OK):
            raise ValueError(f"probe is not an executable file: {probe}")
        renode = args.renode.expanduser().resolve()
        report["probe"] = {"path": str(probe), "sha256": digest(probe)}
        report["platform_source_sha256"] = inventory(renode)
        # Each invocation has fresh projects, artifacts, and logs even in a reused owned output.
        run_root = output / "runs" / uuid.uuid4().hex
        run_root.mkdir(parents=True)
        report["run_directory"] = str(run_root)
        builds = {}
        boards = list(dict.fromkeys(PLATFORMS[platform][0] for platform in selected))
        report["expected_builds"] = len(boards) * len(optimizations)
        report["expected_observations"] = len(selected) * len(optimizations)
        for optimization in optimizations:
            for board in boards:
                build_output = run_root / "builds" / board / optimization
                build_output.mkdir(parents=True)
                project = build_output / "project"
                run([probe, "--board", board, "--out-dir", project], build_output, "generate",
                    cwd=ROOT, timeout=args.timeout)
                command = [sys.executable, project / "bsp/build.py", "build",
                           "--generated-dir", project / "generated", "--project-dir", project,
                           "--output-dir", project / "artifacts", "--optimization", optimization]
                if args.toolchain:
                    command += ["--toolchain", args.toolchain.expanduser().resolve()]
                run(command, build_output, "build", cwd=ROOT, timeout=args.timeout)
                manifest = verify_manifest(project / "artifacts")
                if manifest.get("board") != board or manifest.get("optimization") != optimization:
                    raise ValueError("BSP build manifest differs from the selected board/optimization")
                dry_run = run([sys.executable, project / "bsp/build.py", "flash", "--output-dir",
                               project / "artifacts", "--dry-run"], build_output, "flash-dry-run",
                              cwd=ROOT, timeout=args.timeout)
                if '"dry_run": true' not in dry_run:
                    raise ValueError("BSP flasher did not produce an explicit dry run")
                builds[board, optimization] = project
                report["builds"].append({"board": board, "optimization": optimization, "status": "passed",
                                         "project": str(project), "resources": manifest["resources"],
                                         "image_sha256": manifest["sha256"]["firmware.bin"]})
                atomic_json(output / "report.json", report)

        def observe(platform, optimization):
            board = PLATFORMS[platform][0]
            case_output = run_root / "cases" / platform / optimization
            case_output.mkdir(parents=True)
            command = [sys.executable, CRATE / "tools/board_probe_stm32f4.py", "--firmware-dir",
                       builds[board, optimization] / "artifacts", "--output", case_output / "observations",
                       "--platform", platform, "--renode", renode, "--timeout", str(args.timeout)]
            try:
                run(command, case_output, "observe", cwd=ROOT, timeout=args.timeout + 30)
                item = json.loads((case_output / "observations/report.json").read_text())
                if item.get("status") != "passed":
                    raise ValueError("observation did not produce a passed report")
                item["optimization"] = optimization
                return item
            except (OSError, ValueError) as error:
                return {"platform": platform, "board": board, "optimization": optimization,
                        "status": "failed", "error": str(error)}

        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            futures = [pool.submit(observe, platform, optimization)
                       for platform in selected for optimization in optimizations]
            for future in concurrent.futures.as_completed(futures):
                case = future.result()
                report["cases"].append(case)
                atomic_json(output / "report.json", report)
                print(f"{case['status'].upper()}: {case['platform']}/{case['optimization']}", flush=True)
        report["cases"].sort(key=lambda item: (selected.index(item["platform"]), optimizations.index(item["optimization"])))
        report["status"] = "passed" if (len(report["cases"]) == report["expected_observations"]
                                             and all(item["status"] == "passed" for item in report["cases"])) else "failed"
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        atomic_json(output / "report.json", report)
        lease.close()
    print(f"{report['status'].upper()}: {output / 'report.json'}")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
