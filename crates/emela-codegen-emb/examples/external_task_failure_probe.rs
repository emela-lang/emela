use emela_codegen_emb::{Options, compile_flow};
use std::{fs, path::PathBuf};

#[path = "support/external_task_failure_flow.rs"]
mod fixture;
#[path = "../tests/support/embedded_sources.rs"]
mod support;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/external-task-failure-probe/generated"));
    let capacity = args
        .next()
        .map(|s| s.to_str().unwrap().parse::<u16>().unwrap())
        .unwrap_or(64);
    let frame_bytes = if capacity <= 64 { 296 } else { 388 };
    let output = compile_flow(
        &fixture::lower(),
        &Options::default()
            .with_heap_bytes(4096)
            .with_string_capacity(capacity)
            .with_fiber_capacity(8)
            .with_frame_capacity(24)
            .with_frame_bytes(frame_bytes)
            .with_deferred_capacity(0)
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_task_adapter("host_wait", "board_wait_start")
            .with_task_adapter("host_request", "board_request_start"),
    )
    .expect("compile failing external Flow ABI");
    fs::create_dir_all(&dir).expect("create probe directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write probe file");
    }
}
