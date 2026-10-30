//! Lower source fixtures with the backend's interfaces and Board handlers.

#[path = "frontend.rs"]
mod frontend;

pub fn try_lower(
    files: &[(&str, &str)],
    roots: emela_core::Roots,
) -> Result<emela_core::Module, Vec<String>> {
    let sources: Vec<_> = emela_core::core_sources()
        .iter()
        .chain(emela_codegen_emb::bundled_sources())
        .copied()
        .collect();
    frontend::try_lower_with(files, roots, &sources, emela_core::bundled_files())
}
