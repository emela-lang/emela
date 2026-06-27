use std::fs;
use std::path::Path;
use std::process::Command;
use std::{env, process};

use crate::ast::{Program, TopLevelItem};
use crate::error::{Error, Result};
use crate::platform::{LinkerKind, PlatformSpec, Target};
use crate::typecheck::{CheckMode, TypedProgram};

use super::{lowering, EmitOptions};

pub(crate) struct NativeBackendProfile {
    pub(super) target: Target,
}

impl NativeBackendProfile {
    pub(super) fn new(target: Target) -> Self {
        Self { target }
    }

    pub(super) fn platform(&self) -> PlatformSpec {
        PlatformSpec::native_for_target(self.target)
    }

    pub(super) fn emit(
        &self,
        platform: &PlatformSpec,
        program: &Program,
        typed: &TypedProgram,
        options: EmitOptions<'_>,
    ) -> Result<()> {
        if options.target != Some(self.target) {
            return Err(Error::new(format!(
                "backend profile `{}` requires target `{}`",
                self.name(),
                self.target
            )));
        }
        let assembly = emit_native_assembly_for_platform(self.target, platform, program, typed)?;
        if let Some(path) = options.artifact {
            fs::write(path, assembly).map_err(|err| {
                Error::new(format!(
                    "failed to write backend output `{}`: {err}",
                    path.display()
                ))
            })?;
            return Ok(());
        }
        if options.mode == CheckMode::Library {
            return Err(Error::new("--library requires --check or --artifact"));
        }
        let Some(output) = options.output else {
            return Err(Error::new(
                "native backend executable builds require --output; use --artifact for assembly",
            ));
        };
        build_native_executable(
            self.target,
            platform,
            program,
            options.input,
            output,
            &assembly,
        )
    }

    fn name(&self) -> &'static str {
        match self.target {
            Target::Aarch64AppleDarwin => "native-aarch64-apple-darwin",
            Target::X86_64UnknownLinuxGnu => "native-x86_64-unknown-linux-gnu",
            Target::Wasm32UnknownUnknown | Target::Wasm32Wasi => "native-unsupported",
        }
    }
}

#[cfg(test)]
pub(crate) fn emit_native_assembly(
    target: Target,
    program: &Program,
    typed: &TypedProgram,
) -> Result<String> {
    let platform = PlatformSpec::native_for_target(target);
    emit_native_assembly_for_platform(target, &platform, program, typed)
}

pub(crate) fn emit_native_assembly_for_platform(
    target: Target,
    platform: &PlatformSpec,
    program: &Program,
    typed: &TypedProgram,
) -> Result<String> {
    lowering::emit_assembly_for_platform(target, platform, program, typed)
}

fn build_native_executable(
    target: Target,
    platform: &PlatformSpec,
    program: &Program,
    input: &Path,
    output: &Path,
    assembly: &str,
) -> Result<()> {
    if !target.supports_native_backend() {
        return Err(Error::new(format!(
            "target `{target}` does not have a native assembly backend yet"
        )));
    }
    if target.spec().linker != LinkerKind::HostCc {
        return Err(Error::new(format!(
            "target `{target}` does not have a host cc linker configuration"
        )));
    }
    if Target::host()? != target {
        return Err(Error::new(format!(
            "building target `{target}` requires running on a matching host; use --artifact for cross-target assembly output"
        )));
    }

    let temp = env::temp_dir().join(format!(
        "emela-{}-{}.s",
        process::id(),
        input.file_stem().and_then(|s| s.to_str()).unwrap_or("out")
    ));
    fs::write(&temp, assembly).map_err(|err| {
        Error::new(format!(
            "failed to write temporary assembly `{}`: {err}",
            temp.display()
        ))
    })?;

    let mut temp_link_sources = Vec::new();
    let mut command = Command::new("cc");
    command.arg(&temp).arg("-o").arg(output);
    for link in native_link_names(platform, program) {
        match link {
            "emela_runtime" => {
                let runtime = write_bundled_native_runtime()?;
                command.arg(&runtime);
                temp_link_sources.push(runtime);
            }
            _ => {
                command.arg(format!("-l{link}"));
            }
        }
    }
    let status = command
        .status()
        .map_err(|err| Error::new(format!("failed to execute cc: {err}")))?;

    let _ = fs::remove_file(&temp);
    for temp_link_source in temp_link_sources {
        let _ = fs::remove_file(temp_link_source);
    }

    if !status.success() {
        return Err(Error::new(format!(
            "cc failed while building `{}`",
            output.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn native_link_args(platform: &PlatformSpec, program: &Program) -> Vec<String> {
    native_link_names(platform, program)
        .into_iter()
        .map(native_link_arg)
        .collect()
}

#[cfg(test)]
fn native_link_arg(link: &str) -> String {
    match link {
        "emela_runtime" => "bundled:backends/native-runtime/emela_runtime.c".to_string(),
        _ => format!("-l{link}"),
    }
}

fn write_bundled_native_runtime() -> Result<String> {
    let path = env::temp_dir().join(format!("emela-runtime-{}.c", process::id()));
    fs::write(&path, super::bundled::NATIVE_RUNTIME_C).map_err(|err| {
        Error::new(format!(
            "failed to write bundled native runtime `{}`: {err}",
            path.display()
        ))
    })?;
    Ok(path.display().to_string())
}

fn native_link_names<'a>(platform: &'a PlatformSpec, program: &Program) -> Vec<&'a str> {
    let mut links = Vec::new();
    for item in &program.items {
        let TopLevelItem::Import(import) = item else {
            continue;
        };
        let Some(function) = platform.externs.resolve_import(&import.path, &import.name) else {
            continue;
        };
        let Some(binding) = &function.bindings.native else {
            continue;
        };
        for link in &binding.links {
            if !links.contains(&link.as_str()) {
                links.push(link.as_str());
            }
        }
    }
    links
}
