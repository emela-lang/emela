//! `Pome.toml` (spec 4.7).
//!
//! `[build]` is not read here: emela-driver reads `[build].target` on its own (spec
//! `[module.pome]`, E0232), since only the build of the project itself uses it.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

use crate::source_path::SourcePath;
use crate::version::Version;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: SourcePath,
    /// `[pome].version`, held as the tag (`v` + the written version).
    pub version: Version,
    /// The minimum compiler version the Pome needs, as written (`0.20.1`). Not compared here:
    /// emela-driver compares it with the compiler's version when it reads the dependencies (spec
    /// `[module.pome]`, E0234 and W0203).
    pub emela: String,
    /// `[pome].module`: the root name the author chose.
    pub module: Option<String>,
    /// `[pome].targets`: the targets the Pome runs on; `None` for all of them.
    pub targets: Option<Vec<String>>,
    pub dependencies: BTreeMap<SourcePath, Requirement>,
    /// `[dev-dependencies]`: used only by the tests under `test/`; ignored when depended on.
    pub dev_dependencies: BTreeMap<SourcePath, Requirement>,
    pub replace: BTreeMap<SourcePath, PathBuf>,
    /// `[external].allow`: the Pomes whose `@external` the Pome allows when it is built.
    pub external: Vec<SourcePath>,
}

/// The targets `[pome].targets` may name (spec chapter 16).
pub const TARGETS: [&str; 3] = ["js", "wasm", "c"];

/// An entry of `[dependencies]` or `[dev-dependencies]`: "this version or above".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub version: Version,
    /// The root name the depending side gives the dependency.
    pub module: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError(pub String);

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ManifestError {}

// The file as written. `[build]` is read by the driver (`[build].target`), so it is not here.
#[derive(Deserialize)]
struct File {
    pome: PomeSection,
    #[serde(default)]
    dependencies: BTreeMap<SourcePath, RequirementEntry>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<SourcePath, RequirementEntry>,
    #[serde(default)]
    replace: BTreeMap<SourcePath, ReplaceEntry>,
    #[serde(default)]
    external: ExternalSection,
}

#[derive(Deserialize, Default)]
struct ExternalSection {
    #[serde(default)]
    allow: Vec<SourcePath>,
}

#[derive(Deserialize)]
struct PomeSection {
    name: SourcePath,
    version: String,
    emela: String,
    module: Option<String>,
    targets: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RequirementEntry {
    Version(Version),
    Table {
        version: Version,
        module: Option<String>,
    },
}

#[derive(Deserialize)]
struct ReplaceEntry {
    path: PathBuf,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
        let file: File = toml::from_str(text).map_err(|e| ManifestError(e.to_string()))?;
        let version = Version::parse(&format!("v{}", file.pome.version)).map_err(|_| {
            ManifestError(format!(
                "`[pome].version` `{}` is not a semver such as `0.3.2`",
                file.pome.version
            ))
        })?;
        if !file.pome.name.is_version_of(&version) {
            return Err(ManifestError(format!(
                "`[pome].version` {} does not match the major of `{}` (v2 and above end in `/vN`)",
                file.pome.version, file.pome.name
            )));
        }
        let dependencies = requirements(file.dependencies)?;
        let dev_dependencies = requirements(file.dev_dependencies)?;
        if let Some(both) = dev_dependencies
            .keys()
            .find(|p| dependencies.contains_key(*p))
        {
            return Err(ManifestError(format!(
                "`{both}` is in both [dependencies] and [dev-dependencies]"
            )));
        }
        if let Some(unknown) = file
            .pome
            .targets
            .iter()
            .flatten()
            .find(|t| !TARGETS.contains(&t.as_str()))
        {
            return Err(ManifestError(format!(
                "`[pome].targets`: unknown target `{unknown}` (expected one of {})",
                TARGETS.join(", ")
            )));
        }
        Ok(Manifest {
            name: file.pome.name,
            version,
            emela: file.pome.emela,
            module: file.pome.module,
            targets: file.pome.targets,
            dependencies,
            dev_dependencies,
            replace: file.replace.into_iter().map(|(k, v)| (k, v.path)).collect(),
            external: file.external.allow,
        })
    }
}

