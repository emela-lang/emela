use emela_codegen_emb::{Options, compile_flow};
use std::{fs, path::PathBuf};
#[path = "support/external_scalar_flow.rs"]
mod fixture;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/external-scalar-probe/generated"));
    let output = compile_flow(
        &fixture::flow(),
        &Options::default()
            .with_heap_bytes(0)
            .with_isr_entry("direct_integer")
            .with_isr_entry("direct_real")
            .with_isr_safe_external("board_integer")
            .with_isr_safe_external("board_real"),
    )
    .expect("compile native scalar external ABI");
    fs::create_dir_all(&dir).expect("create probe directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write probe file");
    }
}
