//! Effect and defect summaries of a function: what evaluating it may do.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Effect
//! Domain / lattice: a product of five Bool facts, one per summary field. The top element is
//!                   `Fact::Unknown`; a `Known` summary is a conjunction of "may" facts, so the
//!                   safe direction is to set a field to true when unsure. A summary itself is a
//!                   known fact even when every field is true: [`EffectSummary::unknown`] is the
//!                   top of the summary lattice, while `Fact::Unknown` says that the analysis has
//!                   no summary at all.
//! Transfer: a function's summary is the union (logical or) of the summaries of the expressions it
//!           evaluates, including calls, lambdas it may run and handler operations it may invoke.
//!           [`of_expr`] summarizes one expression; [`of_module`] adds the summaries of direct
//!           callees by a fixpoint over the call graph.
//! Soundness: `Known(s)` says s.may_fail is false only when no path can fail, and likewise for the
//!            other fields. A defect is never folded into "purity": a type with no effects can
//!            still divide by zero or index out of bounds.
//! Deopt (unknown): a consumer that gets `Unknown` leaves the expression alone. It must not read
//!                  `Unknown` as `may_fail = false` or as "pure".
//! Consumers: the β and δ reductions (whether an argument may be moved or duplicated), the
//!            algebraic laws (whether a removed side is trivial), CSE/gVN (A12/PR-041), fusion
//!            (D3/PR-030) and drop specialization (C3/PR-027) and evidence specialization
//!            (E6/PR-038).
//! ```
//!
//! # What each field covers
//!
//! - `may_use_effect`: an effect operation through the evidence table (`use`, `provide`, `with`, a
//!   handler value, an operation call), a write to stderr (`dbg`, `DebugPrint`) or a cell operation
//!   (`State.run`). The builtin table calls `dbg` pure as a debugging exception
//!   ([`crate::Builtin::info`]); to an optimisation it is an observable write, so it is an effect
//!   here.
//! - `may_fail`: `fail`, a call whose `fails` flag is set, and a call of a function whose body may
//!   fail. An `escape` whose arms contain a catch-all (`_` or a binding, without a guard) removes
//!   the failures of its body.
//! - `may_suspend`: a call whose `suspends` flag is set and the concurrency operations that wait
//!   ([`FiberOp::suspends`]).
//! - `may_defect`: integer division and remainder (a zero divisor), an `assert`, a match without a
//!   catch-all arm, and the builtins and concurrency operations that can raise one. A defect is not
//!   in the type, so an indirect call may always raise one.
//! - `may_diverge`: a `loop`, a function on a cycle of the call graph, a builtin that calls a user
//!   function (the map of an array range, a `Show` impl), and a call whose callee is unknown.
//!
//! # Conservatism
//!
//! - The analysis is not a constant folder. `x / 2` and `x / 0` both set `may_defect`: the divisor
//!   is not inspected. Only the operand type decides whether a division is checked (Int and Int64;
//!   Float division by zero is IEEE, not a defect).
//! - A call through a function value uses the value's function type for the three facts it carries
//!   (its E, its R and whether it takes the evidence table). The body is not visible, so
//!   `may_defect` and `may_diverge` stay true and an unknown callee type falls back to
//!   [`EffectSummary::unknown`].
//! - A lambda expression is an allocation; its body runs only when the lambda is called. In
//!   [`of_expr`] the body is therefore not part of the allocation. It is part of the enclosing
//!   function's summary in [`of_module`] (a function may pass the lambda to something that calls
//!   it), and of a concurrency operation that runs its function argument at once (`scope`,
//!   `attempt`, `fork`).
//! - A function on a cycle of the direct-call graph may not terminate (`diverges = true`). Its
//!   other facts are the union of its body. Propagation to callers is the ordinary fixpoint, so a
//!   caller of a recursive function also diverges.
//!
//! # Status
//!
//! B1/PR-020 implements the summary; B2/PR-021 owns the shared call graph, so the small graph this
//! module builds is private to it.

use rustc_hash::FxHashMap;

use crate::intrinsics::Builtin;
use crate::ir::{Arm, BinOp, Callee, Expr, FiberOp, FnId, FnType, Module, OpTy, Pat, Type};

