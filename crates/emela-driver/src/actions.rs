//! コードアクション（LSP の `textDocument/codeAction`）．
//!
//! 今あるのは，トップレベルの fn で省いた注釈（戻り値の型，fails，use）に推論結果を挿入するもの
//! だけ（4.5）．

use emela_resolve::hir::{DefId, DefKind, DefModule, Item, Program};
use emela_resolve::{ModuleId, ModuleName};
use emela_syntax::SyntaxKind;
use emela_syntax::ast::{self, AstNode};
use emela_types::{Row, RowElem, Ty};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// コードアクション．`edits` は同じファイルの中の置き換え（挿入は空の範囲）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    pub edits: Vec<(TextRange, String)>,
}

/// `file` の `range` に掛かるコードアクション．
pub(crate) fn code_actions(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    range: TextRange,
) -> Vec<CodeAction> {
    let Some(module) = analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)
    else {
        return Vec::new();
    };
    let text = analysis.sources[file].text();
    let parse = emela_syntax::parse(text);
    let mut actions = Vec::new();
    for item in parse.tree().items() {
        let ast::Item::Fn(f) = item else { continue };
        // 見出し（`fn` から本体の `{` の前まで）に掛かるときだけ．
        let Some(body) = f.body() else { continue };
        let header = TextRange::new(
            f.syntax().text_range().start(),
            body.syntax().text_range().start(),
        );
        if header.intersect(range).is_none() && !header.contains_inclusive(range.start()) {
            continue;
        }
        if let Some(action) = insert_annotations(program, analysis, module, &f) {
            actions.push(action);
        }
    }
    actions
}

/// 省いた注釈に推論結果を挿入する（4.5）．
///
/// - 戻り値の型を省いていれば `-> T` と，推論した E が空でなければ `fails E` を足す．戻り値の型を
///   書くと E は書いた fails 節に固定される（7.4）ので，2つは一緒に足す
/// - `use` 節を省いていれば，推論した R が空でなければ `use R` を足す（R は E と別に推論する）
/// - 引数の関数型の見えない集合は書けない．書いた節に暗に足される（9.2）ので E と R から除く．
///   戻り値の型に見えない集合が現れるときは戻り値の型を足さない
/// - このファイルで修飾なしに書けない定義（import で列挙していない他のモジュールの型やエラー）が
///   現れるときは，その節を足さない
fn insert_annotations(
    program: &TypedProgram,
    analysis: &Analysis,
    module: ModuleId,
    f: &ast::FnDecl,
) -> Option<CodeAction> {
    let hir = &program.hir;
    let name = f.name()?;
    let def = hir.defs.iter().find_map(|(def, data)| {
        (data.kind == DefKind::Fn
            && data
                .span
                .is_some_and(|s| s.module == module && s.range == name.text_range()))
        .then_some(def)
    })?;
    let Some(Item::Fn(item)) = hir.item(def) else {
        return None;
    };
    let sig = program.types.sigs.get(def)?;
    let Ty::Fn(fn_ty) = &sig.scheme.ty else {
        return None;
    };
    let declared = item.sig.type_params.len() as u32;
    let params = &sig.scheme.params;
    let cons = &program.types.ty_table.cons;
    let names = Names::new(program, analysis, module);

    let mut head = String::new();
    let mut parts = Vec::new();
    if item.sig.ret.is_none()
        && !mentions_hidden(&fn_ty.ret, declared)
        && names.writable_ty(&fn_ty.ret)
    {
        let fails = visible(&fn_ty.fails, declared);
        if names.writable_row(&fails) {
            head.push_str(&format!(" -> {}", fn_ty.ret.display_with(cons, params)));
            parts.push(format!("-> {}", fn_ty.ret.display_with(cons, params)));
            if !fails.is_empty() {
                let text = format!("fails {}", fails.display_with(cons, params));
                head.push_str(&format!(" {text}"));
                parts.push(text);
            }
        }
    }
    let mut uses_text = None;
    if item.sig.uses.is_none() {
        let uses = visible(&fn_ty.uses, declared);
        if !uses.is_empty() && names.writable_row(&uses) {
            let text = format!("use {}", uses.display_effects(cons, params));
            parts.push(text.clone());
            uses_text = Some(text);
        }
    }
    if parts.is_empty() {
        return None;
    }
    // 挿入する位置．戻り値と fails は引数の並びの後ろ，use は fails 節か戻り値の型の後ろ．
    let node = f.syntax();
    let end_of = |kind: SyntaxKind| {
        node.children()
            .find(|n| n.kind() == kind)
            .map(|n| n.text_range().end())
    };
    let after_params = end_of(SyntaxKind::PARAM_LIST)?;
    let mut edits: Vec<(TextSize, String)> = Vec::new();
    if !head.is_empty() {
        edits.push((after_params, head));
    }
    if let Some(text) = uses_text {
        let at = end_of(SyntaxKind::FAILS_CLAUSE)
            .or_else(|| end_of(SyntaxKind::RET_TYPE))
            .unwrap_or(after_params);
        match edits.iter_mut().find(|(p, _)| *p == at) {
            Some((_, head)) => head.push_str(&format!(" {text}")),
            None => edits.push((at, format!(" {text}"))),
        }
    }
    Some(CodeAction {
        title: format!("Add inferred `{}`", parts.join(" ")),
        edits: edits
            .into_iter()
            .map(|(at, text)| (TextRange::empty(at), text))
            .collect(),
    })
}

