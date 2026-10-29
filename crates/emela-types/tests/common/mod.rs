//! Shared test setup: resolves a single source into a `Program`.
//!
//! Each test file uses different functions, so unused-function warnings are silenced.

#![allow(dead_code)]

use std::fmt::Write;
use std::path::Path;

use emela_resolve::hir::{DefId, DefKind, DefModule, Program};
use emela_resolve::{
    Diagnostic, MemoryFs, ModuleId, ModuleName, NoBuiltins, build_import_graph, collect_modules,
    imports, resolve,
};
use emela_types::{
    Diagnostic as TypeDiagnostic, DiagnosticKind, Signatures, TyCons, TyTable, check_program,
};
use la_arena::ArenaMap;

/// Resolves `main.emel` together with the bundled Prelude and core sources. Syntax errors are not
/// allowed. Returns the module of `main.emel`.
pub fn resolve_main(src: &str) -> (Program, ModuleId, Vec<Diagnostic>) {
    resolve_main_with(src, &[])
}

/// [`resolve_main`] with `extra` (module name and source) added as bundled core modules. Used to
/// check core sources not bundled yet (`State`).
pub fn resolve_main_with(
    src: &str,
    extra: &[(&str, &str)],
) -> (Program, ModuleId, Vec<Diagnostic>) {
    let fs = MemoryFs::new(["src/main.emel".to_string()]);
    let (mut modules, diagnostics) = collect_modules(&fs, Path::new("src"));
    assert_eq!(diagnostics, []);
    let (main, _) = modules.iter().next().unwrap();
    let mut texts: ArenaMap<ModuleId, &str> = ArenaMap::default();
    texts.insert(main, src);
    for (name, text) in emela_core::core_sources().iter().chain(extra) {
        let (id, shadowed) =
            modules.add_core(ModuleName::parse(name), Path::new("<core>").join(name));
        assert_eq!(shadowed, None);
        texts.insert(id, text);
    }

    let mut trees = ArenaMap::default();
    let mut import_lists = ArenaMap::default();
    for (id, text) in texts.iter() {
        let parse = emela_syntax::parse(text);
        assert!(
            parse.diagnostics().is_empty(),
            "{}: {:?}",
            modules[id].file.display(),
            parse.diagnostics()
        );
        import_lists.insert(id, imports(&parse.tree()));
        trees.insert(id, parse.tree());
    }
    let (_, graph_diagnostics) = build_import_graph(&modules, &import_lists);
    assert_eq!(graph_diagnostics, []);
    let (program, diagnostics) = resolve(&modules, &trees, &NoBuiltins);
    (program, main, diagnostics)
}

