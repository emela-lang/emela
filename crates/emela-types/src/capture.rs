//! Capture sets (`[effect.escape]`, `[fiber.escape]`): which capabilities a value may hold, and
//! the check that a value does not leave the `with` or `Fiber.scope` that provides one of them
//! (W0402; E0444 with the experimental feature `capture-checking`, decided by the driver).
//!
//! Capture sets are inferred after the types of a group of definitions are settled, by walking
//! the bodies once more. They are not part of the types: a capture set cannot be written in
//! source, and nothing in type checking depends on it.
//!
//! - An element of a capture set is a root ([`Holder`]): the handler of an entry of a `with`, the
//!   Scope of a `Fiber.scope` call, or, inside a function, what its caller passes: a parameter, or
//!   the handler of an effect `use`d where no `with` of the same top-level definition provides it.
//! - A value's capture set ([`Val`]) is computed from its expression: a closure holds what its
//!   captured variables hold, a data structure what its elements hold, the result of a call what
//!   the callee's return value holds with the parameters replaced by the arguments.
//! - A function value also carries what its return value holds ([`FnVal`]), so that calling a
//!   closure or a local fn gives what its body returns.
//! - A value whose type cannot contain a capability ([`may_hold`]) holds nothing.
//! - The return value of a top-level definition is summarized per definition, in terms of its
//!   parameters and the effects its caller provides. Within a group of definitions that call each
//!   other, the summaries are computed again until they no longer grow.

use std::collections::BTreeSet;
use std::rc::Rc;

use emela_resolve::hir::{
    BinaryOp, DefId, DefKind, ExprId, ExprKind, Item, LocalId, LocalKind, PatId, PatKind, Program,
    Res, Span, Stmt, StrPart, WithItem,
};
use la_arena::ArenaMap;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use crate::body::fn_parts;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::fiber::Fibers;
use crate::layer::{self, Supplier};
use crate::lower::TyTable;
use crate::signature::Signatures;
use crate::ty::{Ty, TyConKind};

/// A function, closure or local fn, as the owner of parameters and of what its caller provides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Binder {
    Def(DefId),
    Lambda(ExprId),
    LocalFn(LocalId),
}

/// An element of a capture set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Holder {
    /// The handler of entry `1` of the `with` expression `0` (`layer::with_entries`).
    With(ExprId, usize),
    /// The handler that item `1` of the `with` expression `0` opens as a handler member
    /// (`x = Files.Open(p)`, `[effect.with]`).
    Opened(ExprId, usize),
    /// The Scope of the call of a scope function (`Fiber.scope`).
    Scope(ExprId),
    /// What parameter `1` of the binder holds.
    Param(Binder, usize),
    /// The handler of the effect that the caller of the binder provides (`use E` outside any
    /// `with` providing E).
    Caller(Binder, DefId),
}

type Holds = BTreeSet<Holder>;

/// What a function value returns when called: its return value's capture set, with the
/// parameters and caller-provided effects of `binder` to be replaced at the call.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FnVal {
    binder: Binder,
    ret: Holds,
}

/// The abstract value of an expression.
#[derive(Clone, Debug, Default)]
struct Val {
    holds: Holds,
    call: Option<Rc<FnVal>>,
}

impl Val {
    fn of(holds: Holds) -> Val {
        Val { holds, call: None }
    }
}

/// How a root of a capture set is shown (`[lsp.hover]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// The variable of its binding, the parameter that received it, the handler of an unbound
    /// `with` item, or the effect provided by the caller.
    pub name: SmolStr,
    /// Where it is provided: a `with` or a scope function. `None` for what the caller passes.
    pub provider: Option<Provider>,
}

/// A `with` or a scope function that provides a root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    /// `with PrimaryDb`, or the scope function (`Fiber.scope`).
    pub what: SmolStr,
    /// The `with` item, or the callee of the scope function.
    pub span: Span,
}

/// The capture sets of a program, for the language server.
#[derive(Clone, Debug, Default)]
pub struct Captures {
    /// Expressions whose value may hold a root.
    pub exprs: ArenaMap<ExprId, Vec<Held>>,
    /// Local variables that may hold a root.
    pub locals: ArenaMap<LocalId, Vec<Held>>,
    /// Definitions whose return value may hold what their parameters hold or what their caller
    /// provides.
    pub defs: ArenaMap<DefId, Vec<Held>>,
}

/// Inputs to the analysis of one group.
pub(crate) struct Input<'a> {
    pub(crate) program: &'a Program,
    pub(crate) ty_table: &'a TyTable,
    pub(crate) sigs: &'a Signatures,
    pub(crate) fibers: &'a Fibers,
    pub(crate) exprs: &'a ArenaMap<ExprId, Ty>,
    pub(crate) locals: &'a ArenaMap<LocalId, Ty>,
    pub(crate) calls: &'a ArenaMap<ExprId, Vec<Option<ExprId>>>,
    pub(crate) ops: &'a ArenaMap<ExprId, DefId>,
    pub(crate) methods: &'a ArenaMap<ExprId, DefId>,
    /// The type arguments of the items of `with`s and of layers (`[effect.with]`).
    pub(crate) with_ty_args: &'a ArenaMap<ExprId, Vec<Vec<Ty>>>,
    pub(crate) layer_ty_args: &'a ArenaMap<DefId, Vec<Vec<Ty>>>,
}

