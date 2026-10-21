use emela_codegen_emb::{Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/aggregate-suspend-probe/generated"));
    let deferred = args.next().is_some_and(|mode| mode == "deferred");
    let source = if deferred {
        include_str!("support/deferred_aggregate.emel")
    } else {
        include_str!("support/aggregate_suspend.emel")
    };
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported)
        .expect("lower aggregate ownership source");
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(4096)
            .with_string_capacity(64)
            .with_fiber_capacity(8)
            .with_frame_capacity(24)
            .with_frame_bytes(if deferred { 400 } else { 384 })
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_deferred_capacity(u16::from(deferred))
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .expect("compile aggregate ownership source");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
