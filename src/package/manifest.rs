use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ProjectManifest {
    pub(crate) package: ProjectIdentity,
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, GitDependency>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ProjectIdentity {
    pub(crate) name: String,
    pub(crate) version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct GitDependency {
    pub(crate) git: String,
    pub(crate) rev: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PackageManifest {
    pub(crate) name: String,
    #[allow(dead_code)]
    pub(crate) version: Option<String>,
    pub(crate) source: String,
}

impl ProjectManifest {
    pub(crate) fn read_from(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|err| {
            Error::new(format!(
                "failed to read project manifest `{}`: {err}",
                path.display()
            ))
        })?;
        let manifest: Self = serde_json::from_str(&source).map_err(|err| {
            Error::new(format!(
                "failed to parse project manifest `{}`: {err}",
                path.display()
            ))
        })?;
        if manifest.package.name.trim().is_empty() {
            return Err(Error::new("project package name must not be empty"));
        }
        if manifest.package.version.trim().is_empty() {
            return Err(Error::new("project package version must not be empty"));
        }
        for (name, dependency) in &manifest.dependencies {
            if name.trim().is_empty() {
                return Err(Error::new("dependency name must not be empty"));
            }
            if dependency.git.trim().is_empty() {
                return Err(Error::new(format!(
                    "dependency `{name}` git URL must not be empty"
                )));
            }
            if dependency.rev.trim().is_empty() {
                return Err(Error::new(format!(
                    "dependency `{name}` rev must not be empty"
                )));
            }
        }
        Ok(manifest)
    }

    pub(crate) fn write_to(&self, path: &Path) -> Result<()> {
        let source = serde_json::to_string_pretty(self).map_err(|err| {
            Error::new(format!(
                "failed to serialize project manifest `{}`: {err}",
                path.display()
            ))
        })?;
        fs::write(path, format!("{source}\n")).map_err(|err| {
            Error::new(format!(
                "failed to write project manifest `{}`: {err}",
                path.display()
            ))
        })
    }
}

impl PackageManifest {
    pub(crate) fn read_from(package_root: &Path) -> Result<Self> {
        let manifest_path = package_root.join("emela-package.json");
        let source = fs::read_to_string(&manifest_path).map_err(|err| {
            Error::new(format!(
                "failed to read package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
        let manifest: Self = serde_json::from_str(&source).map_err(|err| {
            Error::new(format!(
                "failed to parse package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
        if manifest.name.trim().is_empty() {
            return Err(Error::new(format!(
                "package manifest `{}` has an empty name",
                manifest_path.display()
            )));
        }
        if manifest.source.trim().is_empty() {
            return Err(Error::new(format!(
                "package manifest `{}` has an empty source",
                manifest_path.display()
            )));
        }
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::ProjectManifest;

    #[test]
    fn parses_project_manifest_with_git_dependency() {
        let root = std::env::temp_dir().join(format!(
            "emela-project-manifest-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("emela.json");
        fs::write(
            &path,
            r#"{
  "package": {"name":"app","version":"0.1.0"},
  "dependencies": {
    "std": {"git":"https://github.com/emela-lang/stdlib.git","rev":"0123456789abcdef"}
  }
}"#,
        )
        .unwrap();

        let manifest = ProjectManifest::read_from(&path).unwrap();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(manifest.package.name, "app");
        assert_eq!(
            manifest.dependencies["std"].git,
            "https://github.com/emela-lang/stdlib.git"
        );
    }

    #[test]
    fn rejects_project_manifest_missing_package() {
        let root = std::env::temp_dir().join(format!(
            "emela-project-manifest-missing-package-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("emela.json");
        fs::write(
            &path,
            r#"{"dependencies":{"std":{"git":"https://example.test/std.git","rev":"abc"}}}"#,
        )
        .unwrap();

        let error = ProjectManifest::read_from(&path).unwrap_err();
        let _ = fs::remove_dir_all(&root);
        assert!(error
            .to_string()
            .contains("failed to parse project manifest"));
    }
}
