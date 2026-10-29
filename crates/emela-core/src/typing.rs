//! The types of IR expressions ([`Typer::type_of`]).
//!
//! Every expression's type follows from the IR alone: locals, functions, lambdas, constructors,
//! lists, builtins and operations carry what cannot be derived (`LocalData::ty`, `Function::ret`,
//! `Expr::Lambda::ty`, `OpDef`, the `ty_args` of calls, function values, constructors and builtins,
//! `Expr::List::elem`), and the rest is computed from them.
//!
//! The type of an `if`, `match` or `escape` is that of its first branch that is not `Never` (a
//! branch that never returns, like `fail` or `recur`, takes any type), and `Never` if all are.
//! An operation call (`Callee::Op`) has the operation's return type (`OpDef::ret`) with its
//! `ty_args` substituted. The handles `FiberOp::Fork` and `FiberOp::Deferred` return are
//! `Type::Any` (their types differ per use).
//!
//! # Cost
//!
//! [`Typer::type_of`] visits only the nodes that decide the type: the bodies of `let`, `with` and
//! `loop`, the branches of `if` / `match` / `escape` up to the first that is not `Never`, the
//! callee of an indirect call, the base of a field access, the elements of a tuple and the first
//! argument of some concurrency operations. It never visits `let` values, call arguments, guards or
//! lambda bodies. A `let` / `with` chain is walked once: the type found at its end is remembered
//! for every node of the chain, so asking at each node of a chain of length n costs O(n) in total,
//! not O(n²). A single call costs O(the nodes it visits), which apart from `let` chains is the
//! nesting depth of branches in tail position. A backend that needs the type of every node can also
//! compute types bottom-up as it walks, as [`crate::verify`] does.

use std::borrow::Cow;

use rustc_hash::FxHashMap;

use crate::intrinsics::{Builtin, Sig};
use crate::ir::*;

/// Computes types of the expressions of one module.
///
/// Types the module's table does not have yet (a tuple type no local has, say) are added to a copy
/// of the table, made the first time one is needed. Their IDs extend the module's, so IDs of the
/// module stay valid; read the structure with [`Typer::ty`] rather than [`Module::ty`].
pub struct Typer<'a> {
    m: &'a Module,
    types: Cow<'a, TypeTable>,
    /// Types of `let` / `with` nodes found by `type_of`, by address. Expressions are borrowed for
    /// `'a`, so none is moved or freed while the typer lives, and an address names one node.
    chains: FxHashMap<*const Expr, TypeId>,
}

impl<'a> Typer<'a> {
    pub fn new(m: &'a Module) -> Self {
        Typer {
            m,
            types: Cow::Borrowed(&m.types),
            chains: FxHashMap::default(),
        }
    }