use super::{Analysis, AnalysisId, Fact};

/// What evaluating a function may do. Every field is a "may": `true` is the conservative value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EffectSummary {
    /// The function may perform an effect operation through the evidence table.
    pub may_use_effect: bool,
    /// The function may fail (the `E` of its type is not empty on some path).
    pub may_fail: bool,
    /// The function may suspend.
    pub may_suspend: bool,
    /// The function may reach a defect (division by zero, array out of bounds, an assertion).
    pub may_defect: bool,
    /// The function may diverge.
    pub may_diverge: bool,
}

impl EffectSummary {
    /// The bottom of the summary lattice: evaluating the expression does nothing.
    pub const fn pure() -> Self {
        EffectSummary {
            may_use_effect: false,
            may_fail: false,
            may_suspend: false,
            may_defect: false,
            may_diverge: false,
        }
    }

    /// The top: anything may happen. This is the summary of a call whose callee is not known.
    pub const fn unknown() -> Self {
        EffectSummary {
            may_use_effect: true,
            may_fail: true,
            may_suspend: true,
            may_defect: true,
            may_diverge: true,
        }
    }

    /// Whether no field is set. Not the same as `Fact::Known`: a `pure` summary is a proof.
    pub const fn is_pure(self) -> bool {
        !self.may_use_effect
            && !self.may_fail
            && !self.may_suspend
            && !self.may_defect
            && !self.may_diverge
    }

    /// The least upper bound: a fact of either summary holds in the join.
    pub const fn join(self, other: Self) -> Self {
        EffectSummary {
            may_use_effect: self.may_use_effect || other.may_use_effect,
            may_fail: self.may_fail || other.may_fail,
            may_suspend: self.may_suspend || other.may_suspend,
            may_defect: self.may_defect || other.may_defect,
            may_diverge: self.may_diverge || other.may_diverge,
        }
    }

    /// [`EffectSummary::join`], in place.
    pub fn join_assign(&mut self, other: Self) {
        *self = self.join(other);
    }
}

impl Default for EffectSummary {
    /// The conservative default: a summary that proves nothing. A derived `Default` would be
    /// `pure`, which is the wrong direction for a "may" fact.
    fn default() -> Self {
        EffectSummary::unknown()
    }
}

/// The summaries of the functions of a module, keyed by [`FnId`].
///
/// The map is computed in arena order, so the same module gives the same values; the iteration
/// order of the map itself is unspecified. A consumer that reports or zips summaries iterates
/// [`Module::functions`](crate::ir::Module::functions) and looks each [`FnId`] up here.
pub type EffectSummaries = FxHashMap<FnId, EffectSummary>;

/// The effect analysis.
pub struct Effect;

impl Analysis for Effect {
    type Output = Fact<EffectSummaries>;

    const ID: AnalysisId = AnalysisId::Effect;

    fn analyze(&self, m: &Module) -> Self::Output {
        Fact::Known(of_module(m))
    }
}

/// The effect summary of every function of `m` (see the module contract).
///
/// The result is a fixpoint over the direct-call graph: a function's summary joins the summaries of
/// the functions it calls. A function on a cycle of that graph has `may_diverge` set; its other
/// fields are the union of its body.
pub fn analyze(m: &Module) -> Fact<EffectSummaries> {
    Effect.analyze(m)
}

