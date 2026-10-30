"""C artifact readers and BSP input selection, without any Emela/Flow generation.

The BSP command tests stop at the linker boundary. They check the real build
scripts' inputs/flags, not MCU instruction encoding, ELF layout, or image creation.
"""
import copy
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "boards"))
from artifact_manifest import MANIFEST_FILE, include_directories, load_manifest

FIXTURE = ROOT / "tests/fixtures/artifact"


def load_script(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ArtifactFixture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.generated = self.root / "generated C with spaces"
        shutil.copytree(FIXTURE, self.generated)
        self.manifest = self.generated / MANIFEST_FILE
        self.metadata = json.loads(self.manifest.read_text())


class ContractTests(ArtifactFixture):
    def test_legacy_v1_contract_is_rejected_with_regeneration_guidance(self):
        self.metadata["schema_version"] = 1
        self.metadata["abi_version"] = 1
        del self.metadata["metadata"]
        self.manifest.write_text(json.dumps(self.metadata))
        with self.assertRaisesRegex(ValueError, "expected 2/2; regenerate the artifact"):
            load_manifest(self.generated)

    def test_shared_invalid_corpus_is_rejected(self):
        cases = json.loads((ROOT / "tests/fixtures/artifact-invalid.json").read_text())
        for case in cases:
            with self.subTest(case=case["name"]):
                self.manifest.write_text(case.get("json", json.dumps(case.get("manifest"))))
                with self.assertRaisesRegex(ValueError, "invalid generated artifact"):
                    load_manifest(self.generated)

    def test_libraries_may_omit_aliases_but_board_builds_require_them(self):
        self.metadata["files"] = [entry for entry in self.metadata["files"] if entry["role"] != "aliases_header"]
        self.manifest.write_text(json.dumps(self.metadata))
        self.assertEqual(load_manifest(self.generated)["aliases_header"], [])
        with self.assertRaisesRegex(ValueError, "aliases_header"):
            load_manifest(self.generated, require_aliases=True)

    def test_missing_manifest_and_missing_declared_files_are_rejected(self):
        for path in [self.manifest, self.generated / "include/value.h", self.generated / "support/value.c"]:
            with self.subTest(path=path):
                contents = path.read_bytes()
                path.unlink()
                with self.assertRaisesRegex(ValueError, "invalid generated artifact"):
                    load_manifest(self.generated)
                path.write_bytes(contents)

    def test_files_cannot_escape_through_a_symlink(self):
        outside = self.root / "outside.c"
        outside.write_text("/* outside */")
        source = self.generated / "module.c"
        source.unlink()
        source.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "escaping generated file"):
            load_manifest(self.generated)

    def test_fixed_c_bundle_links_and_runs_with_a_platform_implementation(self):
        inputs = load_manifest(self.generated, require_aliases=True)
        consumer = self.root / "consumer.c"
        consumer.write_text('#include "emela_embedded_exports.h"\n#include "emela_board.h"\n'
                            'int32_t fixture_platform_read(void) { return INT32_C(40); }\n'
                            'int main(void) { return board_answer() == INT32_C(42) ? 0 : 1; }\n')
        # A leftover C file must not be discovered or compiled by the consumer.
        (self.generated / "stale.c").write_text("#error stale generated source\n")
        for optimization in ["O0", "O2"]:
            with self.subTest(optimization=optimization):
                executable = self.root / ("fixture-" + optimization)
                command = ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-" + optimization,
                           *["-I" + str(path) for path in sorted(include_directories(inputs))],
                           *inputs["source"], consumer, "-o", executable]
                subprocess.run(command, check=True, capture_output=True, timeout=30)
                subprocess.run([executable], check=True, capture_output=True, timeout=5)


class ReachedLink(Exception):
    pass


class BoardInputTests(ArtifactFixture):
    def test_both_builders_consume_only_manifest_sources_and_explicit_adapters(self):
        for board in ["stm32f4", "esp32c3"]:
            for optimization in ["O0", "O2"]:
                with self.subTest(board=board, optimization=optimization):
                    script = ROOT / ("boards/stm32/stm32f4/build.py" if board == "stm32f4" else "boards/esp32c3/build.py")
                    module = load_script("artifact_test_" + board, script)
                    output = self.root / (board + optimization)
                    extra = self.root / "user adapter.c"
                    extra.write_text("/* supplied separately from generated C */\n")
                    stale = self.generated / "emela_embedded.c"
                    stale.write_text("#error old filename must not be compiled\n")
                    args = SimpleNamespace(output_dir=output, generated_dir=self.generated, project_dir=self.root,
                                           source=[extra], optimization=optimization, toolchain=None)
                    calls = []

                    def run(argv, *unused):
                        command = list(map(str, argv))
                        calls.append(command)
                        if "--version" in command:
                            return "fixture compiler\n"
                        if "-c" not in command:
                            raise ReachedLink()
                        return ""

                    with mock.patch.object(module, "run", side_effect=run), \
                            mock.patch("subprocess.run", side_effect=AssertionError("unexpected real tool invocation")):
                        if board == "stm32f4":
                            profile, chip = module.selected_profile()
                            with mock.patch.object(module, "resolve_tool", side_effect=lambda name, root: name), \
                                    self.assertRaises(ReachedLink):
                                module.build(args, profile, chip)
                        else:
                            with mock.patch.object(module, "toolchain", return_value={"gcc": "gcc"}), \
                                    mock.patch.object(module, "esptool", return_value=[]), self.assertRaises(ReachedLink):
                                module.build(args)
                    compiled = [Path(command[command.index("-c") + 1]) for command in calls if "-c" in command]
                    generated = [path for path in compiled if self.generated in path.parents]
                    self.assertEqual(generated, [self.generated / "module.c", self.generated / "support/value.c"])
                    self.assertIn(extra, compiled)
                    self.assertNotIn(stale, compiled)
                    self.assertEqual(len(compiled), 6 if board == "stm32f4" else 5)
                    for command in calls:
                        if "-c" in command:
                            self.assertIn("-" + optimization, command)
                            self.assertIn("-std=c11", command)
                            self.assertIn("-I" + str(self.generated / "include"), command)
                    # Reject a malformed contract before consulting cross tools.
                    invalid = copy.deepcopy(self.metadata)
                    invalid["schema_version"] = 1
                    invalid["abi_version"] = 1
                    del invalid["metadata"]
                    self.manifest.write_text(json.dumps(invalid))
                    tool = "resolve_tool" if board == "stm32f4" else "toolchain"
                    with mock.patch.object(module, tool, side_effect=AssertionError("tool used before validation")), \
                            self.assertRaisesRegex(ValueError, "expected 2/2; regenerate the artifact"):
                        if board == "stm32f4":
                            module.build(args, profile, chip)
                        else:
                            module.build(args)
                    self.manifest.write_text(json.dumps(self.metadata))


if __name__ == "__main__":
    unittest.main()
