//! Call signatures (LSP `textDocument/signatureHelp`).
//!
//! Shows the callee type and labels of the innermost call containing the cursor. The active
//! argument is found by counting commas in the text before the cursor. For a call on the right of
//! a pipe (`x |> f(a)`), the left side fills the first argument (spec 6.7), so it is shown
//! without the first parameter.

use std::ops::Range;

use emela_resolve::hir::{BinaryOp, DefId, ExprId, ExprKind, Res};
use emela_types::{Ty, TyParam};
use line_index::TextSize;

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// `label` looks like `find(id: Int) -> User fails NotFound`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub label: String,
    /// The callee's `##` comment, as Markdown.
    pub documentation: Option<String>,
    /// The byte range of each parameter within `label` (`id: Int`).
    pub params: Vec<Range<usize>>,
    /// The argument under the cursor; `None` past the last parameter.
    pub active: Option<usize>,
}

pub(crate) fn signature_help(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<SignatureInfo> {
    let text = analysis.sources[file].text();
    let call = call_at(program, analysis, file, offset)?;
    let (name, ty, params, labels) = callee_type(program, call.callee)?;
    let Ty::Fn(mut f) = ty else {
        return None;
    };
    let labels = without_piped(call.piped, &mut f.params, labels);
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
    let args = text.get(usize::from(call.open) + 1..usize::from(offset))?;
    let active = active_param(args, &labels, ranges.len());
    Some(SignatureInfo {
        label,
        params: ranges,
        documentation: callee_def(program, call.callee).and_then(|def| {
            crate::documentation::Documentation::default().definition(program, analysis, def)
        }),
        active,
    })
}

/// A call whose parentheses contain the cursor.
pub(crate) struct CallAt {
    pub callee: ExprId,
    pub open: TextSize,
    /// Whether the call is on the right of a pipe, in which case the left value fills the first
    /// argument (spec 6.7).
    pub piped: bool,
}

/// The innermost call whose parentheses contain the cursor. For an unclosed call, the end of its
/// range also counts as inside.
pub(crate) fn call_at(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<CallAt> {
    let module = analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)?;
    let hir = &program.hir;
    let text = analysis.sources[file].text();
    hir.exprs
        .iter()
        .filter_map(|(id, e)| {
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
            inside.then_some((id, *callee, open, range.len()))
        })
        .min_by_key(|&(_, _, _, len)| len)
        .map(|(call, callee, open, _)| CallAt {
            callee,
            open,
            piped: is_piped(program, call),
        })
}

fn is_piped(program: &TypedProgram, call: ExprId) -> bool {
    program.hir.exprs.iter().any(
        |(_, e)| matches!(e.kind, ExprKind::Binary { op: BinaryOp::Pipe, rhs, .. } if rhs == call),
    )
}

/// For a call on the right of a pipe, drops the first parameter (filled by the left value) from
/// the parameter types and labels. The type checker prepends the left side to the positional
/// arguments, so it fills the first parameter however named arguments are written (spec 6.2,
/// 6.7). Returns the remaining labels; callees without labels (function values) stay empty.
pub(crate) fn without_piped<T>(
    piped: bool,
    params: &mut Vec<T>,
    mut labels: Vec<Option<smol_str::SmolStr>>,
) -> Vec<Option<smol_str::SmolStr>> {
    if piped && !params.is_empty() {
        params.remove(0);
        if !labels.is_empty() {
            labels.remove(0);
        }
    }
    labels
}

/// The callee's name, type, type parameters and labels. Looks at definitions (functions,
/// constructors), effect operations, then the type of any other expression.
pub(crate) type Callee = (String, Ty, Vec<TyParam>, Vec<Option<smol_str::SmolStr>>);

pub(crate) fn callee_type(program: &TypedProgram, callee: ExprId) -> Option<Callee> {
    let hir = &program.hir;
    let types = &program.types;
    let def = callee_def(program, callee);
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

/// The definition of a named callee, including capability operations.
pub(crate) fn callee_def(program: &TypedProgram, callee: ExprId) -> Option<DefId> {
    match &program.hir.exprs[callee].kind {
        ExprKind::Path(Res::Def(def)) => Some(*def),
        ExprKind::Field { .. } => program.types.ops.get(callee).copied(),
        _ => None,
    }
}

/// Splits the text inside the parentheses into arguments at commas outside nested brackets
/// (ignoring strings). The last element is the argument being written.
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

/// The label if the argument starts with `label:`.
pub(crate) fn arg_label(arg: &str) -> Option<&str> {
    let arg = arg.trim_start();
    let len = arg
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(arg.len());
    (len > 0 && arg[len..].trim_start().starts_with(':')).then(|| &arg[..len])
}

/// The argument under the cursor: if the argument being written starts with `label:`, the
/// parameter with that label (none if no such label); otherwise the number of commas outside
/// nested brackets.
fn active_param(args: &str, labels: &[Option<smol_str::SmolStr>], count: usize) -> Option<usize> {
    let parts = split_args(args);
    let index = parts.len() - 1;
    if !labels.is_empty()
        && let Some(name) = arg_label(parts[index])
    {
        return labels.iter().position(|l| l.as_deref() == Some(name));
    }
    (index < count).then_some(index)
}
