use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use emela_codegen::{
    Artifact, Backend, BackendOptions, BackendRegistry, EmitMode, IrProgram, Tier, emit_text,
};

use crate::error::{Error, Result};
use crate::external;
use crate::imports;
use crate::lower;
use crate::parser::parse_program;
use crate::typecheck;

const DEFAULT_BACKEND: &str = "js-node";

/// The set of built-in backends, in display order.
pub(crate) fn registry() -> BackendRegistry {
    let mut registry = BackendRegistry::new();
    #[cfg(feature = "backend-wasm")]
    registry.register(Box::new(emela_backend_wasm::WasmBackend));
    #[cfg(feature = "backend-wasm-wasip2")]
    registry.register(Box::new(emela_backend_wasm_wasip2::Wasip2Backend));
    #[cfg(feature = "backend-js")]
    registry.register(Box::new(emela_backend_js::JsBackend));
    registry
}

/// Canonicalize a user-facing backend name to a registered name.
fn canonical_backend(name: &str) -> &str {
    match name {
        "js" | "js-bun" => "js-node",
        "wasm" => "wasm-wasi",
        other => other,
    }
}

pub fn run() -> Result<()> {
    use clap::Parser;
    // clap owns `--help`/`-h`, `--version`/`-V`, and argument errors (it prints
    // to the right stream and exits on its own). Everything below is the domain
    // path, whose errors flow through `main` to stderr with exit code 1.
    match Cli::parse().command {
        Commands::Check { args } => {
            // `--library` compile-checks a module that has no `main` (spec 0003).
            let registry =
                build_platform_registry(&args.host_interfaces, &args.packages, &args.input)?;
            let _ = compile_frontend(&args.input, &args.packages, !args.library, &registry)?;
            Ok(())
        }
        Commands::Build { args } => {
            reject_library(&args, "build")?;
            let mode = args.emit_mode()?;
            let artifact = build(
                &args.input,
                &args.packages,
                &args.host_interfaces,
                args.backend.as_deref(),
                mode,
            )?;
            write_artifact(artifact, args.output)
        }
        Commands::Ir { args, rc } => {
            reject_library(&args, "ir")?;
            let registry =
                build_platform_registry(&args.host_interfaces, &args.packages, &args.input)?;
            let mut ir = compile_to_ir(&args.input, &args.packages, &registry)?;
            if rc {
                emela_codegen::insert_rc_ops(&mut ir);
            }
            let text = emit_text(&ir);
            match args.output {
                Some(output) => fs::write(&output, text).map_err(|err| {
                    Error::new(format!("failed to write `{}`: {err}", output.display()))
                }),
                None => {
                    print!("{text}");
                    Ok(())
                }
            }
        }
        Commands::Run { args } => {
            reject_library(&args, "run")?;
            reject_run_flags(&args)?;
            run_program(
                &args.input,
                &args.packages,
                &args.host_interfaces,
                args.backend.as_deref(),
            )
        }
        Commands::Backends => {
            for (name, tier) in registry().list() {
                println!("{name}\t{}", tier.label());
            }
            Ok(())
        }
        // `emela test` (spec 0040): run the current Pome's `@test` functions.
        Commands::Test { host_interfaces } => {
            let project = crate::pome::project_dir()?;
            let input = project.join("src").join("main.emel");
            let registry = build_platform_registry(&host_interfaces, &[], &input)?;
            run_tests(&registry)
        }
        // `emela lsp` (spec 0033): the language server over stdio.
        Commands::Lsp {
            packages,
            host_interfaces,
        } => {
            let input = match crate::pome::project_dir() {
                Ok(project) => project.join("src").join("main.emel"),
                Err(_) => PathBuf::from(".").join("dummy.emel"),
            };
            let registry = build_platform_registry(&host_interfaces, &packages, &input)?;
            crate::lsp::run(packages, registry)
        }
        Commands::Fmt { check, mut paths } => {
            // No paths means the current directory (spec 0035 C1).
            if paths.is_empty() {
                paths.push(PathBuf::from("."));
            }
            crate::fmt::run(&paths, check)
        }
        Commands::Lint {
            inputs,
            packages,
            host_interfaces,
        } => {
            let input = inputs.first().cloned().unwrap_or_default();
            let registry = build_platform_registry(&host_interfaces, &packages, &input)?;
            crate::lint::run(&inputs, &packages, &registry)
        }
        Commands::New { name } => crate::pome::scaffold(&name),
        Commands::Pome { args } => crate::pome::run(&args),
    }
}

