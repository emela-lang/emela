use std::collections::HashMap;
use std::env;
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
fn registry() -> BackendRegistry {
    let mut registry = BackendRegistry::new();
    #[cfg(feature = "backend-wasm")]
    registry.register(Box::new(emela_backend_wasm::WasmBackend));
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
    match parse_args()? {
        Command::Check {
            input,
            packages,
            library,
        } => {
            // `--library` compile-checks a module that has no `main` (spec 0003).
            let _ = compile_frontend(&input, &packages, !library)?;
            Ok(())
        }
        Command::Build {
            input,
            output,
            packages,
            backend,
            mode,
        } => {
            let artifact = build(&input, &packages, backend.as_deref(), mode)?;
            write_artifact(artifact, output)
        }
        Command::Ir {
            input,
            output,
            packages,
        } => {
            let ir = compile_to_ir(&input, &packages)?;
            let text = emit_text(&ir);
            match output {
                Some(output) => fs::write(&output, text).map_err(|err| {
                    Error::new(format!("failed to write `{}`: {err}", output.display()))
                }),
                None => {
                    print!("{text}");
                    Ok(())
                }
            }
        }
        Command::Backends => {
            for (name, tier) in registry().list() {
                println!("{name}\t{}", tier.label());
            }
            Ok(())
        }
        Command::Run {
            input,
            packages,
            backend,
        } => run_program(&input, &packages, backend.as_deref()),
        // `emela lsp` (spec 0033): the language server over stdio.
        Command::Lsp { packages } => crate::lsp::run(packages),
        Command::Fmt { paths, check } => crate::fmt::run(&paths, check),
        Command::Lint { inputs, packages } => crate::lint::run(&inputs, &packages),
        Command::New { name } => crate::pome::scaffold(&name),
        Command::Pome { args } => crate::pome::run(&args),
        Command::Version => {
            println!(
                "{}",
                option_env!("EMELA_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
            );
            Ok(())
        }
    }
}

fn build(
    input: &PathBuf,
    package_paths: &[PathBuf],
    backend: Option<&str>,
    mode: EmitMode,
) -> Result<Artifact> {
    let ir = compile_to_ir(input, package_paths)?;
    let options = BackendOptions {
        mode,
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

/// Builds `input` to a `wasm-wasi` module and executes it in-process, exiting
/// the process with the program's exit code. `run` runs WebAssembly, so only the
/// wasm backend is accepted.
#[cfg(feature = "run")]
fn run_program(input: &PathBuf, packages: &[PathBuf], backend: Option<&str>) -> Result<()> {
    if let Some(name) = backend
        && canonical_backend(name) != "wasm-wasi"
    {
        return Err(Error::new(format!(
            "`run` executes WebAssembly; backend `{name}` is not supported (use `wasm-wasi`)"
        )));
    }
    let artifact = build(input, packages, Some("wasm-wasi"), EmitMode::Default)?;
    let code = crate::run::execute(&artifact.bytes)?;
    std::process::exit(code)
}

/// Fallback when the `run` feature is disabled: report it clearly instead of
/// silently failing to build the module.
#[cfg(not(feature = "run"))]
fn run_program(_input: &PathBuf, _packages: &[PathBuf], _backend: Option<&str>) -> Result<()> {
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

fn compile_to_ir(input: &PathBuf, package_paths: &[PathBuf]) -> Result<IrProgram> {
    // Lowering and the backends need a `main` (the `_start` entrypoint), so IR and
    // build always require it — only `check --library` relaxes this.
    let (program, typed) = compile_frontend(input, package_paths, true)?;
    Ok(lower::lower(&program, &typed))
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
    Ok(packages)
}

pub(crate) fn compile_frontend(
    input: &PathBuf,
    package_paths: &[PathBuf],
    require_main: bool,
) -> Result<(crate::ast::Program, typecheck::TypedProgram)> {
    let source = fs::read_to_string(input)
        .map_err(|err| Error::new(format!("failed to read `{}`: {err}", input.display())))?;
    let packages = load_import_roots(input, package_paths)?;
    let (program, typed, errors) =
        compile_frontend_source_all(input, &source, &packages, require_main, &HashMap::new());
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
) -> (crate::ast::Program, typecheck::TypedProgram, Vec<Error>) {
    let label = input.display().to_string();
    let (program, mut errors) = parse_program(&label, source);
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
    let (typed, check_errors) = typecheck::check(&program, require_main);
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
) -> Result<(crate::ast::Program, typecheck::TypedProgram)> {
    let (program, typed, mut errors) =
        compile_frontend_source_all(input, source, packages, require_main, &HashMap::new());
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
    program.traits.extend(prelude.traits);
    program.impls.extend(prelude.impls);
    Ok(())
}

/// Type-checks an in-memory source string. Filesystem-free entry point used by
/// embedders such as the WebAssembly playground.
pub(crate) fn check_source(label: &str, source: &str) -> Result<()> {
    compile_frontend_source(Path::new(label), source, &[], true)?;
    Ok(())
}

/// Lowers an in-memory source string to IR and renders it as text.
pub(crate) fn ir_source(label: &str, source: &str) -> Result<String> {
    let (program, typed) = compile_frontend_source(Path::new(label), source, &[], true)?;
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
    let (program, typed) = compile_frontend_source(Path::new(label), source, &[], true)?;
    let ir = lower::lower(&program, &typed);
    let options = BackendOptions {
        mode,
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

enum Command {
    Check {
        input: PathBuf,
        packages: Vec<PathBuf>,
        library: bool,
    },
    Build {
        input: PathBuf,
        output: Option<PathBuf>,
        packages: Vec<PathBuf>,
        backend: Option<String>,
        mode: EmitMode,
    },
    Ir {
        input: PathBuf,
        output: Option<PathBuf>,
        packages: Vec<PathBuf>,
    },
    /// `emela run FILE` — build to `wasm-wasi` and execute it in-process
    /// (requires the `run` feature).
    Run {
        input: PathBuf,
        packages: Vec<PathBuf>,
        backend: Option<String>,
    },
    Backends,
    Version,
    /// `emela lsp` — the language server over stdio (spec 0033).
    Lsp {
        packages: Vec<PathBuf>,
    },
    /// `emela fmt [--check] [PATH ...]` — canonical formatting (spec 0035).
    Fmt {
        paths: Vec<PathBuf>,
        check: bool,
    },
    /// `emela lint [--package DIR] FILE ...` — lint warnings (spec 0035).
    Lint {
        inputs: Vec<PathBuf>,
        packages: Vec<PathBuf>,
    },
    /// `emela new <name>` — scaffold a new Pome (spec 0032 C2).
    New {
        name: String,
    },
    /// `emela pome <verb> ...` — package management (spec 0032 C1).
    Pome {
        args: Vec<String>,
    },
}

fn parse_args() -> Result<Command> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage());
    };
    match command.as_str() {
        "--version" | "-V" => Ok(Command::Version),
        "backends" => Ok(Command::Backends),
        "new" => {
            let name = args
                .next()
                .ok_or_else(|| Error::new("usage: emela new <name>"))?;
            if args.next().is_some() {
                return Err(Error::new("usage: emela new <name>"));
            }
            Ok(Command::New { name })
        }
        "pome" => Ok(Command::Pome {
            args: args.collect(),
        }),
        "lsp" => {
            // The server takes only `--package` roots; everything else comes
            // in over the protocol (spec 0033).
            let mut packages = Vec::new();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--package" => {
                        let Some(path) = args.next() else {
                            return Err(Error::new("missing value for --package"));
                        };
                        packages.push(PathBuf::from(path));
                    }
                    other => {
                        return Err(Error::new(format!(
                            "unsupported option `{other}` for `lsp` (expected `--package DIR`)"
                        )));
                    }
                }
            }
            Ok(Command::Lsp { packages })
        }
        "fmt" => {
            let mut check = false;
            let mut paths = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "--check" => check = true,
                    flag if flag.starts_with('-') => {
                        return Err(Error::new(format!("unsupported option `{flag}` for `fmt`")));
                    }
                    path => paths.push(PathBuf::from(path)),
                }
            }
            // No paths means the current directory (spec 0035 C1).
            if paths.is_empty() {
                paths.push(PathBuf::from("."));
            }
            Ok(Command::Fmt { paths, check })
        }
        "lint" => {
            let mut packages = Vec::new();
            let mut inputs = Vec::new();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--package" => {
                        let Some(path) = args.next() else {
                            return Err(Error::new("missing value for --package"));
                        };
                        packages.push(PathBuf::from(path));
                    }
                    flag if flag.starts_with('-') => {
                        return Err(Error::new(format!(
                            "unsupported option `{flag}` for `lint`"
                        )));
                    }
                    path => inputs.push(PathBuf::from(path)),
                }
            }
            if inputs.is_empty() {
                return Err(Error::new("usage: emela lint [--package DIR] FILE ..."));
            }
            Ok(Command::Lint { inputs, packages })
        }
        "check" => {
            let parsed = parse_compile_args(args)?;
            Ok(Command::Check {
                input: parsed.input,
                packages: parsed.packages,
                library: parsed.library,
            })
        }
        "build" => {
            let parsed = parse_compile_args(args)?;
            reject_library(&parsed, "build")?;
            Ok(Command::Build {
                input: parsed.input,
                output: parsed.output,
                packages: parsed.packages,
                backend: parsed.backend,
                mode: parsed.mode,
            })
        }
        "ir" => {
            let parsed = parse_compile_args(args)?;
            reject_library(&parsed, "ir")?;
            Ok(Command::Ir {
                input: parsed.input,
                output: parsed.output,
                packages: parsed.packages,
            })
        }
        "run" => {
            let parsed = parse_compile_args(args)?;
            reject_library(&parsed, "run")?;
            reject_run_flags(&parsed)?;
            Ok(Command::Run {
                input: parsed.input,
                packages: parsed.packages,
                backend: parsed.backend,
            })
        }
        _ => Err(usage()),
    }
}

