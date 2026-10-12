//! 名前の移動（LSP の定義へ移動，参照の検索，文書のシンボル）．
//!
//! 定義へ移動と参照の検索は名前解決の参照の表（`Program::refs`）と型推論の操作の表
//! （`ProgramTypes::ops`）から引く．文書のシンボルは構文木だけから作る．

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, ExprKind, LocalId, RefTarget, Span};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{SyntaxNode, SyntaxToken};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// ソースの中の位置．移動先と参照の位置．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NavTarget {
    pub file: FileId,
    pub range: TextRange,
}

/// 名前の指すもの．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Symbol {
    Def(DefId),
    Local(LocalId),
    Module(ModuleId),
}

/// `file` の `offset` の名前の定義．同梱の core の定義，ソースのない定義（組み込みのモジュールの
/// 関数と型，組み込みの Prelude）は移動先がないので `None`．モジュールはファイルの先頭．
pub(crate) fn definition(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<NavTarget> {
    let module = module_of(analysis, file)?;
    match symbol_at(program, module, offset)? {
        Symbol::Def(def) => target(analysis, program.hir.defs[def].span?),
        Symbol::Local(local) => target(analysis, program.hir.locals[local].span),
        Symbol::Module(m) => target(
            analysis,
            Span {
                module: m,
                range: TextRange::empty(0.into()),
            },
        ),
    }
}

/// `file` の `offset` の名前の参照．`declaration` なら定義の名前も含める．
/// 同梱の core の中の参照は含めない．ファイルと位置の順．
pub(crate) fn references(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
    declaration: bool,
) -> Vec<NavTarget> {
    let Some(module) = module_of(analysis, file) else {
        return Vec::new();
    };
    let Some(symbol) = symbol_at(program, module, offset) else {
        return Vec::new();
    };
    let hir = &program.hir;
    let same = |other: Symbol| same_symbol(program, symbol, other);
    let mut spans: Vec<Span> = hir
        .refs
        .iter()
        .filter(|r| same(to_symbol(r.target)))
        .map(|r| r.span)
        .collect();
    // effect の操作（`log.info` の `info`）．
    spans.extend(
        program
            .types
            .ops
            .iter()
            .filter(|&(_, &op)| same(Symbol::Def(op)))
            .filter_map(|(expr, _)| match &hir.exprs[expr].kind {
                ExprKind::Field { name, .. } => Some(name.span),
                _ => None,
            }),
    );
    if declaration {
        match symbol {
            Symbol::Def(_) => spans.extend(
                hir.defs
                    .iter()
                    .filter(|&(def, _)| same(Symbol::Def(def)))
                    .filter_map(|(_, data)| data.span),
            ),
            Symbol::Local(local) => spans.push(hir.locals[local].span),
            Symbol::Module(_) => {}
        }
    }
    let mut targets: Vec<NavTarget> = spans
        .into_iter()
        .filter_map(|span| target(analysis, span))
        .collect();
    targets.sort_by_key(|t| (t.file, t.range.start(), t.range.end()));
    targets.dedup();
    targets
}

/// 位置にある名前．参照，effect の操作の参照，定義の名前，局所変数を束縛した位置の順に見る．
///
/// 範囲の終わりの位置（名前の直後のカーソル）も含める．範囲の中にあるものを先に選ぶ．
fn symbol_at(program: &TypedProgram, module: ModuleId, offset: TextSize) -> Option<Symbol> {
    let hir = &program.hir;
    let mut found: Vec<(TextRange, Symbol)> = Vec::new();
    let mut add = |span: Span, symbol: Symbol| {
        if span.module == module && span.range.contains_inclusive(offset) {
            found.push((span.range, symbol));
        }
    };
    for r in &hir.refs {
        add(r.span, to_symbol(r.target));
    }
    for (expr, &op) in program.types.ops.iter() {
        if let ExprKind::Field { name, .. } = &hir.exprs[expr].kind {
            add(name.span, Symbol::Def(op));
        }
    }
    for (def, data) in hir.defs.iter() {
        if let Some(span) = data.span {
            add(span, Symbol::Def(def));
        }
    }
    for (local, data) in hir.locals.iter() {
        add(data.span, Symbol::Local(local));
    }
    found
        .iter()
        .find(|(range, _)| range.contains(offset))
        .or_else(|| found.first())
        .map(|&(_, symbol)| symbol)
}

/// 同じものを指すか．名前の位置が同じ定義（type とその構成子，error のタグと構成子）は同じとみなす．
fn same_symbol(program: &TypedProgram, a: Symbol, b: Symbol) -> bool {
    match (a, b) {
        (Symbol::Def(a), Symbol::Def(b)) => {
            let defs = &program.hir.defs;
            a == b || (defs[a].span.is_some() && defs[a].span == defs[b].span)
        }
        _ => a == b,
    }
}

fn to_symbol(target: RefTarget) -> Symbol {
    match target {
        RefTarget::Def(def) => Symbol::Def(def),
        RefTarget::Local(local) => Symbol::Local(local),
        RefTarget::Module(module) => Symbol::Module(module),
    }
}

/// ソースのモジュールの位置．同梱の core と，読まなかったモジュールは `None`．
fn target(analysis: &Analysis, span: Span) -> Option<NavTarget> {
    if analysis.modules.is_core(span.module) {
        return None;
    }
    let file = *analysis.files.get(span.module)?;
    Some(NavTarget {
        file,
        range: span.range,
    })
}

fn module_of(analysis: &Analysis, file: FileId) -> Option<ModuleId> {
    analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)
}