fn build(
    input: &PathBuf,
    package_paths: &[PathBuf],
    host_interfaces: &[String],
    backend: Option<&str>,
    mode: EmitMode,
) -> Result<Artifact> {
    let platform_registry = build_platform_registry(host_interfaces, package_paths, input)?;
    let ir = compile_to_ir(input, package_paths, &platform_registry)?;
    let options = BackendOptions {
        mode,
        platform_registry,
        ..Default::default()
    };
    let requested = backend.unwrap_or(DEFAULT_BACKEND);

    // A `--backend PATH` pointing at a descriptor selects an external process.
    if external::is_descriptor_path(requested) {
        let backend = external::load_backend(Path::new(requested))?;
        note_tier(&backend);
        return backend.compile(&ir, &options).map_err(Error::from);
    }

    let registry = registry();
    let name = canonical_backend(requested);
    let backend = registry.get(name).ok_or_else(|| {
        let available = registry
            .list()
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        Error::new(format!(
            "unknown backend `{name}`; available backends: {available}"
        ))
    })?;
    note_tier(backend);
    backend.compile(&ir, &options).map_err(Error::from)
}

/// Builds the platform registry for capability manifest generation (spec 0025)
/// and backend coverage checking. Host interface externs (spec 0026) are
/// appended when activated via `--host-interface`.
fn build_platform_registry(
    host_interfaces: &[String],
    package_paths: &[PathBuf],
    input: &Path,
) -> Result<Vec<emela_codegen::PlatformFn>> {
    let mut registry = emela_codegen::platform_interface();
    let host_entries = load_host_interface_externs(host_interfaces, package_paths, input)?;
    registry.extend(host_entries);
    Ok(registry)
}

/// Parses host interface package source files (spec 0026) and extracts
/// `PlatformFn` entries for each `extern fn` declaration found in `host.*`
/// modules.
fn load_host_interface_externs(
    host_interfaces: &[String],
    package_paths: &[PathBuf],
    input: &Path,
) -> Result<Vec<emela_codegen::PlatformFn>> {
    let mut entries = Vec::new();
    if host_interfaces.is_empty() {
        return Ok(entries);
    }
    let input_dir = input.parent().unwrap_or_else(|| Path::new("."));

    for name in host_interfaces {
        if name.contains('.') {
            return Err(Error::new(format!(
                "invalid host interface name `{name}`: dots are not allowed (use e.g. `gpio`, not `host.gpio`)"
            )));
        }
        let relative_path = format!("host/{name}.emel");
        let host_capability = format!("host.{name}");

        // Try relative to the input file first, then each --package directory.
        let mut found = None;
        let candidate = input_dir.join(&relative_path);
        if candidate.is_file() {
            found = Some(candidate);
        }
        if found.is_none() {
            for package_path in package_paths {
                if let Some(root) = package_source_root(package_path) {
                    let candidate = root.join(&relative_path);
                    if candidate.is_file() {
                        found = Some(candidate);
                        break;
                    }
                }
            }
        }
        let path = found.ok_or_else(|| {
            Error::new(format!(
                "host interface `{name}` not found: expected a file at `{relative_path}` relative to the input file or a package directory"
            ))
        })?;

        let source = fs::read_to_string(&path).map_err(|err| {
            Error::new(format!(
                "failed to read host interface `{}`: {err}",
                path.display()
            ))
        })?;
        let (program, parse_errors) = parse_program(&path.display().to_string(), &source);
        if !parse_errors.is_empty() {
            return Err(aggregate_errors(&parse_errors));
        }

        let module_path = format!("host.{name}");
        for declaration in &program.externs {
            if declaration.is_intrinsic {
                continue;
            }
            let canonical = declaration.canonical();
            // Validate the extern lives in the expected host module.
            match &declaration.module {
                Some(m) if m == &module_path => {}
                Some(m) => {
                    return Err(Error::new(format!(
                        "host interface `{name}`: `extern fn {canonical}` is declared in module `{m}`, expected `{module_path}`"
                    )));
                }
                None => {
                    return Err(Error::new(format!(
                        "host interface `{name}`: `extern fn {}` is not inside `module {module_path}`",
                        declaration.name
                    )));
                }
            }
            // Validate the uses clause references the host capability.
            if !declaration.effects.effects.contains(&host_capability) {
                return Err(Error::new(format!(
                    "host interface `{name}`: `extern fn {canonical}` must declare `uses {{ {host_capability} }}`"
                )));
            }
            entries.push(emela_codegen::PlatformFn {
                path: vec!["host".to_string(), name.clone()],
                name: declaration.name.clone(),
                params: declaration.params.iter().map(|p| p.ty.clone()).collect(),
                ret: declaration.ret.clone(),
                throws: declaration.throws.clone(),
                capability: host_capability.clone(),
            });
        }
    }
    Ok(entries)
}

