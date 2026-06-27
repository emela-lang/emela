use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::ast::{Block, BlockItem, Expr, ImportDecl, ImportOrigin, Program, TopLevelItem};
use crate::backend::{Backend, EmitOptions};
use crate::error::{Error, Result};
use crate::lexer::lex;
use crate::parser::Parser;
use crate::platform::{PlatformSpec, Target};
use crate::typecheck::{CheckMode, TypeChecker, TypedProgram};

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
    artifact: Option<PathBuf>,
    check_only: bool,
    library: bool,
    target: Option<Target>,
    backend: Option<String>,
    packages: Vec<PathBuf>,
}

#[derive(Clone)]
struct PackageSource {
    name: String,
    source_root: PathBuf,
}

#[derive(Deserialize)]
struct PackageManifest {
    name: String,
    source: String,
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
    expand_package_imports(&mut program, &[])?;
    let typed = TypeChecker::new(&program, platform).check()?;
    Ok((program, typed))
}

#[cfg(test)]
pub(crate) fn compile_internal_source_for_platform(
    source: &str,
    platform: &PlatformSpec,
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program(source)?;
    mark_stdlib_origin(&mut program);
    let typed = TypeChecker::new(&program, platform).check()?;
    Ok((program, typed))
}

#[cfg(test)]
pub(crate) fn compile_source_for_platform_with_mode(
    source: &str,
    platform: &PlatformSpec,
    mode: CheckMode,
) -> Result<(Program, TypedProgram)> {
    compile_source_for_platform_with_packages(source, platform, mode, &[])
}

fn compile_source_for_platform_with_packages(
    source: &str,
    platform: &PlatformSpec,
    mode: CheckMode,
    packages: &[PackageSource],
) -> Result<(Program, TypedProgram)> {
    let mut program = parse_program(source)?;
    expand_package_imports(&mut program, packages)?;
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

    let backend_name = args
        .backend
        .as_deref()
        .ok_or_else(|| Error::new("--backend is required"))?;
    let backend = Backend::parse(backend_name)?;
    let backend_target = backend.target();
    if let (Some(explicit), Some(profile)) = (args.target, backend_target) {
        if explicit != profile {
            return Err(Error::new(format!(
                "backend profile `{backend_name}` requires target `{profile}`, got `{explicit}`"
            )));
        }
    }
    let target = backend_target.or(args.target);
    let platform = backend.platform();

    let mode = if args.library {
        CheckMode::Library
    } else {
        CheckMode::Executable
    };
    let packages = load_package_sources(&args.packages)?;
    let (program, typed) =
        compile_source_for_platform_with_packages(&source, &platform, mode, &packages)?;
    if !args.check_only {
        backend.emit(
            &platform,
            &program,
            &typed,
            EmitOptions {
                target,
                mode,
                input: &args.input,
                output: args.output.as_deref(),
                artifact: args.artifact.as_deref(),
            },
        )?;
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut check_only = false;
    let mut library = false;
    let mut artifact = None;
    let mut target = None;
    let mut backend = None;
    let mut packages = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--library" => library = true,
            "--artifact" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--artifact requires a path"))?;
                artifact = Some(PathBuf::from(path));
            }
            "--target" => {
                let value = args
                    .next()
                    .ok_or_else(|| Error::new("--target requires a target triple"))?;
                target = Some(Target::parse(&value)?);
            }
            "--backend" => {
                let value = args.next().ok_or_else(|| {
                    Error::new("--backend requires a backend profile name or manifest path")
                })?;
                backend = Some(value);
            }
            "--package" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--package requires a package directory path"))?;
                packages.push(PathBuf::from(path));
            }
            "--output" => {
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
    if check_only && (artifact.is_some() || output.is_some()) {
        return Err(Error::new(
            "--check cannot be combined with --artifact or --output",
        ));
    }
    if artifact.is_some() && output.is_some() {
        return Err(Error::new("--artifact and --output are mutually exclusive"));
    }
    if library && !check_only && artifact.is_none() {
        return Err(Error::new("--library requires --check or --artifact"));
    }
    Ok(Args {
        input,
        output,
        artifact,
        check_only,
        library,
        target,
        backend,
        packages,
    })
}

fn print_help() {
    eprintln!(
        "Usage: compiler --backend PROFILE|PATH [--target TARGET] [--package DIR]... [--check] [--library] [--artifact PATH] [--output PATH] INPUT.emel"
    );
}