// ---------------------------------------------------------------------------
// 文書のシンボル

/// ファイルの中の宣言．`range` は宣言全体（ドキュメントコメントと注釈を含む），
/// `selection` は名前．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub range: TextRange,
    pub selection: TextRange,
    pub children: Vec<DocumentSymbol>,
}

/// 宣言の種類．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Fn,
    Type,
    Enum,
    Variant,
    Error,
    Const,
    Effect,
    /// effect の操作．
    Op,
    Handler,
    Layer,
    Trait,
    Impl,
    /// trait，impl，handler の中の fn．
    Method,
    Field,
}

/// ソースの中の宣言を書いた順に並べる．構文の誤りがあっても読めた宣言は返す．
/// 名前の読めない宣言は飛ばす．
pub fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let parse = emela_syntax::parse(text);
    parse
        .tree()
        .items()
        .filter_map(|item| item_symbol(&item))
        .collect()
}

fn item_symbol(item: &ast::Item) -> Option<DocumentSymbol> {
    use ast::Item;
    Some(match item {
        Item::Import(_) => return None,
        Item::Fn(f) => symbol(f.syntax(), f.name()?, SymbolKind::Fn, Vec::new()),
        Item::Type(t) => symbol(
            t.syntax(),
            t.name()?,
            SymbolKind::Type,
            fields(t.field_list()),
        ),
        Item::Error(e) => symbol(
            e.syntax(),
            e.name()?,
            SymbolKind::Error,
            fields(e.field_list()),
        ),
        Item::Enum(e) => {
            let variants = e
                .variants()
                .filter_map(|v| {
                    Some(symbol(
                        v.syntax(),
                        v.name()?,
                        SymbolKind::Variant,
                        fields(v.field_list()),
                    ))
                })
                .collect();
            symbol(e.syntax(), e.name()?, SymbolKind::Enum, variants)
        }
        Item::Const(c) => symbol(c.syntax(), c.name()?, SymbolKind::Const, Vec::new()),
        Item::Effect(e) => {
            let ops = e
                .ops()
                .filter_map(|op| Some(symbol(op.syntax(), op.name()?, SymbolKind::Op, Vec::new())))
                .collect();
            symbol(e.syntax(), e.name()?, SymbolKind::Effect, ops)
        }
        Item::Handler(h) => {
            let mut children = fields(h.field_list());
            children.extend(methods(h.fns()));
            symbol(h.syntax(), h.name()?, SymbolKind::Handler, children)
        }
        Item::Layer(l) => symbol(l.syntax(), l.name()?, SymbolKind::Layer, Vec::new()),
        Item::Trait(t) => symbol(t.syntax(), t.name()?, SymbolKind::Trait, methods(t.fns())),
        Item::Impl(i) => {
            // 名前の字句がないので，`impl Show for User` の見出しを名前にし，Trait の名前を選ぶ．
            let trait_ref = i.trait_ref()?;
            let mut name = format!("impl {}", trait_ref.syntax().text());
            if let Some(ty) = i.self_ty() {
                name.push_str(&format!(" for {}", ty.text()));
            }
            DocumentSymbol {
                name,
                kind: SymbolKind::Impl,
                range: i.syntax().text_range(),
                selection: trait_ref.syntax().text_range(),
                children: methods(i.fns()),
            }
        }
    })
}

fn symbol(
    node: &SyntaxNode,
    name: SyntaxToken,
    kind: SymbolKind,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.text().to_owned(),
        kind,
        range: node.text_range(),
        selection: name.text_range(),
        children,
    }
}

fn fields(list: Option<ast::FieldList>) -> Vec<DocumentSymbol> {
    let Some(list) = list else {
        return Vec::new();
    };
    list.fields()
        .filter_map(|f| Some(symbol(f.syntax(), f.name()?, SymbolKind::Field, Vec::new())))
        .collect()
}

fn methods(fns: impl Iterator<Item = ast::FnDecl>) -> Vec<DocumentSymbol> {
    fns.filter_map(|f| {
        Some(symbol(
            f.syntax(),
            f.name()?,
            SymbolKind::Method,
            Vec::new(),
        ))
    })
    .collect()
}