/// Reads a package's `emela-package.json` to discover its `source_root`.
fn package_source_root(package_path: &Path) -> Option<PathBuf> {
    let manifest_path = package_path.join("emela-package.json");
    let manifest_source = fs::read_to_string(&manifest_path).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_source).ok()?;
    let source = manifest.get("source")?.as_str()?;
    Some(package_path.join(source))
}

/// Builds `input` to a `wasm-wasi` module and executes it in-process, exiting
/// the process with the program's exit code. `run` runs WebAssembly, so only the
/// wasm backend is accepted.
#[cfg(feature = "run")]
fn run_program(
    input: &PathBuf,
    packages: &[PathBuf],
    host_interfaces: &[String],
    backend: Option<&str>,
) -> Result<()> {
    if let Some(name) = backend
        && canonical_backend(name) != "wasm-wasi"
    {
        return Err(Error::new(format!(
            "`run` executes WebAssembly; backend `{name}` is not supported (use `wasm-wasi`)"
        )));
    }
    let artifact = build(
        input,
        packages,
        host_interfaces,
        Some("wasm-wasi"),
        EmitMode::Default,
    )?;
    let code = crate::run::execute(&artifact.bytes)?;
    std::process::exit(code)
}

/// Fallback when the `run` feature is disabled: report it clearly instead of
/// silently failing to build the module.
#[cfg(not(feature = "run"))]
fn run_program(
    _input: &PathBuf,
    _packages: &[PathBuf],
    _host_interfaces: &[String],
    _backend: Option<&str>,
) -> Result<()> {
    Err(Error::new(
        "this `emela` was built without the `run` feature; rebuild with `--features run`",
    ))
}

/// `emela test` executes tests in-process like `run`, so it needs the same
/// feature.
#[cfg(feature = "run")]
fn run_tests(platform_registry: &[emela_codegen::PlatformFn]) -> Result<()> {
    crate::test_runner::run(platform_registry)
}

#[cfg(not(feature = "run"))]
fn run_tests(_platform_registry: &[emela_codegen::PlatformFn]) -> Result<()> {
    Err(Error::new(
        "this `emela` was built without the `run` feature; rebuild with `--features run`",
    ))
}

/// Warns when building with a backend that is not fully supported (Tier 1).
fn note_tier(backend: &dyn Backend) {
    if backend.tier() != Tier::Tier1 {
        eprintln!(
            "note: backend `{}` is {} (build + smoke only)",
            backend.name(),
            backend.tier().label()
        );
    }
}

fn write_artifact(artifact: Artifact, output: Option<PathBuf>) -> Result<()> {
    match output {
        Some(output) => fs::write(&output, &artifact.bytes)
            .map_err(|err| Error::new(format!("failed to write `{}`: {err}", output.display()))),
        None => {
            if artifact.kind.is_text() {
                print!("{}", String::from_utf8_lossy(&artifact.bytes));
                Ok(())
            } else {
                Err(Error::new(
                    "binary artifact; pass -o FILE to write it to disk",
                ))
            }
        }
    }
}

fn compile_to_ir(
    input: &PathBuf,
    package_paths: &[PathBuf],
    platform_registry: &[emela_codegen::PlatformFn],
) -> Result<IrProgram> {
    // Lowering and the backends need a `main` (the `_start` entrypoint), so IR and
    // build always require it — only `check --library` relaxes this.
    let (program, typed) = compile_frontend(input, package_paths, true, platform_registry)?;
    let (program, typed) = strip_tests(program, typed);
    check_dev_reachability(input, &program)?;
    Ok(lower::lower(&program, &typed))
}

