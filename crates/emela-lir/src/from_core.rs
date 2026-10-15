//! The conversion from monomorphized Core IR to the Flow IR ([`from_core`]).
//!
//! Each Core function becomes a Flow IR function of the same name, and every Core expression
//! becomes statements in evaluation order (spec 14.1, as the JS backend evaluates them): left to
//! right, the callee of an indirect or operation call before the arguments, a list's tail after its
//! elements. What changes on the way:
//!
//! - Values of a type without representation (`()`, `Never`) get no variable: parameters, results
//!   and arguments of those types are dropped (still evaluated). In containers (constructor fields,
//!   tuple and list elements, cells, concurrency operation arguments) they keep their slot with the
//!   placeholder [`Const::Unit`].
//! - `if`, `match`, `escape`, loops and `with` become compound statements that give their value
//!   through their destination (`dst` and `yield` / `break`).
//! - Patterns become decision trees ([`pat`]).
//! - Fallible calls get explicit failure edges: to the block of the innermost enclosing `escape`, or
//!   out of the function (or closure).
//! - Showing (string interpolation, `dbg`, assert messages) and structural equality become calls to
//!   functions generated per type ([`derive`]).
//! - Releases of `with` become `protect` / `cleanup`.
//! - Lambdas become closures with explicit capture lists ([`captures`]).
//! - `panic` and `todo` become `defect`.
//!
//! The input must satisfy `emela_core::verify_monomorphic`. What the conversion cannot make sense
//! of (a type that cannot be shown, a local used before it is bound) is reported as an error; the
//! function is still converted, with a placeholder in its place.

mod captures;
mod derive;
mod pat;

use emela_core::{
    Assert, AssertCond, BinOp, Builtin, Callee as CoreCallee, EffectId, Expr, FiberOp, FnType, Lit,
    Local, OpTy, Sig, StrPart, Type, TypeId, UnOp, WithBind,
};
use la_arena::Arena;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dump::core_ty;
use crate::ir::{
    Callee, Closure, Const, FnId, Function, LabelData, LabelId, Module, OnFail, Operand, PrimOp,
    PrimTy, RuntimeOp, Seq, SeqId, Stmt, StmtId, Term, Value, VarData, VarId,
};
use crate::repr::{Repr, repr_of};

/// Something the conversion could not make sense of; a bug in the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FromCoreError {
    /// The Flow IR function being made.
    pub function: String,
    pub message: String,
}

impl std::fmt::Display for FromCoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fn {}: {}", self.function, self.message)
    }
}

/// Converts a monomorphized Core module (one that passes `emela_core::verify_monomorphic`) to the
/// Flow IR. The Flow IR module owns the Core module, with the types the conversion needed added
/// to its table. The Core functions come first, in the same order and with the same names; the
/// generated show and equality functions (`show$List[Int]`, `eq$Point`) follow.
pub fn from_core(mut core: emela_core::Module) -> Result<Module, Vec<FromCoreError>> {
    // The bodies are read while the module's type table grows, so they are kept apart meanwhile.
    let bodies: Vec<Expr> = core
        .functions
        .iter_mut()
        .map(|(_, f)| std::mem::replace(&mut f.body, Expr::Lit(Lit::Unit)))
        .collect();
    let mut sh = Shared {
        core,
        functions: Arena::new(),
        fn_map: Vec::new(),
        names: FxHashSet::default(),
        derived: FxHashMap::default(),
        pending: Vec::new(),
        errors: Vec::new(),
    };
    for (_, f) in sh.core.functions.iter() {
        sh.fn_map.push(sh.functions.alloc(Function::new(&f.name)));
        sh.names.insert(f.name.clone());
    }
    let ids: Vec<emela_core::FnId> = sh.core.functions.iter().map(|(id, _)| id).collect();
    for (i, id) in ids.into_iter().enumerate() {
        let f = Lower::function(&mut sh, id, &bodies[i]);
        let target = sh.fn_map[i];
        sh.functions[target] = f;
    }
    while let Some((kind, ty, id)) = sh.pending.pop() {
        let name = sh.functions[id].name.clone();
        let f = derive::build(&mut sh, kind, ty, &name);
        sh.functions[id] = f;
    }
    for ((_, f), body) in sh.core.functions.iter_mut().zip(bodies) {
        f.body = body;
    }
    if !sh.errors.is_empty() {
        return Err(sh.errors);
    }
    Ok(Module {
        core: sh.core,
        functions: sh.functions,
        stage: crate::ir::Stage::Flow,
    })
}

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

/// The hint of temporaries. A `let` renames the temporary its value ends in to the local's name.
const TMP: &str = "t";

/// The functions generated per type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Derive {
    /// `show$T(x: T) -> String`: spec 10.6 (a String inside is quoted).
    Show,
    /// `eq$T(a: T, b: T) -> Bool`: structural equality.
    Eq,
}

/// What all functions share while converting.
struct Shared {
    core: emela_core::Module,
    functions: Arena<Function>,
    /// Core function → Flow IR function, by Core index.
    fn_map: Vec<FnId>,
    /// Function names in use.
    names: FxHashSet<String>,
    derived: FxHashMap<(Derive, TypeId), FnId>,
    /// Generated functions declared but not built yet.
    pending: Vec<(Derive, TypeId, FnId)>,
    errors: Vec<FromCoreError>,
}

