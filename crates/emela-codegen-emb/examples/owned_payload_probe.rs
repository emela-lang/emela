use emela_codegen_emb::{Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/owned-payload-probe/generated"));
    let core = support::try_lower(
        &[("main.emel", include_str!("support/owned_payload.emel"))],
        Roots::Public,
    )
    .expect("lower owned payload source");
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(1024)
            .with_string_capacity(64),
    )
    .expect("compile owned payload source");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