fn requirements(
    entries: BTreeMap<SourcePath, RequirementEntry>,
) -> Result<BTreeMap<SourcePath, Requirement>, ManifestError> {
    let mut requirements = BTreeMap::new();
    for (path, entry) in entries {
        let requirement = match entry {
            RequirementEntry::Version(version) => Requirement {
                version,
                module: None,
            },
            RequirementEntry::Table { version, module } => Requirement { version, module },
        };
        if !path.is_version_of(&requirement.version) {
            return Err(ManifestError(format!(
                "`{path}` = \"{}\": the major does not match the path (v2 and above end in `/vN`)",
                requirement.version
            )));
        }
        requirements.insert(path, requirement);
    }
    Ok(requirements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_example_of_the_spec() {
        let m = Manifest::parse(
            r#"
[pome]
name = "github.com/emela-lang/quince"
version = "0.3.2"
emela = "0.20.1"
module = "Quince"

[dependencies]
"github.com/emela-lang/x" = "v0.2.0"
"github.com/acme/http"    = { version = "v1.4.0", module = "AcmeHttp" }

[replace]
"github.com/emela-lang/x" = { path = "../x" }

[build]
target = "c"
"#,
        )
        .unwrap();
        assert_eq!(m.name.as_str(), "github.com/emela-lang/quince");
        assert_eq!(m.version.as_str(), "v0.3.2");
        assert_eq!(m.module.as_deref(), Some("Quince"));
        let http = &m.dependencies[&SourcePath::parse("github.com/acme/http").unwrap()];
        assert_eq!(
            (http.version.as_str(), http.module.as_deref()),
            ("v1.4.0", Some("AcmeHttp"))
        );
        assert_eq!(
            m.replace[&SourcePath::parse("github.com/emela-lang/x").unwrap()],
            PathBuf::from("../x")
        );
    }

    #[test]
    fn rejects_a_major_that_does_not_match_the_path() {
        let base =
            "[pome]\nname = \"github.com/acme/x\"\nversion = \"0.1.0\"\nemela = \"0.20.1\"\n";
        assert!(
            Manifest::parse(&format!(
                "{base}[dependencies]\n\"github.com/acme/http\" = \"v2.0.0\"\n"
            ))
            .is_err()
        );
        assert!(
            Manifest::parse(&format!(
                "{base}[dependencies]\n\"github.com/acme/http/v2\" = \"v2.0.0\"\n"
            ))
            .is_ok()
        );
        assert!(Manifest::parse(&base.replace("0.1.0", "2.0.0")).is_err());
    }

    #[test]
    fn requires_name_version_and_emela() {
        assert!(
            Manifest::parse("[pome]\nname = \"github.com/acme/x\"\nversion = \"0.1.0\"\n").is_err()
        );
        assert!(
            Manifest::parse(
                "[pome]\nname = \"github.com/acme/x\"\nversion = \"v0.1.0\"\nemela = \"0.20.1\"\n"
            )
            .is_err()
        );
    }

    #[test]
    fn reads_external_and_targets() {
        let base =
            "[pome]\nname = \"github.com/acme/x\"\nversion = \"0.1.0\"\nemela = \"0.21.0\"\n";
        let m = Manifest::parse(&format!("{base}targets = [\"js\"]\n\n[external]\nallow = [\"github.com/emela-lang/x/platform-cloudflare\"]\n")).unwrap();
        assert_eq!(m.targets, Some(vec!["js".to_string()]));
        assert_eq!(
            m.external,
            [SourcePath::parse("github.com/emela-lang/x/platform-cloudflare").unwrap()]
        );
        let m = Manifest::parse(base).unwrap();
        assert_eq!((m.targets, m.external.len()), (None, 0));
        assert!(Manifest::parse(&format!("{base}targets = [\"node\"]\n")).is_err());
    }

    #[test]
    fn reads_dev_dependencies() {
        let base =
            "[pome]\nname = \"github.com/acme/x\"\nversion = \"0.1.0\"\nemela = \"0.22.0\"\n";
        let m = Manifest::parse(&format!(
            "{base}[dev-dependencies]\n\"github.com/emela-lang/brix\" = \"v0.2.0\"\n"
        ))
        .unwrap();
        let brix = SourcePath::parse("github.com/emela-lang/brix").unwrap();
        assert_eq!(m.dev_dependencies[&brix].version.as_str(), "v0.2.0");
        assert!(m.dependencies.is_empty());
        let both = Manifest::parse(&format!(
            "{base}[dependencies]\n\"github.com/emela-lang/brix\" = \"v0.2.0\"\n\n[dev-dependencies]\n\"github.com/emela-lang/brix\" = \"v0.2.0\"\n"
        ))
        .unwrap_err();
        assert!(both.0.contains("in both"), "{both}");
    }
}
