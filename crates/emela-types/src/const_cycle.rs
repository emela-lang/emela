//! Consts that refer to each other by name only (E0231, spec 5.6).
//!
//! A const's value is evaluated on its first reference. If evaluating it always evaluates
//! another const, which in turn always evaluates the first (`const A = B`, `const B = A`), every
//! reference is a defect, so this is an error at compile time. Only references that are certainly
//! evaluated count: those in a branch of `if` or `match`, the right side of `&&` and `||`, a lambda
//! body, or a function called from the value are left to the defect at run time.

use emela_resolve::hir::{
    BinaryOp, Block, DefId, ExprId, ExprKind, Item, Program, Res, Stmt, StrPart,
};
use rustc_hash::FxHashMap;

use crate::check::strongly_connected;
use crate::diagnostic::{Diagnostic, DiagnosticKind};

pub(crate) fn check(program: &Program, diags: &mut Vec<Diagnostic>) {
    let consts: Vec<DefId> = program
        .defs
        .iter()
        .map(|(def, _)| def)
        .filter(|&def| matches!(program.item(def), Some(Item::Const(_))))
        .collect();
    let mut edges: FxHashMap<DefId, Vec<DefId>> = FxHashMap::default();
    for &def in &consts {
        let Some(Item::Const(item)) = program.item(def) else {
            continue;
        };
        let mut refs = Vec::new();
        eager_refs(program, item.value, &mut refs);
        refs.dedup();
        edges.insert(def, refs);
    }

    let mut groups = strongly_connected(&consts, &edges);
    groups.sort_by_key(|group| group[0].into_raw());
    for group in groups {
        let start = group[0];
        let Some(cycle) = shortest_cycle(start, &group, &edges) else {
            continue;
        };
        diags.push(Diagnostic {
            span: program.defs[start].span,
            kind: DiagnosticKind::ConstCycle {
                cycle: cycle
                    .into_iter()
                    .map(|def| program.defs[def].name.clone())
                    .collect(),
            },
        });
    }
}

/// The shortest path from `start` back to itself within `group`, both ends included. `None` if
/// there is none (a single const that does not refer to itself).
fn shortest_cycle(
    start: DefId,
    group: &[DefId],
    edges: &FxHashMap<DefId, Vec<DefId>>,
) -> Option<Vec<DefId>> {
    let mut prev: FxHashMap<DefId, DefId> = FxHashMap::default();
    let mut queue = std::collections::VecDeque::from([start]);
    while let Some(def) = queue.pop_front() {
        for &next in edges.get(&def).map(Vec::as_slice).unwrap_or(&[]) {
            if next == start {
                let mut path = vec![start, def];
                let mut at = def;
                while at != start {
                    at = prev[&at];
                    path.push(at);
                }
                path.reverse();
                return Some(path);
            }
            if group.contains(&next) && !prev.contains_key(&next) {
                prev.insert(next, def);
                queue.push_back(next);
            }
        }
    }
    None
}

/// The consts that evaluating `expr` certainly evaluates, in source order.
fn eager_refs(program: &Program, expr: ExprId, out: &mut Vec<DefId>) {
    let go = |e: ExprId, out: &mut Vec<DefId>| eager_refs(program, e, out);
    match &program.exprs[expr].kind {
        ExprKind::Path(Res::Def(def)) => {
            if matches!(program.item(*def), Some(Item::Const(_))) {
                out.push(*def);
            }
        }
        ExprKind::Missing
        | ExprKind::Lit(_)
        | ExprKind::Path(_)
        | ExprKind::Unit
        | ExprKind::Use { .. }
        | ExprKind::Lambda { .. } => {}
        ExprKind::Interpolated(parts) => {
            for part in parts {
                if let StrPart::Interp(e) = part {
                    go(*e, out);
                }
            }
        }
        ExprKind::Field { base, .. } => go(*base, out),
        ExprKind::Call { callee, args } => {
            go(*callee, out);
            for arg in args {
                go(arg.value, out);
            }
        }
        ExprKind::Unary { operand, .. } => go(*operand, out),
        ExprKind::Binary { op, lhs, rhs } => {
            go(*lhs, out);
            if !matches!(op, BinaryOp::And | BinaryOp::Or) {
                go(*rhs, out);
            }
        }
        ExprKind::Tuple(elems) => elems.iter().for_each(|&e| go(e, out)),
        ExprKind::List { elems, rest } => {
            elems.iter().for_each(|&e| go(e, out));
            if let Some(rest) = rest {
                go(*rest, out);
            }
        }
        ExprKind::Block(block) => eager_block_refs(program, block, out),
        ExprKind::If { cond, .. } => go(*cond, out),
        ExprKind::Match { scrutinee, .. }
        | ExprKind::Escape {
            expr: scrutinee, ..
        } => go(*scrutinee, out),
        ExprKind::Fail(e) | ExprKind::Assert(e) => go(*e, out),
        ExprKind::Update { ctor, fields, base } => {
            go(*ctor, out);
            for field in fields {
                go(field.value, out);
            }
            go(*base, out);
        }
        ExprKind::With { handlers, body } => {
            for item in handlers {
                crate::layer::item_args(item).for_each(|e| go(e, out));
            }
            go(*body, out);
        }
    }
}

fn eager_block_refs(program: &Program, block: &Block, out: &mut Vec<DefId>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { value, .. } => eager_refs(program, *value, out),
            Stmt::Expr(e) => eager_refs(program, *e, out),
        }
    }
    if let Some(tail) = block.tail {
        eager_refs(program, tail, out);
    }
}
