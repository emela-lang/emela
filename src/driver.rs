use std::env;
use std::fs;
use std::path::PathBuf;

use crate::ast::Program;
use crate::codegen::{build, emit_assembly};
use crate::error::{Error, Result};
use crate::lexer::lex;
use crate::parser::Parser;
use crate::platform::Target;
use crate::typecheck::{TypeChecker, TypedProgram};

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    check_only: bool,
    emit_asm: Option<PathBuf>,
    target: Target,
}

#[cfg(test)]
pub(crate) fn compile_source(source: &str) -> Result<(Program, TypedProgram)> {
    compile_source_for_target(source, Target::host()?)
}

pub(crate) fn compile_source_for_target(
    source: &str,
    target: Target,
) -> Result<(Program, TypedProgram)> {
    let tokens = lex(source)?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    let typed = TypeChecker::new(&program, target).check()?;
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

    let (program, typed) = compile_source_for_target(&source, args.target)?;
    let assembly = if !args.check_only || args.emit_asm.is_some() {
        Some(emit_assembly(args.target, &program, &typed)?)
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

    if !args.check_only {
        let assembly = assembly
            .as_ref()
            .expect("assembly is generated when building");
        build(args.target, &args.input, &args.output, assembly)?;
        eprintln!("built {}", args.output.display());
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut check_only = false;
    let mut emit_asm_path = None;
    let mut target = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--emit-asm" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--emit-asm requires a path"))?;
                emit_asm_path = Some(PathBuf::from(path));
            }
            "--target" => {
                let value = args
                    .next()
                    .ok_or_else(|| Error::new("--target requires a target triple"))?;
                target = Some(Target::parse(&value)?);
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
    let output = output.unwrap_or_else(|| input.with_extension(""));
    let target = match target {
        Some(target) => target,
        None => Target::host()?,
    };

    Ok(Args {
        input,
        output,
        check_only,
        emit_asm: emit_asm_path,
        target,
    })
}

fn print_help() {
    eprintln!(
        "Usage: compiler [--target TARGET] [--check] [--emit-asm PATH] [-o OUTPUT] INPUT.emel"
    );
}