/// The summary of every function of `m`, in arena order.
///
/// The fixpoint memoises one summary per function; a consumer that asks about several functions or
/// expressions calls this once and [`of_expr_in`] per expression.
pub fn of_module(m: &Module) -> EffectSummaries {
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    let recursive = recursive_functions(m, &ids);
    // Kleene iteration from the bottom (pure everywhere). The transfer function only joins, so
    // every pass moves a field from false to true and the iteration terminates.
    let mut table: EffectSummaries = ids.iter().map(|&f| (f, EffectSummary::pure())).collect();
    loop {
        let mut changed = false;
        for (i, &f) in ids.iter().enumerate() {
            let old = table[&f];
            let next = old.join(summary_of_function(m, f, &table, recursive[i]));
            if next != old {
                table.insert(f, next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    table
}

/// The summary of `f`.
///
/// A caller that needs several functions should call [`of_module`] once instead: this function
/// computes the whole module's fixpoint. An `f` that is not a function of `m` answers
/// [`EffectSummary::unknown`].
pub fn of_function(m: &Module, f: FnId) -> EffectSummary {
    of_module(m)
        .get(&f)
        .copied()
        .unwrap_or_else(EffectSummary::unknown)
}

/// The summary of evaluating `e` on its own.
///
/// The bodies of the functions it calls are not visible here. A call to a local function keeps the
/// call's `fails` / `suspends` flags (the IR proves those) but treats everything the body could do
/// as unknown: it may use an effect, reach a defect or diverge. For a call-aware summary use
/// [`of_expr_in`] with the table of [`of_module`].
pub fn of_expr(m: &Module, e: &Expr) -> EffectSummary {
    of_expr_in(m, e, &EffectSummaries::default())
}

/// The summary of evaluating `e`, with the summaries of the module's functions from [`of_module`].
///
/// `summaries` must be the fixpoint result for the same module. A call to a function missing from
/// it is treated as an unknown callee; a stale entry is not detected, so do not pass a table built
/// from another module or before a rewrite.
pub fn of_expr_in(m: &Module, e: &Expr, summaries: &EffectSummaries) -> EffectSummary {
    Walker::new(m, summaries).expr(e, Mode::Expr)
}

/// Whether the expression is analysed as a value that is evaluated now (`Expr`) or as the body of a
/// function that may be called (`Function`).
///
/// The difference is a lambda: its allocation is pure, but a function's body may pass or call it,
/// so [`of_module`] includes its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Expr,
    Function,
}

/// How a function on a cycle is treated: it may not terminate.
fn recursive_functions(m: &Module, ids: &[FnId]) -> Vec<bool> {
    use rustc_hash::FxHashMap as Map;
    let n = ids.len();
    let mut position: Map<_, usize> = Map::default();
    for (i, &f) in ids.iter().enumerate() {
        position.insert(f.into_raw(), i);
    }
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, &f) in ids.iter().enumerate() {
        let mut callees = Vec::new();
        collect_direct_calls(&m.functions[f].body, &mut callees);
        for callee in callees {
            if let Some(&j) = position.get(&callee.into_raw()) {
                edges[i].push(j);
            }
        }
    }
    // Tarjan's strongly connected components, iteratively (a module may have deep call chains).
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut component = vec![usize::MAX; n];
    let mut sizes: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut frames: Vec<(usize, usize)> = vec![(root, 0)];
        while !frames.is_empty() {
            let top = frames.len() - 1;
            let (v, child) = frames[top];
            if index[v] == usize::MAX {
                index[v] = next_index;
                low[v] = next_index;
                next_index += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = edges[v].get(child) {
                frames[top].1 += 1;
                if index[w] == usize::MAX {
                    frames.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                if low[v] == index[v] {
                    let id = sizes.len();
                    let mut size = 0;
                    loop {
                        let w = stack.pop().expect("a component has a member");
                        on_stack[w] = false;
                        component[w] = id;
                        size += 1;
                        if w == v {
                            break;
                        }
                    }
                    sizes.push(size);
                }
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    low[parent] = low[parent].min(low[v]);
                }
            }
        }
    }
    (0..n)
        .map(|i| sizes[component[i]] > 1 || edges[i].contains(&i))
        .collect()
}

/// Every `Callee::Direct` in `e`, in the bodies of lambdas too.
fn collect_direct_calls(e: &Expr, out: &mut Vec<FnId>) {
    if let Expr::Call {
        callee: Callee::Direct { f, .. },
        ..
    } = e
    {
        out.push(*f);
    }
    e.for_each_child(&mut |child| collect_direct_calls(child, out));
}

/// The summary of the body of `f`, using `callees` for direct calls.
fn summary_of_function(
    m: &Module,
    f: FnId,
    callees: &EffectSummaries,
    recursive: bool,
) -> EffectSummary {
    let func = &m.functions[f];
    if func.external.is_some() {
        // A host function may do anything observable: print, raise a defect (a host exception is
        // a defect, `[ext.exception]`) or not return. It never fails: an external function's E is
        // empty (spec 12.5), and a suspending external is marked `suspends` (spec 12.2).
        return EffectSummary {
            may_use_effect: true,
            may_fail: false,
            may_suspend: func.suspends,
            may_defect: true,
            may_diverge: true,
        };
    }
    let mut summary = Walker::new(m, callees).expr(&func.body, Mode::Function);
    if func.constant {
        // A const is evaluated once, on the first call. A self-reference while it is evaluated,
        // or a defect remembered from the first evaluation, carries the const's name (spec 5.6).
        summary.may_defect = true;
    }
    summary.may_diverge |= recursive;
    summary
}

/// A walker over one expression with the callee summaries of [`of_module`].
struct Walker<'a, 't> {
    m: &'a Module,
    callees: &'t EffectSummaries,
}

impl<'a, 't> Walker<'a, 't> {
    fn new(m: &'a Module, callees: &'t EffectSummaries) -> Self {
        Walker { m, callees }
    }

    /// Joins the summaries of every subexpression `for_each_child` reports.
    fn children(&self, e: &Expr, mode: Mode) -> EffectSummary {
        let mut summary = EffectSummary::pure();
        e.for_each_child(&mut |child| summary.join_assign(self.expr(child, mode)));
        summary
    }

    fn join_exprs(&self, exprs: &[Expr], mode: Mode) -> EffectSummary {
        let mut summary = EffectSummary::pure();
        for e in exprs {
            summary.join_assign(self.expr(e, mode));
        }
        summary
    }

    fn expr(&self, e: &Expr, mode: Mode) -> EffectSummary {
        match e {
            Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } => EffectSummary::pure(),
            // The allocation of a lambda is pure; its body runs when something calls it. The body
            // of a lambda in a function's body is part of that function's summary, because the
            // function may hand the lambda to something that calls it.
            Expr::Lambda { body, .. } => {
                if mode == Mode::Function {
                    self.expr(body, mode)
                } else {
                    EffectSummary::pure()
                }
            }
            // A handler value installs a way to perform an effect. Its operation lambdas are not
            // called here; in a function's body they are part of its summary like any lambda.
            Expr::Handler { ops, .. } => {
                let mut summary = if mode == Mode::Function {
                    self.join_exprs(ops, mode)
                } else {
                    EffectSummary::pure()
                };
                summary.may_use_effect = true;
                summary
            }
            Expr::Call {
                callee,
                args,
                fails,
                suspends,
            } => {
                let mut summary = match callee {
                    Callee::Direct { f, .. } => self.direct(*f),
                    Callee::Indirect(callee) => self.called(callee, mode),
                    Callee::Op { handler, op, .. } => {
                        let mut summary = self.expr(handler, mode);
                        summary.may_use_effect = true;
                        // A literal handler exposes its implementation. When the handler comes
                        // from a variable, the implementation is not visible: the body may
                        // defect or diverge.
                        match handler.as_ref() {
                            Expr::Handler { ops, .. } if *op < ops.len() => {
                                if mode == Mode::Expr {
                                    summary.join_assign(self.called(&ops[*op], mode));
                                }
                            }
                            _ => {
                                summary.may_defect = true;
                                summary.may_diverge = true;
                            }
                        }
                        summary
                    }
                };
                summary.may_fail |= *fails;
                summary.may_suspend |= *suspends;
                summary.join_assign(self.join_exprs(args, mode));
                summary
            }
            Expr::Builtin { op, args, .. } => {
                let mut summary = self.join_exprs(args, mode);
                summary.join_assign(builtin_summary(*op));
                summary
            }
            Expr::Fail(value) => {
                let mut summary = self.expr(value, mode);
                summary.may_fail = true;
                summary
            }
            Expr::Escape { body, arms } => {
                let mut summary = self.expr(body, mode);
                // A catch-all arm catches every failure of the body. Without one, a failure whose
                // error no arm matches propagates (the IR does not record coverage: the type
                // checker removed the error only when the arms cover it, so a failure some value
                // of which may go unmatched is still in E). Failures in arms always propagate.
                if has_catch_all(arms) {
                    summary.may_fail = false;
                }
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        summary.join_assign(self.expr(guard, mode));
                    }
                    summary.join_assign(self.expr(&arm.body, mode));
                }
                summary
            }
            Expr::Match { arms, .. } => {
                let mut summary = self.children(e, mode);
                if !has_catch_all(arms) {
                    // No arm matches: the match is a defect.
                    summary.may_defect = true;
                }
                summary
            }
            Expr::Binary {
                op, ty, lhs, rhs, ..
            } => {
                let mut summary = self.expr(lhs, mode);
                summary.join_assign(self.expr(rhs, mode));
                // A defect on a zero divisor. Int and Int64 only: Float division by zero is IEEE,
                // and the minimum value divided by -1 wraps around (spec 16.1). The divisor is
                // not inspected: this is not a constant folder.
                if matches!(op, BinOp::Div | BinOp::Rem) && matches!(ty, OpTy::Int | OpTy::Int64) {
                    summary.may_defect = true;
                }
                summary
            }
            Expr::Loop { .. } => {
                let mut summary = self.children(e, mode);
                summary.may_diverge = true;
                summary
            }
            // `assert expr` is a defect when the condition is false. Constant conditions are not
            // folded here.
            Expr::Assert(_) => {
                let mut summary = self.children(e, mode);
                summary.may_defect = true;
                summary
            }
            // Reading the evidence table is the use of an effect.
            Expr::Use { .. } => {
                let mut summary = self.children(e, mode);
                summary.may_use_effect = true;
                summary
            }
            // Building an evidence table binds handlers; installing it is the effect.
            Expr::Provide { .. } => {
                let mut summary = self.children(e, mode);
                summary.may_use_effect = true;
                summary
            }
            // A `with` installs handlers (and evaluates their releases however it ends).
            Expr::With { .. } => {
                let mut summary = self.children(e, mode);
                summary.may_use_effect = true;
                summary
            }
            Expr::Fiber { op, args } => {
                let mut summary = self.children(e, mode);
                summary.may_use_effect = true;
                summary.may_fail |= op.fails();
                summary.may_suspend |= op.suspends();
                summary.may_defect |= fiber_may_defect(*op);
                if fiber_waits(*op) {
                    // A child that does not finish, or a body that does not return.
                    summary.may_diverge = true;
                }
                if mode == Mode::Expr {
                    // The operations that run a function value now: its body is not the
                    // allocation of the lambda.
                    let invoked = match op {
                        FiberOp::Scope { .. } | FiberOp::Attempt { .. } => args.first(),
                        FiberOp::Fork => args.get(1),
                        _ => None,
                    };
                    if let Some(f) = invoked {
                        summary.join_assign(self.called(f, mode));
                    }
                }
                summary
            }
            _ => self.children(e, mode),
        }
    }

    /// The summary of a function value that is called at this point.
    fn called(&self, callee: &Expr, mode: Mode) -> EffectSummary {
        match callee {
            Expr::Fn { f, .. } => self.direct(*f),
            Expr::Lambda { body, .. } => self.expr(body, Mode::Function),
            _ => {
                let mut summary = self.expr(callee, mode);
                match fn_type(self.m, callee) {
                    Some(ty) => {
                        summary.may_use_effect |= ty.ev;
                        summary.may_fail |= ty.fails;
                        summary.may_suspend |= ty.suspends;
                    }
                    // The type does not say what the function may do.
                    None => summary.join_assign(EffectSummary::unknown()),
                }
                // The body is not in the IR here: a defect is not in the type, and a function
                // value may not terminate.
                summary.may_defect = true;
                summary.may_diverge = true;
                summary
            }
        }
    }

    /// The summary of a direct call to `f`, without the call node's own flags.
    fn direct(&self, f: FnId) -> EffectSummary {
        let unknown_body = EffectSummary {
            // The body is not visible: it may print and it may use the evidence table.
            may_use_effect: true,
            // The call's own flags say whether it may fail or suspend; the caller joins them.
            may_fail: false,
            may_suspend: false,
            // A defect is not in the type and the function may not terminate.
            may_defect: true,
            may_diverge: true,
        };
        if u32::from(f.into_raw()) as usize >= self.m.functions.len() {
            return unknown_body;
        }
        let func = &self.m.functions[f];
        if func.external.is_some() {
            // A host function may do anything observable: print, raise a defect (a host exception
            // is a defect, `[ext.exception]`) or not return. It never fails: an external
            // function's E is empty (spec 12.5); `suspends` is `@external suspend fn`.
            return EffectSummary {
                may_suspend: func.suspends,
                ..unknown_body
            };
        }
        self.callees.get(&f).copied().unwrap_or(unknown_body)
    }
}