impl Input<'_> {
    /// The type arguments of the items of the `with` `expr`.
    pub(crate) fn insts(&self, expr: ExprId, items: &[WithItem]) -> layer::Insts {
        let args = self.with_ty_args.get(expr).cloned().unwrap_or_default();
        layer::Insts::new(self.program, items, args, |l| {
            self.layer_ty_args.get(l).cloned()
        })
    }
}

/// The summaries of the definitions analyzed so far, and their display.
#[derive(Default)]
pub(crate) struct State {
    summaries: FxHashMap<DefId, Holds>,
    pub(crate) captures: Captures,
}

/// Computes the capture sets of a group of definitions that refer to each other, and pushes
/// W0402 for values that leave the `with` or scope providing a root they hold.
pub(crate) fn check_group(
    input: &Input<'_>,
    state: &mut State,
    group: &[DefId],
    diags: &mut Vec<Diagnostic>,
) {
    let program = input.program;
    let defs: Vec<DefId> = group
        .iter()
        .copied()
        .filter(|&def| {
            matches!(program.item(def), Some(Item::Handler(_)))
                || fn_parts(program, def).is_some_and(|p| p.body.is_some())
        })
        .collect();
    if defs.is_empty() {
        return;
    }
    // Summaries only grow (each analysis is monotone in the summaries it reads), and they are
    // subsets of a finite set of roots, so this ends. The bound only guards against a mistake.
    for _ in 0..16 {
        let mut changed = false;
        for &def in &defs {
            let mut cx = Cx::new(input, state, false);
            let ret = cx.def(def);
            if let Some(ret) = ret
                && state.summaries.get(&def) != Some(&ret)
            {
                state.summaries.insert(def, ret);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for &def in &defs {
        let mut cx = Cx::new(input, state, true);
        cx.def(def);
        let Cx {
            diags: found,
            exprs,
            locals,
            names,
            ..
        } = cx;
        diags.extend(found);
        let shown = Shown {
            input,
            names: &names,
            cache: Default::default(),
        };
        for (expr, holds) in exprs {
            state.captures.exprs.insert(expr, shown.list(&holds));
        }
        for (local, holds) in locals {
            state.captures.locals.insert(local, shown.list(&holds));
        }
        if let Some(ret) = state.summaries.get(&def)
            && !ret.is_empty()
        {
            let list = shown.list(ret);
            state.captures.defs.insert(def, list);
        }
    }
}

/// The analysis of one definition.
struct Cx<'a, 'b> {
    input: &'a Input<'b>,
    state: &'a State,
    /// Whether to report diagnostics and record capture sets (the last round).
    report: bool,
    env: FxHashMap<LocalId, Val>,
    /// The `with` expressions enclosing the current expression, innermost last, with the
    /// effect each entry provides.
    withs: Vec<(ExprId, Vec<Option<DefId>>)>,
    /// The binders enclosing the current expression, innermost last.
    binders: Vec<Binder>,
    /// For each closure or local fn in `binders`, the length of `withs` when its body was
    /// entered: the `with`s before it do not provide its R (`[effect.with]`).
    binder_withs: Vec<usize>,
    /// The variable bound to each root, for display.
    names: FxHashMap<Holder, LocalId>,
    diags: Vec<Diagnostic>,
    exprs: Vec<(ExprId, Holds)>,
    locals: Vec<(LocalId, Holds)>,
}

impl<'a, 'b> Cx<'a, 'b> {
    fn new(input: &'a Input<'b>, state: &'a State, report: bool) -> Self {
        Cx {
            input,
            state,
            report,
            env: FxHashMap::default(),
            withs: Vec::new(),
            binders: Vec::new(),
            binder_withs: Vec::new(),
            names: FxHashMap::default(),
            diags: Vec::new(),
            exprs: Vec::new(),
            locals: Vec::new(),
        }
    }

    fn program(&self) -> &'b Program {
        self.input.program
    }

    /// Analyzes the bodies of a definition. Returns the capture set of the return value of a
    /// function; `None` for a handler, whose operations are called through their declarations.
    fn def(&mut self, def: DefId) -> Option<Holds> {
        let program = self.program();
        self.binders.push(Binder::Def(def));
        if let Some(Item::Handler(item)) = program.item(def) {
            // Inside a handler, `self` and the parameters hold nothing that a `with` of the
            // handler's own bodies could provide.
            for body in item
                .init
                .iter()
                .chain(item.release.as_ref().map(|(_, b)| b))
            {
                self.eval(*body);
            }
            for op in &item.ops {
                if let Some(body) = op.body {
                    self.eval(body);
                }
            }
            return None;
        }
        let parts = fn_parts(program, def)?;
        for (i, p) in parts.params.iter().enumerate() {
            let param = Holder::Param(Binder::Def(def), i);
            self.names.insert(param, p.local);
            self.bind(p.local, Val::of(Holds::from([param])));
        }
        let body = parts.body?;
        let val = self.eval(body);
        Some(val.holds)
    }

    /// The innermost binder.
    fn binder(&self) -> Binder {
        *self.binders.last().expect("inside a definition")
    }

    fn ty(&self, expr: ExprId) -> Option<&'a Ty> {
        self.input.exprs.get(expr)
    }

    /// Binds a local variable, keeping only what its type can hold.
    fn bind(&mut self, local: LocalId, val: Val) {
        let val = match self.input.locals.get(local) {
            Some(ty) if !may_hold(self.input, ty) => Val::default(),
            _ => val,
        };
        if self.report && !val.holds.is_empty() {
            self.locals.push((local, val.holds.clone()));
        }
        self.env.insert(local, val);
    }

    /// Binds the variables of a pattern to what the matched value holds.
    fn bind_pat(&mut self, pat: PatId, val: &Val) {
        let program = self.program();
        match &program.pats[pat].kind {
            PatKind::Bind(local) => self.bind(*local, val.clone()),
            PatKind::Ctor { args, .. } => {
                for arg in args.iter().flatten() {
                    self.bind_pat(arg.pat, &Val::of(val.holds.clone()));
                }
            }
            PatKind::Tuple(pats) => {
                for &p in pats {
                    self.bind_pat(p, &Val::of(val.holds.clone()));
                }
            }
            PatKind::List { elems, rest } => {
                for &p in elems {
                    self.bind_pat(p, &Val::of(val.holds.clone()));
                }
                if let Some(local) = rest.as_ref().and_then(|r| r.binding) {
                    self.bind(local, Val::of(val.holds.clone()));
                }
            }
            PatKind::Wild
            | PatKind::Lit(_)
            | PatKind::Const(_)
            | PatKind::Unit
            | PatKind::Missing => {}
        }
    }

    /// The root `use E` refers to here (`[effect.with]`): the innermost `with` that provides E
    /// inside the current function, closures and local fns included, or else what its caller
    /// passes.
    fn use_root(&self, effect: DefId) -> Holder {
        let floor = self.binder_withs.last().copied().unwrap_or(0);
        for (with, provides) in self.withs[floor..].iter().rev() {
            if let Some(entry) = provides.iter().position(|&e| e == Some(effect)) {
                return Holder::With(*with, entry);
            }
        }
        Holder::Caller(self.binder(), effect)
    }

    /// The value of an expression. Every subexpression is visited, so that the `with`s and
    /// scopes inside are checked.
    fn eval(&mut self, expr: ExprId) -> Val {
        let mut val = self.eval_kind(expr);
        if let Some(ty) = self.ty(expr)
            && !may_hold(self.input, ty)
        {
            val.holds.clear();
        }
        if self.report && !val.holds.is_empty() {
            self.exprs.push((expr, val.holds.clone()));
        }
        val
    }

    fn eval_all(&mut self, exprs: impl IntoIterator<Item = ExprId>) -> Holds {
        let mut holds = Holds::new();
        for e in exprs {
            holds.extend(self.eval(e).holds);
        }
        holds
    }

    fn eval_kind(&mut self, expr: ExprId) -> Val {
        let program = self.program();
        match &program.exprs[expr].kind {
            ExprKind::Missing | ExprKind::Lit(_) | ExprKind::Unit => Val::default(),
            ExprKind::Interpolated(parts) => {
                for part in parts {
                    if let StrPart::Interp(e) = part {
                        self.eval(*e);
                    }
                }
                Val::default()
            }
            ExprKind::Path(Res::Local(local)) => self.env.get(local).cloned().unwrap_or_default(),
            ExprKind::Path(Res::Def(def)) => self.def_value(*def),
            ExprKind::Path(_) => Val::default(),
            ExprKind::Field { base, .. } => {
                let base_val = self.eval(*base);
                if let Some(&op) = self.input.ops.get(expr) {
                    // An operation not called: calling it gives what its arguments and the
                    // handler hold.
                    let handler = self.handler_holds(*base, &base_val);
                    return Val {
                        holds: base_val.holds.clone(),
                        call: Some(Rc::new(FnVal {
                            binder: Binder::Def(op),
                            ret: self.default_ret(op, handler),
                        })),
                    };
                }
                Val::of(base_val.holds)
            }
            ExprKind::Call { callee, args } => {
                let callee = *callee;
                let written: Vec<ExprId> = args.iter().map(|a| a.value).collect();
                self.call(expr, callee, &written)
            }
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                let (callee, mut written) = match &program.exprs[*rhs].kind {
                    ExprKind::Call { callee, args } => {
                        (*callee, args.iter().map(|a| a.value).collect::<Vec<_>>())
                    }
                    _ => (*rhs, Vec::new()),
                };
                written.insert(0, *lhs);
                self.call(expr, callee, &written)
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.eval(*lhs);
                self.eval(*rhs);
                Val::default()
            }
            ExprKind::Unary { operand, .. } => {
                self.eval(*operand);
                Val::default()
            }
            ExprKind::Tuple(elems) => Val::of(self.eval_all(elems.iter().copied())),
            ExprKind::List { elems, rest } => {
                Val::of(self.eval_all(elems.iter().copied().chain(*rest)))
            }
            ExprKind::Update { ctor, fields, base } => {
                self.eval(*ctor);
                let mut holds = self.eval_all(fields.iter().map(|f| f.value));
                holds.extend(self.eval(*base).holds);
                Val::of(holds)
            }
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    match stmt {
                        Stmt::Let { pat, value, .. } => {
                            let val = self.eval(*value);
                            self.name_root(*pat, *value);
                            self.bind_pat(*pat, &val);
                        }
                        Stmt::LocalFn(f) => self.local_fn(f.local, &f.sig.params, f.body),
                        Stmt::Expr(e) => {
                            self.eval(*e);
                        }
                    }
                }
                match block.tail {
                    Some(tail) => self.eval(tail),
                    None => Val::default(),
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.eval(*cond);
                let mut val = self.eval(*then_branch);
                if let Some(e) = else_branch {
                    let other = self.eval(*e);
                    val = join(val, other);
                }
                val
            }
            ExprKind::Match { scrutinee, arms } => {
                let scrutinee = self.eval(*scrutinee);
                let mut out = Holds::new();
                for arm in arms {
                    self.bind_pat(arm.pat, &scrutinee);
                    if let Some(guard) = arm.guard {
                        self.eval(guard);
                    }
                    out.extend(self.eval(arm.body).holds);
                }
                Val::of(out)
            }
            ExprKind::Escape { expr: inner, arms } => {
                let mut out = self.eval(*inner).holds;
                for arm in arms {
                    self.bind_pat(arm.pat, &Val::default());
                    if let Some(guard) = arm.guard {
                        self.eval(guard);
                    }
                    out.extend(self.eval(arm.body).holds);
                }
                Val::of(out)
            }
            ExprKind::Fail(e) | ExprKind::Assert(e) => {
                self.eval(*e);
                Val::default()
            }
            // An effect alias and `(use E).op(...)` hold nothing (`[effect.escape]`).
            ExprKind::Use { .. } if self.program().alias_uses.get(expr).is_some() => Val::default(),
            ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } => Val::of(Holds::from([self.use_root(*effect)])),
            ExprKind::Use { .. } => Val::default(),
            ExprKind::With { handlers, body } => self.with(expr, handlers, *body),
            ExprKind::Lambda { params, body } => {
                let params: Vec<LocalId> = params.iter().map(|p| p.local).collect();
                self.closure(Binder::Lambda(expr), &params, *body)
            }
        }
    }

    /// Records the variable `pat` binds as the name of the root `value` refers to, when `value`
    /// is `use E` (`[lsp.hover]`).
    fn name_root(&mut self, pat: PatId, value: ExprId) {
        let program = self.program();
        if let PatKind::Bind(local) = program.pats[pat].kind
            && let ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } = program.exprs[value].kind
        {
            let root = self.use_root(effect);
            self.names.entry(root).or_insert(local);
        }
    }

    /// The value of a closure: what its captured variables hold, and what its body returns.
    fn closure(&mut self, binder: Binder, params: &[LocalId], body: ExprId) -> Val {
        let mut inside = FxHashSet::default();
        bound_in(self.program(), body, &mut inside);
        inside.extend(params.iter().copied());
        let mut captured = Holds::new();
        self.captured(body, &inside, &mut captured);
        self.binders.push(binder);
        self.binder_withs.push(self.withs.len());
        for (i, &p) in params.iter().enumerate() {
            let param = Holder::Param(binder, i);
            self.names.insert(param, p);
            self.bind(p, Val::of(Holds::from([param])));
        }
        let ret = self.eval(body).holds;
        self.binders.pop();
        self.binder_withs.pop();
        Val {
            holds: captured,
            call: Some(Rc::new(FnVal { binder, ret })),
        }
    }

    /// What the variables used inside `expr` and bound outside it (not in `inside`) hold. The
    /// environment may still have the variables bound inside from an earlier evaluation of the
    /// same body, so those are left out by name.
    fn captured(&self, expr: ExprId, inside: &FxHashSet<LocalId>, out: &mut Holds) {
        let program = self.program();
        if let ExprKind::Path(Res::Local(local)) = program.exprs[expr].kind
            && !inside.contains(&local)
            && let Some(val) = self.env.get(&local)
        {
            out.extend(val.holds.iter().copied());
        }
        for child in children(program, expr) {
            self.captured(child, inside, out);
        }
    }

    /// A local fn (`[fn.local]`). It may call itself, so its value is computed until what it
    /// returns no longer grows.
    fn local_fn(&mut self, local: LocalId, params: &[emela_resolve::hir::Param], body: ExprId) {
        let params: Vec<LocalId> = params.iter().map(|p| p.local).collect();
        let binder = Binder::LocalFn(local);
        let report = std::mem::replace(&mut self.report, false);
        self.env.insert(
            local,
            Val {
                holds: Holds::new(),
                call: Some(Rc::new(FnVal {
                    binder,
                    ret: Holds::new(),
                })),
            },
        );
        let mut val = Val::default();
        for _ in 0..16 {
            val = self.closure(binder, &params, body);
            let old = self.env.get(&local).and_then(|v| v.call.clone());
            let same = old.as_deref() == val.call.as_deref()
                && self.env.get(&local).is_some_and(|v| v.holds == val.holds);
            self.env.insert(local, val.clone());
            if same {
                break;
            }
        }
        self.report = report;
        if report {
            // Once more to record the capture sets and report what is inside.
            val = self.closure(binder, &params, body);
        }
        self.bind(local, val);
    }

    /// A reference to a top-level definition.
    fn def_value(&self, def: DefId) -> Val {
        let program = self.program();
        match program.defs[def].kind {
            DefKind::Fn | DefKind::Ctor | DefKind::Error => Val {
                holds: Holds::new(),
                call: Some(Rc::new(FnVal {
                    binder: Binder::Def(def),
                    ret: self.summary(def),
                })),
            },
            _ => Val::default(),
        }
    }

    /// What the return value of a definition holds, in terms of its parameters and the effects
    /// its caller provides. A definition without a body returns what its arguments hold
    /// (`[effect.escape]`).
    fn summary(&self, def: DefId) -> Holds {
        if let Some(holds) = self.state.summaries.get(&def) {
            return holds.clone();
        }
        let has_body = fn_parts(self.program(), def).is_some_and(|p| p.body.is_some());
        if has_body {
            // In the group being analyzed and not summarized yet.
            return Holds::new();
        }
        self.default_ret(def, Holds::new())
    }

    /// The return value of a declaration without a body: what its arguments hold, and `extra`
    /// (the handler of an operation).
    fn default_ret(&self, def: DefId, extra: Holds) -> Holds {
        let n = match self.input.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => f.params.len(),
            _ => 0,
        };
        let mut holds: Holds = (0..n).map(|i| Holder::Param(Binder::Def(def), i)).collect();
        holds.extend(extra);
        holds
    }

    /// The handler that an operation called on `base` reaches, as the return value of the
    /// operation holds it (`[effect.escape]`). An effect alias and `(use E)` hold nothing
    /// themselves, but the call reaches the handler of E where it is evaluated: the root of the
    /// innermost `with` in the current function, or what its caller passes. For `x.op` without a
    /// call this is where it is written, which is the same place unless the function value is
    /// called elsewhere.
    fn handler_holds(&self, base: ExprId, base_val: &Val) -> Holds {
        let program = self.program();
        match program.exprs[base].kind {
            ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } if program.alias_uses.get(base).is_some() => Holds::from([self.use_root(effect)]),
            _ => base_val.holds.clone(),
        }
    }

    /// A call. `written` are the arguments as written (for a pipe, the left side first).
    fn call(&mut self, expr: ExprId, callee: ExprId, written: &[ExprId]) -> Val {
        let program = self.program();
        // The arguments in the order of the parameters (`ProgramTypes::calls`); a method call
        // and a pipe put the first argument first.
        let order: Vec<Option<ExprId>> = match self.input.calls.get(expr) {
            Some(order) => order.clone(),
            None => written.iter().copied().map(Some).collect(),
        };
        let method = self.input.methods.get(callee).copied();
        let op = self.input.ops.get(callee).copied();
        // The callee: for a method or an operation, the base is evaluated as an argument or as
        // the handler.
        let (callee_val, handler) = match (&program.exprs[callee].kind, method, op) {
            // The base is the first argument, in `order`.
            (ExprKind::Field { .. }, Some(_), _) => (Val::default(), Holds::new()),
            (ExprKind::Field { base, .. }, None, Some(_)) => {
                let base_val = self.eval(*base);
                (Val::default(), self.handler_holds(*base, &base_val))
            }
            _ => (self.eval(callee), Holds::new()),
        };
        let mut args: Vec<Val> = Vec::with_capacity(order.len());
        let mut seen = Vec::new();
        for e in &order {
            match e {
                Some(e) => {
                    seen.push(*e);
                    args.push(self.eval(*e));
                }
                None => args.push(Val::default()),
            }
        }
        // Arguments written but not placed (an error elsewhere) are still visited.
        for e in written {
            if !seen.contains(e) && !order.contains(&Some(*e)) {
                self.eval(*e);
            }
        }
        let arg_tys: Vec<Option<&Ty>> = order.iter().map(|e| e.and_then(|e| self.ty(e))).collect();
        let ret_ty = self.ty(expr);

        // A scope function: the Scope of its body is a new root (`[fiber.escape]`).
        let named = match program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => Some(def),
            _ => method,
        };
        if let Some(def) = named
            && let Some(scope) = self.input.fibers.scope_fns.iter().find(|s| s.def == def)
        {
            return self.scope_call(expr, callee, scope.effect, &args);
        }

        let (binder, ret, bodyless) = if let Some(def) = method {
            (Binder::Def(def), self.summary(def), !has_body(program, def))
        } else if let Some(op) = op {
            (Binder::Def(op), self.default_ret(op, handler.clone()), true)
        } else if let Some(f) = &callee_val.call {
            let bodyless = match f.binder {
                Binder::Def(def) => !has_body(program, def),
                _ => false,
            };
            (f.binder, f.ret.clone(), bodyless)
        } else {
            // A function value whose body is not known. A parameter of function type returns what
            // its arguments hold (`[effect.escape]`); another value may also return what it
            // holds.
            let is_param = matches!(
                program.exprs[callee].kind,
                ExprKind::Path(Res::Local(l))
                    if matches!(program.locals[l].kind, LocalKind::Param | LocalKind::LambdaParam)
            );
            let mut holds: Holds = args.iter().flat_map(|a| a.holds.iter().copied()).collect();
            if !is_param {
                holds.extend(callee_val.holds.iter().copied());
            }
            let holds = self.without_scope_roots_of_handles(holds, &args, &arg_tys, ret_ty);
            return Val::of(holds);
        };
        if bodyless {
            // `[effect.escape]`, `[fiber.escape]`: a handle passes on what it holds except the
            // roots of scopes, unless the value returned is itself a handle.
            let keep = ret_ty.is_some_and(|t| self.contains_handle(t));
            if !keep {
                for (arg, ty) in args.iter_mut().zip(&arg_tys) {
                    if ty.is_some_and(|t| self.is_handle(t)) {
                        arg.holds.retain(|h| !self.is_scope_root(*h));
                    }
                }
            }
        }
        Val::of(self.apply(binder, &ret, &args))
    }

    /// For a call to a function value whose body is not known: drop the roots of scopes that
    /// handles pass on, as for a declaration without a body.
    fn without_scope_roots_of_handles(
        &self,
        holds: Holds,
        args: &[Val],
        arg_tys: &[Option<&Ty>],
        ret_ty: Option<&Ty>,
    ) -> Holds {
        if ret_ty.is_some_and(|t| self.contains_handle(t)) {
            return holds;
        }
        let only_from_handles = |h: &Holder| {
            args.iter()
                .zip(arg_tys)
                .filter(|(a, _)| a.holds.contains(h))
                .all(|(_, t)| t.is_some_and(|t| self.is_handle(t)))
        };
        holds
            .into_iter()
            .filter(|h| !(self.is_scope_root(*h) && only_from_handles(h)))
            .collect()
    }

    /// Replaces the parameters and the caller-provided effects of `binder` in `ret`.
    fn apply(&self, binder: Binder, ret: &Holds, args: &[Val]) -> Holds {
        let mut out = Holds::new();
        for &h in ret {
            match h {
                Holder::Param(b, i) if b == binder => {
                    if let Some(arg) = args.get(i) {
                        out.extend(arg.holds.iter().copied());
                    }
                }
                Holder::Caller(b, effect) if b == binder => {
                    out.insert(self.use_root(effect));
                }
                h => {
                    out.insert(h);
                }
            }
        }
        out
    }

    /// A call of a scope function (`Fiber.scope`, `[fiber.escape]`): its body is called with a
    /// new Scope. Its value may hold what the body returns and what the argument holds; holding
    /// the Scope, or a handle it made, is W0402.
    fn scope_call(&mut self, expr: ExprId, callee: ExprId, effect: DefId, args: &[Val]) -> Val {
        let root = Holder::Scope(expr);
        let mut holds = Holds::new();
        for arg in args {
            holds.extend(arg.holds.iter().copied());
            if let Some(f) = &arg.call {
                for &h in &f.ret {
                    match h {
                        Holder::Caller(b, e) if b == f.binder && e == effect => {
                            holds.insert(root);
                        }
                        h => {
                            holds.extend(self.apply(f.binder, &Holds::from([h]), &[]));
                        }
                    }
                }
            }
        }
        if let Some(ty) = self.ty(expr)
            && !may_hold(self.input, ty)
        {
            holds.clear();
        }
        if holds.contains(&root) {
            let name = self
                .input
                .fibers
                .provider(effect)
                .cloned()
                .unwrap_or_default();
            let value = args_body_tail(self.program(), expr).unwrap_or(expr);
            let at = self.program().exprs[callee].span;
            self.report_escape(root, Some(name), at, value);
            holds.remove(&root);
        }
        Val::of(holds)
    }

    /// `with H1, ..., Hn { body }`.
    fn with(&mut self, expr: ExprId, items: &[WithItem], body: ExprId) -> Val {
        let input = self.input;
        let program = self.program();
        // The arguments of init are evaluated outside the `with` (`[effect.with]`).
        for item in items {
            for arg in layer::item_args(item) {
                self.eval(arg);
            }
        }
        let insts = input.insts(expr, items);
        let entries = layer::with_entries(program, input.sigs, input.ty_table, items, &insts);
        // Only providers by name are found by `use E`; the indices stay those of `entries`.
        let provides: Vec<Option<DefId>> = entries
            .iter()
            .map(|e| e.named.then_some(e.effect.def))
            .collect();
        // A name bound to an item holds the handler of that item (`[effect.with]`).
        for (i, item) in items.iter().enumerate() {
            let Some(local) = item.bind else { continue };
            let holder = match entries.iter().position(|e| e.origin == i && !e.from_layer) {
                Some(entry) => Holder::With(expr, entry),
                None => Holder::Opened(expr, i),
            };
            self.names.insert(holder, local);
            self.bind(local, Val::of(Holds::from([holder])));
        }
        self.withs.push((expr, provides));
        let mut val = self.eval(body);
        self.withs.pop();
        if let Some(ty) = self.ty(expr)
            && !may_hold(input, ty)
        {
            val.holds.clear();
        }
        let roots: Vec<Holder> = val
            .holds
            .iter()
            .copied()
            .filter(|h| matches!(h, Holder::With(w, _) | Holder::Opened(w, _) if *w == expr))
            .collect();
        for root in roots {
            let span = match root {
                Holder::With(_, entry) => entries.get(entry).map(|e| e.span),
                Holder::Opened(_, item) => items.get(item).map(|i| i.span),
                _ => None,
            };
            if let Some(span) = span {
                self.report_escape(root, None, span, tail(program, body));
            }
            val.holds.remove(&root);
        }
        val
    }

    /// W0402 at `value`, the value of the body, for `root` provided at `provided_at`: a `with`
    /// item, or the callee of the scope function `scope`.
    fn report_escape(
        &mut self,
        root: Holder,
        scope: Option<SmolStr>,
        provided_at: Span,
        value: ExprId,
    ) {
        if !self.report {
            return;
        }
        let shown = Shown {
            input: self.input,
            names: &self.names,
            cache: Default::default(),
        };
        let root = shown.name(root);
        self.diags.push(Diagnostic {
            span: Some(self.program().exprs[value].span),
            kind: DiagnosticKind::CaptureEscape {
                root,
                scope,
                provided_at,
            },
        });
    }

    fn is_scope_root(&self, h: Holder) -> bool {
        match h {
            Holder::Scope(_) => true,
            Holder::Caller(_, effect) => self
                .input
                .fibers
                .scope_fns
                .iter()
                .any(|s| s.effect == effect),
            _ => false,
        }
    }

    /// Whether `ty` is a type that belongs to a scope (Fiber, Deferred, Semaphore, Queue).
    fn is_handle(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Named(con, _)
            if self.input.fibers.handles.contains(&self.input.ty_table.def(*con)))
    }

    fn contains_handle(&self, ty: &Ty) -> bool {
        if self.is_handle(ty) {
            return true;
        }
        let mut found = false;
        ty.for_each_child(|child| found |= self.contains_handle(child));
        found
    }
}

