//! 呼び出しのシグネチャ（LSP の `textDocument/signatureHelp`）．
//!
//! カーソルを含む一番内側の呼び出しの，呼ぶ先の型とラベルを見せる．引数の位置はカーソルの前の
//! テキストのカンマを数えて決める．

use std::ops::Range;

use emela_resolve::hir::{ExprId, ExprKind, Res};
use emela_types::{Ty, TyParam};
use line_index::TextSize;

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// 呼び出しのシグネチャ．`label` は `find(id: Int) -> User fails NotFound` の形．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub label: String,
    /// 引数ごとの `label` の中のバイトの範囲（`id: Int`）．
    pub params: Vec<Range<usize>>,
    /// カーソルのある引数．引数の数を超えれば `None`．
    pub active: Option<usize>,
}

pub(crate) fn signature_help(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<SignatureInfo> {
    let text = analysis.sources[file].text();
    let (callee, open) = call_at(program, analysis, file, offset)?;
    let (name, ty, params, labels) = callee_type(program, callee)?;
    let Ty::Fn(f) = ty else {
        return None;
    };
    let cons = &program.types.ty_table.cons;
    let mut label = format!("{name}(");
    let mut ranges = Vec::new();
    for (i, param) in f.params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = label.len();
        if let Some(Some(l)) = labels.get(i) {
            label.push_str(&format!("{l}: "));
        }
        label.push_str(&param.display_with(cons, &params).to_string());
        ranges.push(start..label.len());
    }
    label.push_str(&format!(") -> {}", f.ret.display_with(cons, &params)));
    if !f.fails.is_empty() {
        label.push_str(&format!(" fails {}", f.fails.display_with(cons, &params)));
    }
    if !f.uses.is_empty() {
        label.push_str(&format!(" use {}", f.uses.display_effects(cons, &params)));
    }
    let args = text.get(usize::from(open) + 1..usize::from(offset))?;
    let active = active_param(args, &labels, ranges.len());
    Some(SignatureInfo {
        label,
        params: ranges,
        active,
    })
}

/// カーソルを括弧の中に含む一番内側の呼び出しの，呼ぶ先の式と開き括弧の位置．閉じていない
/// 呼び出しは範囲の終わりの位置も括弧の中とみなす．
pub(crate) fn call_at(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<(ExprId, TextSize)> {
    let module = analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)?;
    let hir = &program.hir;
    let text = analysis.sources[file].text();
    hir.exprs
        .iter()
        .filter_map(|(_, e)| {
            let ExprKind::Call { callee, .. } = &e.kind else {
                return None;
            };
            if e.span.module != module {
                return None;
            }
            let range = e.span.range;
            let open = hir.exprs[*callee].span.range.end();
            let closed = text[..usize::from(range.end())].ends_with(')');
            let inside =
                open < offset && (offset < range.end() || (!closed && offset == range.end()));
            inside.then_some((*callee, open, range.len()))
        })
        .min_by_key(|&(_, _, len)| len)
        .map(|(callee, open, _)| (callee, open))
}

/// 呼ぶ先の名前，型，型引数，ラベル．定義（関数，構成子），effect の操作，それ以外の式の型の順に見る．
pub(crate) type Callee = (String, Ty, Vec<TyParam>, Vec<Option<smol_str::SmolStr>>);

pub(crate) fn callee_type(program: &TypedProgram, callee: ExprId) -> Option<Callee> {
    let hir = &program.hir;
    let types = &program.types;
    let def = match &hir.exprs[callee].kind {
        ExprKind::Path(Res::Def(def)) => Some(*def),
        ExprKind::Field { .. } => types.ops.get(callee).copied(),
        _ => None,
    };
    if let Some(def) = def
        && let Some(sig) = types.sigs.get(def)
    {
        return Some((
            hir.defs[def].name.to_string(),
            sig.scheme.ty.clone(),
            sig.scheme.params.clone(),
            sig.labels.clone(),
        ));
    }
    let name = match &hir.exprs[callee].kind {
        ExprKind::Path(Res::Local(local)) => hir.locals[*local].name.to_string(),
        _ => "fn".to_owned(),
    };
    let ty = types
        .exprs
        .get(callee)
        .cloned()
        .or_else(|| match &hir.exprs[callee].kind {
            ExprKind::Path(Res::Local(local)) => types.locals.get(*local).cloned(),
            _ => None,
        })?;
    let params = crate::hover::enclosing_def(
        program,
        hir.exprs[callee].span.module,
        hir.exprs[callee].span.range.start(),
    )
    .and_then(|def| types.sigs.get(def))
    .map(|sig| sig.scheme.params.clone())
    .unwrap_or_default();
    Some((name, ty, params, Vec::new()))
}

/// 括弧の中の書いたテキストを，括弧の入れ子の外のカンマで引数に分ける（文字列の中は数えない）．
/// 最後の要素が書きかけの引数．
pub(crate) fn split_args(args: &str) -> Vec<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut start = 0;
    let mut prev = '\0';
    let mut parts = Vec::new();
    for (i, c) in args.char_indices() {
        if in_string {
            if c == '"' && prev != '\\' {
                in_string = false;
            }
        } else {
            match c {
                '"' => in_string = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    parts.push(&args[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        prev = c;
    }
    parts.push(&args[start..]);
    parts
}

/// 引数が `ラベル:` で始まれば，そのラベル．
pub(crate) fn arg_label(arg: &str) -> Option<&str> {
    let arg = arg.trim_start();
    let len = arg
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(arg.len());
    (len > 0 && arg[len..].trim_start().starts_with(':')).then(|| &arg[..len])
}

/// カーソルのある引数．書きかけの引数が `ラベル:` で始まればそのラベルの引数，そうでなければ
/// 括弧の入れ子の外のカンマの数．
fn active_param(args: &str, labels: &[Option<smol_str::SmolStr>], count: usize) -> Option<usize> {
    let parts = split_args(args);
    let index = parts.len() - 1;
    if let Some(name) = arg_label(parts[index])
        && let Some(i) = labels.iter().position(|l| l.as_deref() == Some(name))
    {
        return Some(i);
    }
    (index < count).then_some(index)
}