struct CompileArgs {
    input: PathBuf,
    output: Option<PathBuf>,
    packages: Vec<PathBuf>,
    backend: Option<String>,
    mode: EmitMode,
    library: bool,
}

/// `--library` only makes sense for `check`: `build`/`ir` need a `main` to lower
/// and run, so reject the flag there rather than silently ignoring it.
fn reject_library(parsed: &CompileArgs, command: &str) -> Result<()> {
    if parsed.library {
        return Err(Error::new(format!(
            "`--library` is only valid for `check`, not `{command}`"
        )));
    }
    Ok(())
}

/// `run` executes the module in-process rather than emitting a file, so `-o` and
/// `--emit` have no meaning there.
fn reject_run_flags(parsed: &CompileArgs) -> Result<()> {
    if parsed.output.is_some() {
        return Err(Error::new("`-o`/`--output` is not valid for `run`"));
    }
    if !matches!(parsed.mode, EmitMode::Default) {
        return Err(Error::new("`--emit` is not valid for `run`"));
    }
    Ok(())
}

fn parse_compile_args(args: impl Iterator<Item = String>) -> Result<CompileArgs> {
    let mut input = None;
    let mut output = None;
    let mut packages = Vec::new();
    let mut backend = None;
    let mut mode = EmitMode::Default;
    let mut library = false;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--library" | "--lib" => {
                library = true;
            }
            "-o" | "--output" => {
                let Some(path) = args.next() else {
                    return Err(Error::new("missing value for --output"));
                };
                output = Some(PathBuf::from(path));
            }
            "--backend" => {
                let Some(value) = args.next() else {
                    return Err(Error::new("missing value for --backend"));
                };
                backend = Some(value);
            }
            "--emit" => {
                let Some(value) = args.next() else {
                    return Err(Error::new("missing value for --emit"));
                };
                mode = match value.as_str() {
                    "default" => EmitMode::Default,
                    "text" => EmitMode::Text,
                    other => {
                        return Err(Error::new(format!(
                            "unknown --emit value `{other}` (expected `default` or `text`)"
                        )));
                    }
                };
            }
            "--package" => {
                let Some(path) = args.next() else {
                    return Err(Error::new("missing value for --package"));
                };
                packages.push(PathBuf::from(path));
            }
            flag if flag.starts_with('-') => {
                return Err(Error::new(format!("unsupported option `{flag}`")));
            }
            path => {
                if input.replace(PathBuf::from(path)).is_some() {
                    return Err(Error::new("multiple input files are not supported"));
                }
            }
        }
    }
    let input = input.ok_or_else(usage)?;
    Ok(CompileArgs {
        input,
        output,
        packages,
        backend,
        mode,
        library,
    })
}

fn usage() -> Error {
    Error::new(
        "usage: emela check [--library] [--backend NAME] [--package DIR] FILE \
         | emela build [--backend NAME] [--emit default|text] [--package DIR] [-o FILE] FILE \
         | emela run [--package DIR] FILE \
         | emela ir [--package DIR] [-o FILE] FILE \
         | emela lsp [--package DIR] \
         | emela fmt [--check] [PATH ...] \
         | emela lint [--package DIR] FILE \
         | emela new <name> \
         | emela pome <add|remove|list|update|install|search> ... \
         | emela backends | emela --version",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontend_errors(source: &str) -> (crate::ast::Program, Vec<String>) {
        let (program, _, errors) =
            compile_frontend_source_all(Path::new("test.emel"), source, &[], true, &HashMap::new());
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
            "import std.io\n\nfn main() -> Unit uses { io } {\n    io.print(\"hi\\n\")\n}\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
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
            .filter(|message| message.starts_with("failed to resolve module"))
            .count();
        assert_eq!(import_errors, 2, "{errors:?}");
    }
}
