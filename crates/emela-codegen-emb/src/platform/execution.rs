//! File/environment/process operations, kept outside all pure compiler and connection stages.
use super::{contract::*, fingerprint::digest};
use crate::{
    GeneratedFile, Output,
    artifact::{MANIFEST_FILE, Manifest},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn load_artifact(directory: &Path) -> Result<Output, String> {
    let root = directory.canonicalize().map_err(|e| e.to_string())?;
    let contents = fs::read_to_string(root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    let manifest = Manifest::from_json(&contents).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for file in &manifest.files {
        let path = root
            .join(&file.path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !path.starts_with(&root) {
            return Err(format!("artifact path escapes directory: {}", file.path));
        }
        files.push(GeneratedFile {
            path: file.path.clone(),
            contents: fs::read_to_string(path).map_err(|e| e.to_string())?,
        });
    }
    files.push(GeneratedFile {
        path: MANIFEST_FILE.into(),
        contents,
    });
    Ok(Output {
        files,
        exports: manifest.metadata.exports,
    })
}
pub fn inspect_toolchain(compiler: &Path) -> Result<ToolchainInfo, String> {
    let resolved = if compiler.components().count() > 1 {
        compiler.canonicalize().map_err(|e| e.to_string())?
    } else {
        std::env::split_paths(&std::env::var_os("PATH").ok_or("PATH is unavailable")?)
            .map(|directory| directory.join(compiler))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| format!("compiler {} not found", compiler.display()))?
            .canonicalize()
            .map_err(|e| e.to_string())?
    };
    let version = Command::new(&resolved)
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    if !version.status.success() {
        return Err("compiler --version failed".into());
    }
    // Include binary bytes: replacing a compiler without changing its version invalidates state.
    let binary = fs::read(&resolved).map_err(|e| e.to_string())?;
    let identity = format!(
        "{}\n{:x}",
        String::from_utf8_lossy(&version.stdout),
        Sha256::digest(binary)
    );
    Ok(ToolchainInfo {
        compiler: resolved,
        identity,
    })
}
fn contained_write(root: &Path, path: &str, contents: &[u8]) -> Result<(), String> {
    if path.contains(['\\', ':', '\0'])
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err("invalid relative build path".into());
    }
    let target = root.join(path);
    let mut parent = root.to_owned();
    let parts: Vec<_> = path.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        parent.push(part);
        match parent.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(format!("invalid build directory {}", parent.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&parent).map_err(|e| e.to_string())?
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    if target
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(format!("build path is a symlink: {path}"));
    }
    fs::write(target, contents).map_err(|e| e.to_string())
}
pub fn execute(plan: &BuildPlan, directory: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let root = directory.canonicalize().map_err(|e| e.to_string())?;
    let state = root.join("connection-success.json");
    match fs::remove_file(&state) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    if inspect_toolchain(&plan.toolchain.compiler)? != plan.toolchain {
        return Err("toolchain changed after connection planning".into());
    }
    for file in &plan.files {
        contained_write(&root, &file.path, &file.contents)?;
    }
    contained_write(&root, &plan.executable, &[])?;
    for command in &plan.commands {
        let result = Command::new(&command.program)
            .args(&command.arguments)
            .current_dir(&root)
            .output()
            .map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(format!(
                "{} failed ({}):\n{}{}",
                command.program.display(),
                result.status,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            ));
        }
    }
    // Detect a source edit made during compilation before sealing successful state.
    let current = plan
        .files
        .iter()
        .map(|file| {
            Ok(PackageFile {
                path: file.path.clone(),
                contents: fs::read(root.join(&file.path)).map_err(|e| e.to_string())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if digest(&current) != digest(&plan.files) {
        return Err("build inputs changed while compiling".into());
    }
    let executable = root.join(&plan.executable);
    let binary_hash = format!(
        "{:x}",
        Sha256::digest(fs::read(&executable).map_err(|e| e.to_string())?)
    );
    let record = serde_json::json!({"schema_version":1,"common_hash":plan.common_hash,"platform_hash":plan.platform_hash,"inputs_hash":digest(&current),"executable":plan.executable,"binary_hash":binary_hash});
    contained_write(
        &root,
        "connection-success.json",
        serde_json::to_string_pretty(&record)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    Ok(executable)
}
pub fn verify_success(plan: &BuildPlan, directory: &Path) -> Result<(), String> {
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("connection-success.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let current = plan
        .files
        .iter()
        .map(|file| {
            Ok(PackageFile {
                path: file.path.clone(),
                contents: fs::read(directory.join(&file.path)).map_err(|e| e.to_string())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let binary_hash = format!(
        "{:x}",
        Sha256::digest(fs::read(directory.join(&plan.executable)).map_err(|e| e.to_string())?)
    );
    if record["common_hash"] != plan.common_hash
        || record["platform_hash"] != plan.platform_hash
        || record["inputs_hash"] != digest(&current)
        || record["inputs_hash"] != digest(&plan.files)
        || record["binary_hash"] != binary_hash
        || inspect_toolchain(&plan.toolchain.compiler)? != plan.toolchain
    {
        return Err("connection build is stale".into());
    }
    Ok(())
}
