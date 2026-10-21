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
        .unwrap_or_else(|| PathBuf::from("target/function-aggregate-probe/generated"));
    let mode = args.next().unwrap_or_default();
    let native = mode == "native";
    let failure = mode == "failure";
    let source = if native {
        include_str!("support/function_aggregate.emel").to_owned()
    } else if failure {
        [
            include_str!("support/fiber_aggregate_abi.emel"),
            include_str!("support/fiber_aggregate_failure.emel"),
        ]
        .concat()
    } else {
        include_str!("support/fiber_aggregate_abi.emel").to_owned()
    };
    let core = support::try_lower(&[("main.emel", &source)], Roots::Exported)
        .expect("lower aggregate ABI source");
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(4096)
            .with_string_capacity(64)
            .with_fiber_capacity(8)
            .with_frame_capacity(24)
            .with_frame_bytes(if failure { 848 } else { 176 })
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_deferred_capacity(0)
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .expect("compile aggregate ABI source");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
