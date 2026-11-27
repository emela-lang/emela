//! Collecting and rendering diagnostics. Uncolored output is snapshotted.

use std::path::Path;

use emela_driver::{Analysis, Frontend, Input, MemoryFiles, check};

mod effects;
mod exports;
mod project;
mod types;

/// The diagnostics of checking `path`, but W4401: most programs here still write `use E` as an
/// expression (deprecated, `[effect.use]`). [`check_files_with_deprecated`] keeps them.
fn check_files(files: &[(&str, &str)], path: &str, frontend: &mut impl Frontend) -> Analysis {
    let mut analysis = check_files_with_deprecated(files, path, frontend);
    analysis
        .diagnostics
        .retain(|d| d.code != Some(emela_driver::code::DEPRECATED_USE));
    analysis
}

fn check_files_with_deprecated(
    files: &[(&str, &str)],
    path: &str,
    frontend: &mut impl Frontend,
) -> Analysis {
    let fs = MemoryFiles::new(files.iter().copied());
    let input = Input::resolve(&fs, Path::new(path)).unwrap();
    check(&fs, &input, frontend).0
}
