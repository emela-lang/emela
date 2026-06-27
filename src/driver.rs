use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{ImportDecl, Program, TopLevelItem};
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
    stdlib: PathBuf,
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

#[cfg(test)]
pub(crate) fn compile_source_for_platform(
    source: &str,
    platform: &PlatformSpec,
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program(source)?;
    expand_stdlib_imports(&mut program, &default_stdlib_path())?;
    let typed = TypeChecker::new(&program, platform).check()?;
    Ok((program, typed))
}

#[cfg(test)]
pub(crate) fn compile_source_for_platform_with_mode(
    source: &str,
    platform: &PlatformSpec,
    mode: CheckMode,
) -> Result<(Program, TypedProgram)> {
    compile_source_for_platform_with_stdlib(source, platform, mode, &default_stdlib_path())
}

pub(crate) fn compile_source_for_platform_with_stdlib(
    source: &str,
    platform: &PlatformSpec,
    mode: CheckMode,
    stdlib: &Path,
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program(source)?;
    expand_stdlib_imports(&mut program, stdlib)?;
    let typed = TypeChecker::new_with_mode(&program, platform, mode).check()?;
    Ok((program, typed))
}

fn parse_program(source: &str) -> Result<Program> {
    let tokens = lex(source)?;
    let mut parser = Parser::new(tokens);
    parser.parse_program()
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
    let (program, typed) =
        compile_source_for_platform_with_stdlib(&source, &platform, mode, &args.stdlib)?;
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
    let mut stdlib = None;

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
            "--stdlib" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--stdlib requires a directory path"))?;
                stdlib = Some(PathBuf::from(path));
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
    let stdlib = stdlib.unwrap_or_else(default_stdlib_path);

    Ok(Args {
        input,
        output,
        check_only,
        library,
        emit_asm: emit_asm_path,
        emit_js: emit_js_path,
        target,
        platform,
        stdlib,
    })
}

fn print_help() {
    eprintln!(
        "Usage: compiler [--target TARGET] [--platform PATH] [--stdlib DIR] [--check] [--library] [--emit-asm PATH] [--emit-js PATH] [-o OUTPUT] INPUT.emel"
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

fn default_stdlib_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../stdlib")
}

fn expand_stdlib_imports(program: &mut Program, stdlib: &Path) -> Result<()> {
    let mut loaded_modules = BTreeSet::new();
    expand_stdlib_imports_with_loaded(program, stdlib, &mut loaded_modules)
}

fn expand_stdlib_imports_with_loaded(
    program: &mut Program,
    stdlib: &Path,
    loaded_modules: &mut BTreeSet<String>,
) -> Result<()> {
    let mut std_imports = Vec::new();
    program.items.retain(|item| {
        let TopLevelItem::Import(import) = item else {
            return true;
        };
        if is_std_import(import) {
            std_imports.push(import.clone());
            false
        } else {
            true
        }
    });

    for import in std_imports {
        let module_path = std_module_path(&import)?;
        let module_key = module_path.join(".");
        let mut module = load_stdlib_module(stdlib, &module_path)?;
        if !module_exports(&module, &import.name) {
            return Err(Error::new(format!(
                "stdlib module `std.{}` does not export `{}`",
                module_key, import.name
            )));
        }
        if loaded_modules.insert(module_key) {
            expand_stdlib_imports_with_loaded(&mut module, stdlib, loaded_modules)?;
            program.items.extend(module.items);
        }
    }

    Ok(())
}

fn is_std_import(import: &ImportDecl) -> bool {
    import.path.first().is_some_and(|package| package == "std")
}

fn std_module_path(import: &ImportDecl) -> Result<Vec<String>> {
    if import.path.len() < 2 {
        return Err(Error::new(format!(
            "stdlib import `{}` must include a module path",
            format_import_path(&import.path, &import.name)
        )));
    }
    Ok(import.path[1..].to_vec())
}

fn load_stdlib_module(stdlib: &Path, module_path: &[String]) -> Result<Program> {
    let mut path = stdlib.join("std");
    for part in module_path {
        path.push(part);
    }
    path.set_extension("emel");
    let source = fs::read_to_string(&path).map_err(|err| {
        Error::new(format!(
            "failed to read stdlib module `{}`: {err}",
            path.display()
        ))
    })?;
    parse_program(&source)
}

fn module_exports(module: &Program, name: &str) -> bool {
    module.items.iter().any(|item| match item {
        TopLevelItem::Function(function) => function.name == name,
        TopLevelItem::Import(_) => false,
        TopLevelItem::Struct(decl) => decl.name == name,
        TopLevelItem::Enum(decl) => decl.name == name,
    })
}

fn format_import_path(path: &[String], name: &str) -> String {
    let mut parts = path.to_vec();
    parts.push(name.to_string());
    parts.join(".")
}
