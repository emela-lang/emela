//! Experimental features (spec `[intro.experimental]`): the features the compiler knows, which of
//! them each Pome enables, and the warnings for the names it does not know (W0206) and for values
//! that are not booleans (W0207).
//!
//! A Pome enables a feature in the `[experimental]` table of its `Pome.toml` (`[module.pome]`);
//! the commands add the flags `--experimental-<name>` for the Pome being built (`[cmd.build]`).
//! The checker asks [`crate::Analysis::experimental`] for the features of the Pome a module
//! belongs to.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use line_index::{TextRange, TextSize};
use serde::Deserialize;
use toml::Spanned;

use crate::code;
use crate::diagnostic::{Diagnostic, Location, Span};
use crate::source::SourceDb;

/// The prefix of the flag that enables a feature for one run: `--experimental-<name>`.
pub const FLAG_PREFIX: &str = "--experimental-";

/// Where a feature is in its life (`[intro.experimental]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Enabled by `[experimental]` or by a flag.
    Experimental,
    /// Always on. Its name is still accepted, with W0206, for at least one minor version.
    Stable,
    /// Gone. Its name is accepted with W0206.
    Removed,
}

/// A feature in the table of `[intro.experimental]`, or one that has left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    /// Lower-case English words joined by `-` (`[intro.experimental]`).
    pub name: &'static str,
    /// The version that introduced it.
    pub since: &'static str,
    pub stage: Stage,
    /// What it changes, as in the table of `[intro.experimental]`. `--help` shows it.
    pub summary: &'static str,
}

/// Every feature the compiler knows, in the order of the table of `[intro.experimental]`. A
/// feature that becomes stable or is removed stays here with its [`Stage`], so that its name is
/// still accepted with W0206.
pub const FEATURES: &[Feature] = &[];

/// The features that can be enabled now, for the flags `--help` lists (`[cmd.build]`).
pub fn experimental_features() -> impl Iterator<Item = &'static Feature> {
    FEATURES.iter().filter(|f| f.stage == Stage::Experimental)
}

/// The experimental features enabled for one Pome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features(BTreeSet<String>);