/// Rejects a build whose artifact would reach a dev-dependency's code (spec
/// 0040 D4). With `@test` functions stripped (T8), everything reachable from
/// `main` is artifact code, and none of it may resolve into a dev-only import
/// root. A private helper that only tests call may keep using a dev dependency:
/// it is simply unreachable here. (Reachability follows function references;
/// a dev dependency's trait impls and types travel with its functions.)
fn check_dev_reachability(input: &Path, program: &crate::ast::Program) -> Result<()> {
    let dev_roots = crate::pome::dev_import_roots(input)?;
    if dev_roots.is_empty() {
        return Ok(());
    }
    let table = crate::resolve::FnTable::build(program);
    let mut queue: Vec<usize> = program
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| function.module_path.is_empty() && function.name == "main")
        .map(|(index, _)| index)
        .collect();
    let mut seen: std::collections::HashSet<usize> = queue.iter().copied().collect();
    while let Some(index) = queue.pop() {
        let function = &program.functions[index];
        if let Some(root) = function.module_path.first()
            && dev_roots.iter().any(|dev| dev == root)
        {
            return Err(Error::new(format!(
                "`{}.{}` is provided by dev-dependency import root `{root}` and must not be \
                 reachable from build artifacts (spec 0040); call it from a `@test` fn only, \
                 or move the dependency to `[dependencies]`",
                function.module_path.join("."),
                function.name
            )));
        }
        walk_block_refs(
            &function.body,
            &function.module_path,
            &table,
            &mut |target| {
                if seen.insert(target) {
                    queue.push(target);
                }
            },
        );
    }
    Ok(())
}

/// Walks a block for function references, resolving each the way the lowerer
/// does, and reports the referenced function indices.
fn walk_block_refs(
    block: &crate::ast::Block,
    module: &[String],
    table: &crate::resolve::FnTable,
    visit: &mut impl FnMut(usize),
) {
    use crate::ast::BlockItem;
    for item in &block.items {
        match item {
            BlockItem::Let { value, .. } => walk_expr_refs(value, module, table, visit),
            BlockItem::Expr(expr) => walk_expr_refs(expr, module, table, visit),
        }
    }
}

fn walk_expr_refs(
    expr: &crate::ast::Expr,
    module: &[String],
    table: &crate::resolve::FnTable,
    visit: &mut impl FnMut(usize),
) {
    use crate::ast::Expr;
    use crate::resolve::Resolved;
    let resolve_ref = |segments: &[String], visit: &mut dyn FnMut(usize)| {
        if let Resolved::One(entry) = table.resolve_in(segments, module) {
            visit(entry.index);
        }
    };
    match expr {
        Expr::Var(name, _) => resolve_ref(std::slice::from_ref(name), visit),
        Expr::Path { segments, .. } => resolve_ref(segments, visit),
        Expr::Call { callee, args, .. } => {
            walk_expr_refs(callee, module, table, visit);
            for arg in args {
                walk_expr_refs(arg, module, table, visit);
            }
        }
        Expr::Fn { body, .. } => walk_block_refs(body, module, table, visit),
        Expr::Binary { left, right, .. } => {
            walk_expr_refs(left, module, table, visit);
            walk_expr_refs(right, module, table, visit);
        }
        Expr::Block(block) => walk_block_refs(block, module, table, visit),
        Expr::If {
            cond, then, els, ..
        } => {
            walk_expr_refs(cond, module, table, visit);
            walk_block_refs(then, module, table, visit);
            walk_block_refs(els, module, table, visit);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => {
            walk_expr_refs(value, module, table, visit);
        }
        Expr::Panic { message, .. } => walk_expr_refs(message, module, table, visit),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            walk_expr_refs(scrutinee, module, table, visit);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk_expr_refs(guard, module, table, visit);
                }
                walk_expr_refs(&arm.body, module, table, visit);
            }
        }
        Expr::Try { body, arms, .. } => {
            walk_block_refs(body, module, table, visit);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk_expr_refs(guard, module, table, visit);
                }
                walk_expr_refs(&arm.body, module, table, visit);
            }
        }
        Expr::Array(elements, _) => {
            for element in elements {
                walk_expr_refs(element, module, table, visit);
            }
        }
        Expr::RecordLiteral { fields, .. } => {
            for (_, _, value) in fields {
                walk_expr_refs(value, module, table, visit);
            }
        }
        Expr::Field { target, .. } => walk_expr_refs(target, module, table, visit),
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::TypePath { .. } => {}
    }
}