    pub fn module(&self) -> &'a Module {
        self.m
    }

    /// The structure of the type, including types added by this typer.
    pub fn ty(&self, id: TypeId) -> &Type {
        &self.types[id]
    }

    /// The type table: the module's, plus the types added by this typer.
    pub fn types(&self) -> &TypeTable {
        &self.types
    }

    /// The table with the types this typer added, to replace the module's (`Module::types`; the
    /// IDs extend it). `None` if it added none.
    pub fn into_types(self) -> Option<TypeTable> {
        match self.types {
            Cow::Borrowed(_) => None,
            Cow::Owned(t) => Some(t),
        }
    }

    /// The ID of the type, adding it to the typer's copy of the table if new.
    pub fn intern(&mut self, ty: Type) -> TypeId {
        match self.types.get(&ty) {
            Some(id) => id,
            None => self.types.to_mut().intern(ty),
        }
    }

    /// Replaces `Type::Param(i)` with `args[i]`. A parameter without an argument is left as is (only
    /// in malformed IR, which [`crate::verify`] reports).
    pub fn subst(&mut self, ty: TypeId, args: &[TypeId]) -> TypeId {
        if args.is_empty() {
            return ty;
        }
        subst_type(self, ty, args)
    }

    pub fn subst_all(&mut self, tys: &[TypeId], args: &[TypeId]) -> Vec<TypeId> {
        tys.iter().map(|&t| self.subst(t, args)).collect()
    }

    /// The type of the expression. See the module documentation for what it visits.
    pub fn type_of(&mut self, e: &'a Expr) -> TypeId {
        let mut e = e;
        let mut chain = Vec::new();
        let ty = loop {
            match e {
                Expr::Let { body, .. } | Expr::With { body, .. } => {
                    if let Some(&t) = self.chains.get(&std::ptr::from_ref(e)) {
                        break t;
                    }
                    chain.push(std::ptr::from_ref(e));
                    e = body;
                }
                _ => break self.node_type(e),
            }
        };
        for node in chain {
            self.chains.insert(node, ty);
        }
        ty
    }

    /// `type_of` for an expression that is not a `let` or `with`.
    fn node_type(&mut self, e: &'a Expr) -> TypeId {
        let m = self.m;
        match e {
            Expr::Lit(lit) => lit_type(lit),
            Expr::Var(l) => m.locals[*l].ty,
            Expr::Fn { f, ty_args } => self.fn_value(*f, ty_args),
            Expr::Let { .. } | Expr::With { .. } => self.type_of(e),
            Expr::Call { callee, .. } => match callee {
                Callee::Direct { f, ty_args } => self.subst(m.functions[*f].ret, ty_args),
                Callee::Indirect(c) => {
                    let t = self.type_of(c);
                    self.fn_ret(t)
                }
                Callee::Op {
                    effect,
                    op,
                    ty_args,
                    ..
                } => self.op_sig(*effect, *op, ty_args).1,
            },
            Expr::Builtin { op, ty_args, .. } => self.builtin_sig(*op, ty_args).1,
            Expr::Ctor { ctor, ty_args, .. } => {
                self.intern(Type::Enum(ctor.enum_id, ty_args.clone()))
            }
            Expr::Field { base, ctor, index } => {
                let b = self.type_of(base);
                self.field_type(b, *ctor, *index).unwrap_or(TypeId::ANY)
            }
            Expr::Tuple(es) => {
                let ts = es.iter().map(|e| self.type_of(e)).collect();
                self.intern(Type::Tuple(ts))
            }
            Expr::List { elem, .. } => self.intern(Type::List(*elem)),
            Expr::If { then, else_, .. } => {
                let t = self.type_of(then);
                if t == TypeId::NEVER {
                    self.type_of(else_)
                } else {
                    t
                }
            }
            Expr::Match { arms, .. } => self.first_arm(TypeId::NEVER, arms),
            Expr::Escape { body, arms } => {
                let t = self.type_of(body);
                self.first_arm(t, arms)
            }
            Expr::Binary { op, ty, .. } => bin_result(*op, *ty),
            Expr::Unary { op, ty, .. } => match op {
                UnOp::Neg => op_ty_type(*ty),
                UnOp::Not => TypeId::BOOL,
            },
            Expr::Lambda { ty, .. } => *ty,
            Expr::Concat(_) => TypeId::STRING,
            Expr::Loop { body, .. } => self.type_of(body),
            Expr::Recur(_) | Expr::Fail(_) => TypeId::NEVER,
            Expr::Assert(_) => TypeId::UNIT,
            Expr::Use { effect, .. } => {
                let h = self.m.handler_effect(*effect);
                self.intern(Type::Handler(h))
            }
            Expr::Handler { effect, .. } => self.intern(Type::Handler(*effect)),
            Expr::Provide { .. } => TypeId::EVIDENCE,
            Expr::Fiber { op, args } => {
                let first = match op {
                    FiberOp::Scope { .. }
                    | FiberOp::Join
                    | FiberOp::Await { .. }
                    | FiberOp::Attempt { .. }
                    | FiberOp::DeferredAwait
                    | FiberOp::Poll => args.first().map(|a| self.type_of(a)),
                    _ => None,
                };
                self.fiber_result(*op, first)
            }
        }
    }

    /// `first` if it is not `Never`, else the type of the first arm body that is not `Never`.
    fn first_arm(&mut self, first: TypeId, arms: &'a [Arm]) -> TypeId {
        if first != TypeId::NEVER {
            return first;
        }
        for arm in arms {
            let t = self.type_of(&arm.body);
            if t != TypeId::NEVER {
                return t;
            }
        }
        TypeId::NEVER
    }

    /// The return type of calling a value of type `f`: `Any` unless it is a function type (or
    /// `Never`).
    pub fn fn_ret(&self, f: TypeId) -> TypeId {
        match self.ty(f) {
            Type::Fn(f) => f.ret,
            Type::Never => TypeId::NEVER,
            _ => TypeId::ANY,
        }
    }

    /// The type of function `f` used as a value with type arguments `ty_args` (`Expr::Fn`).
    pub fn fn_value(&mut self, f: FnId, ty_args: &[TypeId]) -> TypeId {
        let m = self.m;
        let func = &m.functions[f];
        let ev = m.takes_ev(f);
        let declared = &func.params[..func.params.len() - usize::from(ev)];
        let params: Vec<TypeId> = declared.iter().map(|&p| m.locals[p].ty).collect();
        let fn_ty = FnType {
            params: self.subst_all(&params, ty_args),
            ret: self.subst(func.ret, ty_args),
            fails: func.fails,
            suspends: func.suspends,
            ev,
            // The flags are the function's own: they describe how it is called, which for a
            // generic function over-approximates the instantiation (`Function::sets`).
            sets: None,
        };
        self.intern(Type::Fn(fn_ty))
    }

    /// The parameter types (including the evidence table) and the return type of a direct call to
    /// `f` with type arguments `ty_args`.
    pub fn direct_sig(&mut self, f: FnId, ty_args: &[TypeId]) -> (Vec<TypeId>, TypeId) {
        let m = self.m;
        let func = &m.functions[f];
        let params: Vec<TypeId> = func.params.iter().map(|&p| m.locals[p].ty).collect();
        (
            self.subst_all(&params, ty_args),
            self.subst(func.ret, ty_args),
        )
    }

    /// The parameter and return types of a call to operation `op` of `effect` with type arguments
    /// `ty_args` (`Callee::Op`). `Any` if there is no such operation (only in malformed IR).
    pub fn op_sig(
        &mut self,
        effect: EffectId,
        op: usize,
        ty_args: &[TypeId],
    ) -> (Vec<TypeId>, TypeId) {
        let m = self.m;
        let Some(def) = m.effects[effect].ops.get(op) else {
            return (Vec::new(), TypeId::ANY);
        };
        (
            self.subst_all(&def.params, ty_args),
            self.subst(def.ret, ty_args),
        )
    }

    /// The parameter and return types of a builtin with type arguments `ty_args`.
    pub fn builtin_sig(&mut self, op: Builtin, ty_args: &[TypeId]) -> (Vec<TypeId>, TypeId) {
        let arg = ty_args.first().copied().unwrap_or(TypeId::ANY);
        match op {
            Builtin::CellNew => {
                let cell = self.intern(Type::Cell(arg));
                return (vec![arg], cell);
            }
            Builtin::CellGet => {
                let cell = self.intern(Type::Cell(arg));
                return (vec![cell], arg);
            }
            Builtin::CellSet => {
                let cell = self.intern(Type::Cell(arg));
                return (vec![cell, arg], TypeId::UNIT);
            }
            _ => {}
        }
        let info = op.info();
        let params = info
            .params
            .iter()
            .map(|s| self.sig_type(*s, ty_args))
            .collect();
        (params, self.sig_type(info.ret, ty_args))
    }

    fn sig_type(&mut self, sig: Sig, ty_args: &[TypeId]) -> TypeId {
        match sig {
            Sig::Int => TypeId::INT,
            Sig::Int64 => TypeId::INT64,
            Sig::Float => TypeId::FLOAT,
            Sig::Bool => TypeId::BOOL,
            Sig::String => TypeId::STRING,
            Sig::Bytes => TypeId::BYTES,
            Sig::Unit => TypeId::UNIT,
            Sig::Never => TypeId::NEVER,
            Sig::List(t) => {
                let t = self.sig_type(*t, ty_args);
                self.intern(Type::List(t))
            }
            Sig::Option(t) => {
                let t = self.sig_type(*t, ty_args);
                self.option(t)
            }
            Sig::Param(i) => ty_args.get(i as usize).copied().unwrap_or(TypeId::ANY),
        }
    }

    /// The field types of constructor `ctor` with the enum's type arguments `ty_args`.
    pub fn field_types(&mut self, ctor: CtorRef, ty_args: &[TypeId]) -> Vec<TypeId> {
        let tys = &self.m.variant(ctor).tys;
        self.subst_all(tys, ty_args)
    }

    /// The type of field `index` of a value of type `base` built with `ctor`. `None` unless `base`
    /// is an application of `ctor`'s enum with a field `index`.
    pub fn field_type(&mut self, base: TypeId, ctor: CtorRef, index: usize) -> Option<TypeId> {
        let Type::Enum(id, args) = self.ty(base) else {
            return None;
        };
        if *id != ctor.enum_id {
            return None;
        }
        let args = args.clone();
        let field = *self.m.variant(ctor).tys.get(index)?;
        Some(self.subst(field, &args))
    }

    /// The `i`th type argument of an enum type (`A` of `Fiber[A]`); `Any` if it is not one.
    pub fn enum_arg(&self, ty: TypeId, i: usize) -> TypeId {
        match self.ty(ty) {
            Type::Enum(_, args) => args.get(i).copied().unwrap_or(TypeId::ANY),
            Type::Never => TypeId::NEVER,
            _ => TypeId::ANY,
        }
    }

    /// The type of a concurrency operation. `first` is the type of its first argument, needed by
    /// `Scope`, `Join`, `Await`, `Attempt`, `DeferredAwait` and `Poll`.
    pub fn fiber_result(&mut self, op: FiberOp, first: Option<TypeId>) -> TypeId {
        let first = first.unwrap_or(TypeId::ANY);
        match op {
            // `Fiber.scope(body: fn(scope) -> A) -> A`.
            FiberOp::Scope { .. } => self.fn_ret(first),
            // Handles whose types differ per use (`Type::Any`).
            FiberOp::Fork | FiberOp::Deferred => TypeId::ANY,
            // `Fiber[A] -> A`, `Deferred[A] -> A`.
            FiberOp::Join | FiberOp::DeferredAwait => self.enum_arg(first, 0),
            // `Fiber[A] -> Exit[A, {}]`.
            FiberOp::Await { exit } => {
                let a = self.enum_arg(first, 0);
                self.exit_type(exit, a)
            }
            // `fn() -> A fails E` → `Exit[A, E]`.
            FiberOp::Attempt { exit, .. } => {
                let a = self.fn_ret(first);
                self.exit_type(exit, a)
            }
            FiberOp::Interrupt => TypeId::UNIT,
            FiberOp::Complete => TypeId::BOOL,
            // `Deferred[A] -> Option[A]`.
            FiberOp::Poll => {
                let a = self.enum_arg(first, 0);
                self.option(a)
            }
            FiberOp::CauseKind { kind } => {
                let args = vec![TypeId::UNIT; self.m.enums[kind].params];
                self.intern(Type::Enum(kind, args))
            }
            FiberOp::CauseToString => TypeId::STRING,
            FiberOp::CauseRaise { .. } => TypeId::NEVER,
        }
    }

    /// `Option[a]`; `Any` in a module without Option (only hand-built IR, as lowering adds it
    /// first).
    fn option(&mut self, a: TypeId) -> TypeId {
        match self.m.option {
            Some(option) => self.intern(Type::Enum(option, vec![a])),
            None => TypeId::ANY,
        }
    }

    /// `Exit[a, E]`, with the error set E (a set type argument) as the placeholder `()`.
    fn exit_type(&mut self, exit: EnumId, a: TypeId) -> TypeId {
        let n = self.m.enums[exit].params;
        let mut args = vec![TypeId::UNIT; n];
        if let Some(first) = args.first_mut() {
            *first = a;
        }
        self.intern(Type::Enum(exit, args))
    }
}

