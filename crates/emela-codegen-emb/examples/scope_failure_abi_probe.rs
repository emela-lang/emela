use emela_codegen_emb::{Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/scope-failure-abi-probe/generated"));
    let core = support::try_lower(
        &[("main.emel", include_str!("support/scope_failure_abi.emel"))],
        Roots::Exported,
    )
    .expect("lower mixed Scope source");
    let options = Options::default()
        .with_heap_bytes(4096)
        .with_string_capacity(64)
        .with_fiber_capacity(4)
        .with_frame_capacity(24)
        .with_frame_bytes(204)
        .with_task_capacity(0)
        .with_event_capacity(0)
        .with_deferred_capacity(0);
    let output = compile(&core, &options).expect("compile mixed Scope source");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
