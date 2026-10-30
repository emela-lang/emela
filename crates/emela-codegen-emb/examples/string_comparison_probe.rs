use emela_codegen_emb::{Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};
#[path = "../tests/support/embedded_sources.rs"]
mod support;
fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/string-comparison-probe/generated"));
    let core = support::try_lower(
        &[("main.emel", include_str!("support/string_comparison.emel"))],
        Roots::Public,
    )
    .expect("lower String comparisons");
    let output = compile(&core, &Options::default().with_isr_entry("lt"))
        .expect("compile heapless String comparisons");
    fs::create_dir_all(&dir).unwrap();
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).unwrap();
    }
}
