//! Static specialization (`docs/design/c-embedded-architecture.md`, ② static specialization):
//! closure records, handlers and evidence tables whose contents are known at compile time stop
//! being runtime values that are looked into. Every function the entries reach is copied once per
//! combination of the static values its parameters get, and in the copy:
//!
//! - A closure record (`make_closure F[captures]`) is a tuple of its captures; calling it
//!   (`call_indirect`) is a direct call of (a copy of) `F` taking the tuple as its environment
//!   (`env_get` becomes `tuple_get`). A record of a function that only forwards its arguments
//!   (`g$fn`) calls `g` directly.
//! - A handler value is a tuple of its operations' records, and an operation call is a direct call
//!   of the operation's function.
//! - An evidence table is a tuple of the handlers it binds, in effect order; `ev_lookup` is a
//!   `tuple_get` (or nothing, see below) and `ev_extend` builds the tuple.
//! - The effects the board provides have no handler value: their operations are external
//!   functions (made by this pass, named by [`Options::board_op`]), and looking them up gives an
//!   empty tuple.
//! - `runtime retire` of a static handler goes away: handlers cannot leave their `with` (spec
//!   8.6), so a released handler cannot be used.
//!
//! So no function value, handler or evidence table is left at run time: tuples of the captured
//! values (empty when nothing is captured) stand for them, and they are value types if what they
//! capture is. A static value that would be stored in a data structure, or two different static
//! values meeting at a join, cannot be specialized; these are reported.
//!
//! The pass runs right after closure conversion, on the functions the entries reach; the module's
//! functions are replaced by the copies (the entries' copies are returned).

use emela_core::{EffectId, External, ExternalBinding, HostRef, Type, TypeId};
use la_arena::Arena;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{
    Callee, FnId, Function, LabelId, Module, Operand, RuntimeOp, Seq, SeqId, Stage, Stmt, StmtId,
    Term, Value, VarData, VarId,
};
use crate::repr::{Repr, repr_in};

/// What [`specialize_statics`] needs from the target.
pub struct Options<'a> {
    /// The C name of operation `op` of the effect if the board provides the effect.
    pub board_op: &'a dyn Fn(&emela_core::Module, EffectId, usize) -> Option<String>,
    /// The `State` effects the target provides for the whole run (static states).
    pub states: &'a [StaticState],
}

/// A `State[S]` that lives as long as the program (`@static`), held by the backend in static
/// storage. Its operations read and write it through `load` and `store`, external functions the
/// backend defines, between `enter` and `exit` (a critical section if interrupts may also use it).
pub struct StaticState {
    /// The instantiation `State[S]`, with operations `get`, `set` and `update` in that order.
    pub effect: EffectId,
    /// `fn() -> S`.
    pub load: String,
    /// `fn(S) -> ()`.
    pub store: String,
    /// `fn() -> ()`, before each operation.
    pub enter: String,
    /// `fn() -> ()`, after each operation.
    pub exit: String,
}

/// A value known at compile time, or [`Sv::Dyn`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Sv {
    /// An ordinary value, only known at run time.
    Dyn,
    /// A closure record of the lifted function, with what it captures.
    Closure { function: FnId, captures: Vec<Sv> },
    /// A handler value of the effect: its operations' records.
    Handler { effect: EffectId, ops: Vec<Sv> },
    /// The handler of an effect the board provides.
    Board(EffectId),
    /// An evidence table: the handlers it binds, by effect (sorted). Effects the board provides
    /// are bound to [`Sv::Board`] unless bound here.
    Ev(Vec<(EffectId, Sv)>),
    /// The handler of a static state (index into [`Options::states`]).
    State(usize),
    /// The `resume` of a `suspend fn` operation's implementation. The operation runs to the end
    /// (nothing suspends): its final `resume(v)` returns `v` to the caller.
    Resume,
}

impl Sv {
    fn is_static(&self) -> bool {
        !matches!(self, Sv::Dyn)
    }
}