impl Shared {
    /// The generated function for `ty`, declared on first request and built later.
    fn derived(&mut self, kind: Derive, ty: TypeId) -> FnId {
        if let Some(&f) = self.derived.get(&(kind, ty)) {
            return f;
        }
        let prefix = match kind {
            Derive::Show => "show",
            Derive::Eq => "eq",
        };
        let base = format!("{prefix}${}", core_ty(&self.core, ty));
        let mut name = base.clone();
        let mut n = 1;
        while !self.names.insert(name.clone()) {
            name = format!("{base}${n}");
            n += 1;
        }
        let id = self.functions.alloc(Function::new(&name));
        self.derived.insert((kind, ty), id);
        self.pending.push((kind, ty, id));
        id
    }
}

/// The value of a converted expression: its operand (`None` without representation) and its
/// Core type.
#[derive(Debug, Clone)]
struct Val {
    op: Option<Operand>,
    ty: TypeId,
}

/// Control does not continue after the expression: the current sequence has its terminator.
#[derive(Debug, Clone, Copy)]
struct Div;

type R = Result<Val, Div>;

/// A sequence being built.
struct Open {
    stmts: Vec<StmtId>,
    term: Option<Term>,
    /// The length of [`Lower::cache`] when the sequence was opened.
    cache_mark: usize,
}

/// Values read out of others while matching patterns ([`pat`]), shared by the rows of a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Tuple(VarId, usize),
    /// Base, constructor index, field index.
    Field(VarId, usize, usize),
    Head(VarId),
    Tail(VarId),
}

/// Builds one Flow IR function.
struct Lower<'s> {
    sh: &'s mut Shared,
    f: Function,
    /// The sequences being built, innermost last. The first is the body.
    open: Vec<Open>,
    locals: FxHashMap<Local, Option<Operand>>,
    /// The blocks of the enclosing `escape`s of the current function or closure body, innermost
    /// last. Empty: failures leave the body.
    fail: Vec<LabelId>,
    /// Whether the current function or closure body may fail.
    fails: bool,
    /// Values read while matching, visible in the current sequence (see [`Open::cache_mark`]).
    cache: Vec<(Key, VarId)>,
}

/// A piece of a string to build.
enum Piece {
    Text(String),
    Op(Operand),
}

impl<'s> Lower<'s> {
    fn new(sh: &'s mut Shared, name: &str) -> Self {
        let mut l = Lower {
            sh,
            f: Function::new(name),
            open: Vec::new(),
            locals: FxHashMap::default(),
            fail: Vec::new(),
            fails: false,
            cache: Vec::new(),
        };
        l.open();
        l
    }

    /// The Flow IR function for a Core function. `body` is its body (kept apart from the module).
    fn function(sh: &'s mut Shared, id: emela_core::FnId, body: &Expr) -> Function {
        let cf = &sh.core.functions[id];
        let (name, params, external) = (cf.name.clone(), cf.params.clone(), cf.external.clone());
        let mut l = Lower::new(sh, &name);
        let cf = &l.sh.core.functions[id];
        l.f.exported = cf.exported;
        l.f.ret = cf.ret;
        l.f.fails = cf.fails;
        l.fails = cf.fails;
        l.f.suspends = cf.suspends;
        l.f.constant = cf.constant;
        for p in params {
            let v = l.local_var(p);
            l.f.params.extend(v);
        }
        if let Some(ext) = external {
            l.f.external = Some(ext);
            l.open.clear();
            return l.f;
        }
        if let Ok(v) = l.expr(body) {
            l.term(Term::Return(v.op));
        }
        l.finish()
    }

    fn finish(mut self) -> Function {
        debug_assert_eq!(self.open.len(), 1, "unbalanced sequences");
        let body = self.close();
        self.f.body = Some(body);
        captures::fill(&mut self.f);
        self.f
    }

    fn error(&mut self, message: String) {
        self.sh.errors.push(FromCoreError {
            function: self.f.name.clone(),
            message,
        });
    }

    // ---- Building ----

    fn open(&mut self) {
        self.open.push(Open {
            stmts: Vec::new(),
            term: None,
            cache_mark: self.cache.len(),
        });
    }

    /// Ends the innermost open sequence (with `yield` without value unless it has a terminator).
    fn close(&mut self) -> SeqId {
        let open = self.open.pop().expect("no open sequence");
        self.cache.truncate(open.cache_mark);
        self.f.seqs.alloc(Seq {
            stmts: open.stmts,
            term: open.term.unwrap_or(Term::Yield(None)),
        })
    }

    /// An empty sequence (`{}`).
    fn empty(&mut self) -> SeqId {
        self.open();
        self.close()
    }

    fn push(&mut self, stmt: Stmt) {
        let id = self.f.stmts.alloc(stmt);
        let open = self.open.last_mut().expect("no open sequence");
        debug_assert!(open.term.is_none(), "a statement after a terminator");
        open.stmts.push(id);
    }

    fn term(&mut self, term: Term) {
        let open = self.open.last_mut().expect("no open sequence");
        debug_assert!(open.term.is_none(), "a second terminator");
        open.term = Some(term);
    }

    fn repr(&self, ty: TypeId) -> Repr {
        repr_of(&self.sh.core, ty)
    }

    fn intern(&mut self, ty: Type) -> TypeId {
        self.sh.core.intern(ty)
    }

    fn ty(&self, ty: TypeId) -> &Type {
        self.sh.core.ty(ty)
    }

    fn subst(&mut self, ty: TypeId, args: &[TypeId]) -> TypeId {
        if args.is_empty() {
            return ty;
        }
        self.sh.core.types.subst(ty, args)
    }