/// Formats type diagnostics as `` `List[Int, Int]` WrongTypeArgCount { ... } ``, and mismatches
/// as `` `"a"` mismatch: expected `Int`, found `String` ``.
pub fn describe(src: &str, cons: &TyCons, d: &TypeDiagnostic) -> String {
    let text = d.span.map_or("<no span>", |s| {
        let range = s.range;
        &src[usize::from(range.start())..usize::from(range.end())]
    });
    match &d.kind {
        DiagnosticKind::Mismatch(e) => format!(
            "`{text}` mismatch: expected `{}`, found `{}`",
            e.expected.display(cons),
            e.actual.display(cons)
        ),
        DiagnosticKind::NoField { ty, field } => {
            format!("`{text}` no field `{field}` on type `{}`", ty.display(cons))
        }
        DiagnosticKind::NoFieldOrFunction { ty, name } => format!(
            "`{text}` no field or function `{name}` for type `{}`",
            ty.display(cons)
        ),
        DiagnosticKind::NoFields { ty, field } => {
            format!(
                "`{text}` type `{}` has no fields (`{field}`)",
                ty.display(cons)
            )
        }
        DiagnosticKind::OpaqueField { ty, field } => format!(
            "`{text}` cannot access field `{field}` of opaque type `{}`",
            ty.display(cons)
        ),
        DiagnosticKind::NotNumeric { found, integer } => format!(
            "`{text}` not numeric (integer: {integer}): `{}`",
            found.display(cons)
        ),
        DiagnosticKind::IntLiteralOutOfRange { literal, ty } => format!(
            "`{text}` integer literal `{literal}` out of range for `{}`",
            ty.display(cons)
        ),
        DiagnosticKind::NonExhaustiveMatch { missing } => {
            format!(
                "`{text}` non-exhaustive match: {} not covered",
                missing.join(", ")
            )
        }
        DiagnosticKind::RefutableBinding { missing } => {
            format!(
                "`{text}` refutable pattern in binding: {} not covered",
                missing.join(", ")
            )
        }
        DiagnosticKind::UnreachableArm => format!("`{text}` warning: unreachable match arm"),
        DiagnosticKind::MissingPatternFields { ctor, fields } => {
            format!(
                "`{text}` pattern `{ctor}` does not mention {}",
                fields.join(", ")
            )
        }
        DiagnosticKind::PatternFieldCount {
            ctor,
            expected,
            found,
        } => format!("`{text}` pattern `{ctor}` has {expected} fields, found {found}"),
        DiagnosticKind::Unsupported { what } => format!("`{text}` not supported yet: {what}"),
        DiagnosticKind::UnhandledErrors {
            errors,
            declared,
            layer_arg: None,
        } => format!(
            "`{text}` unhandled errors {} (fails written: {declared})",
            errors.join(" | ")
        ),
        DiagnosticKind::UnhandledErrors {
            errors,
            layer_arg: Some(arg),
            ..
        } => format!(
            "`{text}` unhandled errors {} (in the arguments of `{}` in layer `{}`)",
            errors.join(" | "),
            arg.handler,
            arg.layer
        ),
        DiagnosticKind::NotAnError { found } => {
            format!("`{text}` not an error type: `{}`", found.display(cons))
        }
        DiagnosticKind::EscapeArmNotError => format!("`{text}` escape arm is not an error"),
        DiagnosticKind::ErrorSetMismatch { expected, found } => format!(
            "`{text}` error set mismatch: expected `fails {expected}`, found `fails {found}`"
        ),
        DiagnosticKind::UnraisedError { error } => {
            format!("`{text}` warning: `{error}` is never raised here")
        }
        DiagnosticKind::UnhandledEffects {
            effects,
            context,
            scope,
        } => {
            let note = match scope {
                Some(scope) => format!(" [provide outside `{scope}`]"),
                None => String::new(),
            };
            format!(
                "`{text}` unhandled effects {} ({context:?}){note}",
                effects.join(", ")
            )
        }
        DiagnosticKind::EffectSetMismatch { expected, found } => {
            format!("`{text}` effect set mismatch: expected `use {expected}`, found `use {found}`")
        }
        kind => format!("`{text}` {kind:?}"),
    }
}

/// Collects the declared types of `main.emel` and renders the types and diagnostics of the
/// definitions `include` selects.
///
/// One `name: type` per line. With `show_labels`, labels follow as `(id:, name:)`; positional
/// fields are labelled `_`.
pub fn render_signatures(
    src: &str,
    show_labels: bool,
    include: impl Fn(&Program, DefId) -> bool,
) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let table = TyTable::new(&program);
    let mut diags = Vec::new();
    let sigs = Signatures::collect(&program, &table, &mut diags);

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if data.module != DefModule::Source(module) || !include(&program, def) {
            continue;
        }
        let Some(sig) = sigs.get(def) else {
            continue;
        };
        write!(out, "{}: {}", data.name, sig.scheme.display(&table.cons)).unwrap();
        if show_labels && !sig.labels.is_empty() {
            let labels: Vec<String> = sig
                .labels
                .iter()
                .map(|l| l.as_ref().map_or("_".to_string(), |l| format!("{l}:")))
                .collect();
            write!(out, "  ({})", labels.join(", ")).unwrap();
        }
        out.push('\n');
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    for d in &diags {
        writeln!(out, "{}", describe(src, &table.cons, d)).unwrap();
    }
    out
}

/// Type-checks the whole program for `main.emel` and renders each fn's declared type, its local
/// variables' types, and the type diagnostics of `main.emel` in source order.
pub fn render_bodies(src: &str) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let types = check_program(&program);
    let cons = &types.ty_table.cons;

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if data.module != DefModule::Source(module) || data.kind != DefKind::Fn {
            continue;
        }
        match types.sigs.get(def) {
            Some(sig) => writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap(),
            None => writeln!(out, "{}: <no type>", data.name).unwrap(),
        }
        for (local, ty) in types.locals.iter() {
            if types.local_owner.get(local) == Some(&def) {
                writeln!(
                    out,
                    "  {}: {}",
                    program.locals[local].name,
                    ty.display(cons)
                )
                .unwrap();
            }
        }
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    let mut diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| d.span.is_some_and(|s| s.module == module))
        .collect();
    diags.sort_by_key(|d| d.span.map(|s| s.range.start()));
    for d in diags {
        writeln!(out, "{}", describe(src, cons, d)).unwrap();
    }
    out
}
