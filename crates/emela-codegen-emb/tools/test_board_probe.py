"""Runner checks using temporary files and host Python; no board tools are needed."""
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True

from board_probe_common import atomic_json, claim_output, digest, elf_symbols, metric, run_captured, verify_manifest
from board_probe_stm32f4 import EXPECTED_FILES, copy_platforms, inventory

TOOLS = Path(__file__).resolve().parent


class OutputTests(unittest.TestCase):
    def test_unknown_nonempty_output_is_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            precious = output / "keep.txt"
            precious.write_text("keep")
            with self.assertRaisesRegex(ValueError, "refusing"):
                claim_output(output, "stm32f4")
            self.assertEqual(precious.read_text(), "keep")
            self.assertEqual(list(output.iterdir()), [precious])

    def test_owned_output_is_locked_and_reusable(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            first = claim_output(output, "stm32f4")
            try:
                with self.assertRaisesRegex(ValueError, "already in use"):
                    claim_output(output, "stm32f4")
                with self.assertRaisesRegex(ValueError, "refusing"):
                    claim_output(output, "esp32c3")
            finally:
                first.close()
            second = claim_output(output, "stm32f4")
            second.close()

    def test_failed_invocations_replace_an_old_success_report(self):
        cases = [
            ("stm32f4", "board_probe_stm32f4.py", ["--firmware-dir", "missing", "--renode", "missing"]),
            ("esp32c3", "board_probe_esp32c3.py", ["--firmware-dir", "missing", "--qemu", "missing"]),
            ("matrix", "board_probe_matrix.py", ["--probe", "missing", "--renode", "missing"]),
        ]
        for kind, runner, arguments in cases:
            with self.subTest(runner=runner), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / "observations"
                claim_output(output, kind).close()
                atomic_json(output / "report.json", {"status": "passed", "old": True})
                result = subprocess.run([sys.executable, TOOLS / runner, "--output", output, *arguments],
                                        cwd=temporary, text=True, capture_output=True, timeout=10)
                self.assertNotEqual(result.returncode, 0)
                report = json.loads((output / "report.json").read_text())
                self.assertEqual(report["status"], "failed", result.stdout + result.stderr)
                self.assertNotIn("old", report)


class ParsingTests(unittest.TestCase):
    def test_metric_requires_one_unbroken_marker_and_value(self):
        self.assertEqual(metric('echo "EMELA_METRIC_ticks"\n0x0000000A\n', "EMELA_METRIC_ticks"), 10)
        for text in ["", "EMELA_METRIC_ticks\nEMELA_METRIC_next\n0x1\n",
                     "EMELA_METRIC_ticks\n0x1\nEMELA_METRIC_ticks\n0x2\n"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                metric(text, "EMELA_METRIC_ticks")

    def test_manifest_hashes_and_resources_are_checked(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            for name in ["firmware.elf", "firmware.bin", "firmware.map"]:
                (output / name).write_text(name)
            atomic_json(output / "resources.json", {"static_ram_bytes": 256})
            names = ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"]
            manifest = {"schema_version": 1, "artifacts": names,
                        "resources": {"static_ram_bytes": 256},
                        "sha256": {name: digest(output / name) for name in names}}
            atomic_json(output / "firmware.json", manifest)
            self.assertEqual(verify_manifest(output), manifest)
            (output / "firmware.bin").write_text("changed")
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                verify_manifest(output)
            (output / "firmware.bin").write_text("firmware.bin")
            manifest["artifacts"].append("../outside")
            atomic_json(output / "firmware.json", manifest)
            with self.assertRaisesRegex(ValueError, "filename"):
                verify_manifest(output)

    def test_elf32_symbols_are_read_without_a_toolchain(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "probe.elf"
            data = bytearray(96 + 3 * 40)
            data[:7] = b"\x7fELF\x01\x01\x01"
            struct.pack_into("<H", data, 18, 40)
            struct.pack_into("<I", data, 32, 96)
            struct.pack_into("<HH", data, 46, 40, 3)
            names = b"\0board_ticks\0"
            data[52:52 + len(names)] = names
            struct.pack_into("<IIIBBH", data, 80, 1, 0x20000000, 4, 1, 0, 1)
            struct.pack_into("<10I", data, 136, 0, 3, 0, 0, 52, len(names), 0, 0, 1, 0)
            struct.pack_into("<10I", data, 176, 0, 2, 0, 0, 64, 32, 1, 0, 4, 16)
            path.write_bytes(data)
            self.assertEqual(elf_symbols(path, 40)["board_ticks"], (0x20000000, 4, 1))
            with self.assertRaisesRegex(ValueError, "machine"):
                elf_symbols(path, 243)
            path.write_bytes(data[:40])
            with self.assertRaises(ValueError):
                elf_symbols(path, 40)

    def test_pinned_f4_inventory_and_flash_alias_are_explicit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in EXPECTED_FILES:
                path = root / "platforms" / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('flash: Memory.MappedMemory @ sysbus 0x08000000\n    size: 0x100000\n')
            renode = root / "renode"
            renode.touch()
            self.assertEqual(set(inventory(renode)), EXPECTED_FILES)
            output = root / "out"
            copy_platforms(renode, output, output)
            self.assertIn("sysbus 0x00000000", (output / "platforms/cpus/stm32f4.repl").read_text())
            (root / "platforms/cpus/stm32f499.repl").touch()
            with self.assertRaisesRegex(ValueError, "inventory changed"):
                inventory(renode)


class ProcessTests(unittest.TestCase):
    def test_timeout_is_reported_after_stopping_the_process(self):
        code, text, timed_out = run_captured([sys.executable, "-c", "import time; time.sleep(30)"],
                                            cwd=TOOLS, env=dict(os.environ), timeout=0.1)
        self.assertIsNone(code)
        self.assertTrue(timed_out)
        self.assertIsInstance(text, str)


if __name__ == "__main__":
    unittest.main()