/// Specializes the functions `roots` reach on the static values they pass, replacing the module's
/// functions by the copies. Returns the copy of each root, or the problems that prevent it (with
/// the function they are in). The module must be at [`crate::Stage::ClosuresConverted`].
pub fn specialize_statics(
    m: &mut Module,
    roots: &[FnId],
    options: &Options<'_>,
) -> Result<Vec<FnId>, Vec<String>> {
    assert_eq!(
        m.stage,
        Stage::ClosuresConverted,
        "static specialization runs right after closure conversion"
    );
    let orig = std::mem::take(&mut m.functions);
    let mut s = Specializer {
        orig: &orig,
        core: &mut m.core,
        out: Arena::new(),
        instances: FxHashMap::default(),
        board_fns: FxHashMap::default(),
        origin: FxHashMap::default(),
        externs: FxHashMap::default(),
        state_fns: FxHashMap::default(),
        names: FxHashSet::default(),
        errors: Vec::new(),
        options,
    };
    let mut copies = Vec::new();
    for &root in roots {
        let f = &orig[root];
        let params: Vec<Sv> = f
            .params
            .iter()
            .map(|&p| match s.core.ty(f.vars[p].ty) {
                Type::Evidence => Sv::Ev(Vec::new()),
                _ => Sv::Dyn,
            })
            .collect();
        let (copy, _) = s.instance(root, params);
        s.out[copy].exported = f.exported;
        copies.push(copy);
    }
    let errors = std::mem::take(&mut s.errors);
    m.functions = s.out;
    m.error_shows.clear();
    if errors.is_empty() {
        Ok(copies)
    } else {
        Err(errors)
    }
}

/// Makes every function run to completion: nothing suspends but the `suspend fn` external
/// functions, which the backend calls as ordinary functions that wait (the embedded C target has
/// one fiber, so waiting and suspending are the same; `docs/design/c-embedded-architecture.md`,
/// 決定: fiber は当面拒否し、suspend は待つ形で扱う). Clears `suspends` on the other functions and
/// their calls, so that no state machine is made. Run after [`specialize_statics`], which has
/// already made `suspend fn` operations return what they resume with.
pub fn run_to_completion(m: &mut Module) {
    let external: Vec<bool> = m
        .functions
        .iter()
        .map(|(_, f)| f.external.is_some())
        .collect();
    for (_, f) in m.functions.iter_mut() {
        if f.external.is_some() {
            continue;
        }
        f.suspends = false;
        for (_, s) in f.stmts.iter_mut() {
            if let Stmt::Let {
                value: Value::Call {
                    callee, suspends, ..
                },
                ..
            } = s
            {
                let to_external = matches!(callee, Callee::Direct(g)
                    if external[u32::from(g.into_raw()) as usize]);
                if !to_external {
                    *suspends = false;
                }
            }
        }
    }
}

struct Specializer<'a> {
    orig: &'a Arena<Function>,
    core: &'a mut emela_core::Module,
    out: Arena<Function>,
    /// (function, its parameters' static values) → its copy and the static value it returns
    /// (`None` while the copy is being made).
    instances: FxHashMap<(FnId, Vec<Sv>), (FnId, Option<Sv>)>,
    /// (board effect, operation) → its external function.
    board_fns: FxHashMap<(EffectId, usize), FnId>,
    /// Copy → the original it was made from.
    origin: FxHashMap<FnId, FnId>,
    /// External functions made by the pass, by C name.
    externs: FxHashMap<String, FnId>,
    /// (static state, operation, the function given to `update`) → the operation's function.
    state_fns: FxHashMap<(usize, usize, Option<Sv>), FnId>,
    names: FxHashSet<String>,
    errors: Vec<String>,
    options: &'a Options<'a>,
}

/// The function being rewritten.
struct Cx {
    f: Function,
    /// The original name, for messages.
    name: String,
    svs: FxHashMap<VarId, Sv>,
    /// The values yielded to each enclosing compound statement, innermost last.
    yields: Vec<Vec<Sv>>,
    /// The values given to each block's label.
    breaks: FxHashMap<LabelId, Vec<Sv>>,
    /// The loops' variables, innermost last.
    loops: Vec<Vec<VarId>>,
    returns: Vec<Sv>,
    /// For a `suspend fn` operation's implementation, its `resume` parameter.
    resume: Option<VarId>,
}

impl Cx {
    fn sv(&self, o: &Operand) -> Sv {
        match o {
            Operand::Var(v) => self.svs.get(v).cloned().unwrap_or(Sv::Dyn),
            Operand::Const(_) => Sv::Dyn,
        }
    }

    fn var_sv(&self, v: VarId) -> Sv {
        self.svs.get(&v).cloned().unwrap_or(Sv::Dyn)
    }

    fn let_(&mut self, dst: Option<VarId>, value: Value) -> StmtId {
        self.f.stmts.alloc(Stmt::Let {
            dst,
            value,
            on_defect: None,
            on_suspend: None,
        })
    }
}

