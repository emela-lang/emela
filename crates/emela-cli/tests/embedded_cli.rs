//! CLI boundaries that need no cross compiler, network access, or board connection.

use std::path::Path;
use std::process::{Command, Output};

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn emela(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("EMELA_POMEPROXY", "off")
        .env("EMELA_POME_CACHE", root.join(".cache"))
        .env("EMELA_CC", root.join("no-native-compiler"))
        .env("ARM_TOOLCHAIN_PATH", root.join("no-arm-toolchain"))
        .env("EMELA_ESP32C3_TOOLCHAIN", root.join("no-riscv-toolchain"))
        .output()
        .unwrap()
}

fn succeeds(root: &Path, args: &[&str]) -> Output {
    let output = emela(root, args);
    assert!(
        output.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn fails(root: &Path, args: &[&str], expected: &str) {
    let output = emela(root, args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{args:?}: {stderr}");
    assert!(stderr.contains(expected), "{args:?}: {stderr}");
}

#[test]
fn c_only_build_emits_headers_and_aliases_without_a_main_or_native_compiler() {
    for target_flag in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "Pome.toml", "[build]\ntarget = \"c-embedded\"\n");
        write(root, "src/main.emel", "pub fn answer() -> Int { 42 }\n");
        write(
            root,
            "embedded.toml",
            "string_capacity = 32\nheap_bytes = 0\n[exports]\nboard_answer = \"answer\"\n",
        );
        let mut args = vec!["build", "--embedded-config", "embedded.toml"];
        if target_flag {
            args.extend(["--target", "c-embedded"]);
        }
        succeeds(root, &args);
        let output = root.join("target/emela/c-embedded");
        for file in [
            "emela_embedded.c",
            "emela_embedded.h",
            "emela_board.h",
            "emela_embedded_exports.h",
        ] {
            assert!(output.join(file).is_file(), "{file}");
        }
        let aliases = std::fs::read_to_string(output.join("emela_embedded_exports.h")).unwrap();
        assert!(
            aliases.contains("#define board_answer emela_export_answer_"),
            "{aliases}"
        );
        assert!(aliases.contains("#include \"emela_embedded.h\""));
        assert!(!output.join("firmware").exists());
        assert!(!output.join("bsp").exists());
    }
}

#[test]
fn embedded_configuration_reports_invalid_fields_ranges_and_export_names() {
    let cases = [
        ("mystery = 1\n", "unknown field"),
        ("heap_bytes = -1\n", "invalid embedded configuration"),
        ("string_capacity = 0\n", "string_capacity"),
        ("string_capacity = 513\n", "string_capacity"),
        (
            "peripheral_buffer_capacity = 0\n",
            "peripheral_buffer_capacity",
        ),
        (
            "[exports]\nboard_answer = \"missing\"\n",
            "unknown public function",
        ),
        ("[exports]\nemela_scheduler_run = \"answer\"\n", "reserved"),
        (
            "[exports]\n\"bad-name\" = \"answer\"\n",
            "invalid export alias",
        ),
        (
            "[task_adapters]\nwait = \"bad-name\"\n",
            "invalid task adapter symbol",
        ),
    ];
    for (config, expected) in cases {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "Pome.toml", "");
        write(root, "src/main.emel", "pub fn answer() -> Int { 42 }\n");
        write(root, "embedded.toml", config);
        fails(
            root,
            &[
                "build",
                "--target",
                "c-embedded",
                "--embedded-config",
                "embedded.toml",
            ],
            expected,
        );
        assert!(
            !root
                .join("target/emela/c-embedded/emela_embedded.c")
                .exists()
        );
    }
}

#[test]
fn embedded_options_require_the_embedded_target_and_a_known_board() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "Pome.toml", "");
    write(root, "src/main.emel", "pub fn main() -> Int { 42 }\n");
    write(root, "embedded.toml", "");
    fails(
        root,
        &[
            "build",
            "--target",
            "js",
            "--embedded-config",
            "embedded.toml",
        ],
        "requires `--target c-embedded`",
    );
    fails(
        root,
        &["build", "--board", "stm32f446re", "--target", "js"],
        "requires target `c-embedded`",
    );
    fails(
        root,
        &["check", "--board", "unknown-board"],
        "unknown embedded board",
    );
    fails(root, &["flash", "--dry-run"], "requires `--board`");
}

