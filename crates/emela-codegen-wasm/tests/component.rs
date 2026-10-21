//! The emitted `main.wasm` is a WASI 0.3 component exporting `wasi:cli/run@0.3.0`, and its
//! process outcomes are unchanged (0 on success, 1 for an error reaching main, 101 for a defect).

mod common;

use common::*;
use wasmparser::{Encoding, Parser, Payload};

use std::process::{Command, Stdio};

#[test]
fn output_is_a_wasm_component() {
    let bytes = compile_checked("fn main() { dbg(1) }\n");
    let mut encoding = None;
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::Version { encoding: e, .. } = payload.unwrap() {
            encoding = Some(e);
            break;
        }
    }
    assert_eq!(
        encoding,
        Some(Encoding::Component),
        "main.wasm must be a component, not a core module"
    );
    let mut validator = wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all());
    validator
        .validate_all(&bytes)
        .expect("the component is valid");
}

#[test]
fn exit_codes_are_zero_one_and_101() {
    let Some(ok) = run("exit-0", "fn main() { dbg(1) }\n") else {
        return;
    };
    assert_eq!(ok.code, 0, "{}", ok.stderr);

    let Some(failure) = run(
        "exit-1",
        "error Nope(why: String) derive Show\n\
         fn fail_it() -> Int fails Nope { fail Nope(why: \"no\") }\n\
         fn main() { dbg(fail_it()) }\n",
    ) else {
        return;
    };
    assert_eq!(failure.code, 1);
    assert_eq!(failure.stderr, "error: Nope(why: \"no\")\n");

    let Some(defect) = run("exit-101", "fn main() { assert 1 + 1 == 3 }\n") else {
        return;
    };
    assert_eq!(defect.code, 101);
    assert!(
        defect.stderr.starts_with("defect: assertion failed"),
        "{}",
        defect.stderr
    );
}

#[test]
fn large_output_waits_for_the_stream() {
    // More than a pipe buffer: the host cannot accept the whole stream at once, so `write`
    // must wait on its waitable set and resume. The reader sleeps first so the pipe is full
    // while the guest is in the wait.
    let Some(wt) = wasmtime() else {
        return;
    };
    let text = "x".repeat(1_000_000);
    let source = format!("fn main() {{ dbg(\"{text}\") }}\n");
    let bytes = compile_checked(&source);
    let dir = out_dir("large-output");
    std::fs::write(dir.join("main.wasm"), &bytes).unwrap();
    let mut child = Command::new(wt)
        .arg("run")
        .arg("-S")
        .arg("p3=y")
        .arg("main.wasm")
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let mut written = Vec::new();
    std::io::Read::read_to_end(&mut stderr, &mut written).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "wasmtime exited with {status}");
    assert_eq!(written.len(), text.len() + 3);
    assert_eq!(written[0], b'"');
    assert_eq!(written[written.len() - 2], b'"');
    assert_eq!(written[written.len() - 1], b'\n');
    assert!(written[1..written.len() - 2].iter().all(|&b| b == b'x'));
}
