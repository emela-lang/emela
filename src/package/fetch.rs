use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::package::cache::git_cache_path;
use crate::package::manifest::{GitDependency, PackageManifest};

pub(crate) fn fetch_git_dependency(name: &str, dependency: &GitDependency) -> Result<PathBuf> {
    let package_root = git_cache_path(dependency);
    if !package_root.exists() {
        if let Some(parent) = package_root.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                Error::new(format!(
                    "failed to create cache directory `{}`: {err}",
                    parent.display()
                ))
            })?;
        }
        run_git(
            [
                "clone",
                dependency.git.as_str(),
                path_arg(&package_root).as_str(),
            ],
            None,
        )?;
        run_git(
            ["checkout", "--detach", dependency.rev.as_str()],
            Some(&package_root),
        )?;
    }
    verify_package_name(name, &package_root)?;
    Ok(package_root)
}

pub(crate) fn verify_package_name(expected: &str, package_root: &Path) -> Result<()> {
    let manifest = PackageManifest::read_from(package_root)?;
    if manifest.name != expected {
        return Err(Error::new(format!(
            "dependency `{expected}` resolved to package `{}` at `{}`",
            manifest.name,
            package_root.display()
        )));
    }
    Ok(())
}

fn run_git<const N: usize>(args: [&str; N], current_dir: Option<&Path>) -> Result<()> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let output = command
        .output()
        .map_err(|err| Error::new(format!("failed to run git: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(format!("git command failed: {}", stderr.trim())));
    }
    Ok(())
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::verify_package_name;

    #[test]
    fn rejects_package_name_mismatch() {
        let root = std::env::temp_dir().join(format!(
            "emela-package-name-mismatch-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("emela-package.json"),
            r#"{"name":"actual","version":"0.1.0","source":"src"}"#,
        )
        .unwrap();

        let error = verify_package_name("expected", &root).unwrap_err();
        let _ = fs::remove_dir_all(&root);
        assert!(error.to_string().contains("dependency `expected`"));
    }
}
