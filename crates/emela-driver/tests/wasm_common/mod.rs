//! Helpers shared by the WASM corpus and example harnesses.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use emela_driver::{Analysis, wasmtime_program};

/// `wasmtime 49` or later (`EMELA_WASMTIME` or `wasmtime`), because the tests need WASI 0.3.
pub fn have_wasmtime() -> bool {
    let wasmtime = wasmtime_program();
    let output = match Command::new(&wasmtime).arg("--version").output() {
        Ok(output) if output.status.success() => output,
        _ => {
            eprintln!("skipping: cannot start `{}`", wasmtime.display());
            return false;
        }
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let major = text
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .and_then(|word| word.split('.').next())
        .and_then(|major| major.parse::<u32>().ok());
    match major {
        Some(major) if major >= 49 => true,
        Some(found) => {
            eprintln!(
                "skipping: wasmtime 49 or later is required, found {found} ({})",
                text.trim()
            );
            false
        }
        None => {
            eprintln!(
                "skipping: cannot read the version of `{}` ({})",
                wasmtime.display(),
                text.trim()
            );
            false
        }
    }
}

/// The diagnostics of an analysis, one `CODE message` per line.
pub fn messages(analysis: &Analysis) -> String {
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A fresh directory under the system temporary directory. The caller may create it or let the
/// driver do so.
pub fn out_dir(kind: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "emela-driver-wasm-{kind}-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

pub fn matches_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

/// Runs `f` on a helper thread and gives up after `limit`. A skipped program that starts to hang
/// must not hang CI; the helper thread is detached, but its program uses a temporary directory of
/// its own, so the rest of the tests are unaffected.
pub fn with_timeout<T: Send + 'static>(
    limit: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(value) => Ok(value),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!("timed out after {limit:?}")),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("the helper thread panicked".to_owned()),
    }
}
