//! 位置の情報（LSP のホバー）．型検査の結果から，カーソルの下の名前や式の型を引く．
//!
//! E（エラーの集合）と R（効果の集合）は普段ソースに現れないので，ここで見せる（9.3）．

use emela_resolve::hir::{DefId, ExprKind, Item, Res};
use emela_types::{Row, TyParam};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// ホバーの中身．`contents` は Markdown．`range` はホバーした名前か式の位置．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: TextRange,
    pub contents: String,
}

/// `file` の `offset` の位置の情報．
///
/// - 定義の名前（fn，const，構成子）: `名前: 型`．型は宣言の型（推論したものは汎化した型）．
///   suspend fn（エフェクトの操作と外部関数）は型の前に `suspend` を付ける
/// - handler の名前: `handler H implements E` と，依存があれば `use R`（Async が残るか）
/// - 定義か局所変数の参照: 同じく `名前: 型`
/// - それ以外の式: 型と，E が空でなければ `fails E`，R が空でなければ `use R`
///
/// 一番内側の（範囲の短い）ものを選ぶ．
pub(crate) fn hover(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<HoverInfo> {
    let module = analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)?;
    let hir = &program.hir;
    let types = &program.types;
    let cons = &types.ty_table.cons;
    let contains = |range: TextRange| range.contains(offset);

    // 定義の名前．
    let def = hir.defs.iter().find(|(_, data)| {
        data.span
            .is_some_and(|s| s.module == module && contains(s.range))
    });
    if let Some((def, data)) = def
        && let Some(sig) = types.sigs.get(def)
    {
        return Some(HoverInfo {
            range: data.span?.range,
            contents: code(&format!(
                "{}: {}{}",
                data.name,
                suspend_prefix(hir, def),
                sig.scheme.display(cons)
            )),
        });
    }
    // handler の名前: 実装するエフェクトと依存（8.4．Async が残るかが分かる，8.7）．
    if let Some((def, data)) = def
        && let Some(Item::Handler(item)) = hir.item(def)
    {
        let mut text = format!("handler {}", data.name);
        if let Res::Def(effect) = item.effect {
            text.push_str(&format!(" implements {}", hir.defs[effect].name));
        }
        if let Some(deps) = types.handler_uses.get(def)
            && !deps.is_empty()
        {
            text.push_str(&format!("\nuse {}", deps.display_effects(cons, &[])));
        }
        return Some(HoverInfo {
            range: data.span?.range,
            contents: code(&text),
        });
    }

    // 外側の定義の型引数（本体の中の型の `Bound(i)` の名前）．
    let owner = enclosing_def(program, module, offset);
    let params: Vec<TyParam> = owner
        .and_then(|def| types.sigs.get(def))
        .map(|sig| sig.scheme.params.clone())
        .unwrap_or_default();

    // 局所変数を束縛した位置（引数，束縛のパターン）．
    let local = hir
        .locals
        .iter()
        .filter(|(_, data)| data.span.module == module && contains(data.span.range))
        .min_by_key(|(_, data)| data.span.range.len());
    let expr = hir
        .exprs
        .iter()
        .filter(|(_, e)| e.span.module == module && contains(e.span.range))
        .min_by_key(|(_, e)| e.span.range.len());

    match (local, expr) {
        (Some((local, data)), e)
            if e.is_none_or(|(_, e)| data.span.range.len() <= e.span.range.len()) =>
        {
            let ty = types.locals.get(local)?;
            Some(HoverInfo {
                range: data.span.range,
                contents: code(&format!(
                    "{}: {}",
                    data.name,
                    ty.display_with(cons, &params)
                )),
            })
        }
        (_, Some((expr, e))) => {
            let text = match &e.kind {
                ExprKind::Path(Res::Def(def)) => {
                    let sig = types.sigs.get(*def)?;
                    format!(
                        "{}: {}{}",
                        hir.defs[*def].name,
                        suspend_prefix(hir, *def),
                        sig.scheme.display(cons)
                    )
                }
                ExprKind::Path(Res::Local(local)) => {
                    let ty = types.locals.get(*local)?;
                    format!(
                        "{}: {}",
                        hir.locals[*local].name,
                        ty.display_with(cons, &params)
                    )
                }
                _ => {
                    let ty = types.exprs.get(expr)?;
                    // 能力の値の suspend な操作（`clock.sleep`）．
                    let prefix = types
                        .ops
                        .get(expr)
                        .map_or("", |&op| suspend_prefix(hir, op));
                    let mut text = format!("{prefix}{}", ty.display_with(cons, &params));
                    let errors = types.errors.get(expr).cloned().unwrap_or_else(Row::empty);
                    if !errors.is_empty() {
                        text.push_str(&format!("\nfails {}", errors.display_with(cons, &params)));
                    }
                    let effects = types.effects.get(expr).cloned().unwrap_or_else(Row::empty);
                    if !effects.is_empty() {
                        text.push_str(&format!("\nuse {}", effects.display_effects(cons, &params)));
                    }
                    text
                }
            };
            Some(HoverInfo {
                range: e.span.range,
                contents: code(&text),
            })
        }
        _ => None,
    }
}

/// `offset` を本体に含むトップレベルの定義（fn と const）．
pub(crate) fn enclosing_def(
    program: &TypedProgram,
    module: emela_resolve::ModuleId,
    offset: TextSize,
) -> Option<DefId> {
    let hir = &program.hir;
    hir.defs.iter().map(|(def, _)| def).find(|&def| {
        let body = match hir.item(def) {
            Some(Item::Fn(item)) => item.body,
            Some(Item::Const(item)) => Some(item.value),
            _ => None,
        };
        body.is_some_and(|body| {
            let span = hir.exprs[body].span;
            span.module == module && span.range.contains(offset)
        })
    })
}

/// suspend fn（エフェクトの操作と `@external suspend fn`）の型の前に付ける印．
fn suspend_prefix(hir: &emela_resolve::hir::Program, def: DefId) -> &'static str {
    let suspend = match hir.item(def) {
        Some(Item::Op(op)) => op.sig.is_suspend,
        Some(Item::Fn(f)) => f.sig.is_suspend,
        _ => false,
    };
    if suspend { "suspend " } else { "" }
}

/// Emela のコードのブロック．
fn code(text: &str) -> String {
    format!("```emela\n{text}\n```")
}