/// The function type of `e`, when the IR makes it visible without a type computation.
fn fn_type<'a>(m: &'a Module, e: &Expr) -> Option<&'a FnType> {
    let ty = match e {
        Expr::Var(var) => m.locals[*var].ty,
        Expr::Lambda { ty, .. } => *ty,
        _ => return None,
    };
    match &m.types[ty] {
        Type::Fn(ty) => Some(ty),
        _ => None,
    }
}

/// Whether an arm catches every value, making a `match` exhaustive and an `escape` total.
fn has_catch_all(arms: &[Arm]) -> bool {
    arms.iter()
        .any(|arm| arm.guard.is_none() && matches!(arm.pat, Pat::Wild | Pat::Bind(_)))
}

/// The facts of a builtin call, without its arguments.
///
/// The classification follows the defect table of the LIR (`emela-lir`'s `heap::defects`), which
/// reads the same spec rules after monomorphization, plus the writes the Core IR still has.
fn builtin_summary(op: Builtin) -> EffectSummary {
    let mut summary = EffectSummary::pure();
    summary.may_use_effect = matches!(
        op,
        // Writes to stderr (`dbg` and the print `dbg` becomes for a value shown through Show).
        Builtin::Dbg
            | Builtin::DebugPrint
            // The cell of `State.run`: mutable state behind the handler.
            | Builtin::CellNew
            | Builtin::CellGet
            | Builtin::CellSet
    );
    summary.may_defect = matches!(
        op,
        // A negative shift amount (spec 7.5).
        Builtin::IntShiftLeft
            | Builtin::IntShiftRight
            | Builtin::IntShiftRightUnsigned
            | Builtin::Int64ShiftLeft
            | Builtin::Int64ShiftRight
            | Builtin::Int64ShiftRightUnsigned
            // NaN, infinities and values out of the range of Int (spec 16.1).
            | Builtin::FloatFloor
            | Builtin::FloatCeil
            | Builtin::FloatRound
            | Builtin::FloatTruncate
            // A position out of range (`[api.std.array]`).
            | Builtin::ArraySet
            // Target c splits by grapheme clusters only ASCII text so far; an empty separator
            // that turns up at run time on other text is a defect there.
            | Builtin::StringSplit
            // The explicit defects.
            | Builtin::Panic
            | Builtin::Todo
            | Builtin::AssertFail
            // Calls a function value once per element; a defect is not in its type, and the
            // function value may not terminate.
            | Builtin::ArrayMapRange
            // Becomes a call of the type's Show at monomorphization; a Show impl may defect.
            | Builtin::ShowNested
    );
    // Both of these call a function value that the IR does not show here.
    summary.may_diverge = matches!(op, Builtin::ArrayMapRange | Builtin::ShowNested);
    summary
}

/// Whether a concurrency operation may raise a defect in the current fiber.
///
/// The same classification as the LIR: a child's defect, an interrupted child, a raised cause, a
/// scope's body, or a use after the scope ended (spec 11.4, 11.8).
fn fiber_may_defect(op: FiberOp) -> bool {
    !matches!(op, FiberOp::CauseKind { .. } | FiberOp::CauseToString)
}

/// Whether a concurrency operation waits for work that may never finish.
fn fiber_waits(op: FiberOp) -> bool {
    matches!(
        op,
        FiberOp::Scope { .. }
            | FiberOp::Join
            | FiberOp::Await { .. }
            | FiberOp::Interrupt
            | FiberOp::DeferredAwait
            | FiberOp::Attempt { .. }
    )
}
