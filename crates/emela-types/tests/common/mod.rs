//! テストの共通の準備．ソース1つを名前解決して `Program` を作る．
//!
//! テストのファイルごとに使う関数が違うので，使わない関数の警告は止める．

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

/// `main.emel` を，同梱の Prelude と core のソースと一緒に名前解決する．構文の誤りは許さない．
/// 返すモジュールは `main.emel` のもの．
pub fn resolve_main(src: &str) -> (Program, ModuleId, Vec<Diagnostic>) {
    let fs = MemoryFs::new(["src/main.emel".to_string()]);
    let (mut modules, diagnostics) = collect_modules(&fs, Path::new("src"));
    assert_eq!(diagnostics, []);
    let (main, _) = modules.iter().next().unwrap();
    let mut texts: ArenaMap<ModuleId, &str> = ArenaMap::default();
    texts.insert(main, src);
    for (name, text) in emela_core::core_sources() {
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

/// 型の診断を `` `List[Int, Int]` WrongTypeArgCount { ... } `` の形にする．
/// 型が合わない診断は `` `"a"` mismatch: expected `Int`, found `String` `` の形にする．
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
        kind => format!("`{text}` {kind:?}"),
    }
}

/// `main.emel` の宣言の型を集め，`include` が選んだ定義の型と診断を文字列にする．
///
/// 1行に1つ，`名前: 型` と出す．`show_labels` なら，ラベルを後ろに `(id:, name:)` の形で添える．
/// 位置だけのフィールドのラベルは `_`．
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

/// `main.emel` をプログラム全体の型検査にかけ，fn ごとの宣言の型と局所変数の型，
/// `main.emel` の型の診断（位置の順）を文字列にする．
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
