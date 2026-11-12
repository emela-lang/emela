//! Lowering helpers shared by the integration tests.

#![allow(dead_code)]

use std::path::Path;

use emela_core::{LowerError, LowerInput, Module, Roots, lower};
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
    // bundled sources. The syntax check exempts them now (`emela_syntax` validation). Warnings do
    // not stop a build.
    let diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|d| !d.to_string().contains("E0130") && !d.kind.is_warning())
        .collect();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (program, modules, entry, texts)
}

pub fn lower_files(files: &[(&str, &str)]) -> Module {
    try_lower(files, Roots::Exported).unwrap_or_else(|e| panic!("{e:?}"))
}

/// Lowers after type checking. Errors are formatted as `` `source text` what ``.
pub fn try_lower(files: &[(&str, &str)], roots: Roots) -> Result<Module, Vec<String>> {
    try_lower_with(
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
    let options = Options {
        roots,
        core,
        bundled_files,
        ..Options::default()
    };
    lower_with(files, &options).map(|l| l.module)
}

/// What [`lower_with`] lowers for.
pub struct Options<'a> {
    pub roots: Roots,
    /// The build target; `None` stops after lowering, before monomorphization.
    pub target: Option<&'a str>,
    /// The fns of the entry, by name, that are test functions (`LowerInput::tests`).
    pub tests: &'a [&'a str],
    /// The source path of the entry module (`LowerInput::paths`); none by default.
    pub entry_path: Option<&'a str>,
    pub core: &'a [(&'a str, &'a str)],
    pub bundled_files: &'a [(&'a str, &'a str)],
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options {
            roots: Roots::Exported,
            target: None,
            tests: &[],
            entry_path: None,
            core: emela_core::core_sources(),
            bundled_files: emela_core::bundled_files(),
        }
    }
}

/// Lowers after type checking, with the roots, target and test functions of `options`. Errors
/// are formatted as `` `source text` what ``.
pub fn lower_with(
    files: &[(&str, &str)],
    options: &Options,
) -> Result<emela_core::Lowered, Vec<String>> {
    let Options {
        roots,
        target,
        tests,
        entry_path,
        core,
        bundled_files,
    } = *options;
    let (mut program, modules, entry, texts) = resolve_files(files, core);
    // As the driver does: `@entry` adds the fns it makes before type checking.
    let entry_diagnostics = emela_types::add_entry_exports(&mut program, Some(entry));
    assert!(entry_diagnostics.is_empty(), "{entry_diagnostics:?}");
    let tests: Vec<_> = tests
        .iter()
        .map(|name| {
            program
                .defs
                .iter()
                .find(|(_, d)| {
                    d.name == *name && d.module == emela_resolve::hir::DefModule::Source(entry)
                })
                .map(|(id, _)| id)
                .unwrap_or_else(|| panic!("no fn {name}"))
        })
        .collect();
    let types = check_program(&program);
    let errors: Vec<_> = types
        .diags
        .iter()
        .filter(|d| !d.kind.is_warning())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
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
        tests,
        paths: entry_path
            .map(|p| std::iter::once((entry, Path::new(p))).collect())
            .unwrap_or_default(),
        shown_paths,
        bundled_files,
        target,
    };
    let lowered = lower(&input, roots);
    if let Ok(l) = &lowered {
        assert_well_typed(&l.module);
    }
    lowered.map_err(|errors| {
        errors
            .iter()
            .map(|e| {
                let (what, span) = match e {
                    LowerError::Unsupported { what, span } => (what.clone(), span),
                    LowerError::MissingExternal { name, target, span } => {
                        (format!("missing {target} external {name}"), span)
                    }
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