impl Features {
    pub fn new<S: Into<String>>(names: impl IntoIterator<Item = S>) -> Features {
        Features(names.into_iter().map(Into::into).collect())
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The names, in order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

/// The features a dependency enables, from the `[experimental]` of its own `Pome.toml`
/// (`[module.pome]`). Names that are not experimental features are left out without a warning:
/// W0206 is reported only when building that Pome (`[pome.manifest]`).
pub fn dependency_features(settings: &BTreeMap<String, bool>) -> Features {
    enabled_in(FEATURES, settings)
}

fn enabled_in(registry: &[Feature], settings: &BTreeMap<String, bool>) -> Features {
    Features::new(
        settings
            .iter()
            .filter(|&(name, &on)| on && lookup(registry, name).is_ok())
            .map(|(name, _)| name.clone()),
    )
}

/// The features of the Pome being built, from its `Pome.toml` (`text` at `path`; `None` in
/// single-file mode) and the flags of the command, with W0206 and W0207. `Pome.toml` is added to
/// `sources` only when a diagnostic points into it.
pub(crate) fn project_features(
    manifest: Option<(&str, &Path)>,
    flags: &[String],
    sources: &mut SourceDb,
) -> (Features, Vec<Diagnostic>) {
    project_features_in(FEATURES, manifest, flags, sources)
}

#[derive(Deserialize)]
struct File {
    experimental: Option<BTreeMap<Spanned<String>, Spanned<toml::Value>>>,
}

fn project_features_in(
    registry: &[Feature],
    manifest: Option<(&str, &Path)>,
    flags: &[String],
    sources: &mut SourceDb,
) -> (Features, Vec<Diagnostic>) {
    let mut enabled = BTreeSet::new();
    let mut diagnostics = Vec::new();
    // A file that is not TOML, or whose `[experimental]` is not a table, is left to the stages
    // that read the rest of it: `[pome.manifest]` gives no rule for it here.
    let table = manifest.and_then(|(text, path)| {
        let table = toml::from_str::<File>(text).ok()?.experimental?;
        Some((table, text, path))
    });
    if let Some((table, text, path)) = table {
        let mut file = None;
        let mut at = |range: std::ops::Range<usize>| {
            let file = *file.get_or_insert_with(|| sources.add(path, text));
            Location::Span(Span::new(
                file,
                TextRange::new(
                    TextSize::new(range.start as u32),
                    TextSize::new(range.end as u32),
                ),
            ))
        };
        // In the order of the file, not of the names.
        let mut entries: Vec<_> = table.iter().collect();
        entries.sort_by_key(|(name, _)| name.span().start);
        for (name, value) in entries {
            // `[pome.manifest]`: an unknown, stable or removed name is reported whatever its
            // value, and a value that is not a boolean besides.
            let known = match lookup(registry, name.get_ref()) {
                Ok(_) => true,
                Err(warning) => {
                    diagnostics.push(warning.at(at(name.span())));
                    false
                }
            };
            match value.get_ref() {
                toml::Value::Boolean(on) => {
                    if known && *on {
                        enabled.insert(name.get_ref().clone());
                    }
                }
                _ => diagnostics.push(
                    Diagnostic::warning(format!(
                        "`[experimental].{}` must be `true` or `false`",
                        name.get_ref()
                    ))
                    .with_code(code::EXPERIMENTAL_NOT_A_BOOLEAN)
                    .at(at(value.span())),
                ),
            }
        }
    }
    // A flag only adds (`[cmd.build]`). The same flag twice is reported once.
    for name in flags.iter().collect::<BTreeSet<_>>() {
        match lookup(registry, name) {
            Ok(_) => {
                enabled.insert(name.clone());
            }
            Err(warning) => diagnostics.push(warning),
        }
    }
    (Features(enabled), diagnostics)
}

/// The feature `name` if it can be enabled now; otherwise W0206 without a location.
fn lookup<'a>(registry: &'a [Feature], name: &str) -> Result<&'a Feature, Diagnostic> {
    let message = match registry.iter().find(|f| f.name == name) {
        Some(feature) => match feature.stage {
            Stage::Experimental => return Ok(feature),
            Stage::Stable => format!("experimental feature `{name}` is now stable and always on"),
            Stage::Removed => format!("experimental feature `{name}` was removed"),
        },
        None => format!("unknown experimental feature `{name}`"),
    };
    Err(Diagnostic::warning(message).with_code(code::UNKNOWN_EXPERIMENTAL_FEATURE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render;

    const REGISTRY: &[Feature] = &[
        Feature {
            name: "trying",
            since: "0.29.0",
            stage: Stage::Experimental,
            summary: "",
        },
        Feature {
            name: "settled",
            since: "0.27.0",
            stage: Stage::Stable,
            summary: "",
        },
        Feature {
            name: "dropped",
            since: "0.27.0",
            stage: Stage::Removed,
            summary: "",
        },
    ];

    fn project(table: &str, flags: &[&str]) -> (Vec<String>, String) {
        let text = format!("[experimental]\n{table}");
        let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
        let mut sources = SourceDb::new();
        let (features, diagnostics) = project_features_in(
            REGISTRY,
            Some((&text, Path::new("Pome.toml"))),
            &flags,
            &mut sources,
        );
        (
            features.iter().map(str::to_string).collect(),
            render(&diagnostics, &sources, false),
        )
    }

    /// `[intro.experimental]`, `[pome.manifest]`: an experimental feature set to `true` is
    /// enabled; `false` leaves it off. A stable or removed name is W0206 with its own message, at
    /// the key, and enables nothing.
    #[test]
    fn reads_the_table() {
        let (enabled, rendered) = project("trying = true\nsettled = true\ndropped = false\n", &[]);
        assert_eq!(enabled, ["trying"]);
        insta::assert_snapshot!(rendered, @r"
        warning[W0206]: experimental feature `settled` is now stable and always on
           ╭─[ Pome.toml:3:1 ]
           │
         3 │ settled = true
           │ ───┬───  
           │    ╰───── 
        ───╯

        warning[W0206]: experimental feature `dropped` was removed
           ╭─[ Pome.toml:4:1 ]
           │
         4 │ dropped = false
           │ ───┬───  
           │    ╰───── 
        ───╯

        2 warnings
        ");
        assert_eq!(project("trying = false\n", &[]), (vec![], String::new()));
    }

    /// `[pome.manifest]`: a value that is not a boolean is W0207 at the value, and the key has
    /// no effect.
    #[test]
    fn a_value_that_is_not_a_boolean_has_no_effect() {
        let (enabled, rendered) = project("trying = \"yes\"\n", &[]);
        assert!(enabled.is_empty());
        insta::assert_snapshot!(rendered, @r#"
        warning[W0207]: `[experimental].trying` must be `true` or `false`
           ╭─[ Pome.toml:2:10 ]
           │
         2 │ trying = "yes"
           │          ──┬──  
           │            ╰──── 
        ───╯

        1 warning
        "#);
    }

    /// `[cmd.build]`: a flag adds to `[experimental]` and cannot disable what it enables. A flag
    /// of a stable, removed or unknown feature is W0206 without a location.
    #[test]
    fn flags_add_to_the_table() {
        assert_eq!(
            project("trying = true\n", &["trying"]),
            (vec!["trying".to_string()], String::new())
        );
        assert_eq!(
            project("trying = false\n", &["trying"]),
            (vec!["trying".to_string()], String::new())
        );
        let (enabled, rendered) = project("", &["settled", "dropped", "nope", "nope"]);
        assert!(enabled.is_empty());
        insta::assert_snapshot!(rendered, @r"
        warning[W0206]: experimental feature `dropped` was removed

        warning[W0206]: unknown experimental feature `nope`

        warning[W0206]: experimental feature `settled` is now stable and always on

        3 warnings
        ");
    }

    /// `[module.pome]`: a dependency enables the experimental features its own `[experimental]`
    /// sets to `true`; other names are ignored without a warning.
    #[test]
    fn features_of_a_dependency() {
        let settings = BTreeMap::from([
            ("trying".to_string(), true),
            ("settled".to_string(), true),
            ("nope".to_string(), true),
        ]);
        assert_eq!(enabled_in(REGISTRY, &settings), Features::new(["trying"]));
        let off = BTreeMap::from([("trying".to_string(), false)]);
        assert!(enabled_in(REGISTRY, &off).is_empty());
    }

    /// `[intro.experimental]`: names are lower-case English words joined by `-`, and each
    /// feature is listed once.
    #[test]
    fn names_of_the_features() {
        for (i, feature) in FEATURES.iter().enumerate() {
            assert!(
                feature
                    .name
                    .split('-')
                    .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase())),
                "{}",
                feature.name
            );
            assert!(
                FEATURES[..i].iter().all(|f| f.name != feature.name),
                "{}",
                feature.name
            );
        }
    }
}