fn join(a: Val, b: Val) -> Val {
    let mut holds = a.holds;
    holds.extend(b.holds);
    let call = match (a.call, b.call) {
        (Some(x), Some(y)) if x.binder == y.binder => Some(Rc::new(FnVal {
            binder: x.binder,
            ret: x.ret.union(&y.ret).copied().collect(),
        })),
        _ => None,
    };
    Val { holds, call }
}

fn has_body(program: &Program, def: DefId) -> bool {
    fn_parts(program, def).is_some_and(|p| p.body.is_some())
}

/// Whether a value of type `ty` can hold a capability: whether its type contains a function type,
/// a capability, a type parameter, a handle of a scope or a handler value (`[effect.escape]`).
fn may_hold(input: &Input<'_>, ty: &Ty) -> bool {
    match ty {
        // A type variable left in the body of a definition without a declared type may be one
        // of its type parameters.
        Ty::Fn(_) | Ty::Cap(_) | Ty::Bound(_) | Ty::Var(_) => true,
        Ty::Prim(_) | Ty::Never | Ty::Set(..) | Ty::Error => false,
        Ty::Tuple(elems) => elems.iter().any(|t| may_hold(input, t)),
        Ty::List(elem) => may_hold(input, elem),
        Ty::Named(con, args) => {
            let data = &input.ty_table.cons[*con];
            data.kind == TyConKind::Handler
                || input.fibers.handles.contains(&input.ty_table.def(*con))
                || data
                    .params
                    .iter()
                    .any(|p| p.kind == crate::ty::ParamKind::Set)
                || args.iter().any(|t| may_hold(input, t))
        }
    }
}