fn load_package_sources(paths: &[PathBuf]) -> Result<Vec<PackageSource>> {
    let mut packages = Vec::new();
    for path in paths {
        let manifest_path = path.join("emela-package.json");
        let manifest_source = fs::read_to_string(&manifest_path).map_err(|err| {
            Error::new(format!(
                "failed to read package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
        let manifest: PackageManifest = serde_json::from_str(&manifest_source).map_err(|err| {
            Error::new(format!(
                "failed to parse package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
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

fn expand_package_imports(program: &mut Program, packages: &[PackageSource]) -> Result<()> {
    let mut loaded_modules = BTreeSet::new();
    expand_package_imports_with_loaded(program, packages, &mut loaded_modules)
}

fn expand_package_imports_with_loaded(
    program: &mut Program,
    packages: &[PackageSource],
    loaded_modules: &mut BTreeSet<String>,
) -> Result<()> {
    let mut source_imports = Vec::new();
    program.items.retain(|item| {
        let TopLevelItem::Import(import) = item else {
            return true;
        };
        if is_source_package_import(import, packages) {
            source_imports.push(import.clone());
            false
        } else {
            true
        }
    });

    for import in source_imports {
        let package_name = import.path.first().expect("import path is not empty");
        let module_path = std_module_path(&import)?;
        let module_key = module_path.join(".");
        let import_key = format!("{package_name}.{module_key}.{}", import.name);
        let mut module = load_source_package_module(package_name, packages, &module_path)?;
        if !module_exports(&module, &import.name) {
            return Err(Error::new(format!(
                "package module `{package_name}.{}` does not export `{}`",
                module_key, import.name
            )));
        }
        retain_stdlib_item_dependencies(&mut module, &import.name);
        if loaded_modules.insert(import_key) {
            expand_package_imports_with_loaded(&mut module, packages, loaded_modules)?;
            if package_name == "std" {
                mark_stdlib_origin(&mut module);
            }
            program.items.extend(module.items);
        }
    }

    Ok(())
}

fn mark_stdlib_origin(program: &mut Program) {
    for item in &mut program.items {
        if let TopLevelItem::Import(import) = item {
            import.origin = ImportOrigin::Stdlib;
        }
    }
}

fn is_source_package_import(import: &ImportDecl, packages: &[PackageSource]) -> bool {
    let Some(package) = import.path.first() else {
        return false;
    };
    package == "std" || packages.iter().any(|source| source.name == *package)
}

fn std_module_path(import: &ImportDecl) -> Result<Vec<String>> {
    if import.path.len() < 2 {
        return Err(Error::new(format!(
            "source package import `{}` must include a module path",
            format_import_path(&import.path, &import.name)
        )));
    }
    Ok(import.path[1..].to_vec())
}

fn load_source_package_module(
    package_name: &str,
    packages: &[PackageSource],
    module_path: &[String],
) -> Result<Program> {
    let source = if package_name == "std" {
        if let Some(package) = packages.iter().find(|package| package.name == "std") {
            load_package_module(package, module_path)?
        } else {
            load_embedded_stdlib_module(module_path)?
        }
    } else {
        let package = packages
            .iter()
            .find(|package| package.name == package_name)
            .ok_or_else(|| Error::new(format!("unknown package `{package_name}`")))?;
        load_package_module(package, module_path)?
    };
    parse_program(&source)
}

fn load_package_module(package: &PackageSource, module_path: &[String]) -> Result<String> {
    load_module_file(
        package.source_root.clone(),
        module_path,
        &format!("package `{}`", package.name),
    )
}

fn load_module_file(mut root: PathBuf, module_path: &[String], label: &str) -> Result<String> {
    for part in module_path {
        root.push(part);
    }
    root.set_extension("emel");
    fs::read_to_string(&root).map_err(|err| {
        Error::new(format!(
            "failed to read {label} module `{}`: {err}",
            root.display()
        ))
    })
}

fn load_embedded_stdlib_module(module_path: &[String]) -> Result<String> {
    match module_path {
        [module] if module == "io" => Ok(include_str!("../../stdlib/std/io.emel").to_string()),
        [module] if module == "clock" => {
            Ok(include_str!("../../stdlib/std/clock.emel").to_string())
        }
        _ => Err(Error::new(format!(
            "embedded stdlib module `std.{}` is not bundled",
            module_path.join(".")
        ))),
    }
}

fn module_exports(module: &Program, name: &str) -> bool {
    module.items.iter().any(|item| match item {
        TopLevelItem::Function(function) => function.name == name,
        TopLevelItem::Import(_) => false,
        TopLevelItem::Struct(decl) => decl.name == name,
        TopLevelItem::Enum(decl) => decl.name == name,
    })
}

fn retain_stdlib_item_dependencies(module: &mut Program, export_name: &str) {
    let mut needed = BTreeSet::from([export_name.to_string()]);
    loop {
        let before = needed.len();
        for function in module.functions() {
            if needed.contains(&function.name) {
                collect_block_dependencies(&function.body, &mut needed);
            }
        }
        if needed.len() == before {
            break;
        }
    }

    module.items.retain(|item| match item {
        TopLevelItem::Function(function) => needed.contains(&function.name),
        TopLevelItem::Import(import) => needed.contains(&import.name),
        TopLevelItem::Struct(_) | TopLevelItem::Enum(_) => true,
    });
}

fn collect_block_dependencies(block: &Block, needed: &mut BTreeSet<String>) {
    for item in &block.items {
        match item {
            BlockItem::Binding { expr, .. } | BlockItem::Expr(expr) => {
                collect_expr_dependencies(expr, needed);
            }
        }
    }
}

fn collect_expr_dependencies(expr: &Expr, needed: &mut BTreeSet<String>) {
    match expr {
        Expr::Var(name) => {
            needed.insert(name.clone());
        }
        Expr::String(_) => {}
        Expr::Call { name, args } => {
            needed.insert(name.clone());
            for arg in args {
                collect_expr_dependencies(arg, needed);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_dependencies(receiver, needed);
            for arg in args {
                collect_expr_dependencies(arg, needed);
            }
        }
        Expr::FieldAccess { receiver, .. } => collect_expr_dependencies(receiver, needed),
        Expr::StructLiteral { value, .. } => collect_expr_dependencies(value, needed),
        Expr::Binary { left, right, .. } => {
            collect_expr_dependencies(left, needed);
            collect_expr_dependencies(right, needed);
        }
        Expr::Match { scrutinee, arms } => {
            collect_expr_dependencies(scrutinee, needed);
            for arm in arms {
                collect_expr_dependencies(&arm.expr, needed);
            }
        }
        Expr::Block(block) => collect_block_dependencies(block, needed),
        Expr::Int(_) | Expr::Bool(_) | Expr::Unit => {}
    }
}

fn format_import_path(path: &[String], name: &str) -> String {
    let mut parts = path.to_vec();
    parts.push(name.to_string());
    parts.join(".")
}