    /// A new variable; the type must have a representation.
    fn var(&mut self, hint: &str, ty: TypeId) -> VarId {
        let repr = self.repr(ty);
        debug_assert!(repr != Repr::Void, "a variable without representation");
        self.f.vars.alloc(VarData {
            hint: if hint.is_empty() { "v" } else { hint }.to_owned(),
            repr,
            ty,
        })
    }

    /// A new variable if the type has a representation.
    fn new_var(&mut self, hint: &str, ty: TypeId) -> Option<VarId> {
        (self.repr(ty) != Repr::Void).then(|| self.var(hint, ty))
    }

    fn param(&mut self, hint: &str, ty: TypeId) -> VarId {
        let v = self.var(hint, ty);
        self.f.params.push(v);
        v
    }

    /// A variable for a Core local (a parameter or loop variable), mapped for its uses.
    fn local_var(&mut self, l: Local) -> Option<VarId> {
        let data = &self.sh.core.locals[l];
        let (hint, ty) = (data.hint.clone(), data.ty);
        let v = self.new_var(&hint, ty);
        self.locals.insert(l, v.map(Operand::Var));
        v
    }

    fn label(&mut self, hint: &str) -> LabelId {
        self.f.labels.alloc(LabelData {
            hint: hint.to_owned(),
        })
    }

    /// `dst = value`, with a destination if the type has a representation.
    fn let_(&mut self, hint: &str, ty: TypeId, value: Value) -> Option<VarId> {
        let dst = self.new_var(hint, ty);
        self.push(Stmt::Let {
            dst,
            value,
            on_defect: None,
        });
        dst
    }

    /// Whether a value of type `from` needs [`Value::Coerce`] to be used at type `to`: the types
    /// differ other than in set type arguments (`Exit[Int, ()]` and `Exit[Int, {}]` are the same
    /// values), and `to` has a representation.
    fn differs(&mut self, from: TypeId, to: TypeId) -> bool {
        if from == to || self.repr(to) == Repr::Void {
            return false;
        }
        let types = &mut self.sh.core.types;
        types.erase_sets(from) != types.erase_sets(to)
    }

    /// A read (a field, an element, a list head) that has no effect: nothing for a type without
    /// representation.
    fn get(&mut self, hint: &str, ty: TypeId, value: Value) -> Option<VarId> {
        (self.repr(ty) != Repr::Void).then(|| self.assign(hint, ty, value))
    }

    /// `let_` for a type with a representation.
    fn assign(&mut self, hint: &str, ty: TypeId, value: Value) -> VarId {
        let v = self.var(hint, ty);
        self.push(Stmt::Let {
            dst: Some(v),
            value,
            on_defect: None,
        });
        v
    }

    /// A temporary bound to `value`, as the expression's value.
    fn emit(&mut self, ty: TypeId, value: Value) -> R {
        let dst = self.let_(TMP, ty, value);
        self.val(dst.map(Operand::Var), ty)
    }

    /// The value, or `unreachable` if its type is `Never`.
    fn val(&mut self, op: Option<Operand>, ty: TypeId) -> R {
        if ty == TypeId::NEVER {
            self.term(Term::Unreachable);
            return Err(Div);
        }
        Ok(Val { op, ty })
    }

    /// The value as a variable (constants are bound to one). `None` without representation.
    fn as_var(&mut self, v: &Val) -> Option<VarId> {
        match &v.op {
            Some(Operand::Var(x)) => Some(*x),
            Some(Operand::Const(c)) => Some(self.assign(TMP, v.ty, Value::Const(c.clone()))),
            None => None,
        }
    }

    /// `as_var` for a value that must have a representation.
    fn must_var(&mut self, v: &Val, what: &str) -> VarId {
        match self.as_var(v) {
            Some(x) => x,
            None => {
                self.error(format!("{what} has no representation"));
                self.assign(TMP, TypeId::ANY, Value::Const(Const::Int(0)))
            }
        }
    }

    /// Where a failure goes from here.
    fn on_fail(&self) -> OnFail {
        match self.fail.last() {
            Some(&l) => OnFail::Break(l),
            None => OnFail::Propagate,
        }
    }

    /// The terminator passing the error value to where failures go from here.
    fn fail_term(&self, error: Operand) -> Term {
        match self.fail.last() {
            Some(&label) => Term::Break {
                label,
                value: Some(error),
            },
            None => Term::Fail(error),
        }
    }

    // ---- Expressions ----

    /// Converts `e`, appending its statements to the current sequence.
    fn expr(&mut self, e: &Expr) -> R {
        // `let` chains are walked in a loop, not by recursion.
        let mut e = e;
        loop {
            match e {
                Expr::Let { var, value, body } => {
                    self.bind(*var, value)?;
                    e = body;
                }
                Expr::With { binds, body } if binds.iter().all(|b| b.release.is_none()) => {
                    for b in binds {
                        self.bind(b.var, &b.value)?;
                    }
                    e = body;
                }
                _ => return self.node(e),
            }
        }
    }

    /// Binds the local to the value. A temporary the value ends in takes the local's name.
    fn bind(&mut self, var: Local, value: &Expr) -> Result<(), Div> {
        let mark = self.f.vars.len();
        let v = self.expr(value)?;
        let local = &self.sh.core.locals[var];
        let (hint, ty) = (local.hint.clone(), local.ty);
        let op = match v.op {
            // A value of another type (a function value with a smaller E or R) is coerced, so
            // that the variable has the local's type.
            Some(Operand::Var(x)) if self.differs(self.f.vars[x].ty, ty) => {
                Some(Operand::Var(self.assign(&hint, ty, Value::Coerce(x))))
            }
            Some(Operand::Var(x)) => {
                if ix(x) >= mark && self.f.vars[x].hint == TMP {
                    self.f.vars[x].hint = hint;
                }
                Some(Operand::Var(x))
            }
            op => op,
        };
        self.locals.insert(var, op);
        Ok(())
    }

