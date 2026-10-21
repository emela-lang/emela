use emela_codegen_emb::{Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/external-task-values-probe/generated"));
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("support/external_task_values.emel"),
        )],
        Roots::Exported,
    )
    .expect("lower typed task source");
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(4096)
            .with_string_capacity(64)
            .with_fiber_capacity(8)
            .with_frame_capacity(24)
            .with_frame_bytes(180)
            .with_deferred_capacity(2)
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_task_adapter("host_packet", "board_packet_start")
            .with_task_adapter("host_text", "board_text_start"),
    )
    .expect("compile typed task probe");
    fs::create_dir_all(&dir).expect("create probe directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write probe file");
    }
}
