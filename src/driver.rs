use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::Program;
use crate::codegen::{build, emit_assembly_for_platform, emit_js, emit_js_library};
use crate::error::{Error, Result};
use crate::lexer::lex;
use crate::parser::Parser;
use crate::platform::{PlatformSpec, Target};
use crate::typecheck::{CheckMode, TypeChecker, TypedProgram};

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    check_only: bool,
    library: bool,
    emit_asm: Option<PathBuf>,
    emit_js: Option<PathBuf>,
    target: Target,
    platform: Option<PathBuf>,
}

#[cfg(test)]
pub(crate) fn compile_source(source: &str) -> Result<(Program, TypedProgram)> {
    compile_source_for_target(source, Target::host()?)
}

#[cfg(test)]
pub(crate) fn compile_source_for_target(
    source: &str,
    target: Target,
) -> Result<(Program, TypedProgram)> {
    let platform = PlatformSpec::native_for_target(target);
    compile_source_for_platform(source, &platform)
}

pub(crate) fn compile_source_for_platform(
    source: &str,
    platform: &PlatformSpec,
) -> Result<(Program, TypedProgram)> {
    let tokens = lex(source)?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    let typed = TypeChecker::new(&program, platform).check()?;
    Ok((program, typed))
}

pub(crate) fn compile_source_for_platform_with_mode(
    source: &str,
    platform: &PlatformSpec,
    mode: CheckMode,
) -> Result<(Program, TypedProgram)> {
    let tokens = lex(source)?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    let typed = TypeChecker::new_with_mode(&program, platform, mode).check()?;
    Ok((program, typed))
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    let source = fs::read_to_string(&args.input).map_err(|err| {
        Error::new(format!(
            "failed to read input file `{}`: {err}",
            args.input.display()
        ))
    })?;

    let platform = match &args.platform {
        Some(path) => PlatformSpec::from_manifest_path(path)?,
        None => PlatformSpec::native_for_target(args.target),
    };

    let mode = if args.library {
        CheckMode::Library
    } else {
        CheckMode::Executable
    };
    let (program, typed) = compile_source_for_platform_with_mode(&source, &platform, mode)?;
    let assembly = if args.emit_js.is_none() && (!args.check_only || args.emit_asm.is_some()) {
        Some(emit_assembly_for_platform(
            args.target,
            &platform,
            &program,
            &typed,
        )?)
    } else {
        None
    };

    if let Some(path) = &args.emit_asm {
        let assembly = assembly
            .as_ref()
            .expect("assembly is generated when --emit-asm is provided");
        fs::write(path, &assembly).map_err(|err| {
            Error::new(format!(
                "failed to write assembly output `{}`: {err}",
                path.display()
            ))
        })?;
    }

    if let Some(path) = &args.emit_js {
        let js = if args.library {
            emit_js_library(&platform, &program, &typed)?
        } else {
            emit_js(&platform, &program, &typed)?
        };
        write_output(path, &js, "js output")?;
    }

    if !args.library && !args.check_only && args.emit_js.is_none() {
        let assembly = assembly
            .as_ref()
            .expect("assembly is generated when building");
        build(
            args.target,
            &platform,
            &program,
            &args.input,
            &args.output,
            assembly,
        )?;
        eprintln!("built {}", args.output.display());
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut check_only = false;
    let mut library = false;
    let mut emit_asm_path = None;
    let mut emit_js_path = None;
    let mut target = None;
    let mut platform = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--library" => library = true,
            "--emit-asm" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--emit-asm requires a path"))?;
                emit_asm_path = Some(PathBuf::from(path));
            }
            "--emit-js" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--emit-js requires a path"))?;
                emit_js_path = Some(PathBuf::from(path));
            }
            "--target" => {
                let value = args
                    .next()
                    .ok_or_else(|| Error::new("--target requires a target triple"))?;
                target = Some(Target::parse(&value)?);
            }
            "--platform" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--platform requires a manifest path"))?;
                platform = Some(PathBuf::from(path));
            }
            "-o" | "--output" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--output requires a path"))?;
                output = Some(PathBuf::from(path));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => {
                return Err(Error::new(format!("unknown option `{arg}`")));
            }
            _ => {
                if input.replace(PathBuf::from(arg)).is_some() {
                    return Err(Error::new("only one input file is supported"));
                }
            }
        }
    }

    let input = input.ok_or_else(|| Error::new("missing input file"))?;
    if input.extension().and_then(|ext| ext.to_str()) != Some("emel") {
        return Err(Error::new("input file extension must be .emel"));
    }
    if emit_asm_path.is_some() && emit_js_path.is_some() {
        return Err(Error::new(
            "--emit-asm and --emit-js cannot be used together",
        ));
    }
    if library && !check_only && emit_asm_path.is_none() && emit_js_path.is_none() {
        return Err(Error::new(
            "--library requires --check, --emit-asm, or --emit-js",
        ));
    }
    let output = output.unwrap_or_else(|| input.with_extension(""));
    let target = match target {
        Some(target) => target,
        None => Target::host()?,
    };

    Ok(Args {
        input,
        output,
        check_only,
        library,
        emit_asm: emit_asm_path,
        emit_js: emit_js_path,
        target,
        platform,
    })
}

fn print_help() {
    eprintln!(
        "Usage: compiler [--target TARGET] [--platform PATH] [--check] [--library] [--emit-asm PATH] [--emit-js PATH] [-o OUTPUT] INPUT.emel"
    );
}

fn write_output(path: &Path, contents: &str, label: &str) -> Result<()> {
    fs::write(path, contents).map_err(|err| {
        Error::new(format!(
            "failed to write {label} `{}`: {err}",
            path.display()
        ))
    })
}