    fn node(&mut self, e: &Expr) -> R {
        match e {
            Expr::Lit(lit) => Ok(lit_val(lit)),
            Expr::Var(l) => {
                let ty = self.sh.core.locals[*l].ty;
                let op = match self.locals.get(l) {
                    Some(op) => op.clone(),
                    None => {
                        let hint = self.sh.core.locals[*l].hint.clone();
                        self.error(format!("local {hint}%{} is used unbound", ix(*l)));
                        None
                    }
                };
                self.val(op, ty)
            }
            Expr::Fn { f, .. } => {
                let ty = self.fn_value_type(*f);
                let target = self.sh.fn_map[ix(*f)];
                self.emit(ty, Value::FnRef(target))
            }
            Expr::Let { .. } => self.expr(e),
            Expr::With { binds, body } => self.with_(binds, body),
            Expr::Call {
                callee,
                args,
                suspends,
                ..
            } => self.call(callee, args, *suspends),
            Expr::Builtin { op, ty_args, args } => self.builtin(*op, ty_args, args),
            Expr::Ctor {
                ctor,
                ty_args,
                args,
            } => {
                let args = self.slots(args)?;
                let ty = self.intern(Type::Enum(ctor.enum_id, ty_args.clone()));
                self.emit(ty, Value::Make { ctor: *ctor, args })
            }
            Expr::Field { base, ctor, index } => {
                let b = self.expr(base)?;
                let Type::Enum(_, args) = self.ty(b.ty).clone() else {
                    self.error("a field of a value that is not an enum".into());
                    return Ok(Val {
                        op: None,
                        ty: TypeId::UNIT,
                    });
                };
                let field = self.sh.core.variant(*ctor).tys[*index];
                let ty = self.subst(field, &args);
                if self.repr(ty) == Repr::Void {
                    return self.val(None, ty);
                }
                let base = self.must_var(&b, "a field's base");
                self.emit(
                    ty,
                    Value::Field {
                        base,
                        ctor: *ctor,
                        index: *index,
                    },
                )
            }
            Expr::Tuple(es) => {
                let mut ops = Vec::with_capacity(es.len());
                let mut tys = Vec::with_capacity(es.len());
                for e in es {
                    let v = self.expr(e)?;
                    ops.push(v.op.unwrap_or(Operand::Const(Const::Unit)));
                    tys.push(v.ty);
                }
                let ty = self.intern(Type::Tuple(tys));
                self.emit(ty, Value::Tuple(ops))
            }
            Expr::List { elem, elems, tail } => {
                let elems = self.slots(elems)?;
                let tail = match tail {
                    Some(t) => self.expr(t)?.op,
                    None => None,
                };
                let ty = self.intern(Type::List(*elem));
                if elems.is_empty()
                    && let Some(t) = tail
                {
                    return self.val(Some(t), ty);
                }
                self.emit(ty, Value::List { elems, tail })
            }
            Expr::If { cond, then, else_ } => {
                let c = self.expr(cond)?;
                let cond = c.op.unwrap_or(Operand::Const(Const::Bool(false)));
                let (then, t) = self.branch(then);
                let (else_, f) = self.branch(else_);
                let ty = t.or(f);
                let dst = ty.and_then(|t| self.new_var(TMP, t));
                self.push(Stmt::If {
                    dst,
                    cond,
                    then,
                    else_,
                });
                self.joined(dst, ty)
            }
            Expr::Match { scrutinee, arms } => self.match_(scrutinee, arms),
            Expr::Binary { op, ty, lhs, rhs } => self.binary(*op, *ty, lhs, rhs),
            Expr::Unary { op, ty, operand } => {
                let v = self.expr(operand)?;
                let a = v.op.unwrap_or(Operand::Const(Const::Int(0)));
                let (op, pty, rty) = match op {
                    UnOp::Neg => (PrimOp::Neg, prim_ty(*ty), v.ty),
                    UnOp::Not => (PrimOp::Not, PrimTy::I32, TypeId::BOOL),
                };
                self.emit(
                    rty,
                    Value::Prim {
                        op,
                        ty: pty,
                        args: vec![a],
                    },
                )
            }
            Expr::Lambda {
                params, ty, body, ..
            } => self.lambda(params, *ty, body),
            Expr::Concat(parts) => self.interpolate(parts),
            Expr::Loop { vars, inits, body } => {
                let mut init_ops = Vec::new();
                for i in inits {
                    init_ops.extend(self.expr(i)?.op);
                }
                let mut loop_vars = Vec::new();
                for &l in vars {
                    loop_vars.extend(self.local_var(l));
                }
                let (body, ty) = self.branch(body);
                let dst = ty.and_then(|t| self.new_var(TMP, t));
                self.push(Stmt::Loop {
                    dst,
                    vars: loop_vars,
                    inits: init_ops,
                    body,
                });
                self.joined(dst, ty)
            }
            Expr::Recur(args) => {
                let mut ops = Vec::new();
                for a in args {
                    ops.extend(self.expr(a)?.op);
                }
                self.term(Term::Recur(ops));
                Err(Div)
            }
            Expr::Assert(a) => self.assert(a),
            Expr::Fail(e) => {
                let v = self.expr(e)?;
                let error = v.op.unwrap_or(Operand::Const(Const::Unit));
                let t = self.fail_term(error);
                self.term(t);
                Err(Div)
            }
            Expr::Escape { body, arms } => self.escape(body, arms),
            Expr::Use { ctx, effect } => {
                let c = self.expr(ctx)?;
                let ev = self.must_var(&c, "an evidence table");
                let ty = self.intern(Type::Handler(*effect));
                self.emit(
                    ty,
                    Value::EvLookup {
                        ev,
                        effect: *effect,
                    },
                )
            }
            Expr::Provide { base, bindings } => {
                let base = match base {
                    Some(b) => {
                        let v = self.expr(b)?;
                        Some(self.must_var(&v, "an evidence table"))
                    }
                    None => None,
                };
                let mut bs = Vec::with_capacity(bindings.len());
                for (effect, h) in bindings {
                    let v = self.expr(h)?;
                    bs.push((*effect, self.must_var(&v, "a handler")));
                }
                self.emit(TypeId::EVIDENCE, Value::EvExtend { base, bindings: bs })
            }
            Expr::Handler { name, effect, ops } => self.handler(name, *effect, ops),
            Expr::Fiber { op, args } => self.fiber(*op, args),
        }
    }