/// The expression giving the value of a body, following the last expressions of blocks.
fn tail(program: &Program, mut expr: ExprId) -> ExprId {
    while let ExprKind::Block(block) = &program.exprs[expr].kind {
        match block.tail {
            Some(t) => expr = t,
            None => break,
        }
    }
    expr
}

/// The value of the body of a closure passed to a scope function, for the label of W0402.
fn args_body_tail(program: &Program, call: ExprId) -> Option<ExprId> {
    let arg = match &program.exprs[call].kind {
        ExprKind::Call { args, .. } => args.first()?.value,
        ExprKind::Binary { lhs, .. } => *lhs,
        _ => return None,
    };
    match &program.exprs[arg].kind {
        ExprKind::Lambda { body, .. } => Some(tail(program, *body)),
        _ => Some(arg),
    }
}

/// The local variables bound inside `expr`: by patterns, and as the names and parameters of
/// closures and local fns.
fn bound_in(program: &Program, expr: ExprId, out: &mut FxHashSet<LocalId>) {
    match &program.exprs[expr].kind {
        ExprKind::Block(block) => {
            for stmt in &block.stmts {
                match stmt {
                    Stmt::Let { pat, .. } => pat_locals(program, *pat, out),
                    Stmt::LocalFn(f) => {
                        out.insert(f.local);
                        out.extend(f.sig.params.iter().map(|p| p.local));
                    }
                    Stmt::Expr(_) => {}
                }
            }
        }
        ExprKind::Lambda { params, .. } => out.extend(params.iter().map(|p| p.local)),
        ExprKind::Match { arms, .. } | ExprKind::Escape { arms, .. } => {
            for arm in arms {
                pat_locals(program, arm.pat, out);
            }
        }
        _ => {}
    }
    for child in children(program, expr) {
        bound_in(program, child, out);
    }
}