impl TypeStore for Typer<'_> {
    fn table(&self) -> &TypeTable {
        &self.types
    }

    fn intern_type(&mut self, ty: Type) -> TypeId {
        self.intern(ty)
    }
}

/// The type of a literal.
pub fn lit_type(lit: &Lit) -> TypeId {
    match lit {
        Lit::Int(_) => TypeId::INT,
        Lit::Int64(_) => TypeId::INT64,
        Lit::Float(_) => TypeId::FLOAT,
        Lit::Bool(_) => TypeId::BOOL,
        Lit::String(_) => TypeId::STRING,
        Lit::Unit => TypeId::UNIT,
    }
}

/// The operand type an `OpTy` names; `Any` for `Structural` (any type).
pub fn op_ty_type(ty: OpTy) -> TypeId {
    match ty {
        OpTy::Int => TypeId::INT,
        OpTy::Int64 => TypeId::INT64,
        OpTy::Float => TypeId::FLOAT,
        OpTy::Bool => TypeId::BOOL,
        OpTy::String => TypeId::STRING,
        OpTy::Structural => TypeId::ANY,
    }
}

/// The type of a binary operation: `Bool` for comparisons and `&&` / `||`, else the operand type.
pub fn bin_result(op: BinOp, ty: OpTy) -> TypeId {
    match op {
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => TypeId::BOOL,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => op_ty_type(ty),
    }
}
