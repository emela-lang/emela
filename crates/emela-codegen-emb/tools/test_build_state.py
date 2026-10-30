"""Firmware freshness checks using temporary files; no compiler or board tools are needed."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True

SCRIPT = Path(__file__).resolve().parents[1] / "boards/build_state.py"
ARTIFACT_READER = SCRIPT.with_name("artifact_manifest.py")
sys.path.insert(0, str(SCRIPT.parent))


class BuildStateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project = self.root / "project with spaces"
        self.output = self.project / "target/emela/c-embedded/stm32f446re"
        self.source = self.project / "src/main.emel"
        self.dependency = self.root / "dependency"
        self.prepared = self.output / "board-build-pending.json"
        self.state = self.output / "board-build.json"
        self.spec_path = self.output / "board-inputs.json"
        inputs = {
            self.source: "pub fn main() -> Int use Async { 42 }\n",
            self.project / "Pome.toml": "[embedded]\nboard = 'stm32f446re'\n",
            self.project / "Pome.lock": "version = 1\n",
            self.project / "embedded.toml": "heap_bytes = 2048\n",
            self.project / "native.c": "#include \"include/native.h\"\n",
            self.project / "include/native.h": "int native(void);\n",
            self.dependency / "Pome.toml": "[pome]\nname = 'github.com/acme/value'\n",
            self.dependency / "src/lib.emel": "pub fn answer() -> Int { 42 }\n",
            self.dependency / "src/native.c": "int value(void) { return 42; }\n",
            self.dependency / "src/include/value.h": "int value(void);\n",
            self.output / "bsp/build.py": "# Fixture BSP; not executed.\n",
            self.output / "bsp/artifact_manifest.py": ARTIFACT_READER.read_text(),
            self.output / "bsp/profile.json": "{}\n",
            self.output / "generated/emela_embedded.c": "/* Fixture C; not compiled. */\n",
            self.output / "generated/emela_embedded.h": "/* Fixture header. */\n",
            self.output / "generated/emela_board.h": "/* Fixture platform. */\n",
            self.output / "generated/emela_embedded_exports.h": "/* Fixture aliases. */\n",
            self.output / "generated/nested/adapter.c": "/* Fixture adapter. */\n",
            self.output / "generated/nested/value.h": "/* Fixture nested header. */\n",
            self.output / "firmware/firmware.elf": "fixture ELF bytes\n",
            self.output / "firmware/firmware.bin": "fixture flash bytes\n",
            self.output / "firmware/firmware.map": "fixture map\n",
            self.output / "firmware/resources.json": "{}\n",
        }
        for path, text in inputs.items():
            self.write(path, text)
        self.artifact_path = self.output / "generated/emela_artifact.json"
        self.artifact = {"schema_version": 2, "abi_version": 2, "files": [
            {"path": name, "role": role} for name, role in [
                ("emela_embedded.c", "source"), ("emela_embedded.h", "public_header"),
                ("emela_board.h", "platform_header"), ("emela_embedded_exports.h", "aliases_header"),
                ("nested/adapter.c", "source"), ("nested/value.h", "header")]],
            "metadata": {"entry": None, "exports": [], "services": [],
                         "external_symbols": [], "isr_entries": [],
                         "isr_safe_symbols": [], "task_capacity": 0}}
        self.write(self.artifact_path, json.dumps(self.artifact))
        self.firmware_path = self.output / "firmware/firmware.json"
        self.firmware = {
            "schema_version": 1,
            "board": "stm32f446re",
            "artifacts": ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"],
        }
        self.write(self.firmware_path, json.dumps(self.firmware))
        self.spec = {
            "board": "stm32f446re",
            "settings": {"entry": "main", "optimization": "O2"},
            "source_root": str(self.project / "src"),
            "single_file": False,
            "output": str(self.output),
            "files": [str(path) for path in [
                self.project / "Pome.toml", self.project / "Pome.lock",
                self.project / "embedded.toml", self.project / "native.c",
                self.dependency / "Pome.toml",
            ]],
            "dependencies": [{
                "name": "github.com/acme/value", "directory": str(self.dependency),
                "version": "v0.1.0",
            }],
            "bsp_files": ["build.py", "profile.json", "artifact_manifest.py"],
        }
        self.write(self.spec_path, json.dumps(self.spec))
        self.prepare()
        self.seal()

    @staticmethod
    def write(path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def invoke(self, *args, expected=None):
        result = subprocess.run([sys.executable, "-B", SCRIPT, *args], cwd=self.project,
                                text=True, capture_output=True, timeout=10)
        if expected is None:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn(expected, result.stderr)
        return result

    def prepare(self):
        self.invoke("prepare", "--manifest", str(self.prepared),
                    "--expected", json.dumps(self.spec))

    def seal(self, expected=None):
        self.invoke("seal", "--manifest", str(self.state), "--spec", str(self.spec_path),
                    "--prepared", str(self.prepared), expected=expected)

    def verify(self, expected=None, spec=None):
        self.invoke("verify", "--manifest", str(self.state),
                    "--expected", json.dumps(self.spec if spec is None else spec),
                    expected=expected)

    def test_prepare_seal_and_verify_record_inputs_and_artifacts(self):
        self.verify()
        state = json.loads(self.state.read_text())
        self.assertEqual(state["schema_version"], 1)
        self.assertEqual(state["spec"], self.spec)
        for path in [self.source, self.dependency / "src/lib.emel",
                     self.project / "include/native.h", self.dependency / "Pome.toml",
                     self.output / "generated/emela_embedded.h",
                     self.artifact_path, self.output / "generated/nested/adapter.c",
                     self.output / "generated/nested/value.h",
                     self.output / "bsp/build.py", self.output / "firmware/firmware.bin"]:
            with self.subTest(path=path):
                self.assertEqual(len(state["sha256"][str(path)]), 64)

    def test_changed_inputs_generated_files_bsp_and_firmware_are_rejected(self):
        paths = [self.source, self.project / "Pome.toml", self.project / "Pome.lock",
                 self.project / "embedded.toml", self.project / "native.c",
                 self.project / "include/native.h", self.dependency / "Pome.toml",
                 self.dependency / "src/lib.emel", self.dependency / "src/native.c",
                 self.dependency / "src/include/value.h",
                 self.output / "generated/emela_embedded.c",
                 self.output / "generated/emela_embedded.h", self.output / "bsp/build.py",
                 self.output / "generated/nested/adapter.c", self.output / "generated/nested/value.h",
                 self.output / "firmware/firmware.bin", self.output / "firmware/firmware.map"]
        for path in paths:
            with self.subTest(path=path):
                old = path.read_bytes()
                try:
                    path.write_bytes(old + b"changed\n")
                    self.verify(expected="inputs changed")
                finally:
                    path.write_bytes(old)
                self.verify()

    def test_added_and_removed_project_dependency_and_generated_files_are_rejected(self):
        additions = [self.project / "src/new.emel", self.dependency / "src/new.emel",
                     self.dependency / "src/new.c", self.dependency / "src/new.h",
                     self.dependency / "src/new.S", self.dependency / "src/new.s",
                     self.dependency / "src/new.toml",
                     self.output / "generated/new.h", self.project / "include/new.h"]
        for path in additions:
            with self.subTest(added=path):
                self.write(path, "new input\n")
                try:
                    self.verify(expected="inputs changed")
                finally:
                    path.unlink()
                self.verify()
        for path in [self.source, self.dependency / "src/lib.emel"]:
            with self.subTest(removed=path):
                old = path.read_bytes()
                path.unlink()
                try:
                    self.verify(expected="inputs changed")
                finally:
                    path.write_bytes(old)
                self.verify()

    def test_board_settings_and_dependency_identity_must_match_the_successful_build(self):
        for field, value in [("board", "esp32c3"), ("settings", {"entry": "other"}),
                             ("dependencies", [{"name": "github.com/other/value",
                                                "directory": str(self.dependency),
                                                "version": "v0.2.0"}])]:
            with self.subTest(field=field):
                spec = dict(self.spec, **{field: value})
                self.verify(expected="settings differ", spec=spec)

    def test_invalid_firmware_identity_and_escaping_artifact_paths_are_rejected(self):
        cases = [(dict(self.firmware, board="esp32c3"), "firmware board differs"),
                 (dict(self.firmware, schema_version=99), "metadata version"),
                 (dict(self.firmware, artifacts=self.firmware["artifacts"] + ["../escape"]),
                  "invalid firmware artifact")]
        for metadata, expected in cases:
            with self.subTest(metadata=metadata):
                self.write(self.firmware_path, json.dumps(metadata))
                self.verify(expected=expected)
        self.write(self.firmware_path, json.dumps(self.firmware))
        self.verify()

    def test_inputs_changed_after_prepare_cannot_be_sealed(self):
        self.state.unlink()
        self.source.write_text("pub fn main() -> Int use Async { 41 }\n")
        self.seal(expected="inputs changed during the build")
        self.assertFalse(self.state.exists())

    def test_assembly_sources_track_adjacent_headers(self):
        for suffix in [".S", ".s"]:
            with self.subTest(suffix=suffix):
                assembly = self.project / ("native" + suffix)
                self.write(assembly, "# Fixture assembly.\n")
                self.spec["files"].append(str(assembly))
                self.write(self.spec_path, json.dumps(self.spec))
                self.prepare()
                self.seal()
                header = self.project / "include/native.h"
                old = header.read_bytes()
                try:
                    header.write_bytes(old + b"changed\n")
                    self.verify(expected="inputs changed")
                finally:
                    header.write_bytes(old)
                self.verify()

    def test_seal_rechecks_inputs_while_collecting_artifact_hashes(self):
        module_spec = importlib.util.spec_from_file_location("embedded_build_state_test", SCRIPT)
        module = importlib.util.module_from_spec(module_spec)
        module_spec.loader.exec_module(module)
        snapshot = module.snapshot

        def change_during_artifacts(spec, artifacts=True):
            if artifacts:
                self.source.write_text("pub fn main() -> Int use Async { 41 }\n")
            return snapshot(spec, artifacts=artifacts)

        self.state.unlink()
        arguments = [str(SCRIPT), "seal", "--manifest", str(self.state),
                     "--spec", str(self.spec_path), "--prepared", str(self.prepared)]
        with mock.patch.object(module, "snapshot", side_effect=change_during_artifacts), \
                mock.patch.object(sys, "argv", arguments), \
                self.assertRaisesRegex(ValueError, "inputs changed"):
            module.main()
        self.assertFalse(self.state.exists())

    def test_ignored_source_directories_and_non_input_artifacts_do_not_change_the_state(self):
        for path in [self.project / "src/.git/ignored.emel",
                     self.project / "src/.cache/ignored.emel",
                     self.output / "build.log", self.output / "generated/notes.txt",
                     self.output / "generated/stale.c"]:
            self.write(path, "unrelated\n")
        self.verify()

    def test_artifact_manifest_changes_and_declared_file_deletion_invalidate_firmware(self):
        original = self.artifact_path.read_text()
        self.artifact_path.write_text(original + "\n")
        self.verify(expected="inputs changed")
        self.artifact_path.write_text(original)
        self.verify()
        for path in [self.artifact_path, self.output / "generated/nested/value.h"]:
            with self.subTest(path=path):
                contents = path.read_bytes()
                path.unlink()
                self.verify(expected="invalid generated artifact")
                path.write_bytes(contents)
                self.verify()
        extra = self.output / "generated/new.c"
        self.write(extra, "/* new declared source */\n")
        metadata = json.loads(original)
        metadata["files"].append({"path": "new.c", "role": "source"})
        self.artifact_path.write_text(json.dumps(metadata))
        self.verify(expected="inputs changed")

    def test_source_target_directory_is_part_of_the_module_inventory(self):
        source = self.project / "src/target/device.emel"
        self.write(source, "pub fn answer() -> Int { 42 }\n")
        self.verify(expected="inputs changed")
        self.prepare()
        self.seal()
        self.verify()
        old = source.read_bytes()
        source.write_bytes(old + b"changed\n")
        self.verify(expected="inputs changed")
        source.write_bytes(old)
        self.verify()
        source.unlink()
        self.verify(expected="inputs changed")

    def test_symlinked_source_directories_track_contents_and_retargeting(self):
        shared = self.root / "shared"
        other = self.root / "other shared"
        text = "pub fn answer() -> Int { 42 }\n"
        self.write(shared / "lib.emel", text)
        self.write(other / "lib.emel", text)
        link = self.project / "src/linked"
        link.symlink_to(shared, target_is_directory=True)
        self.prepare()
        self.seal()
        self.verify()
        (shared / "lib.emel").write_text(text + "changed\n")
        self.verify(expected="inputs changed")
        (shared / "lib.emel").write_text(text)
        self.verify()
        link.unlink()
        link.symlink_to(other, target_is_directory=True)
        self.verify(expected="inputs changed")

    def test_an_added_file_symlink_keeps_its_distinct_module_name(self):
        alias = self.project / "src/alias.emel"
        alias.symlink_to(self.source)
        self.verify(expected="inputs changed")
        self.prepare()
        self.seal()
        state = json.loads(self.state.read_text())
        self.assertIn(str(alias), state["sha256"])
        self.assertIn(str(self.source), state["sha256"])
        self.verify()
        alias.unlink()
        self.verify(expected="inputs changed")

    def test_cyclic_source_symlinks_are_rejected_without_recursing_forever(self):
        (self.project / "src/loop").symlink_to(self.project / "src", target_is_directory=True)
        self.verify(expected="cyclic input directory")


if __name__ == "__main__":
    unittest.main()