fn pat_locals(program: &Program, pat: PatId, out: &mut FxHashSet<LocalId>) {
    match &program.pats[pat].kind {
        PatKind::Bind(local) => {
            out.insert(*local);
        }
        PatKind::Ctor { args, .. } => {
            for arg in args.iter().flatten() {
                pat_locals(program, arg.pat, out);
            }
        }
        PatKind::Tuple(pats) => {
            for &p in pats {
                pat_locals(program, p, out);
            }
        }
        PatKind::List { elems, rest } => {
            for &p in elems {
                pat_locals(program, p, out);
            }
            out.extend(rest.as_ref().and_then(|r| r.binding));
        }
        PatKind::Wild | PatKind::Lit(_) | PatKind::Const(_) | PatKind::Unit | PatKind::Missing => {}
    }
}

/// The direct subexpressions of `expr`, including the bodies of closures and local fns.
fn children(program: &Program, expr: ExprId) -> Vec<ExprId> {
    let mut out = Vec::new();
    match &program.exprs[expr].kind {
        ExprKind::Missing
        | ExprKind::Lit(_)
        | ExprKind::Path(_)
        | ExprKind::Unit
        | ExprKind::Use { .. } => {}
        ExprKind::Interpolated(parts) => {
            for part in parts {
                if let StrPart::Interp(e) = part {
                    out.push(*e);
                }
            }
        }
        ExprKind::Field { base, .. } => out.push(*base),
        ExprKind::Call { callee, args } => {
            out.push(*callee);
            out.extend(args.iter().map(|a| a.value));
        }
        ExprKind::Unary { operand, .. } => out.push(*operand),
        ExprKind::Binary { lhs, rhs, .. } => out.extend([*lhs, *rhs]),
        ExprKind::Tuple(elems) => out.extend(elems),
        ExprKind::List { elems, rest } => {
            out.extend(elems);
            out.extend(rest);
        }
        ExprKind::Block(block) => {
            for stmt in &block.stmts {
                match stmt {
                    Stmt::Let { value, .. } => out.push(*value),
                    Stmt::LocalFn(f) => out.push(f.body),
                    Stmt::Expr(e) => out.push(*e),
                }
            }
            out.extend(block.tail);
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            out.extend([*cond, *then_branch]);
            out.extend(else_branch);
        }
        ExprKind::Match { scrutinee, arms } => {
            out.push(*scrutinee);
            for arm in arms {
                out.extend(arm.guard);
                out.push(arm.body);
            }
        }
        ExprKind::Escape { expr, arms } => {
            out.push(*expr);
            for arm in arms {
                out.extend(arm.guard);
                out.push(arm.body);
            }
        }
        ExprKind::Fail(e) | ExprKind::Assert(e) => out.push(*e),
        ExprKind::Update { ctor, fields, base } => {
            out.push(*ctor);
            out.extend(fields.iter().map(|f| f.value));
            out.push(*base);
        }
        ExprKind::With { handlers, body } => {
            for item in handlers {
                out.extend(layer::item_args(item));
            }
            out.push(*body);
        }
        ExprKind::Lambda { body, .. } => out.push(*body),
    }
    out
}