/// Excludes `@test` functions from normal artifacts (spec 0040 T8): they are
/// type-checked by `check`/lint/LSP but never lowered or emitted by
/// `build`/`run`/`ir`, and contribute nothing to the capability manifest (spec
/// 0025). `TypedProgram::functions` is index-aligned with
/// `Program::functions`, so both are filtered together.
pub(crate) fn strip_tests(
    mut program: crate::ast::Program,
    mut typed: typecheck::TypedProgram,
) -> (crate::ast::Program, typecheck::TypedProgram) {
    let keep: Vec<bool> = program.functions.iter().map(|f| !f.is_test).collect();
    let mut kept = keep.iter();
    program.functions.retain(|_| *kept.next().unwrap());
    let mut kept = keep.iter();
    typed.functions.retain(|_| *kept.next().unwrap());
    (program, typed)
}

/// Builds the import roots for `input`: the explicit `--package` roots, plus
/// the dependency Pomes resolved for the project that encloses `input` (spec
/// 0032 M1) — each dependency Pome's modules become importable under its
/// source-path leaf as the import root.
pub(crate) fn load_import_roots(
    input: &Path,
    package_paths: &[PathBuf],
) -> Result<Vec<imports::PackageSource>> {
    let mut packages = imports::load_packages(package_paths)?;
    for (name, source_root) in crate::pome::dependency_packages(input)? {
        if packages.iter().any(|package| package.name() == name) {
            return Err(Error::new(format!(
                "import-root name `{name}` from a dependency Pome collides with another package; \
                 rename the `--package` or the Pome"
            )));
        }
        packages.push(imports::PackageSource::new(name, source_root));
    }
    // With every root assembled — explicit `--package` and dependency Pomes
    // alike — reject a `std` package that shadows an embedded module (spec
    // 0038). This single choke point covers the CLI, lint, and both LSP call
    // sites.
    imports::check_reserved_std_modules(&packages)?;
    Ok(packages)
}

pub(crate) fn compile_frontend(
    input: &PathBuf,
    package_paths: &[PathBuf],
    require_main: bool,
    platform_registry: &[emela_codegen::PlatformFn],
) -> Result<(crate::ast::Program, typecheck::TypedProgram)> {
    let source = fs::read_to_string(input)
        .map_err(|err| Error::new(format!("failed to read `{}`: {err}", input.display())))?;
    let packages = load_import_roots(input, package_paths)?;
    let (program, typed, errors) = compile_frontend_source_all(
        input,
        &source,
        &packages,
        require_main,
        &HashMap::new(),
        platform_registry,
    );
    if errors.is_empty() {
        Ok((program, typed))
    } else {
        // The CLI reports every collected diagnostic (spec 0033), joined into
        // one error whose rendered form separates them with blank lines.
        Err(aggregate_errors(&errors))
    }
}

