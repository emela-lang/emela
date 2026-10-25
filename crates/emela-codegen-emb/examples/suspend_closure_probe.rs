use emela_codegen_emb::{Options, compile_flow, flow_from_core};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/suspend-closure-probe/generated"));
    let core = support::try_lower(
        &[("main.emel", include_str!("support/suspend_handler.emel"))],
        Roots::Public,
    )
    .expect("lower suspend handler source");
    let flow = flow_from_core(&core).expect("verified Flow IR");
    if std::env::args_os().nth(2).is_some_and(|arg| arg == "dump") {
        println!("{}", emela_lir::dump(&flow));
        return;
    }
    let options = Options::default()
        .with_heap_bytes(2048)
        .with_fiber_capacity(4)
        .with_frame_capacity(12)
        .with_frame_bytes(128)
        .with_task_capacity(2)
        .with_event_capacity(2)
        .with_task_adapter("host_wait", "board_wait_start");
    let output = compile_flow(&flow, &options).expect("compile suspend closure probe");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
