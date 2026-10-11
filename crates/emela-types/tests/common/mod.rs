//! テストの共通の準備．ソース1つを名前解決して `Program` を作る．

use std::fmt::Write;
use std::path::Path;

use emela_resolve::hir::{DefId, DefModule, Program};
use emela_resolve::{
    Diagnostic, MemoryFs, ModuleId, ModuleName, NoBuiltins, build_import_graph, collect_modules,
    imports, resolve,
};
use emela_types::{Diagnostic as TypeDiagnostic, DiagnosticKind, Signatures, TyCons, TyTable};
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