    /// Converts `e` into a new sequence that yields its value. Also returns its type, `None` if
    /// control does not reach the end.
    fn branch(&mut self, e: &Expr) -> (SeqId, Option<TypeId>) {
        self.open();
        let ty = match self.expr(e) {
            Ok(v) => {
                self.term(Term::Yield(v.op));
                Some(v.ty)
            }
            Err(Div) => None,
        };
        (self.close(), ty)
    }

    /// The value of a compound statement just pushed: `dst` of type `ty`, or `unreachable` if no
    /// branch reaches its end.
    fn joined(&mut self, dst: Option<VarId>, ty: Option<TypeId>) -> R {
        match ty {
            Some(ty) => Ok(Val {
                op: dst.map(Operand::Var),
                ty,
            }),
            None => {
                self.term(Term::Unreachable);
                Err(Div)
            }
        }
    }

    /// Values for container slots: `()` gets the placeholder.
    fn slots(&mut self, es: &[Expr]) -> Result<Vec<Operand>, Div> {
        let mut ops = Vec::with_capacity(es.len());
        for e in es {
            ops.push(self.expr(e)?.op.unwrap_or(Operand::Const(Const::Unit)));
        }
        Ok(ops)
    }

    /// The type of a top-level function used as a value (`Typer::fn_value`).
    fn fn_value_type(&mut self, f: emela_core::FnId) -> TypeId {
        let core = &self.sh.core;
        let func = &core.functions[f];
        let ev = core.takes_ev(f);
        let declared = &func.params[..func.params.len() - usize::from(ev)];
        let fn_ty = FnType {
            params: declared.iter().map(|&p| core.locals[p].ty).collect(),
            ret: func.ret,
            fails: func.fails,
            suspends: func.suspends,
            ev,
            sets: None,
        };
        self.intern(Type::Fn(fn_ty))
    }

    fn call(&mut self, callee: &CoreCallee, args: &[Expr], suspends: bool) -> R {
        let (callee, ret, fails) = match callee {
            CoreCallee::Direct { f, .. } => {
                let func = &self.sh.core.functions[*f];
                (Callee::Direct(self.sh.fn_map[ix(*f)]), func.ret, func.fails)
            }
            CoreCallee::Indirect(c) => {
                let v = self.expr(c)?;
                let Type::Fn(ft) = self.ty(v.ty).clone() else {
                    self.error("a call of a value that is not a function".into());
                    return Ok(Val {
                        op: None,
                        ty: TypeId::UNIT,
                    });
                };
                let c = self.must_var(&v, "a function value");
                (Callee::Indirect(c), ft.ret, ft.fails)
            }
            CoreCallee::Op {
                handler,
                effect,
                op,
                ty_args,
            } => {
                let v = self.expr(handler)?;
                let handler = self.must_var(&v, "a handler");
                let def = &self.sh.core.effects[*effect].ops[*op];
                let (ret, fails) = (def.ret, def.fails);
                let ret = self.subst(ret, ty_args);
                (
                    Callee::Op {
                        handler,
                        effect: *effect,
                        op: *op,
                    },
                    ret,
                    fails,
                )
            }
        };
        let mut ops = Vec::with_capacity(args.len());
        for a in args {
            ops.extend(self.expr(a)?.op);
        }
        let on_fail = fails.then(|| self.on_fail());
        self.emit(
            ret,
            Value::Call {
                callee,
                args: ops,
                suspends,
                on_fail,
            },
        )
    }