/// Turns roots into what the language server and the diagnostics show.
struct Shown<'a, 'b> {
    input: &'a Input<'b>,
    names: &'a FxHashMap<Holder, LocalId>,
    /// Each root shown once: many expressions hold the same roots.
    cache: std::cell::RefCell<FxHashMap<Holder, Held>>,
}

impl Shown<'_, '_> {
    fn list(&self, holds: &Holds) -> Vec<Held> {
        let mut out: Vec<Held> = Vec::new();
        for &h in holds {
            let cached = self.cache.borrow().get(&h).cloned();
            let held = match cached {
                Some(held) => held,
                None => {
                    let held = self.held(h);
                    self.cache.borrow_mut().insert(h, held.clone());
                    held
                }
            };
            if !out.contains(&held) {
                out.push(held);
            }
        }
        out
    }

    fn name(&self, h: Holder) -> SmolStr {
        self.held(h).name
    }

    fn held(&self, h: Holder) -> Held {
        let input = self.input;
        let program = input.program;
        let bound = self
            .names
            .get(&h)
            .map(|&local| program.locals[local].name.clone());
        match h {
            Holder::With(with, entry) => {
                let ExprKind::With { handlers, .. } = &program.exprs[with].kind else {
                    unreachable!("a root of a with");
                };
                let insts = input.insts(with, handlers);
                let entries =
                    layer::with_entries(program, input.sigs, input.ty_table, handlers, &insts);
                let e = &entries[entry];
                let handler = match &e.supplier {
                    Supplier::Handler(h) => program.defs[*h].name.clone(),
                    Supplier::Effect(t) => program.defs[t.def].name.clone(),
                    Supplier::Member { member, .. } => program.defs[*member].name.clone(),
                };
                Held {
                    name: bound.unwrap_or_else(|| handler.clone()),
                    provider: Some(Provider {
                        what: SmolStr::new(format!("with {handler}")),
                        span: e.span,
                    }),
                }
            }
            Holder::Opened(with, item) => {
                let ExprKind::With { handlers, .. } = &program.exprs[with].kind else {
                    unreachable!("a root of a with");
                };
                let item = &handlers[item];
                let member = match (item.res, &item.cap) {
                    (Res::Def(m), _) => program.defs[m].name.clone(),
                    (_, Some((_, name))) => name.name.clone(),
                    _ => SmolStr::default(),
                };
                Held {
                    name: bound.unwrap_or_else(|| member.clone()),
                    provider: Some(Provider {
                        what: SmolStr::new(format!("with {member}")),
                        span: item.span,
                    }),
                }
            }
            Holder::Scope(call) => {
                let callee = match &program.exprs[call].kind {
                    ExprKind::Call { callee, .. } => *callee,
                    ExprKind::Binary { rhs, .. } => match &program.exprs[*rhs].kind {
                        ExprKind::Call { callee, .. } => *callee,
                        _ => *rhs,
                    },
                    _ => call,
                };
                let scope = match program.exprs[callee].kind {
                    ExprKind::Path(Res::Def(def)) => {
                        input.fibers.scope_fns.iter().find(|s| s.def == def)
                    }
                    _ => None,
                };
                let what = scope.map(|s| s.display.clone()).unwrap_or_default();
                let effect = scope
                    .map(|s| program.defs[s.effect].name.clone())
                    .unwrap_or_default();
                Held {
                    name: bound.unwrap_or(effect),
                    provider: Some(Provider {
                        what,
                        span: program.exprs[callee].span,
                    }),
                }
            }
            Holder::Param(binder, i) => {
                let name = bound
                    .or_else(|| {
                        param_local(program, binder, i).map(|l| program.locals[l].name.clone())
                    })
                    .unwrap_or_else(|| SmolStr::new(format!("#{i}")));
                Held {
                    name,
                    provider: None,
                }
            }
            Holder::Caller(_, effect) => Held {
                name: program.defs[effect].name.clone(),
                provider: None,
            },
        }
    }
}

/// The local variable of parameter `i` of a binder.
fn param_local(program: &Program, binder: Binder, i: usize) -> Option<LocalId> {
    match binder {
        Binder::Def(def) => fn_parts(program, def).and_then(|p| p.params.get(i).map(|p| p.local)),
        Binder::Lambda(expr) => match &program.exprs[expr].kind {
            ExprKind::Lambda { params, .. } => params.get(i).map(|p| p.local),
            _ => None,
        },
        Binder::LocalFn(_) => None,
    }
}
