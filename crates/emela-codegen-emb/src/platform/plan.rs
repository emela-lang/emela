//! Pure capability checking and build command construction.
use super::contract::*;
use super::fingerprint::digest;
use crate::{
    GeneratedFile, Output,
    artifact::{ABI_VERSION, FileRole, MANIFEST_FILE, Manifest},
};
use std::collections::HashSet;

fn path(value: &str) -> bool {
    !value.contains(['\\', ':', '\0'])
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}
pub fn prepare_connection(
    output: &Output,
    package: &PlatformPackage,
    options: &ConnectOptions,
    toolchain: &ToolchainInfo,
) -> Result<BuildPlan, String> {
    let manifest_file = output
        .files
        .iter()
        .find(|file| file.path == MANIFEST_FILE)
        .ok_or("missing artifact manifest")?;
    let manifest =
        Manifest::from_json(&manifest_file.contents).map_err(|error| error.to_string())?;
    if package.abi_version != ABI_VERSION {
        return Err(format!(
            "platform {} supports ABI {}, artifact requires {}",
            package.id, package.abi_version, ABI_VERSION
        ));
    }
    if manifest.metadata.exports != output.exports {
        return Err("artifact exports differ from manifest".into());
    }
    for service in &manifest.metadata.services {
        if !package.services.contains(service) {
            return Err(format!(
                "platform {} is missing required service {service}",
                package.id
            ));
        }
    }
    for symbol in &manifest.metadata.external_symbols {
        if !package.external_symbols.contains(symbol) {
            return Err(format!(
                "platform {} is missing external symbol {symbol}",
                package.id
            ));
        }
    }
    if !manifest.metadata.isr_entries.is_empty() && !package.supports_isr {
        return Err(format!(
            "platform {} does not support ISR entries",
            package.id
        ));
    }
    for symbol in &manifest.metadata.isr_safe_symbols {
        if !package.isr_safe_symbols.contains(symbol) {
            return Err(format!(
                "platform {} does not guarantee ISR safety for {symbol}",
                package.id
            ));
        }
    }
    if !toolchain.compiler.is_absolute() || toolchain.identity.is_empty() {
        return Err("toolchain must have an absolute compiler path and explicit identity".into());
    }
    if !path(&options.executable) {
        return Err("invalid executable path".into());
    }
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    // Only inventoried common inputs participate. Reject duplicate/unlisted files.
    if output.files.len() != manifest.files.len() + 1 {
        return Err("artifact has unlisted or duplicate files".into());
    }
    for file in &manifest.files {
        let matches: Vec<_> = output
            .files
            .iter()
            .filter(|input| input.path == file.path)
            .collect();
        if matches.len() != 1 {
            return Err(format!("missing or duplicate artifact input {}", file.path));
        }
        files.push(PackageFile {
            path: format!("common/{}", file.path),
            contents: matches[0].contents.as_bytes().to_vec(),
        });
    }
    files.push(PackageFile {
        path: format!("common/{MANIFEST_FILE}"),
        contents: manifest_file.contents.as_bytes().to_vec(),
    });
    let common_hash = digest(&files);
    for file in &package.files {
        if !path(&file.path) || !seen.insert(&file.path) {
            return Err(format!("invalid or duplicate package path {}", file.path));
        }
        files.push(PackageFile {
            path: format!("platform/{}", file.path),
            contents: file.contents.clone(),
        });
    }
    for source in &package.sources {
        if !seen.contains(source) || !(source.ends_with(".c") || source.ends_with(".S")) {
            return Err(format!("invalid package source {source}"));
        }
    }
    if options.main {
        let entry = manifest.metadata.entry.as_ref().ok_or(
            "artifact has no program entry; disable generated main and supply package startup",
        )?;
        files.push(
            GeneratedFile {
                path: "entry/main.c".into(),
                contents: format!(
                    "#include \"emela_embedded.h\"\nint main(void) {{ {entry}(); return 0; }}\n"
                ),
            }
            .into(),
        );
    }
    let mut arguments = vec![
        "-std=c11".into(),
        match options.optimization {
            Optimization::O0 => "-O0",
            Optimization::O2 => "-O2",
        }
        .into(),
        "-Icommon".into(),
        "-Iplatform".into(),
        format!(
            "-DEMELA_PLATFORM_TASK_CAPACITY={}",
            manifest.metadata.task_capacity.max(1)
        ),
    ];
    for service in &manifest.metadata.services {
        arguments.push(format!(
            "-DEMELA_REQUIRE_{}=1",
            service.replace('.', "_").to_ascii_uppercase()
        ));
    }
    let mut includes = std::collections::BTreeSet::new();
    for file in &manifest.files {
        if let Some((parent, _)) = file.path.rsplit_once('/') {
            includes.insert(format!("-Icommon/{parent}"));
        }
    }
    for file in &package.files {
        if let Some((parent, _)) = file.path.rsplit_once('/') {
            includes.insert(format!("-Iplatform/{parent}"));
        }
    }
    arguments.extend(includes);
    arguments.extend(package.compile_flags.clone());
    arguments.extend(
        manifest
            .files
            .iter()
            .filter(|file| file.role == FileRole::Source)
            .map(|file| format!("common/{}", file.path)),
    );
    arguments.extend(
        package
            .sources
            .iter()
            .map(|source| format!("platform/{source}")),
    );
    if options.main {
        arguments.push("entry/main.c".into());
    }
    arguments.extend(package.link_flags.clone());
    arguments.extend(["-o".into(), format!("out/{}", options.executable)]);
    let commands = vec![CommandPlan {
        program: toolchain.compiler.clone(),
        arguments,
    }];
    let mut platform_inputs: Vec<_> = files
        .iter()
        .filter(|file| !file.path.starts_with("common/"))
        .cloned()
        .collect();
    platform_inputs.push(PackageFile {
        path: "connection.json".into(),
        contents: format!("{package:?}\n{options:?}\n{toolchain:?}\n{commands:?}").into_bytes(),
    });
    Ok(BuildPlan {
        files,
        commands,
        executable: format!("out/{}", options.executable),
        common_hash,
        platform_hash: digest(&platform_inputs),
        toolchain: toolchain.clone(),
    })
}