    fn builtin(&mut self, op: Builtin, ty_args: &[TypeId], args: &[Expr]) -> R {
        match op {
            Builtin::Dbg => {
                let v = self.expr(&args[0])?;
                let shown = self.show(ty_args[0], v.op.clone());
                self.let_(
                    TMP,
                    TypeId::UNIT,
                    Value::Runtime {
                        op: RuntimeOp::DebugPrint,
                        args: vec![shown],
                    },
                );
                Ok(v)
            }
            Builtin::Panic => {
                let v = self.expr(&args[0])?;
                let msg = v.op.unwrap_or(Operand::Const(Const::String(String::new())));
                self.term(Term::Defect(msg));
                Err(Div)
            }
            Builtin::Todo => {
                self.term(Term::Defect(Operand::Const(Const::String(
                    "not yet implemented".into(),
                ))));
                Err(Div)
            }
            Builtin::CellNew | Builtin::CellGet | Builtin::CellSet => {
                // A cell is a container: its `()` contents keep a slot.
                let args = self.slots(args)?;
                let content = ty_args[0];
                let ty = match op {
                    Builtin::CellNew => self.intern(Type::Cell(content)),
                    Builtin::CellGet => content,
                    _ => TypeId::UNIT,
                };
                self.emit(ty, Value::Builtin { op, args })
            }
            _ => {
                let mut ops = Vec::with_capacity(args.len());
                for a in args {
                    ops.extend(self.expr(a)?.op);
                }
                let ty = self.sig_type(op.info().ret, ty_args);
                self.emit(ty, Value::Builtin { op, args: ops })
            }
        }
    }

    fn sig_type(&mut self, sig: Sig, ty_args: &[TypeId]) -> TypeId {
        match sig {
            Sig::Int => TypeId::INT,
            Sig::Int64 => TypeId::INT64,
            Sig::Float => TypeId::FLOAT,
            Sig::Bool => TypeId::BOOL,
            Sig::String => TypeId::STRING,
            Sig::Unit => TypeId::UNIT,
            Sig::Never => TypeId::NEVER,
            Sig::List(t) => {
                let t = self.sig_type(*t, ty_args);
                self.intern(Type::List(t))
            }
            Sig::Option(t) => {
                let t = self.sig_type(*t, ty_args);
                let option = self.sh.core.option_enum();
                self.intern(Type::Enum(option, vec![t]))
            }
            Sig::Param(i) => ty_args.get(i as usize).copied().unwrap_or(TypeId::ANY),
        }
    }

    fn binary(&mut self, op: BinOp, ty: OpTy, lhs: &Expr, rhs: &Expr) -> R {
        if let BinOp::And | BinOp::Or = op {
            let a = self.expr(lhs)?;
            let cond = a.op.unwrap_or(Operand::Const(Const::Bool(false)));
            let short = Operand::Const(Const::Bool(op == BinOp::Or));
            let (right, _) = self.branch(rhs);
            self.open();
            self.term(Term::Yield(Some(short)));
            let shorted = self.close();
            let (then, else_) = if op == BinOp::And {
                (right, shorted)
            } else {
                (shorted, right)
            };
            let dst = self.var(TMP, TypeId::BOOL);
            self.push(Stmt::If {
                dst: Some(dst),
                cond,
                then,
                else_,
            });
            return self.val(Some(Operand::Var(dst)), TypeId::BOOL);
        }
        let a = self.expr(lhs)?;
        let b = self.expr(rhs)?;
        let (op, ty) = self.binop(op, ty, &a, &b);
        self.val(Some(op), ty)
    }

    /// A binary operation other than `&&` / `||` on converted operands, and its type.
    fn binop(&mut self, op: BinOp, ty: OpTy, a: &Val, b: &Val) -> (Operand, TypeId) {
        let zero = Operand::Const(Const::Int(0));
        let (x, y) = (
            a.op.clone().unwrap_or(zero.clone()),
            b.op.clone().unwrap_or(zero),
        );
        if ty == OpTy::Structural {
            let eq = self.eq(a.ty, a.op.clone(), b.op.clone());
            let r = match op {
                BinOp::Eq => eq,
                _ => Operand::Var(self.assign(
                    TMP,
                    TypeId::BOOL,
                    Value::Prim {
                        op: PrimOp::Not,
                        ty: PrimTy::I32,
                        args: vec![eq],
                    },
                )),
            };
            return (r, TypeId::BOOL);
        }
        let prim = match op {
            BinOp::Add => PrimOp::Add,
            BinOp::Sub => PrimOp::Sub,
            BinOp::Mul => PrimOp::Mul,
            BinOp::Div => PrimOp::Div,
            BinOp::Rem => PrimOp::Rem,
            BinOp::Eq => PrimOp::Eq,
            BinOp::Ne => PrimOp::Ne,
            BinOp::Lt => PrimOp::Lt,
            BinOp::Le => PrimOp::Le,
            BinOp::Gt => PrimOp::Gt,
            BinOp::Ge => PrimOp::Ge,
            BinOp::And | BinOp::Or => unreachable!("short-circuiting operators are branches"),
        };
        let rty = if prim.is_comparison() {
            TypeId::BOOL
        } else {
            a.ty
        };
        let v = self.assign(
            TMP,
            rty,
            Value::Prim {
                op: prim,
                ty: prim_ty(ty),
                args: vec![x, y],
            },
        );
        (Operand::Var(v), rty)
    }

    fn lambda(&mut self, params: &[Local], ty: TypeId, body: &Expr) -> R {
        let Type::Fn(ft) = self.ty(ty).clone() else {
            self.error("a lambda whose type is not a function type".into());
            return Ok(Val {
                op: None,
                ty: TypeId::UNIT,
            });
        };
        // A closure body has its own failure target.
        let fail = std::mem::take(&mut self.fail);
        let fails = std::mem::replace(&mut self.fails, ft.fails);
        let mut ps = Vec::with_capacity(params.len());
        for &p in params {
            ps.extend(self.local_var(p));
        }
        self.open();
        if let Ok(v) = self.expr(body) {
            self.term(Term::Return(v.op));
        }
        let body = self.close();
        self.fail = fail;
        self.fails = fails;
        let dst = self.var("f", ty);
        // A closure whose type says it may suspend is made suspending even if its body does not:
        // calls of a function value follow its type.
        self.push(Stmt::Let {
            dst: Some(dst),
            value: Value::Closure(Box::new(Closure {
                params: ps,
                captures: Vec::new(),
                body,
                suspends: ft.suspends,
            })),
            on_defect: None,
        });
        Ok(Val {
            op: Some(Operand::Var(dst)),
            ty,
        })
    }

