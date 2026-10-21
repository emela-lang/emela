//! Checks CLI orchestration with real freshness verification and mocked BSP tools.
//! The fixture artifacts do not model an MCU, a cross compiler, or physical flashing.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE: &str = "pub fn main() -> Int use Async { 42 }\n";

const TOOL_WRAPPER: &str = r#"
import hashlib
import json
import os
from pathlib import Path
import sys

trace = Path(os.environ["EMELA_TEST_TRACE"])
if sys.argv[1] == "-c" or Path(sys.argv[1]).name == "build_state.py":
    os.execv(REAL_PYTHON, [REAL_PYTHON, *sys.argv[1:]])

script = Path(sys.argv[1])
if script.name != "build.py":
    raise SystemExit("unexpected fixture tool")
action = sys.argv[2]
output = Path(sys.argv[sys.argv.index("--output-dir") + 1])
with (trace / "calls").open("a") as log:
    log.write(action + "\n")
if action == "build":
    if (trace / "fail-build").exists():
        raise SystemExit(7)
    output.mkdir(parents=True, exist_ok=True)
    artifacts = ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"]
    for name in artifacts:
        (output / name).write_text("fixture artifact " + name)
    board = json.loads((script.parent / "profile.json").read_text())["board"]
    metadata = {"schema_version": 1, "board": board, "artifacts": artifacts,
                "sha256": {name: hashlib.sha256((output / name).read_bytes()).hexdigest()
                           for name in artifacts}}
    (output / "firmware.json").write_text(json.dumps(metadata))
elif action == "flash":
    if "--dry-run" not in sys.argv:
        raise SystemExit("the fixture permits dry-run only")
    (trace / "flash-argv").write_text("\0".join(sys.argv[1:]))
    print(json.dumps({"dry_run": True, "argv": sys.argv[1:]}))
else:
    raise SystemExit("unexpected fixture action")
"#;

struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
    output: PathBuf,
    trace: PathBuf,
    wrapper: PathBuf,
}

impl Project {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project with spaces");
        let output = root.join("output with spaces ; literal");
        let trace = root.join("trace");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&trace).unwrap();
        std::fs::write(root.join("Pome.toml"), "").unwrap();
        std::fs::write(root.join("src/main.emel"), SOURCE).unwrap();
        let python = Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        assert!(python.status.success());
        let python = String::from_utf8(python.stdout).unwrap().trim().to_owned();
        let wrapper = root.join("python tool wrapper");
        std::fs::write(
            &wrapper,
            format!("#!{python}\nREAL_PYTHON = {python:?}\n{TOOL_WRAPPER}"),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _temp: temp,
            root,
            output,
            trace,
            wrapper,
        }
    }

    fn emela(&self, action: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_emela"))
            .arg(action)
            .args(["--board", "stm32f446re", "--out-dir"])
            .arg(&self.output)
            .args(extra)
            .current_dir(&self.root)
            .env("NO_COLOR", "1")
            .env("EMELA_POMEPROXY", "off")
            .env("EMELA_POME_CACHE", self.root.join(".cache"))
            .env("EMELA_PYTHON", &self.wrapper)
            .env("EMELA_TEST_TRACE", &self.trace)
            .output()
            .unwrap()
    }

    fn succeeds(&self, action: &str, extra: &[&str]) -> Output {
        let output = self.emela(action, extra);
        assert!(
            output.status.success(),
            "{action}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn state(&self) -> PathBuf {
        self.output.join("board-build.json")
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.trace.join("calls")).unwrap()
    }
}

#[test]
fn dry_run_uses_literal_arguments_and_rejects_changed_sources_before_the_flasher() {
    let project = Project::new();
    project.succeeds("build", &[]);
    assert!(project.state().is_file());
    let aliases =
        std::fs::read_to_string(project.output.join("generated/emela_embedded_exports.h")).unwrap();
    assert!(aliases.contains("#define board_main_start "), "{aliases}");

    let port = "probe; touch should-not-exist";
    let output = project.succeeds("flash", &["--port", port, "--dry-run"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"dry_run\": true"));
    let arguments = std::fs::read_to_string(project.trace.join("flash-argv")).unwrap();
    let arguments: Vec<_> = arguments.split('\0').collect();
    let port_index = arguments.iter().position(|arg| *arg == "--port").unwrap();
    assert_eq!(arguments[port_index + 1], port);
    let output_index = arguments
        .iter()
        .position(|arg| *arg == "--output-dir")
        .unwrap();
    assert_eq!(
        Path::new(arguments[output_index + 1]),
        project.output.join("firmware")
    );
    assert!(!project.root.join("should-not-exist").exists());
    assert_eq!(project.calls(), "build\nflash\n");

    std::fs::write(
        project.root.join("src/main.emel"),
        SOURCE.replace("42", "43"),
    )
    .unwrap();
    let rejected = project.emela("flash", &["--dry-run"]);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("inputs changed"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(project.calls(), "build\nflash\n");
}

#[test]
fn a_failed_mock_board_rebuild_cannot_flash_the_previous_success() {
    let project = Project::new();
    project.succeeds("build", &[]);
    assert!(project.state().is_file());
    std::fs::write(project.trace.join("fail-build"), "fail this build").unwrap();
    let failed = project.emela("build", &[]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(!project.state().exists());
    let flash = project.emela("flash", &["--dry-run"]);
    assert_eq!(flash.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&flash.stderr).contains("no successful firmware build exists"),
        "{}",
        String::from_utf8_lossy(&flash.stderr)
    );
    assert_eq!(project.calls(), "build\nbuild\n");
    assert!(!project.trace.join("flash-argv").exists());
}
