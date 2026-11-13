//! Build targets (spec chapter 16) and the default target of a project (`[build].target` in
//! `Pome.toml`, spec 4.7).

use std::path::Path;

use crate::code;
use crate::diagnostic::{Diagnostic, Location};
use crate::pipeline::{Input, PROJECT_FILE};
use crate::source::FileSystem;

/// What build, run and test compile to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    /// JS run on node (Tier 1).
    #[default]
    Js,
    /// C compiled with cc to a native executable (Tier 2).
    C,
    /// WebAssembly (WASI) run with `wasmtime` (Tier 1).
    Wasm,
}

impl Target {
    /// The names `--target` and `[build].target` accept.
    pub const NAMES: [&'static str; 3] = ["js", "c", "wasm"];

    pub fn parse(name: &str) -> Option<Target> {
        match name {
            "js" => Some(Target::Js),
            "c" => Some(Target::C),
            "wasm" => Some(Target::Wasm),
            _ => None,
        }
    }

    /// The name used by the CLI and `[build].target`.
    pub fn name(self) -> &'static str {
        match self {
            Target::Js => "js",
            Target::C => "c",
            Target::Wasm => "wasm",
        }
    }

    /// The target name used by external bindings.
    pub fn external_target(self) -> &'static str {
        self.name()
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The project's default target, `[build].target` of its `Pome.toml`; `None` if it has none (or
/// for a single file). An unknown target is E0232. Other contents of the file are not read here.
pub fn project_target(fs: &dyn FileSystem, input: &Input) -> Result<Option<Target>, Diagnostic> {
    if input.single_file {
        return Ok(None);
    }
    let path = input.base.join(PROJECT_FILE);
    let Ok(text) = fs.read_to_string(&path) else {
        return Ok(None);
    };
    target_in(&text, &path)
}

/// `[build].target` in the text of a `Pome.toml` at `path`. A file that is not TOML, or whose
/// `target` is not a string, has no target as far as this is concerned.
fn target_in(text: &str, path: &Path) -> Result<Option<Target>, Diagnostic> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Ok(None);
    };
    let Some(name) = table
        .get("build")
        .and_then(|build| build.get("target"))
        .and_then(|target| target.as_str())
    else {
        return Ok(None);
    };
    match Target::parse(name) {
        Some(target) => Ok(Some(target)),
        None => Err(
            Diagnostic::error(format!("unknown target `{name}` in `[build].target`"))
                .with_code(code::UNKNOWN_BUILD_TARGET)
                .with_note(format!("expected one of {}", quoted(&Target::NAMES)))
                .at(Location::Path(path.to_owned())),
        ),
    }
}

/// The build target of the project that contains `path` (`[cmd.doctor]`): `[build].target` of
/// the nearest `Pome.toml` above it, else `js`. `None` outside a project.
///
/// Unlike [`Input::resolve`], any path in the project is accepted, not only the project directory
/// and the source and test roots, and no dependency is resolved, so nothing is fetched.
pub fn enclosing_project_target(
    fs: &dyn FileSystem,
    path: &Path,
) -> Result<Option<Target>, Diagnostic> {
    let not_found = || {
        Diagnostic::error(format!("`{}` not found", path.display())).with_code(code::PATH_NOT_FOUND)
    };
    let absolute = fs.absolute(path).map_err(|_| not_found())?;
    if !fs.is_file(&absolute) && !fs.is_dir(&absolute) {
        return Err(not_found());
    }
    let Some(project) = absolute
        .ancestors()
        .find(|dir| fs.is_file(&dir.join(PROJECT_FILE)))
    else {
        return Ok(None);
    };

    let manifest = project.join(PROJECT_FILE);
    let Ok(text) = fs.read_to_string(&manifest) else {
        return Ok(Some(Target::default()));
    };
    Ok(Some(target_in(&text, &manifest)?.unwrap_or_default()))
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Result<Option<Target>, String> {
        target_in(text, Path::new("Pome.toml")).map_err(|d| format!("{:?} {}", d.code, d.message))
    }

    #[test]
    fn reads_build_target() {
        assert_eq!(target(""), Ok(None));
        assert_eq!(target("[pome]\nname = \"x\"\n"), Ok(None));
        assert_eq!(target("[build]\ntarget = \"c\"\n"), Ok(Some(Target::C)));
        assert_eq!(target("[build]\ntarget = \"js\"\n"), Ok(Some(Target::Js)));
        assert_eq!(
            target("[build]\ntarget = \"node\"\n"),
            Err("Some(\"E0232\") unknown target `node` in `[build].target`".into())
        );
    }
}
