use std::{path::PathBuf, process::Command};

#[test]
fn fixed_scheduler_bounds_work_reuses_pools_and_rejects_stale_handles() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = std::env::temp_dir().join(format!("emela-scheduler-{}", std::process::id()));
    let compiled = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O2"])
        .arg("-I")
        .arg(root.join("src/runtime"))
        .arg(root.join("tests/runtime/scheduler.c"))
        .arg("-o")
        .arg(&output)
        .output()
        .expect("compile fixed scheduler runtime");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let result = Command::new(&output).status().expect("run scheduler probe");
    let _ = std::fs::remove_file(output);
    assert!(result.success());
}
