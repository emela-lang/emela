//! The embedded examples on the Renode emulator (`docs/design/c-embedded-architecture.md`,
//! コンパイラのテスト 3): each example's `renode-f446/run.sh` builds the firmware with the ARM
//! toolchain and STM32CubeF4, runs it on Renode and checks what it does (GPIO registers, UART
//! output, what the I2C and SPI device models saw). The tools are found through
//! `STM32_RENODE_ROOT` (the scripts' layout: `.tools/common/arm`, `.tools/stm32cube`, `models`);
//! without it the tests are skipped, with a message.

use std::path::Path;
use std::process::Command;

fn run_example(name: &str) {
    let Some(root) = std::env::var_os("STM32_RENODE_ROOT").filter(|r| !r.is_empty()) else {
        eprintln!("skipping: STM32_RENODE_ROOT is not set (Renode and the ARM toolchain)");
        return;
    };
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .join("renode-f446/run.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env("STM32_RENODE_ROOT", root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("PASS"),
        "{name} failed on Renode:\n{stdout}\n{stderr}"
    );
    assert!(!stdout.contains("FAIL"), "{stdout}");
}

#[test]
fn gpio_example_on_renode() {
    run_example("embedded-gpio");
}

#[test]
fn uart_example_on_renode() {
    run_example("embedded-uart");
}

/// I2C and SPI against the device models of `STM32_RENODE_ROOT/models`, and UART input.
#[test]
fn bus_example_on_renode() {
    run_example("embedded-bus");
}

/// The ICM-42688-P driver against the example's own device model (`renode-f446/Icm42688.cs`).
#[test]
fn imu_example_on_renode() {
    run_example("embedded-imu");
}

/// `emela test --device`: the GPIO example's tests run as a firmware on Renode through its runner
/// (`renode-f446/test.sh`), and report their results over USART2.
#[test]
fn gpio_example_tests_on_renode() {
    use emela_driver::{
        DeviceTests, Input, LowerTests, OsFs, Output, Target, TestOptions, TestOutcome,
    };
    if std::env::var_os("STM32_RENODE_ROOT").is_none_or(|r| r.is_empty()) {
        eprintln!("skipping: STM32_RENODE_ROOT is not set (Renode and the ARM toolchain)");
        return;
    }
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/embedded-gpio");
    let input = Input::resolve(&OsFs, &project).unwrap();
    let device = DeviceTests {
        options: emela_driver::embedded_options(&OsFs, &input).unwrap(),
        runner: emela_driver::embedded_test_runner(&OsFs, &input)
            .unwrap()
            .expect("the example sets a runner"),
    };
    let options = TestOptions {
        out_dir: project.join("target/emela/test-device"),
        node: "node".into(),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = emela_driver::test_on_device(
        &OsFs,
        &input,
        &mut LowerTests::default()
            .with_target(Target::CEmbedded)
            .for_embedded_project(),
        &device,
        &options,
        |_| {},
        &mut out,
    );
    assert!(!analysis.has_errors(), "{:#?}", analysis.diagnostics);
    let out = String::from_utf8_lossy(&out);
    let report = report.expect("a report");
    assert_eq!(report.results.len(), 3, "{out}");
    assert!(
        report
            .results
            .iter()
            .all(|r| r.outcome == TestOutcome::Passed),
        "{out}"
    );
}