/// Joins collected diagnostics into a single `Error` for the CLI paths, so
/// `check`/`build`/`ir` print all of them, not just the first.
fn aggregate_errors(errors: &[Error]) -> Error {
    Error::new(
        errors
            .iter()
            .map(Error::render)
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

/// Runs the whole frontend over an in-memory source string, collecting every
/// error (spec 0033) instead of stopping at the first. `input` is used only as
/// a diagnostic label and as the base directory for resolving relative imports.
/// `overlay` maps canonicalized module paths to in-memory contents that shadow
/// the filesystem (the LSP's open buffers); pass an empty map otherwise.
///
/// The returned `Program`/`TypedProgram` are partial when errors are present —
/// they hold everything that did parse and register — so callers like the LSP
/// can still extract scope information. They must not be lowered unless the
/// error list is empty.
pub(crate) fn compile_frontend_source_all(
    input: &Path,
    source: &str,
    packages: &[imports::PackageSource],
    require_main: bool,
    overlay: &HashMap<PathBuf, String>,
    platform_registry: &[emela_codegen::PlatformFn],
) -> (crate::ast::Program, typecheck::TypedProgram, Vec<Error>) {
    let label = input.display().to_string();
    let (mut program, mut errors) = parse_program(&label, source);
    // The compilation root is user-authored: its `intrinsic fn` declarations
    // are rejected and dropped (spec 0038) before imports merge in the
    // embedded std's — the only place intrinsics may be declared.
    imports::reject_user_intrinsics(&mut program, &mut errors);
    let (mut program, import_errors) =
        imports::resolve_imports_with_overlay(input, program, packages, overlay);
    errors.extend(import_errors);
    // Merge the embedded Core Prelude (spec 0021): the operator traits and their
    // built-in instances, so `1 + 2` and friends resolve with no explicit import.
    if let Err(error) = merge_prelude(&mut program) {
        errors.push(error);
    }
    // Fill in defaulted trait methods (spec 0020) so type-checking and lowering
    // see fully populated impls.
    typecheck::expand_trait_defaults(&mut program);
    // When recovery already dropped declarations, `main` may be among them;
    // requiring it would only add noise next to the real errors.
    let require_main = require_main && errors.is_empty();
    let (typed, check_errors) = typecheck::check(&program, require_main, platform_registry);
    errors.extend(check_errors);
    crate::error::normalize_errors(&mut errors);
    (program, typed, errors)
}

/// Single-error variant of [`compile_frontend_source_all`], kept for the
/// embedder API (`api.rs`): the playground shows one diagnostic at a time.
fn compile_frontend_source(
    input: &Path,
    source: &str,
    packages: &[imports::PackageSource],
    require_main: bool,
    platform_registry: &[emela_codegen::PlatformFn],
) -> Result<(crate::ast::Program, typecheck::TypedProgram)> {
    let (program, typed, mut errors) = compile_frontend_source_all(
        input,
        source,
        packages,
        require_main,
        &HashMap::new(),
        platform_registry,
    );
    if errors.is_empty() {
        Ok((program, typed))
    } else {
        Err(errors.remove(0))
    }
}

/// Parses the embedded Core Prelude (spec 0021) and merges its declarations into
/// `program`. Because the prelude is embedded, this works with no `--package`:
/// a single-file program still sees the operator traits and their instances.
pub(crate) fn merge_prelude(program: &mut crate::ast::Program) -> Result<()> {
    let (prelude, errors) = parse_program("<core-prelude>", crate::prelude::CORE_SRC);
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    program.functions.extend(prelude.functions);
    program.externs.extend(prelude.externs);
    program.enums.extend(prelude.enums);
    program.records.extend(prelude.records);
    program.traits.extend(prelude.traits);
    program.impls.extend(prelude.impls);
    program.effects.extend(prelude.effects);
    Ok(())
}

/// Type-checks an in-memory source string. Filesystem-free entry point used by
/// embedders such as the WebAssembly playground.
pub(crate) fn check_source(label: &str, source: &str) -> Result<()> {
    compile_frontend_source(
        Path::new(label),
        source,
        &[],
        true,
        &emela_codegen::platform_interface(),
    )?;
    Ok(())
}

/// Lowers an in-memory source string to IR and renders it as text.
pub(crate) fn ir_source(label: &str, source: &str) -> Result<String> {
    let (program, typed) = compile_frontend_source(
        Path::new(label),
        source,
        &[],
        true,
        &emela_codegen::platform_interface(),
    )?;
    let (program, typed) = strip_tests(program, typed);
    let ir = lower::lower(&program, &typed);
    Ok(emit_text(&ir))
}

/// Compiles an in-memory source string with a built-in backend. External
/// (process-based) backends are intentionally not reachable here.
pub(crate) fn compile_source(
    label: &str,
    source: &str,
    backend: &str,
    mode: EmitMode,
) -> Result<Artifact> {
    let platform_registry = emela_codegen::platform_interface();
    let (program, typed) =
        compile_frontend_source(Path::new(label), source, &[], true, &platform_registry)?;
    let (program, typed) = strip_tests(program, typed);
    let ir = lower::lower(&program, &typed);
    let options = BackendOptions {
        mode,
        platform_registry,
        ..Default::default()
    };
    let registry = registry();
    let name = canonical_backend(backend);
    let backend = registry.get(name).ok_or_else(|| {
        let available = registry
            .list()
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        Error::new(format!(
            "unknown backend `{name}`; available backends: {available}"
        ))
    })?;
    backend.compile(&ir, &options).map_err(Error::from)
}

#[derive(clap::Parser)]
#[command(
    name = "emela",
    about = "The Emela compiler and toolchain",
    version = option_env!("EMELA_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Type-check a module without producing output
    Check {
        #[command(flatten)]
        args: CompileArgs,
    },
    /// Compile a module to an artifact
    Build {
        #[command(flatten)]
        args: CompileArgs,
    },
    /// Lower a module to typed IR text
    Ir {
        #[command(flatten)]
        args: CompileArgs,
        /// Show the IR after ARC retain/release insertion (spec 0048), as the
        /// wasm backend consumes it
        #[arg(long)]
        rc: bool,
    },
    /// Compile a module and run it in-process
    Run {
        #[command(flatten)]
        args: CompileArgs,
    },
    /// Discover and run the current Pome's tests (spec 0040)
    Test {
        /// Activate an embedded host interface package (spec 0026). Repeatable.
        #[arg(long = "host-interface", value_name = "NAME")]
        host_interfaces: Vec<String>,
    },
    /// List the available compiler backends
    Backends,
    /// Start the language server over stdio (spec 0033)
    Lsp {
        /// Package root to resolve imports against (repeatable)
        #[arg(long = "package", value_name = "DIR")]
        packages: Vec<PathBuf>,
        /// Activate an embedded host interface package (spec 0026). Repeatable.
        #[arg(long = "host-interface", value_name = "NAME")]
        host_interfaces: Vec<String>,
    },
    /// Format Emela source files (spec 0035)
    Fmt {
        /// Check formatting and exit non-zero instead of rewriting files
        #[arg(long)]
        check: bool,
        /// Files or directories to format (defaults to the current directory)
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
    },
    /// Report lint warnings (spec 0035)
    Lint {
        /// Package root to resolve imports against (repeatable)
        #[arg(long = "package", value_name = "DIR")]
        packages: Vec<PathBuf>,
        /// Activate an embedded host interface package (spec 0026). Repeatable.
        #[arg(long = "host-interface", value_name = "NAME")]
        host_interfaces: Vec<String>,
        /// Source files to lint
        #[arg(value_name = "FILE", required = true)]
        inputs: Vec<PathBuf>,
    },
    /// Scaffold a new Pome package (spec 0032)
    New {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Manage packages: add, remove, list, update, install, search (spec 0032)
    Pome {
        /// The pome verb and its arguments
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "ARGS"
        )]
        args: Vec<String>,
    },
}

/// Flags shared by the compile-style subcommands (`check`/`build`/`ir`/`run`).
/// They present the same surface at the parse layer; per-command rules that
/// reject a flag live in [`reject_library`]/[`reject_run_flags`] so their
/// error messages stay stable across the clap migration.
#[derive(clap::Args)]
struct CompileArgs {
    /// The Emela source file to compile
    #[arg(value_name = "FILE")]
    input: PathBuf,
    /// Check a module that has no `main` (valid for `check` only)
    #[arg(long = "library", visible_alias = "lib")]
    library: bool,
    /// Write output to FILE instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,
    /// Select the compiler backend (e.g. `js-node`, `wasm-wasi`)
    #[arg(long, value_name = "NAME")]
    backend: Option<String>,
    /// Output form: `default` or `text`
    #[arg(long, value_name = "MODE")]
    emit: Option<String>,
    /// Add a package root to resolve imports against (repeatable)
    #[arg(long = "package", value_name = "DIR")]
    packages: Vec<PathBuf>,
    /// Activate an embedded host interface package (spec 0026). Repeatable.
    /// Example: `--host-interface db` makes `import host.db` available.
    #[arg(long = "host-interface", value_name = "NAME")]
    host_interfaces: Vec<String>,
}

impl CompileArgs {
    /// Resolve `--emit` to an [`EmitMode`], preserving the pre-clap error text.
    fn emit_mode(&self) -> Result<EmitMode> {
        match self.emit.as_deref() {
            None | Some("default") => Ok(EmitMode::Default),
            Some("text") => Ok(EmitMode::Text),
            Some(other) => Err(Error::new(format!(
                "unknown --emit value `{other}` (expected `default` or `text`)"
            ))),
        }
    }
}

/// `--library` only makes sense for `check`: `build`/`ir`/`run` need a `main` to
/// lower and run, so reject the flag there rather than silently ignoring it.
fn reject_library(args: &CompileArgs, command: &str) -> Result<()> {
    if args.library {
        return Err(Error::new(format!(
            "`--library` is only valid for `check`, not `{command}`"
        )));
    }
    Ok(())
}

/// `run` executes the module in-process rather than emitting a file, so `-o` and
/// `--emit` have no meaning there.
fn reject_run_flags(args: &CompileArgs) -> Result<()> {
    if args.output.is_some() {
        return Err(Error::new("`-o`/`--output` is not valid for `run`"));
    }
    if args.emit.is_some() {
        return Err(Error::new("`--emit` is not valid for `run`"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontend_errors(source: &str) -> (crate::ast::Program, Vec<String>) {
        let (program, _, errors) = compile_frontend_source_all(
            Path::new("test.emel"),
            source,
            &[],
            true,
            &HashMap::new(),
            &emela_codegen::platform_interface(),
        );
        let messages = errors
            .iter()
            .map(|error| error.message().to_string())
            .collect();
        (program, messages)
    }

    // The embedded std modules (spec 0038) resolve with no packages and no
    // filesystem behind them — the playground entry points (`api.rs`) rely on
    // exactly this path.
    #[test]
    fn embedded_std_resolves_without_packages() {
        let (_, errors) = frontend_errors(
            "import std.io\n\nfn main() -> Unit uses { Io } {\n    Io.print(\"hi\\n\")\n}\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
    }

    // An `intrinsic fn` in a user source is rejected and dropped (spec 0038):
    // only the embedded std declares intrinsics.
    #[test]
    fn user_intrinsic_is_rejected_and_dropped() {
        let (_, errors) = frontend_errors(
            "intrinsic fn i32_add(a: Int, b: Int) -> Int uses {}\n\nfn main() -> Int uses {} {\n    0\n}\n",
        );
        assert!(
            errors.contains(&"Intrinsic outside the embedded std".to_string()),
            "{errors:?}"
        );
    }

    // `typecheck::check` no longer tolerates a repeated intrinsic declaration
    // (spec 0038): the embedded std declares each exactly once, and user
    // sources never reach registration. Calling the checker directly (below
    // the driver's reject-and-drop) exercises the duplicate arm.
    #[test]
    fn duplicate_intrinsic_declaration_is_an_error() {
        let (program, _) = parse_program(
            "test.emel",
            "intrinsic fn i32_add(a: Int, b: Int) -> Int uses {}\nintrinsic fn i32_add(a: Int, b: Int) -> Int uses {}\nfn main() -> Int uses {} {\n    i32_add(1, 2)\n}\n",
        );
        let (_, errors) = typecheck::check(&program, true, &emela_codegen::platform_interface());
        let messages: Vec<String> = errors
            .iter()
            .map(|error| error.message().to_string())
            .collect();
        assert!(
            messages.contains(&"Duplicate function".to_string()),
            "{messages:?}"
        );
    }

    // The intrinsic validation arms (unknown name, signature, purity; spec
    // 0021) still guard the embedded std's own declarations; they are
    // reachable only below the driver's reject-and-drop.
    #[test]
    fn unknown_intrinsic_is_still_validated() {
        let (program, _) = parse_program(
            "test.emel",
            "intrinsic fn bogus_op(a: Int, b: Int) -> Int uses {}\nfn main() -> Int uses {} {\n    bogus_op(1, 2)\n}\n",
        );
        let (_, errors) = typecheck::check(&program, true, &emela_codegen::platform_interface());
        let messages: Vec<String> = errors
            .iter()
            .map(|error| error.message().to_string())
            .collect();
        assert!(
            messages.contains(&"Unknown intrinsic".to_string()),
            "{messages:?}"
        );
    }

    // Two independently broken bodies both report (spec 0033), instead of the
    // second hiding behind the first.
    #[test]
    fn collects_type_errors_across_functions() {
        let (_, errors) = frontend_errors(
            r#"
fn f() -> Int uses {} {
  "text"
}

fn g() -> Int uses {} {
  unknown_name
}

fn main() -> Int uses {} {
  f() + g()
}
"#,
        );
        assert!(errors.contains(&"Type mismatch".to_string()), "{errors:?}");
        assert!(errors.contains(&"Unknown name".to_string()), "{errors:?}");
    }

    // A failed top-level declaration is skipped and parsing resumes at the
    // next one, so both parse errors surface and the valid declarations
    // (the enum and `main`) still reach the type checker.
    #[test]
    fn parser_recovers_at_top_level_declarations() {
        let (program, errors) = frontend_errors(
            r#"
fn broken( -> Int uses {} {
  1
}

enum Color {
  Red
  Green
}

fn also_broken() -> {
  2
}

fn main() -> Int uses {} {
  match Color::Red {
    Red -> 1
    Green -> 2
  }
}
"#,
        );
        assert_eq!(
            errors,
            vec!["Expected a name".to_string(), "Expected a name".to_string()],
            "{errors:?}"
        );
        assert!(program.enums.iter().any(|decl| decl.name == "Color"));
        assert!(program.functions.iter().any(|f| f.name == "main"));
    }

    // Each import statement reports its own failure.
    #[test]
    fn collects_import_errors_per_statement() {
        let (_, errors) = frontend_errors(
            r#"
import nowhere.thing
import missing.item

fn main() -> Int uses {} {
  0
}
"#,
        );
        let import_errors = errors
            .iter()
            .filter(|message| message.starts_with("Unknown module"))
            .count();
        assert_eq!(import_errors, 2, "{errors:?}");
    }
}
