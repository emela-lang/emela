//! Lowering helpers shared by the integration tests.

#![allow(dead_code)]

use std::path::Path;

use emela_core::{LowerError, LowerInput, Module, Roots, lower_with};
use emela_resolve::hir::Program;
use emela_resolve::{
    MemoryFs, ModuleId, ModuleMap, ModuleName, NoBuiltins, build_import_graph, collect_modules,
    imports, resolve,
};
use emela_types::check_program;
use la_arena::ArenaMap;

/// Resolves `files` (paths from `src` and sources) together with the bundled Prelude and core
/// sources. The first file is the entry. Syntax and resolution errors are not allowed.
pub fn resolve_files<'s>(
    files: &[(&str, &'s str)],
    core: &[(&str, &'s str)],
) -> (Program, ModuleMap, ModuleId, ArenaMap<ModuleId, &'s str>) {
    let fs = MemoryFs::new(files.iter().map(|(path, _)| format!("src/{path}")));
    let (mut modules, diagnostics) = collect_modules(&fs, Path::new("src"));
    assert_eq!(diagnostics, []);
    let mut texts: ArenaMap<ModuleId, &str> = ArenaMap::default();
    for (path, src) in files {
        let (id, _) = modules
            .iter()
            .find(|(_, m)| m.file == Path::new("src").join(path))
            .unwrap();
        texts.insert(id, *src);
    }
    let entry = modules
        .iter()
        .find(|(_, m)| m.file == Path::new("src").join(files[0].0))
        .unwrap()
        .0;
    for (name, text) in core {
        let (id, shadowed) =
            modules.add_core(ModuleName::parse(name), Path::new("<core>").join(name));
        assert_eq!(shadowed, None);
        texts.insert(id, text);
    }

    let mut trees = ArenaMap::default();
    let mut import_lists = ArenaMap::default();
    for (id, text) in texts.iter() {
        let parse = emela_syntax::parse(text);
        assert!(parse.diagnostics().is_empty(), "{:?}", parse.diagnostics());
        import_lists.insert(id, imports(&parse.tree()));
        trees.insert(id, parse.tree());
    }
    let (_, graph_diagnostics) = build_import_graph(&modules, &import_lists);
    assert_eq!(graph_diagnostics, []);
    let (program, diagnostics) = resolve(&modules, &trees, &NoBuiltins);
    // E0130 is still allowed: it used to be reported on the `@intrinsic` declarations of the
    // bundled sources. The syntax check exempts them now (`emela_syntax` validation).
    // W0403 (`use E` written as an expression is deprecated, `[effect.use]`) is checked by the
    // tests of diagnostics; the programs here still use the deprecated forms.
    let diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|d| !d.to_string().contains("E0130"))
        .filter(|d| !matches!(d.kind, emela_resolve::DiagnosticKind::DeprecatedUse { .. }))
        .collect();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (program, modules, entry, texts)
}

/// Lowers from the exported roots with the Core IR optimisations off.
///
/// This is the shape lowering produces, which the snapshots pin: an optimisation must not be
/// baked into them (plan §4.4). Tests of the passes use [`lower_files_with`] with
/// `optimize = true`.
pub fn lower_files(files: &[(&str, &str)]) -> Module {
    try_lower_off(files, Roots::Exported).unwrap_or_else(|e| panic!("{e:?}"))
}

/// Lowers after type checking with the Core IR optimisations on (the build's default path).
/// Errors are formatted as `` `source text` what ``.
pub fn try_lower(files: &[(&str, &str)], roots: Roots) -> Result<Module, Vec<String>> {
    try_lower_with(
        files,
        roots,
        emela_core::core_sources(),
        emela_core::bundled_files(),
    )
}

/// [`try_lower`] with the Core IR optimisations off: the shape of lowering, for its snapshots.
pub fn try_lower_off(files: &[(&str, &str)], roots: Roots) -> Result<Module, Vec<String>> {
    try_lower_with_off(
        files,
        roots,
        emela_core::core_sources(),
        emela_core::bundled_files(),
    )
}

/// [`try_lower`] with the bundled core sources (`core`) and host files (`bundled_files`) replaced.
pub fn try_lower_with(
    files: &[(&str, &str)],
    roots: Roots,
    core: &[(&str, &str)],
    bundled_files: &[(&str, &str)],
) -> Result<Module, Vec<String>> {
    try_lower_rooted(files, roots, core, bundled_files, true)
}

/// [`try_lower_with`] with the Core IR optimisations off.
pub fn try_lower_with_off(
    files: &[(&str, &str)],
    roots: Roots,
    core: &[(&str, &str)],
    bundled_files: &[(&str, &str)],
) -> Result<Module, Vec<String>> {
    try_lower_rooted(files, roots, core, bundled_files, false)
}

/// [`try_lower_with`] with the Core IR optimisations turned on or off.
pub fn try_lower_with_opt(
    files: &[(&str, &str)],
    roots: Roots,
    core: &[(&str, &str)],
    bundled_files: &[(&str, &str)],
    optimize: bool,
) -> Result<Module, Vec<String>> {
    try_lower_rooted(files, roots, core, bundled_files, optimize)
}

/// Lowers from the exported roots, with the Core IR optimisations turned on or off.
pub fn lower_files_with(files: &[(&str, &str)], optimize: bool) -> Module {
    try_lower_with_opt(
        files,
        Roots::Exported,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        optimize,
    )
    .unwrap_or_else(|e| panic!("{e:?}"))
}

/// [`lower_files_with`], also returning what the Core IR passes changed.
pub fn lower_files_with_stats(
    files: &[(&str, &str)],
    optimize: bool,
) -> (Module, emela_core::opt::Stats) {
    let lowered = try_lower_rooted_lowered(
        files,
        Roots::Exported,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        optimize,
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
    (lowered.module, lowered.stats)
}

/// Lowers from the exported roots with a target language, so the result is monomorphised as a
/// build's lowering is, and returns what the Core IR passes and the post-monomorphisation δ stage
/// changed.
pub fn lower_files_mono_with_stats(
    files: &[(&str, &str)],
    language: &str,
    optimize: bool,
) -> (Module, emela_core::opt::Stats) {
    let lowered = try_lower_rooted_lowered_lang(
        files,
        Roots::Exported,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        optimize,
        Some(language),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
    (lowered.module, lowered.stats)
}

/// Lowers with the Core IR passes off, then runs the reduction pipeline with `rules`.
///
/// This is how a rule's tests isolate it while the per-rule flags are off by default (A10 flips
/// them on later): the module arrives at the pipeline as lowering made it, and only the requested
/// reduction rules are on. The existing passes (hoist, the fusions, inlining) still run, because
/// they are not behind the new flags. `core_fns` is empty, so a fusion that matches a bundled
/// `List` function does not fire; a rule test does not depend on one.
pub fn lower_files_with_rules(
    files: &[(&str, &str)],
    rules: emela_core::opt::Rules,
) -> (Module, emela_core::opt::Stats) {
    let mut module = lower_files(files);
    let core_fns = emela_core::opt::CoreFns::default();
    let stats = emela_core::opt::run_with(
        &mut module,
        &core_fns,
        emela_core::opt::Options {
            rules,
            ..emela_core::opt::Options::default()
        },
    );
    (module, stats)
}

fn try_lower_rooted(
    files: &[(&str, &str)],
    roots: Roots,
    core: &[(&str, &str)],
    bundled_files: &[(&str, &str)],
    optimize: bool,
) -> Result<Module, Vec<String>> {
    try_lower_rooted_lowered(files, roots, core, bundled_files, optimize).map(|l| l.module)
}

fn try_lower_rooted_lowered(
    files: &[(&str, &str)],
    roots: Roots,
    core: &[(&str, &str)],
    bundled_files: &[(&str, &str)],
    optimize: bool,
) -> Result<emela_core::Lowered, Vec<String>> {
    try_lower_rooted_lowered_lang(files, roots, core, bundled_files, optimize, None)
}

/// [`try_lower_rooted_lowered`] with a target language. `Some` selects the build's path: the
/// result is monomorphised, and lowering runs the δ stage after monomorphisation.
fn try_lower_rooted_lowered_lang(
    files: &[(&str, &str)],
    roots: Roots,
    core: &[(&str, &str)],
    bundled_files: &[(&str, &str)],
    optimize: bool,
    language: Option<&str>,
) -> Result<emela_core::Lowered, Vec<String>> {
    let (program, modules, entry, texts) = resolve_files(files, core);
    let types = check_program(&program);
    // A program that tests how a type parameter that no call determines is instantiated gets
    // W0305 (`[fn.decl]`).
    let diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| !matches!(d.kind, emela_types::DiagnosticKind::UnusedTypeParam { .. }))
        .collect();
    assert!(diags.is_empty(), "{diags:?}");
    // The files under `src`, as diagnostics would show them (the bundled sources have none).
    let shown_paths = modules
        .iter()
        .filter(|(_, m)| m.file.starts_with("src"))
        .map(|(id, m)| (id, m.file.display().to_string()))
        .collect();
    let input = LowerInput {
        hir: &program,
        types: &types,
        modules: &modules,
        sources: texts.clone(),
        entry: Some(entry),
        tests: Vec::new(),
        paths: Default::default(),
        shown_paths,
        bundled_files,
        language,
    };
    let lowered = lower_with(&input, roots, optimize);
    if let Ok(l) = &lowered {
        assert_well_typed(&l.module);
        if language.is_some()
            && let Err(errors) = emela_core::verify_monomorphic(&l.module)
        {
            let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
            panic!("not monomorphic:\n{}", lines.join("\n"));
        }
    }
    lowered.map_err(|errors| {
        errors
            .iter()
            .map(|e| {
                let (what, span) = match e {
                    LowerError::Unsupported { what, span } => (what.clone(), span),
                    LowerError::MissingExternal {
                        name,
                        language,
                        span,
                    } => (format!("missing {language} external {name}"), span),
                    LowerError::MissingCoreFile { path, span } => {
                        (format!("missing core file {path}"), span)
                    }
                    LowerError::SuspendingExport { name, span } => {
                        (format!("suspending export {name}"), span)
                    }
                    LowerError::WasmExport { name, span } => (format!("wasm export {name}"), span),
                    LowerError::HttpServiceOnC { span } => ("HTTP service on c".to_owned(), span),
                };
                let Some(span) = span else {
                    return format!("<no span> {what}");
                };
                let range = usize::from(span.range.start())..usize::from(span.range.end());
                let text = &texts[span.module][range];
                if span.module == entry {
                    format!("`{text}` {what}")
                } else {
                    // Errors in other modules show the module name and the first line of the
                    // expression.
                    let first = text.lines().next().unwrap_or_default();
                    format!("{}: `{first}` {what}", modules[span.module].name)
                }
            })
            .collect()
    })
}

/// Checks with the verifier that the lowered module is well typed, before and after self tail call
/// loopification.
pub fn assert_well_typed(m: &Module) {
    let report = |errors: Vec<emela_core::VerifyError>| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("ill-typed IR:\n{}", lines.join("\n"));
    };
    if let Err(errors) = emela_core::verify(m) {
        report(errors);
    }
    let mut looped = m.clone();
    emela_core::tail::loopify(&mut looped);
    if let Err(errors) = emela_core::verify(&looped) {
        report(errors);
    }
}
