use emela_codegen_emb::{Options, compile_flow};
use std::{fs, path::PathBuf};

#[path = "support/shared_cell_cycle.rs"]
mod fixture;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/shared-cell-cycle-probe/generated"));
    let flow = fixture::flow();
    let output = compile_flow(&flow, &Options::default().with_heap_bytes(2048))
        .expect("compile Cell closure cycle");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
