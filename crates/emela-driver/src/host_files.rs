//! The files `@external` points to, in the JS output (spec 12.1): copied under the output
//! directory and imported by relative paths, so that the output works wherever it is moved.
//!
//! ```text
//! <out>/emela_project/<path under the source root>           the project's files
//! <out>/emela_tests/<path under the test root>                the files of the test root
//! <out>/emela_pomes/<source path>@<version>/<path under src>  a dependency's files
//! ```
//!
//! Every file under a source root other than `.emel` is copied, so that a host file may import
//! its neighbours. A single file has no source root of its own (its directory may be anything), so
//! only the files `@external` points to are copied.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use emela_core::{HostModule, HostRef, Module};

use crate::code;
use crate::diagnostic::{Diagnostic, Location};
use crate::output::OutputFile;
use crate::pipeline::{Analysis, Input, SOURCE_DIR};

pub const PROJECT_DIR: &str = "emela_project";
pub const TESTS_DIR: &str = "emela_tests";
pub const POMES_DIR: &str = "emela_pomes";

/// A directory whose host files go to `out` in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRoot {
    pub src: PathBuf,
    /// Relative to the output directory, `/`-separated.
    pub out: String,
    /// Copy every file under `src` other than `.emel`, not only the ones pointed to.
    pub copy_all: bool,
}

pub(crate) fn roots(input: &Input) -> Vec<HostRoot> {
    let mut roots = vec![HostRoot {
        src: input.root.clone(),
        out: PROJECT_DIR.into(),
        copy_all: !input.single_file,
    }];
    if let Some(tests) = &input.tests {
        roots.push(HostRoot {
            src: tests.clone(),
            out: TESTS_DIR.into(),
            copy_all: true,
        });
    }
    for pome in &input.dependencies.pomes {
        roots.push(HostRoot {
            src: pome.dir.join(SOURCE_DIR),
            out: format!("{POMES_DIR}/{}@{}", pome.name, pome.version),
            copy_all: true,
        });
    }
    roots
}

/// The module with every `@external(target, "./file", ...)` pointing to its copy in the output,
/// and the copies to write.
pub(crate) fn place(
    module: &Module,
    analysis: &Analysis,
    target: &str,
) -> Result<(Module, Vec<OutputFile>), Vec<Diagnostic>> {
    let mut module = module.clone();
    // Compared after resolving `..` and symbolic links: `[replace]` paths are written relative to
    // the project, and lowering resolves a file from its source's directory.
    let roots: Vec<PathBuf> = analysis
        .host_roots
        .iter()
        .map(|r| canonical(&r.src))
        .collect();
    let mut used: BTreeSet<usize> = BTreeSet::new();
    let mut single: BTreeSet<(usize, PathBuf)> = BTreeSet::new();
    for (_, function) in module.functions.iter_mut() {
        let Some(external) = &mut function.external else {
            continue;
        };
        for binding in external.bindings.iter_mut().filter(|b| b.target == target) {
            let HostRef::Import { module: host, .. } = &mut binding.host else {
                continue;
            };
            let HostModule::File(path) = host else {
                continue;
            };
            let file = canonical(Path::new(&*path));
            // The deepest root containing the file (a dependency's directory is never under the
            // project's source root, but `[replace]` may point anywhere).
            let Some(i) = (0..roots.len())
                .filter(|&i| file.starts_with(&roots[i]))
                .max_by_key(|&i| roots[i].components().count())
            else {
                continue;
            };
            let root = &analysis.host_roots[i];
            let relative = file.strip_prefix(&roots[i]).expect("under the root");
            if root.copy_all {
                used.insert(i);
            } else {
                single.insert((i, file.clone()));
            }
            *host = HostModule::Named(format!("./{}/{}", root.out, url_path(relative)));
        }
    }

    let mut files = Vec::new();
    let mut errors = Vec::new();
    for i in used {
        let root = &analysis.host_roots[i];
        match host_files_under(&roots[i]) {
            Ok(found) => {
                for (relative, contents) in found {
                    files.push(OutputFile::new(
                        format!("{}/{}", root.out, relative),
                        contents,
                    ));
                }
            }
            Err(e) => errors.push(read_error(&root.src, e)),
        }
    }
    for (i, file) in single {
        let root = &analysis.host_roots[i];
        let relative = file.strip_prefix(&roots[i]).expect("under the root");
        match std::fs::read(&file) {
            Ok(contents) => files.push(OutputFile::new(
                format!("{}/{}", root.out, slash(relative)),
                contents,
            )),
            Err(e) => errors.push(read_error(&file, e)),
        }
    }
    if errors.is_empty() {
        Ok((module, files))
    } else {
        Err(errors)
    }
}

// Every file under `dir` other than `.emel` and hidden ones, as (`/`-separated relative path,
// contents), sorted.
fn host_files_under(dir: &Path) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else if path.extension().is_none_or(|e| e != "emel") {
                let relative = slash(path.strip_prefix(dir).expect("under dir"));
                out.push((relative, std::fs::read(&path)?));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

// A relative URL path: unreserved characters, `/` and `@` as they are, others percent-encoded.
fn url_path(path: &Path) -> String {
    let mut out = String::new();
    for b in slash(path).bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn read_error(path: &Path, e: std::io::Error) -> Diagnostic {
    Diagnostic::error(format!("cannot read a host file for the output: {e}"))
        .with_code(code::CANNOT_READ)
        .at(Location::Path(path.to_path_buf()))
}