/// 集合から見えない集合の型引数（書いた型引数より後ろの `Bound`）を除いたもの．
fn visible(row: &Row, declared: u32) -> Row {
    Row::from_elems(
        row.elems()
            .iter()
            .filter(|e| !matches!(e, RowElem::Bound(i) if *i >= declared))
            .cloned(),
    )
}

/// 型に見えない集合の型引数が現れるか．
fn mentions_hidden(ty: &Ty, declared: u32) -> bool {
    let row = |row: &Row| {
        row.elems().iter().any(|e| match e {
            RowElem::Bound(i) => *i >= declared,
            RowElem::Of(ty) => mentions_hidden(ty, declared),
            _ => false,
        })
    };
    match ty {
        Ty::Bound(i) => *i >= declared,
        Ty::Fn(f) => {
            f.params.iter().any(|p| mentions_hidden(p, declared))
                || mentions_hidden(&f.ret, declared)
                || row(&f.fails)
                || row(&f.uses)
        }
        Ty::Tuple(tys) | Ty::Named(_, tys) => tys.iter().any(|t| mentions_hidden(t, declared)),
        Ty::List(t) => mentions_hidden(t, declared),
        Ty::Set(_, r) => row(r),
        _ => false,
    }
}

/// このファイルで修飾なしに書ける定義．
struct Names<'a> {
    program: &'a TypedProgram,
    analysis: &'a Analysis,
    module: ModuleId,
    /// `import A.{x}` で列挙した（モジュール，名前）．
    imported: Vec<(ModuleId, String)>,
}

impl<'a> Names<'a> {
    fn new(program: &'a TypedProgram, analysis: &'a Analysis, module: ModuleId) -> Self {
        let text = analysis
            .files
            .get(module)
            .map_or("", |&file| analysis.sources[file].text());
        let imported = emela_syntax::parse(text)
            .tree()
            .imports()
            .filter_map(|import| {
                let path = import.path()?;
                let name = ModuleName::new(path.segments().map(|t| t.text().to_owned()));
                let target = analysis.modules.lookup(&name)?;
                Some(
                    import
                        .names()
                        .map(|t| (target, t.text().to_owned()))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();
        Names {
            program,
            analysis,
            module,
            imported,
        }
    }

    fn hir(&self) -> &Program {
        &self.program.hir
    }

    /// 定義を修飾なしに書けるか．このモジュールの定義，Prelude，import で列挙した名前，
    /// 名前が1区切りの core のモジュールの同じ名前の型（`Map`）．
    fn writable(&self, def: DefId) -> bool {
        let data = &self.hir().defs[def];
        match &data.module {
            DefModule::Prelude | DefModule::Builtin(_) => true,
            DefModule::Source(m) if *m == self.module => true,
            DefModule::Source(m) => {
                let name = &self.analysis.modules[*m].name;
                let core = self.analysis.modules.is_core(*m);
                (core && name.last() == emela_resolve::PRELUDE)
                    || (core && name.segments().len() == 1 && *name.last() == data.name)
                    || self
                        .imported
                        .iter()
                        .any(|(target, n)| target == m && *n == data.name)
            }
        }
    }

    fn writable_ty(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Prim(_) | Ty::Never | Ty::Bound(_) => true,
            Ty::Var(_) | Ty::Cap(_) | Ty::Error => false,
            Ty::Fn(f) => {
                f.params.iter().all(|p| self.writable_ty(p))
                    && self.writable_ty(&f.ret)
                    && self.writable_row(&f.fails)
                    && self.writable_row(&f.uses)
            }
            Ty::Tuple(tys) => tys.iter().all(|t| self.writable_ty(t)),
            Ty::List(t) => self.writable_ty(t),
            Ty::Set(_, row) => self.writable_row(row),
            Ty::Named(con, args) => {
                self.writable(self.program.types.ty_table.def(*con))
                    && args.iter().all(|t| self.writable_ty(t))
            }
        }
    }

    fn writable_row(&self, row: &Row) -> bool {
        row.elems().iter().all(|e| match e {
            RowElem::Tag(tag) => {
                self.writable(tag.def) && tag.args.iter().all(|t| self.writable_ty(t))
            }
            RowElem::Bound(_) => true,
            RowElem::Of(ty) => self.writable_ty(ty),
            RowElem::Var(_) => false,
        })
    }
}