    fn handler(&mut self, name: &str, effect: EffectId, ops: &[Expr]) -> R {
        let mut vs = Vec::with_capacity(ops.len());
        for op in ops {
            let v = self.expr(op)?;
            vs.push(self.must_var(&v, "an operation's implementation"));
        }
        let ty = self.intern(Type::Handler(effect));
        self.emit(
            ty,
            Value::Handler {
                name: name.to_owned(),
                effect,
                ops: vs,
            },
        )
    }

    fn fiber(&mut self, op: FiberOp, args: &[Expr]) -> R {
        let mut ops = Vec::with_capacity(args.len());
        let mut first = None;
        for a in args {
            let v = self.expr(a)?;
            first.get_or_insert(v.ty);
            ops.push(v.op.unwrap_or(Operand::Const(Const::Unit)));
        }
        let ty = self.fiber_result(op, first);
        let on_fail = op.fails().then(|| self.on_fail());
        self.emit(
            ty,
            Value::Fiber {
                op,
                args: ops,
                on_fail,
            },
        )
    }

    /// The type of a concurrency operation (`Typer::fiber_result`). `first` is the type of its
    /// first argument.
    fn fiber_result(&mut self, op: FiberOp, first: Option<TypeId>) -> TypeId {
        let first = first.unwrap_or(TypeId::ANY);
        let fn_ret = |l: &Self, t: TypeId| match l.ty(t) {
            Type::Fn(f) => f.ret,
            Type::Never => TypeId::NEVER,
            _ => TypeId::ANY,
        };
        let enum_arg = |l: &Self, t: TypeId| match l.ty(t) {
            Type::Enum(_, args) => args.first().copied().unwrap_or(TypeId::ANY),
            Type::Never => TypeId::NEVER,
            _ => TypeId::ANY,
        };
        let exit_type = |l: &mut Self, exit: emela_core::EnumId, a: TypeId| {
            let mut args = vec![TypeId::UNIT; l.sh.core.enums[exit].params];
            if let Some(f) = args.first_mut() {
                *f = a;
            }
            l.intern(Type::Enum(exit, args))
        };
        match op {
            FiberOp::Scope { .. } => fn_ret(self, first),
            FiberOp::Fork | FiberOp::Deferred => TypeId::ANY,
            FiberOp::Join | FiberOp::DeferredAwait => enum_arg(self, first),
            FiberOp::Await { exit } => {
                let a = enum_arg(self, first);
                exit_type(self, exit, a)
            }
            FiberOp::Attempt { exit, .. } => {
                let a = fn_ret(self, first);
                exit_type(self, exit, a)
            }
            FiberOp::Interrupt => TypeId::UNIT,
            FiberOp::Complete => TypeId::BOOL,
            FiberOp::Poll => {
                let a = enum_arg(self, first);
                let option = self.sh.core.option_enum();
                self.intern(Type::Enum(option, vec![a]))
            }
            FiberOp::CauseKind { kind } => {
                let args = vec![TypeId::UNIT; self.sh.core.enums[kind].params];
                self.intern(Type::Enum(kind, args))
            }
            FiberOp::CauseToString => TypeId::STRING,
            FiberOp::CauseRaise => TypeId::NEVER,
        }
    }

    /// `with`: the bindings in order; each binding with release puts the rest inside `protect`,
    /// whose cleanup evaluates the release and then marks the handler value released (also when
    /// the release ends in a defect).
    fn with_(&mut self, binds: &[WithBind], body: &Expr) -> R {
        for (i, b) in binds.iter().enumerate() {
            self.bind(b.var, &b.value)?;
            let Some(release) = &b.release else {
                continue;
            };
            let handler = self.locals[&b.var].clone();
            self.open();
            let ty = match self.with_(&binds[i + 1..], body) {
                Ok(v) => {
                    self.term(Term::Yield(v.op));
                    Some(v.ty)
                }
                Err(Div) => None,
            };
            let protected = self.close();
            // The release cannot fail (spec 8.4), so it has no failure target.
            let fail = std::mem::take(&mut self.fail);
            self.open();
            self.open();
            let _ = self.expr(&release.body);
            let released = self.close();
            self.open();
            if let Some(h) = handler {
                self.let_(
                    TMP,
                    TypeId::UNIT,
                    Value::Runtime {
                        op: RuntimeOp::Retire,
                        args: vec![h, Operand::Const(Const::String(release.handler.clone()))],
                    },
                );
            }
            let retire = self.close();
            self.push(Stmt::Protect {
                dst: None,
                body: released,
                cleanup: retire,
            });
            let cleanup = self.close();
            self.fail = fail;
            let dst = ty.and_then(|t| self.new_var(TMP, t));
            self.push(Stmt::Protect {
                dst,
                body: protected,
                cleanup,
            });
            return self.joined(dst, ty);
        }
        self.expr(body)
    }

