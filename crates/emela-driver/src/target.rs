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
}

impl Target {
    /// The names `--target` and `[build].target` accept.
    pub const NAMES: [&'static str; 2] = ["js", "c"];

    pub fn parse(name: &str) -> Option<Target> {
        match name {
            "js" => Some(Target::Js),
            "c" => Some(Target::C),
            _ => None,
        }
    }

    /// The name used in `@external(<name>, ...)`.
    pub fn name(self) -> &'static str {
        match self {
            Target::Js => "js",
            Target::C => "c",
        }
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