impl Specializer<'_> {
    fn error(&mut self, cx: &Cx, message: &str) {
        let name = display(&cx.name);
        let text = format!("`{name}`: {message}");
        if !self.errors.contains(&text) {
            self.errors.push(text);
        }
    }

    fn unique(&mut self, base: &str) -> String {
        let mut name = base.to_owned();
        let mut n = 2;
        while !self.names.insert(name.clone()) {
            name = format!("{base}${n}");
            n += 1;
        }
        name
    }

    /// The type a value of `sv` has at run time (`orig` for an ordinary value).
    fn rt(&mut self, sv: &Sv, orig: TypeId) -> TypeId {
        match sv {
            Sv::Dyn => orig,
            Sv::Closure { function, captures } => {
                let env = self.orig[*function].env.clone().unwrap_or_default();
                let ts: Vec<TypeId> = captures
                    .iter()
                    .zip(env)
                    .map(|(c, t)| self.rt(c, t))
                    .collect();
                self.core.intern(Type::Tuple(ts))
            }
            Sv::Handler { ops, .. } => {
                let ts: Vec<TypeId> = ops.iter().map(|o| self.rt(o, TypeId::ANY)).collect();
                self.core.intern(Type::Tuple(ts))
            }
            Sv::Board(_) | Sv::State(_) | Sv::Resume => self.core.intern(Type::Tuple(Vec::new())),
            Sv::Ev(bindings) => {
                let ts: Vec<TypeId> = bindings
                    .iter()
                    .filter(|(_, b)| !matches!(b, Sv::Board(_) | Sv::State(_)))
                    .map(|(_, b)| b.clone())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(|b| self.rt(b, TypeId::ANY))
                    .collect();
                self.core.intern(Type::Tuple(ts))
            }
        }
    }

    /// Sets the variable's static value, and its type to the runtime type of the value.
    fn set(&mut self, cx: &mut Cx, v: VarId, sv: Sv) {
        if sv.is_static() {
            let ty = self.rt(&sv, cx.f.vars[v].ty);
            cx.f.vars[v].ty = ty;
            cx.f.vars[v].repr = repr_in(self.core, ty, true);
            cx.svs.insert(v, sv);
        }
    }

    /// The copy of `id` for the static values of its parameters, and the static value it returns.
    fn instance(&mut self, id: FnId, params: Vec<Sv>) -> (FnId, Sv) {
        let key = (id, params);
        if let Some((copy, ret)) = &self.instances.get(&key) {
            return (*copy, ret.clone().unwrap_or(Sv::Dyn));
        }
        let src = &self.orig[id];
        let mut f = src.clone();
        // The functions passed as closures name the copy: `Option.map$is_pressed`.
        let mut base = src.name.clone();
        let skip = usize::from(src.env.is_some());
        for sv in key.1.iter().skip(skip) {
            if let Sv::Closure { function, .. } = sv {
                let lifted = &self.orig[*function];
                let name = match wrapped(lifted) {
                    Some(g) => self.orig[g].name.clone(),
                    None => lifted.name.clone(),
                };
                base.push('$');
                base.push_str(display(&name));
            }
        }
        f.name = self.unique(&base);
        f.exported = false;
        let copy = self.out.alloc(Function::new(&f.name));
        self.instances.insert(key.clone(), (copy, None));
        self.origin.insert(copy, id);
        let mut cx = Cx {
            f,
            name: src.name.clone(),
            svs: FxHashMap::default(),
            yields: Vec::new(),
            breaks: FxHashMap::default(),
            loops: Vec::new(),
            returns: Vec::new(),
            resume: None,
        };
        let params = key.1.clone();
        for (i, sv) in params.into_iter().enumerate() {
            let p = cx.f.params[i];
            self.set(&mut cx, p, sv);
        }
        // An operation's implementation that resumes at its end returns the resumed value.
        if let Some(i) = key.1.iter().position(|sv| *sv == Sv::Resume) {
            let p = cx.f.params[i];
            cx.resume = Some(p);
            let resume_ty = self.orig[id].vars[self.orig[id].params[i]].ty;
            cx.f.ret = match self.core.ty(resume_ty) {
                Type::Fn(ft) => ft.params.first().copied().unwrap_or(TypeId::UNIT),
                _ => TypeId::UNIT,
            };
        }
        // A lifted function whose environment is a tuple now takes it as an ordinary parameter.
        if cx.f.env.is_some() && cx.var_sv(cx.f.params[0]).is_static() {
            cx.f.env = None;
        }
        if let Some(body) = cx.f.body {
            self.seq(&mut cx, body);
        }
        let returns = std::mem::take(&mut cx.returns);
        let ret = self.join(&cx, returns, "the function's return value");
        if ret.is_static() {
            let ty = self.rt(&ret, cx.f.ret);
            cx.f.ret = ty;
        }
        self.out[copy] = cx.f;
        if let Some(entry) = self.instances.get_mut(&key) {
            entry.1 = Some(ret.clone());
        }
        (copy, ret)
    }

    /// The static value where `svs` meet: they must agree unless all are ordinary.
    fn join(&mut self, cx: &Cx, svs: Vec<Sv>, what: &str) -> Sv {
        let mut it = svs.into_iter();
        let Some(first) = it.next() else {
            return Sv::Dyn;
        };
        for sv in it {
            if sv != first {
                if sv.is_static() || first.is_static() {
                    self.error(
                        cx,
                        &format!(
                            "{what} gets a function value or handler not known statically, which the embedded C target does not support"
                        ),
                    );
                }
                return Sv::Dyn;
            }
        }
        first
    }

    fn seq(&mut self, cx: &mut Cx, id: SeqId) {
        let mut stmts = cx.f.seqs[id].stmts.clone();
        // `resume(v)` then `return`, at the end of a `suspend fn` operation: `return v`.
        let mut resumed = false;
        if let Some(r) = cx.resume
            && let Some(&last) = stmts.last()
            && let Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Indirect(c),
                        args,
                        ..
                    },
                ..
            } = &cx.f.stmts[last]
            && *c == r
            && cx.f.seqs[id].term == Term::Return(None)
        {
            let value = args.first().cloned();
            stmts.pop();
            cx.f.seqs[id].term = Term::Return(value);
            resumed = true;
        }
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            self.stmt(cx, s, &mut out);
        }
        cx.f.seqs[id].stmts = out;
        if cx.resume.is_some() && !resumed && matches!(cx.f.seqs[id].term, Term::Return(_)) {
            self.error(
                cx,
                "an operation of a suspend fn ends without calling resume last: the embedded C target supports only operations that call resume last",
            );
        }
        let term = cx.f.seqs[id].term.clone();
        match &term {
            Term::Yield(Some(o)) => {
                let sv = cx.sv(o);
                if let Some(y) = cx.yields.last_mut() {
                    y.push(sv);
                }
            }
            Term::Return(Some(o)) => {
                let sv = cx.sv(o);
                cx.returns.push(sv);
            }
            Term::Return(None) => cx.returns.push(Sv::Dyn),
            Term::Break {
                label,
                value: Some(o),
            } => {
                let sv = cx.sv(o);
                cx.breaks.entry(*label).or_default().push(sv);
            }
            Term::Recur(args) => {
                if let Some(vars) = cx.loops.last().cloned() {
                    for (v, a) in vars.iter().zip(args) {
                        if cx.var_sv(*v) != cx.sv(a) {
                            self.error(
                                cx,
                                "a loop variable gets a different function value or handler per iteration, which the embedded C target does not support",
                            );
                        }
                    }
                }
            }
            Term::Fail(o) | Term::Defect(o) => self.escape(cx, o),
            _ => {}
        }
    }

    /// Reports a static value used where it would have to be a runtime value.
    fn escape(&mut self, cx: &Cx, o: &Operand) {
        if cx.sv(o).is_static() {
            self.error(
                cx,
                "a function value or handler is carried around as a value: the embedded C target supports only passing them as arguments",
            );
        }
    }

    /// Runs `body` collecting what is yielded to the statement, and gives the joined value to
    /// `dst`.
    fn compound(
        &mut self,
        cx: &mut Cx,
        dst: Option<VarId>,
        label: Option<LabelId>,
        body: impl FnOnce(&mut Self, &mut Cx),
    ) {
        cx.yields.push(Vec::new());
        body(self, cx);
        let mut svs = cx.yields.pop().unwrap_or_default();
        if let Some(l) = label {
            svs.extend(cx.breaks.remove(&l).unwrap_or_default());
        }
        if let Some(d) = dst {
            let sv = self.join(cx, svs, "the join of branches");
            self.set(cx, d, sv);
        }
    }

    fn stmt(&mut self, cx: &mut Cx, id: StmtId, out: &mut Vec<StmtId>) {
        match cx.f.stmts[id].clone() {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                let Some(value) = self.value(cx, dst, value, out) else {
                    return;
                };
                cx.f.stmts[id] = Stmt::Let {
                    dst,
                    value,
                    on_defect,
                    on_suspend,
                };
                out.push(id);
            }
            Stmt::If {
                dst, then, else_, ..
            } => {
                self.compound(cx, dst, None, |s, cx| {
                    s.seq(cx, then);
                    s.seq(cx, else_);
                });
                out.push(id);
            }
            Stmt::Switch {
                dst,
                cases,
                default,
                ..
            } => {
                self.compound(cx, dst, None, |s, cx| {
                    for c in &cases {
                        s.seq(cx, c.body);
                    }
                    if let Some(d) = default {
                        s.seq(cx, d);
                    }
                });
                out.push(id);
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                for (&v, init) in vars.iter().zip(&inits) {
                    let sv = cx.sv(init);
                    self.set(cx, v, sv);
                }
                cx.loops.push(vars.clone());
                self.compound(cx, dst, None, |s, cx| s.seq(cx, body));
                cx.loops.pop();
                out.push(id);
            }
            Stmt::Block { dst, label, body } => {
                self.compound(cx, dst, Some(label), |s, cx| s.seq(cx, body));
                out.push(id);
            }
            Stmt::Protect { dst, body, cleanup } => {
                self.compound(cx, dst, None, |s, cx| s.seq(cx, body));
                cx.yields.push(Vec::new());
                self.seq(cx, cleanup);
                cx.yields.pop();
                out.push(id);
            }
            Stmt::Dup(_) | Stmt::Drop(_) => out.push(id),
        }
    }

    /// Rewrites a `let`'s value, adding the statements it needs before it to `out`. `None` drops
    /// the statement.
    fn value(
        &mut self,
        cx: &mut Cx,
        dst: Option<VarId>,
        value: Value,
        out: &mut Vec<StmtId>,
    ) -> Option<Value> {
        let result = match value {
            Value::MakeClosure { function, captures } => {
                let sv = Sv::Closure {
                    function,
                    captures: captures.iter().map(|&c| cx.var_sv(c)).collect(),
                };
                if let Some(d) = dst {
                    self.set(cx, d, sv);
                }
                Value::Tuple(captures.into_iter().map(Operand::Var).collect())
            }
            Value::EnvGet { env, index } => match cx.var_sv(env) {
                Sv::Closure { captures, .. } => {
                    if let Some(d) = dst {
                        let sv = captures.get(index).cloned().unwrap_or(Sv::Dyn);
                        self.set(cx, d, sv);
                    }
                    Value::TupleGet { base: env, index }
                }
                _ => Value::EnvGet { env, index },
            },
            Value::Handler { effect, ops, .. } => {
                let sv = Sv::Handler {
                    effect,
                    ops: ops.iter().map(|&o| cx.var_sv(o)).collect(),
                };
                if let Some(d) = dst {
                    self.set(cx, d, sv);
                }
                Value::Tuple(ops.into_iter().map(Operand::Var).collect())
            }
            Value::EvExtend { base, bindings } => {
                let mut table: Vec<(EffectId, Sv, Option<Operand>)> = Vec::new();
                if let Some(b) = base {
                    if let Sv::Ev(old) = cx.var_sv(b) {
                        let mut k = 0;
                        for (e, sv) in old {
                            if matches!(sv, Sv::Board(_) | Sv::State(_)) {
                                table.push((e, sv, None));
                            } else {
                                let ty = self.rt(&sv, TypeId::ANY);
                                let t = cx.f.vars.alloc(VarData {
                                    hint: "h".into(),
                                    repr: repr_in(self.core, ty, true),
                                    ty,
                                });
                                let s = cx.let_(Some(t), Value::TupleGet { base: b, index: k });
                                out.push(s);
                                cx.svs.insert(t, sv.clone());
                                table.push((e, sv, Some(Operand::Var(t))));
                                k += 1;
                            }
                        }
                    } else {
                        self.error(
                            cx,
                            "extends an evidence table not known statically (a compiler bug)",
                        );
                    }
                }
                for (e, h) in bindings {
                    let sv = cx.var_sv(h);
                    table.retain(|(x, _, _)| *x != e);
                    table.push((e, sv, Some(Operand::Var(h))));
                }
                table.sort_by_key(|(e, _, _)| u32::from(e.into_raw()));
                let parts: Vec<Operand> = table
                    .iter()
                    .filter(|(_, sv, _)| !matches!(sv, Sv::Board(_) | Sv::State(_)))
                    .filter_map(|(_, _, o)| o.clone())
                    .collect();
                let sv = Sv::Ev(table.into_iter().map(|(e, sv, _)| (e, sv)).collect());
                if let Some(d) = dst {
                    self.set(cx, d, sv);
                }
                Value::Tuple(parts)
            }
            Value::EvLookup { ev, effect } => {
                let Sv::Ev(table) = cx.var_sv(ev) else {
                    self.error(
                        cx,
                        "looks into an evidence table not known statically (a compiler bug)",
                    );
                    return Some(Value::EvLookup { ev, effect });
                };
                let mut k = 0;
                let mut found = None;
                for (e, sv) in &table {
                    if *e == effect {
                        found = Some((sv.clone(), k));
                        break;
                    }
                    if !matches!(sv, Sv::Board(_) | Sv::State(_)) {
                        k += 1;
                    }
                }
                match found {
                    Some((sv @ (Sv::Board(_) | Sv::State(_)), _)) => {
                        if let Some(d) = dst {
                            self.set(cx, d, sv);
                        }
                        Value::Tuple(Vec::new())
                    }
                    Some((sv, index)) => {
                        if let Some(d) = dst {
                            self.set(cx, d, sv);
                        }
                        Value::TupleGet { base: ev, index }
                    }
                    None if self.options.states.iter().any(|s| s.effect == effect) => {
                        let i = self
                            .options
                            .states
                            .iter()
                            .position(|s| s.effect == effect)
                            .unwrap_or(0);
                        if let Some(d) = dst {
                            self.set(cx, d, Sv::State(i));
                        }
                        Value::Tuple(Vec::new())
                    }
                    None if (self.options.board_op)(self.core, effect, 0).is_some() => {
                        if let Some(d) = dst {
                            self.set(cx, d, Sv::Board(effect));
                        }
                        Value::Tuple(Vec::new())
                    }
                    None => {
                        let name = self.core.effects[effect].name.clone();
                        self.error(
                            cx,
                            &format!("no handler is provided for the effect `{}`", display(&name)),
                        );
                        return Some(Value::EvLookup { ev, effect });
                    }
                }
            }
            Value::Call {
                callee,
                args,
                suspends,
                on_fail,
            } => return Some(self.call(cx, dst, callee, args, suspends, on_fail, out)),
            Value::Coerce(v) => {
                let sv = cx.var_sv(v);
                if let Some(d) = dst
                    && sv.is_static()
                {
                    self.set(cx, d, sv);
                }
                Value::Coerce(v)
            }
            Value::Runtime {
                op: RuntimeOp::Retire,
                args,
            } if args.first().is_some_and(|a| cx.sv(a).is_static()) => return None,
            other => {
                let mut escaped = false;
                other.for_each_var(|v| escaped |= cx.var_sv(v).is_static());
                if escaped {
                    self.error(
                        cx,
                        "a function value or handler is carried around as a value: the embedded C target supports only passing them as arguments",
                    );
                }
                other
            }
        };
        Some(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        cx: &mut Cx,
        dst: Option<VarId>,
        callee: Callee,
        mut args: Vec<Operand>,
        suspends: bool,
        on_fail: Option<crate::ir::OnFail>,
        out: &mut Vec<StmtId>,
    ) -> Value {
        let target = match callee {
            Callee::Direct(g) => {
                let svs: Vec<Sv> = args.iter().map(|a| cx.sv(a)).collect();
                Some(self.instance(g, svs))
            }
            Callee::Indirect(c) => match cx.var_sv(c) {
                Sv::Resume => {
                    self.error(
                        cx,
                        "calls resume somewhere other than last: the embedded C target supports only calling resume last in an operation of a suspend fn",
                    );
                    None
                }
                sv @ Sv::Closure { .. } => {
                    let Sv::Closure { function, .. } = &sv else {
                        unreachable!()
                    };
                    let function = *function;
                    Some(self.call_record(cx, function, sv, c, &mut args))
                }
                _ => None,
            },
            Callee::Op {
                handler,
                effect,
                op,
            } => match cx.var_sv(handler) {
                Sv::Handler { ops, .. } => match ops.get(op).cloned() {
                    Some(sv @ Sv::Closure { .. }) => {
                        let Sv::Closure { function, .. } = &sv else {
                            unreachable!()
                        };
                        let function = *function;
                        let ty = self.rt(&sv, TypeId::ANY);
                        let t = cx.f.vars.alloc(VarData {
                            hint: "op".into(),
                            repr: repr_in(self.core, ty, true),
                            ty,
                        });
                        let s = cx.let_(
                            Some(t),
                            Value::TupleGet {
                                base: handler,
                                index: op,
                            },
                        );
                        out.push(s);
                        cx.svs.insert(t, sv.clone());
                        if self.core.effects[effect].ops[op].suspend {
                            let unit = self.core.intern(Type::Tuple(Vec::new()));
                            let r = cx.f.vars.alloc(VarData {
                                hint: "resume".into(),
                                repr: repr_in(self.core, unit, true),
                                ty: unit,
                            });
                            let s = cx.let_(Some(r), Value::Tuple(Vec::new()));
                            out.push(s);
                            cx.svs.insert(r, Sv::Resume);
                            args.push(Operand::Var(r));
                        }
                        Some(self.call_record(cx, function, sv, t, &mut args))
                    }
                    _ => {
                        self.error(cx, "calls an operation of a handler not known statically");
                        None
                    }
                },
                Sv::Board(e) => Some((self.board_fn(e, op), Sv::Dyn)),
                Sv::State(i) => {
                    let f = args.first().map(|a| cx.sv(a));
                    self.state_fn(cx, i, op, f).map(|f| (f, Sv::Dyn))
                }
                _ => {
                    let _ = effect;
                    None
                }
            },
        };
        let Some((target, ret)) = target else {
            for a in &args {
                self.escape(cx, a);
            }
            return Value::Call {
                callee,
                args,
                suspends,
                on_fail,
            };
        };
        if let Some(d) = dst {
            self.set(cx, d, ret);
        }
        // A copy keeps the flags of its original (and may not be finished yet: a recursive call).
        // Functions made by the pass (board operations, static states) are finished already.
        let (fails, suspends) = match self.origin.get(&target) {
            Some(&o) => (self.orig[o].fails, self.orig[o].suspends),
            None => (self.out[target].fails, self.out[target].suspends),
        };
        Value::Call {
            callee: Callee::Direct(target),
            args,
            suspends,
            on_fail: if fails { on_fail } else { None },
        }
    }

    /// The call of a closure record `record` (of `function`, static value `sv`) with `args`: the
    /// function the record forwards to, or the copy of `function` taking the record as its
    /// environment (put first in `args`).
    fn call_record(
        &mut self,
        cx: &Cx,
        function: FnId,
        sv: Sv,
        record: VarId,
        args: &mut Vec<Operand>,
    ) -> (FnId, Sv) {
        if let Some(g) = wrapped(&self.orig[function]) {
            let svs: Vec<Sv> = args.iter().map(|a| cx.sv(a)).collect();
            return self.instance(g, svs);
        }
        args.insert(0, Operand::Var(record));
        let svs: Vec<Sv> = std::iter::once(sv)
            .chain(args[1..].iter().map(|a| cx.sv(a)))
            .collect();
        self.instance(function, svs)
    }

    /// An external function `name` of the given parameter and result types (made once).
    fn extern_fn(&mut self, name: &str, params: &[TypeId], ret: TypeId) -> FnId {
        if let Some(&f) = self.externs.get(name) {
            return f;
        }
        let mut f = Function::new(&self.unique(&format!("${name}")));
        for (i, &t) in params.iter().enumerate() {
            let repr = repr_in(self.core, t, true);
            if repr != Repr::Void {
                let v = f.vars.alloc(VarData {
                    hint: format!("a{i}"),
                    repr,
                    ty: t,
                });
                f.params.push(v);
            }
        }
        f.ret = ret;
        f.external = Some(External {
            bindings: vec![ExternalBinding {
                target: "embedded".into(),
                host: HostRef::Global(name.to_owned()),
            }],
        });
        let id = self.out.alloc(f);
        self.externs.insert(name.to_owned(), id);
        id
    }

    /// The function of operation `op` (`get`, `set`, `update`) of static state `i`: the access
    /// between its `enter` and `exit`. `update` applies its function (`f`, a closure record) in the
    /// same section (spec 11.1: an update runs at once), so a copy is made per function.
    fn state_fn(&mut self, cx: &Cx, i: usize, op: usize, f: Option<Sv>) -> Option<FnId> {
        let state = &self.options.states[i];
        let (effect, load, store) = (state.effect, state.load.clone(), state.store.clone());
        let (enter_name, exit_name) = (state.enter.clone(), state.exit.clone());
        let ty = self.core.effects[effect].ops[0].ret;
        let key = (i, op, f.clone().filter(|_| op == 2));
        if let Some(&id) = self.state_fns.get(&key) {
            return Some(id);
        }
        let enter = self.extern_fn(&enter_name, &[], TypeId::UNIT);
        let exit = self.extern_fn(&exit_name, &[], TypeId::UNIT);
        let load = self.extern_fn(&load, &[], ty);
        let store = self.extern_fn(&store, &[ty], TypeId::UNIT);
        let op_name = self.core.effects[effect].ops[op].name.clone();
        let name = format!("{}.{op_name}", self.core.effects[effect].name);
        let mut g = Function::new(&self.unique(&name));
        let repr = repr_in(self.core, ty, true);
        let state_var = |g: &mut Function, hint: &str| {
            g.vars.alloc(VarData {
                hint: hint.into(),
                repr,
                ty,
            })
        };
        let call = |callee: FnId, args: Vec<Operand>| Value::Call {
            callee: Callee::Direct(callee),
            args,
            suspends: false,
            on_fail: None,
        };
        let let_ = |g: &mut Function, dst: Option<VarId>, value: Value| {
            g.stmts.alloc(Stmt::Let {
                dst,
                value,
                on_defect: None,
                on_suspend: None,
            })
        };
        let mut stmts = vec![let_(&mut g, None, call(enter, Vec::new()))];
        let term = match op {
            0 => {
                g.ret = ty;
                let v = state_var(&mut g, "s");
                stmts.push(let_(&mut g, Some(v), call(load, Vec::new())));
                stmts.push(let_(&mut g, None, call(exit, Vec::new())));
                Term::Return(Some(Operand::Var(v)))
            }
            1 => {
                let v = state_var(&mut g, "value");
                g.params.push(v);
                stmts.push(let_(&mut g, None, call(store, vec![Operand::Var(v)])));
                stmts.push(let_(&mut g, None, call(exit, Vec::new())));
                Term::Return(None)
            }
            _ => {
                let Some(sv @ Sv::Closure { .. }) = f else {
                    self.error(
                        cx,
                        "passes a function not known statically to update of State",
                    );
                    return None;
                };
                let Sv::Closure { function, .. } = &sv else {
                    unreachable!()
                };
                let function = *function;
                let env_ty = self.rt(&sv, TypeId::ANY);
                let env = g.vars.alloc(VarData {
                    hint: "f".into(),
                    repr: repr_in(self.core, env_ty, true),
                    ty: env_ty,
                });
                g.params.push(env);
                let s = state_var(&mut g, "s");
                stmts.push(let_(&mut g, Some(s), call(load, Vec::new())));
                let (target, args) = match wrapped(&self.orig[function]) {
                    Some(w) => (self.instance(w, vec![Sv::Dyn]).0, vec![Operand::Var(s)]),
                    None => (
                        self.instance(function, vec![sv.clone(), Sv::Dyn]).0,
                        vec![Operand::Var(env), Operand::Var(s)],
                    ),
                };
                let r = state_var(&mut g, "r");
                stmts.push(let_(&mut g, Some(r), call(target, args)));
                stmts.push(let_(&mut g, None, call(store, vec![Operand::Var(r)])));
                stmts.push(let_(&mut g, None, call(exit, Vec::new())));
                Term::Return(None)
            }
        };
        let body = g.seqs.alloc(Seq { stmts, term });
        g.body = Some(body);
        let id = self.out.alloc(g);
        self.state_fns.insert(key, id);
        Some(id)
    }

    /// The external function of operation `op` of a board effect.
    fn board_fn(&mut self, effect: EffectId, op: usize) -> FnId {
        if let Some(&f) = self.board_fns.get(&(effect, op)) {
            return f;
        }
        let def = &self.core.effects[effect];
        let opdef = def.ops[op].clone();
        let name = format!("{}.{}", def.name, opdef.name);
        let c_name = (self.options.board_op)(self.core, effect, op).unwrap_or_default();
        let mut f = Function::new(&self.unique(&name));
        for (i, &t) in opdef.params.iter().enumerate() {
            let repr = repr_in(self.core, t, true);
            if repr != Repr::Void {
                let v = f.vars.alloc(VarData {
                    hint: format!("a{i}"),
                    repr,
                    ty: t,
                });
                f.params.push(v);
            }
        }
        f.ret = opdef.ret;
        // A board operation may fail (`fails PeripheralError`): the backend turns its C error code
        // into the failure.
        f.fails = opdef.fails;
        f.external = Some(External {
            bindings: vec![ExternalBinding {
                target: "embedded".into(),
                host: HostRef::Global(c_name),
            }],
        });
        let id = self.out.alloc(f);
        self.board_fns.insert((effect, op), id);
        id
    }
}

/// The function a lifted function only forwards its arguments to (the `g$fn` of a top-level
/// function used as a value).
fn wrapped(f: &Function) -> Option<FnId> {
    let seq: &Seq = &f.seqs[f.body?];
    let [s] = seq.stmts.as_slice() else {
        return None;
    };
    let Stmt::Let {
        dst,
        value:
            Value::Call {
                callee: Callee::Direct(g),
                args,
                ..
            },
        ..
    } = &f.stmts[*s]
    else {
        return None;
    };
    if f.env.as_ref().is_none_or(|e| !e.is_empty()) {
        return None;
    }
    let forwarded = args.len() + 1 == f.params.len()
        && args
            .iter()
            .zip(&f.params[1..])
            .all(|(a, &p)| *a == Operand::Var(p));
    let returned = match (&seq.term, dst) {
        (Term::Return(Some(Operand::Var(r))), Some(d)) => r == d,
        (Term::Return(None), None) => true,
        _ => false,
    };
    (forwarded && returned).then_some(*g)
}

/// A function name for messages: without the type arguments and the suffixes the compiler adds.
fn display(name: &str) -> &str {
    let end = name.find(['[', '$']).unwrap_or(name.len());
    &name[..end]
}
