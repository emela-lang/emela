use std::env;
use std::fs;
use std::path::PathBuf;

use crate::ast::Program;
use crate::codegen::{build, emit_rust};
use crate::error::{Error, Result};
use crate::lexer::lex;
use crate::parser::Parser;
use crate::typecheck::{TypeChecker, TypedProgram};

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    check_only: bool,
    emit_rust: Option<PathBuf>,
}

pub(crate) fn compile_source(source: &str) -> Result<(Program, TypedProgram)> {
    let tokens = lex(source)?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    let typed = TypeChecker::new(&program).check()?;
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

    let (program, typed) = compile_source(&source)?;
    let rust_source = emit_rust(&program, &typed);

    if let Some(path) = &args.emit_rust {
        fs::write(path, &rust_source).map_err(|err| {
            Error::new(format!(
                "failed to write Rust output `{}`: {err}",
                path.display()
            ))
        })?;
    }

    if !args.check_only {
        build(&args.input, &args.output, &rust_source)?;
        eprintln!("built {}", args.output.display());
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut check_only = false;
    let mut emit_rust_path = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--emit-rust" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--emit-rust requires a path"))?;
                emit_rust_path = Some(PathBuf::from(path));
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

    Ok(Args {
        input,
        output,
        check_only,
        emit_rust: emit_rust_path,
    })
}

fn print_help() {
    eprintln!("Usage: compiler [--check] [--emit-rust PATH] [-o OUTPUT] INPUT.emel");
}
