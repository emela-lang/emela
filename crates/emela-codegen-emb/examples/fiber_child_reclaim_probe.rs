use emela_codegen_emb::{Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "../tests/support/embedded_sources.rs"]
mod support;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/fiber-child-reclaim-probe/generated"));
    let failure = args.next().is_some_and(|mode| mode == "failure");
    let source = if failure {
        include_str!("support/fiber_child_failure_reclaim.emel")
    } else {
        include_str!("support/fiber_child_reclaim.emel")
    };
    let core = support::try_lower(&[("main.emel", source)], Roots::Public)
        .expect("lower child reclamation source");
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(if failure { 4096 } else { 2048 })
            .with_string_capacity(if failure { 64 } else { 32 })
            .with_fiber_capacity(if failure { 8 } else { 4 })
            .with_frame_capacity(if failure { 40 } else { 24 })
            .with_frame_bytes(if failure { 248 } else { 184 })
            .with_task_capacity(if failure { 3 } else { 1 })
            .with_event_capacity(if failure { 3 } else { 1 })
            .with_deferred_capacity(0)
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .expect("compile child reclamation source");
    fs::create_dir_all(&directory).expect("create output directory");
    for file in output.files {
        fs::write(directory.join(file.path), file.contents).expect("write generated file");
    }
}
