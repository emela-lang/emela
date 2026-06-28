use std::env;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::package::cache::git_cache_path;
use crate::package::fetch::fetch_git_dependency;
use crate::package::manifest::{GitDependency, PackageManifest, ProjectManifest};
use crate::package::PackageSource;

pub(crate) fn fetch_project_dependencies_from_current_dir() -> Result<()> {
    let cwd = env::current_dir()
        .map_err(|err| Error::new(format!("failed to get current directory: {err}")))?;
    let manifest_path = find_project_manifest_from(&cwd)
        .ok_or_else(|| Error::new("emela.json was not found from current directory"))?;
    let manifest = ProjectManifest::read_from(&manifest_path)?;
    for (name, dependency) in &manifest.dependencies {
        fetch_git_dependency(name, dependency)?;
    }
    Ok(())
}

pub(crate) fn add_project_dependency_from_current_dir(
    name: String,
    dependency: GitDependency,
) -> Result<()> {
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

    let cwd = env::current_dir()
        .map_err(|err| Error::new(format!("failed to get current directory: {err}")))?;
    let manifest_path = find_project_manifest_from(&cwd)
        .ok_or_else(|| Error::new("emela.json was not found from current directory"))?;
    let mut manifest = ProjectManifest::read_from(&manifest_path)?;
    if manifest.dependencies.contains_key(&name) {
        return Err(Error::new(format!("dependency `{name}` already exists")));
    }
    fetch_git_dependency(&name, &dependency)?;
    manifest.dependencies.insert(name, dependency);
    manifest.write_to(&manifest_path)
}

pub(crate) fn resolve_package_sources(
    input: &Path,
    explicit_packages: &[PathBuf],
) -> Result<Vec<PackageSource>> {
    let mut packages = load_package_sources(explicit_packages)?;
    if let Some(project_manifest_path) = find_project_manifest(input)? {
        let manifest = ProjectManifest::read_from(&project_manifest_path)?;
        for (name, dependency) in &manifest.dependencies {
            if packages.iter().any(|package| package.name == *name) {
                return Err(Error::new(format!(
                    "package `{name}` is provided by both emela.json and --package"
                )));
            }
            let package_root = git_cache_path(dependency);
            if !package_root.exists() {
                return Err(Error::new(format!(
                    "dependency `{name}` is not in the package cache; run `emela package fetch`"
                )));
            }
            let manifest = PackageManifest::read_from(&package_root)?;
            if manifest.name != *name {
                return Err(Error::new(format!(
                    "dependency `{name}` resolved to package `{}` at `{}`",
                    manifest.name,
                    package_root.display()
                )));
            }
            packages.push(PackageSource {
                name: manifest.name,
                source_root: package_root.join(manifest.source),
            });
        }
    }
    Ok(packages)
}

fn find_project_manifest(input: &Path) -> Result<Option<PathBuf>> {
    let input = if input.is_absolute() {
        input.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|err| Error::new(format!("failed to get current directory: {err}")))?
            .join(input)
    };
    let start = input.parent().unwrap_or_else(|| Path::new("."));
    Ok(find_project_manifest_from(start))
}

fn find_project_manifest_from(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let manifest = dir.join("emela.json");
        if manifest.exists() {
            return Some(manifest);
        }
    }
    None
}

fn load_package_sources(paths: &[PathBuf]) -> Result<Vec<PackageSource>> {
    let mut packages = Vec::new();
    for path in paths {
        let manifest = PackageManifest::read_from(path)?;
        if packages
            .iter()
            .any(|package: &PackageSource| package.name == manifest.name)
        {
            return Err(Error::new(format!("duplicate package `{}`", manifest.name)));
        }
        packages.push(PackageSource {
            name: manifest.name,
            source_root: path.join(manifest.source),
        });
    }
    Ok(packages)
}
