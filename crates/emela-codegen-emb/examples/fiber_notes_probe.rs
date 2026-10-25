use emela_codegen_emb::{Options, compile_flow, flow_from_core};
use emela_core::Roots;
use emela_lir::{RuntimeOp, Stmt, Value};
use std::{fs, path::PathBuf};

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/fiber-notes-probe/generated"));
    let core = support::try_lower(
        &[("main.emel", include_str!("support/fiber_notes.emel"))],
        Roots::Public,
    )
    .expect("lower notes source");
    let mut flow = flow_from_core(&core).expect("verified source Flow IR");
    // Keep sibling bodies masked until their failures reach the scope.
    let (_, late) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "late")
        .unwrap();
    let hold = late.stmts.alloc(Stmt::Let {
        dst: None,
        on_defect: None,
        on_suspend: None,
        value: Value::Runtime {
            op: RuntimeOp::Hold,
            args: vec![],
        },
    });
    late.seqs[late.body.unwrap()].stmts.insert(0, hold);
    let options = Options::default()
        .with_heap_bytes(4096)
        .with_string_capacity(256)
        .with_fiber_capacity(8)
        .with_frame_capacity(32)
        .with_frame_bytes(272)
        .with_deferred_capacity(0)
        .with_task_capacity(4)
        .with_event_capacity(4)
        .with_task_adapter("host_wait", "board_wait_start");
    let output = compile_flow(&flow, &options).expect("verify masked Flow and compile notes probe");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