    fn assert(&mut self, a: &Assert) -> R {
        match &a.cond {
            AssertCond::Compare {
                op,
                op_ty,
                ty,
                lhs,
                rhs,
            } => {
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                let (test, _) = self.binop(*op, *op_ty, &l, &r);
                let then = self.empty();
                self.open();
                let left = self.show(*ty, l.op.clone());
                let right = self.show(*ty, r.op.clone());
                let head = if a.text.is_empty() {
                    format!("left {} right", op_text(*op))
                } else {
                    a.text.clone()
                };
                let msg = self.concat(vec![
                    Piece::Text(format!("assertion failed: {head}\n  left: ")),
                    Piece::Op(left),
                    Piece::Text("\n right: ".into()),
                    Piece::Op(right),
                ]);
                self.term(Term::Defect(msg));
                let else_ = self.close();
                self.push(Stmt::If {
                    dst: None,
                    cond: test,
                    then,
                    else_,
                });
            }
            AssertCond::Bool(c) => {
                let v = self.expr(c)?;
                let cond = v.op.unwrap_or(Operand::Const(Const::Bool(false)));
                let then = self.empty();
                self.open();
                let msg = if a.text.is_empty() {
                    "assertion failed".to_owned()
                } else {
                    format!("assertion failed: {}", a.text)
                };
                self.term(Term::Defect(Operand::Const(Const::String(msg))));
                let else_ = self.close();
                self.push(Stmt::If {
                    dst: None,
                    cond,
                    then,
                    else_,
                });
            }
        }
        Ok(Val {
            op: None,
            ty: TypeId::UNIT,
        })
    }

    /// String interpolation: the values left to right, then the string. A String is embedded as
    /// is; other values are shown.
    fn interpolate(&mut self, parts: &[StrPart]) -> R {
        let mut values = Vec::new();
        for p in parts {
            if let StrPart::Value(e, ty) = p {
                values.push((self.expr(e)?.op, *ty));
            }
        }
        let mut values = values.into_iter();
        let mut pieces = Vec::with_capacity(parts.len());
        for p in parts {
            match p {
                StrPart::Lit(text) => pieces.push(Piece::Text(text.clone())),
                StrPart::Value(..) => {
                    let (op, ty) = values.next().expect("one value per part");
                    let shown = match (ty, op) {
                        (TypeId::STRING, Some(op)) => op,
                        (ty, op) => self.show(ty, op),
                    };
                    pieces.push(Piece::Op(shown));
                }
            }
        }
        let s = self.concat(pieces);
        Ok(Val {
            op: Some(s),
            ty: TypeId::STRING,
        })
    }

    /// Concatenates the pieces with `String.append`, merging adjacent texts.
    fn concat(&mut self, pieces: Vec<Piece>) -> Operand {
        let mut ops: Vec<Operand> = Vec::new();
        let mut text: Option<String> = None;
        for p in pieces {
            match p {
                Piece::Text(t) | Piece::Op(Operand::Const(Const::String(t))) if t.is_empty() => {}
                Piece::Text(t) | Piece::Op(Operand::Const(Const::String(t))) => {
                    text.get_or_insert_with(String::new).push_str(&t);
                }
                Piece::Op(o) => {
                    if let Some(t) = text.take() {
                        ops.push(Operand::Const(Const::String(t)));
                    }
                    ops.push(o);
                }
            }
        }
        if let Some(t) = text {
            ops.push(Operand::Const(Const::String(t)));
        }
        let mut ops = ops.into_iter();
        let Some(mut acc) = ops.next() else {
            return Operand::Const(Const::String(String::new()));
        };
        for o in ops {
            acc = Operand::Var(self.assign(
                "s",
                TypeId::STRING,
                Value::Builtin {
                    op: Builtin::StringAppend,
                    args: vec![acc, o],
                },
            ));
        }
        acc
    }
}

fn lit_val(lit: &Lit) -> Val {
    let (c, ty) = match lit {
        Lit::Int(n) => (Const::Int(*n), TypeId::INT),
        Lit::Int64(n) => (Const::Int64(*n), TypeId::INT64),
        Lit::Float(x) => (Const::Float(*x), TypeId::FLOAT),
        Lit::Bool(b) => (Const::Bool(*b), TypeId::BOOL),
        Lit::String(s) => (Const::String(s.clone()), TypeId::STRING),
        Lit::Unit => {
            return Val {
                op: None,
                ty: TypeId::UNIT,
            };
        }
    };
    Val {
        op: Some(Operand::Const(c)),
        ty,
    }
}

fn lit_const(lit: &Lit) -> Option<Const> {
    lit_val(lit).op.map(|o| match o {
        Operand::Const(c) => c,
        Operand::Var(_) => unreachable!(),
    })
}

fn prim_ty(ty: OpTy) -> PrimTy {
    match ty {
        OpTy::Int | OpTy::Bool => PrimTy::I32,
        OpTy::Int64 => PrimTy::I64,
        OpTy::Float => PrimTy::F64,
        OpTy::String | OpTy::Structural => PrimTy::String,
    }
}

fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        _ => "?",
    }
}

/// Whether the pattern matches every value of its type (no test).
fn irrefutable(core: &emela_core::Module, p: &emela_core::Pat) -> bool {
    use emela_core::Pat;
    match p {
        Pat::Wild | Pat::Bind(_) | Pat::Lit(Lit::Unit) => true,
        Pat::Lit(_) => false,
        Pat::Tuple(ps) => ps.iter().all(|p| irrefutable(core, p)),
        Pat::Ctor { ctor, fields } => {
            core.enums[ctor.enum_id].variants.len() == 1
                && fields.iter().all(|p| irrefutable(core, p))
        }
        Pat::List { elems, rest } => {
            elems.is_empty() && rest.as_deref().is_some_and(|r| irrefutable(core, r))
        }
    }
}
