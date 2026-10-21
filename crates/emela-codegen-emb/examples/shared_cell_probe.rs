use emela_codegen_emb::{Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/shared-cell-probe/generated"));
    let core = support::try_lower(
        &[("main.emel", include_str!("support/shared_cell.emel"))],
        Roots::Exported,
    )
    .expect("lower shared Cell source");
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(8192)
            .with_string_capacity(64)
            .with_fiber_capacity(8)
            .with_frame_capacity(24)
            .with_frame_bytes(424)
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_deferred_capacity(0)
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .expect("compile shared Cell source");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