#[test]
fn board_selection_and_module_overlays_are_shared_by_check_and_library_build() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Pome.toml",
        "[embedded]\nboard = \"unknown-default\"\n[embedded.modules]\nDevice = \"devices/{board}.emel\"\n",
    );
    write(
        root,
        "src/lib.emel",
        "import Device\npub fn read() -> Int { Device.read() }\n",
    );
    write(
        root,
        "src/device.emel",
        "pub fn read() -> Missing { missing }\n",
    );
    let original = std::fs::read(root.join("src/device.emel")).unwrap();
    for board in [
        "stm32f407vg",
        "stm32f412zg",
        "stm32f429zi",
        "stm32f446re",
        "stm32f4-discovery",
        "nucleo-f446re",
        "stm32f446",
        "esp32c3",
    ] {
        write(
            root,
            &format!("devices/{board}.emel"),
            "pub fn read() -> Int { 42 }\n",
        );
        succeeds(root, &["check", "--board", board]);
        succeeds(root, &["build", "--board", board]);
        assert_eq!(
            std::fs::read(root.join("src/device.emel")).unwrap(),
            original
        );
        assert!(!root.join("target").exists(), "library-only {board}");
    }
}

#[test]
fn c_only_build_uses_a_read_only_virtual_module_without_a_board() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Pome.toml",
        "[embedded.modules]\nDevice = \"devices/mock.emel\"\n",
    );
    write(
        root,
        "src/main.emel",
        "import Device\npub fn answer() -> Int { Device.read() }\n",
    );
    write(root, "devices/mock.emel", "pub fn read() -> Int { 42 }\n");
    succeeds(root, &["check"]);
    succeeds(root, &["build", "--target", "c-embedded"]);
    assert!(!root.join("src/device.emel").exists());
    assert!(
        root.join("target/emela/c-embedded/emela_embedded.c")
            .is_file()
    );

    std::fs::remove_file(root.join("devices/mock.emel")).unwrap();
    fails(root, &["check"], "overlay module `Device` does not exist");
}

#[test]
fn board_entry_errors_are_reported_before_the_cross_compiler_is_started() {
    for (source, expected) in [
        (
            "pub fn main() -> Int { 42 }\n",
            "board entry `main` must be",
        ),
        (
            "pub fn main(n: Int) -> Int use Async { n }\n",
            "embedded main function with parameters",
        ),
        (
            "pub fn main() -> () use Async { () }\n",
            "board entry `main` must be",
        ),
        (
            "pub fn other() -> Int use Async { 42 }\n",
            "board entry `main` must be",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "Pome.toml", "[embedded]\nboard = \"stm32f446re\"\n");
        write(root, "src/main.emel", source);
        fails(root, &["build"], expected);
        assert!(
            !root
                .join("target/emela/c-embedded/stm32f446re/firmware")
                .exists()
        );
    }
}

#[test]
fn embedded_target_run_and_test_are_explicitly_build_only() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "Pome.toml", "[build]\ntarget = \"c-embedded\"\n");
    write(root, "src/main.emel", "pub fn main() -> Int { 42 }\n");
    for command in ["run", "test"] {
        for explicit in [false, true] {
            let mut args = vec![command];
            if explicit {
                args.extend(["--target", "c-embedded"]);
            }
            fails(root, &args, "is not available for target `c-embedded`");
        }
    }
    assert!(!root.join("target").exists());
}

#[test]
fn a_failed_rebuild_invalidates_the_previous_success_record_before_config_validation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Pome.toml",
        "[embedded]\nboard = \"stm32f446re\"\nconfig = \"embedded.toml\"\n",
    );
    write(
        root,
        "src/main.emel",
        "pub fn main() -> Int use Async { 42 }\n",
    );
    write(root, "embedded.toml", "string_capacity = 0\n");
    let state = "target/emela/c-embedded/stm32f446re/board-build.json";
    write(root, state, "old successful record\n");
    fails(root, &["build"], "string_capacity");
    assert!(!root.join(state).exists());
    fails(
        root,
        &["flash", "--dry-run"],
        "no successful firmware build exists",
    );
}

#[test]
fn host_library_builds_ignore_unused_embedded_runtime_and_module_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Pome.toml",
        "[embedded]\nconfig = \"missing-embedded.toml\"\n[embedded.modules]\nDevice = \"missing-device.emel\"\n",
    );
    write(root, "src/lib.emel", "pub fn answer() -> Int { 42 }\n");
    for target in ["js", "c", "wasm"] {
        succeeds(root, &["build", "--target", target]);
        assert!(!root.join("target").exists(), "library-only {target}");
    }
}
