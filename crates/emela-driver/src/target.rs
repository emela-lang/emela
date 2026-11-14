//! Build targets (spec chapter 16) and the default target of a project (`[build].target` in
//! `Pome.toml`, spec 4.7).

use std::path::Path;

use line_index::{TextRange, TextSize};
use serde::Deserialize;
use toml::Spanned;

use crate::code;
use crate::diagnostic::{Diagnostic, Location, Span};
use crate::pipeline::{Input, PROJECT_FILE};
use crate::source::{FileSystem, SourceDb};

/// What build, run and test compile to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    /// JS run on the JS host (Tier 1).
    #[default]
    Js,
    /// An executable for the machine that builds, made by emitting C and compiling it with the C
    /// compiler of that machine (Tier 2, `[target.native]`).
    Native,
    /// WebAssembly (WASI) run with `wasmtime` (Tier 1).
    Wasm,
}

impl Target {
    /// The names `--target` and `[build].target` accept, without the deprecated
    /// [`Target::DEPRECATED_NATIVE`].
    pub const NAMES: [&'static str; 3] = ["js", "native", "wasm"];

    /// The deprecated name of the native target (`[target.native]`). It selects the native target
    /// and reports W0903 until 0.29, after which it is an unknown target.
    pub const DEPRECATED_NATIVE: &'static str = "c";

    /// The target that `name` names, including the deprecated [`Target::DEPRECATED_NATIVE`].
    pub fn parse(name: &str) -> Option<Target> {
        match name {
            "js" => Some(Target::Js),
            "native" | Target::DEPRECATED_NATIVE => Some(Target::Native),
            "wasm" => Some(Target::Wasm),
            _ => None,
        }
    }

    /// The name used by the CLI and `[build].target`.
    pub fn name(self) -> &'static str {
        match self {
            Target::Js => "js",
            Target::Native => "native",
            Target::Wasm => "wasm",
        }
    }

    /// The language of the code that the target links with: the first argument of the
    /// `@external` and `@export` that take effect on it (`[ext.decl]`).
    pub fn language(self) -> &'static str {
        match self {
            Target::Js => "js",
            Target::Native => "c",
            Target::Wasm => "wasm",
        }
    }

    /// The target whose language is `language` ([`Target::language`]).
    pub fn of_language(language: &str) -> Option<Target> {
        [Target::Js, Target::Native, Target::Wasm]
            .into_iter()
            .find(|t| t.language() == language)
    }
}

/// W0903 for the deprecated build target `c` (`[target.native]`), with no location.
pub fn deprecated_target_warning() -> Diagnostic {
    Diagnostic::warning(format!(
        "target `{}` is deprecated; use `native`",
        Target::DEPRECATED_NATIVE
    ))
    .with_code(code::DEPRECATED_TARGET)
    .with_help(format!(
        "`{}` is removed in 0.29",
        Target::DEPRECATED_NATIVE
    ))
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
/// the nearest `Pome.toml` above it, else `js`. `None` outside a project. The second value is
/// W0903 for the deprecated build target `c` in that `Pome.toml` ([`deprecated_targets`]), whose
/// file is added to `sources`.
///
/// Unlike [`Input::resolve`], any path in the project is accepted, not only the project directory
/// and the source and test roots, and no dependency is resolved, so nothing is fetched.
pub fn enclosing_project_target(
    fs: &dyn FileSystem,
    path: &Path,
    sources: &mut SourceDb,
) -> Result<(Option<Target>, Vec<Diagnostic>), Diagnostic> {
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
        return Ok((None, Vec::new()));
    };

    let manifest = project.join(PROJECT_FILE);
    let Ok(text) = fs.read_to_string(&manifest) else {
        return Ok((Some(Target::default()), Vec::new()));
    };
    let target = target_in(&text, &manifest)?.unwrap_or_default();
    Ok((Some(target), deprecated_targets(&text, &manifest, sources)))
}

#[derive(Deserialize)]
struct Manifest {
    pome: Option<PomeSection>,
    build: Option<BuildSection>,
}

#[derive(Deserialize)]
struct PomeSection {
    targets: Option<Vec<Spanned<toml::Value>>>,
}

#[derive(Deserialize)]
struct BuildSection {
    target: Option<Spanned<toml::Value>>,
}

/// W0903 for each use of the deprecated build target `c` (`[target.native]`) in `text`, the
/// `Pome.toml` at `path`: `[build].target` and each item of `[pome].targets`, at the value.
/// `Pome.toml` is added to `sources` only when there is one. A file that is not TOML, or whose
/// sections do not have the expected shape, is left to the stages that read the rest of it.
pub(crate) fn deprecated_targets(
    text: &str,
    path: &Path,
    sources: &mut SourceDb,
) -> Vec<Diagnostic> {
    let Ok(manifest) = toml::from_str::<Manifest>(text) else {
        return Vec::new();
    };
    let values = manifest
        .build
        .and_then(|b| b.target)
        .into_iter()
        .chain(manifest.pome.and_then(|p| p.targets).into_iter().flatten());
    let ranges: Vec<_> = values
        .filter(|v| v.get_ref().as_str() == Some(Target::DEPRECATED_NATIVE))
        .map(|v| v.span())
        .collect();
    if ranges.is_empty() {
        return Vec::new();
    }
    let file = sources.add(path, text);
    ranges
        .into_iter()
        .map(|range| {
            let range = TextRange::new(
                TextSize::new(range.start as u32),
                TextSize::new(range.end as u32),
            );
            deprecated_target_warning().at(Location::Span(Span::new(file, range)))
        })
        .collect()
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
    fn reports_the_deprecated_target_in_the_manifest() {
        let text = "[pome]\nname = \"x\"\ntargets = [\"js\", \"c\"]\n\n[build]\ntarget = \"c\"\n";
        let mut sources = SourceDb::new();
        let found: Vec<_> = deprecated_targets(text, Path::new("Pome.toml"), &mut sources)
            .into_iter()
            .map(|d| {
                let Some(Location::Span(span)) = d.location else {
                    panic!("no span: {d:?}");
                };
                (d.code, d.message, &text[span.range])
            })
            .collect();
        let w = |at| {
            (
                Some("W0903"),
                "target `c` is deprecated; use `native`".to_owned(),
                at,
            )
        };
        assert_eq!(found, [w("\"c\""), w("\"c\"")]);
        let mut sources = SourceDb::new();
        let none = "[pome]\ntargets = [\"native\"]\n[build]\ntarget = \"native\"\n";
        assert!(deprecated_targets(none, Path::new("Pome.toml"), &mut sources).is_empty());
    }

    #[test]
    fn reads_build_target() {
        assert_eq!(target(""), Ok(None));
        assert_eq!(target("[pome]\nname = \"x\"\n"), Ok(None));
        assert_eq!(
            target("[build]\ntarget = \"native\"\n"),
            Ok(Some(Target::Native))
        );
        assert_eq!(
            target("[build]\ntarget = \"c\"\n"),
            Ok(Some(Target::Native))
        );
        assert_eq!(target("[build]\ntarget = \"js\"\n"), Ok(Some(Target::Js)));
        assert_eq!(
            target("[build]\ntarget = \"node\"\n"),
            Err("Some(\"E0232\") unknown target `node` in `[build].target`".into())
        );
    }
}
