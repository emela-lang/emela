//! Checks that a module is well typed ([`verify`]).
//!
//! Lowering is meant to produce only well-typed IR, so an error here is a compiler bug. The driver
//! runs it after lowering in debug builds, and lowering tests run it on every module they build.
//!
//! Types are computed bottom-up in one pass over each function (the rules of [`Typer`]), and each
//! place a value flows into a typed slot is checked: call arguments against the callee's
//! parameters after substituting `ty_args`, a `let` / `with` value against its local, a body
//! against the function's or lambda's return type, constructor arguments against the field types,
//! branches of `if` / `match` / `escape` against each other, `loop` initial values and `recur`
//! arguments against the loop variables, patterns against the scrutinee, and builtins and
//! concurrency operations against their signatures.
//!
//! A value of type `A` may flow into a slot of type `B` ([`Verifier::compatible`]) when they are
//! equal, and also when:
//!
//! - `A` is `Never` (the value never arrives);
//! - either is `Any` (Scope's fork and operation call results, `Type::Any`);
//! - `B` is `Evidence` and `A` is `()`: where there is no evidence table, `()` is passed (the callee
//!   does not use it, as the type checker has checked R);
//! - both are function types whose parameters and return types are compatible (parameters the
//!   other way round), whatever their `fails`, `suspends` and `ev`. These flags come from the E and
//!   R sets, and a set type argument has no IR type (it is the `()` placeholder in `ty_args`), so a
//!   generic signature's flags over-approximate those of an instantiation, and a value leaving a
//!   generic function keeps the generic flags. The flags that decide a calling convention are
//!   checked where they are used instead: `Expr::Call::fails` / `suspends` against the callee, and
//!   a lambda's parameters against its type's `ev`;
//! - both are lists, tuples or applications of the same enum with compatible components.
//!
//! What is not checked: that locals are bound before use and bound once, the arguments of operation
//! calls and the operations of handler values (`OpDef` has no types yet), and `Expr::Call::fails`
//! of direct calls (a generic function's `Function::fails` over-approximates its instantiations).

use std::fmt;

use crate::dump::type_name;
use crate::ir::*;
use crate::typing::{Typer, bin_result, lit_type, op_ty_type};

/// One ill-typed place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// The function's IR name.
    pub function: String,
    /// Where in the function body, outermost first (`let x%3 > arg 1`). Empty for the body itself.
    pub path: String,
    pub message: String,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.function, self.message)
        } else {
            write!(f, "{}: {}: {}", self.function, self.path, self.message)
        }
    }
}

/// Checks that every function body in the module is well typed. Returns all errors found.
pub fn verify(m: &Module) -> Result<(), Vec<VerifyError>> {
    let mut v = Verifier {
        t: Typer::new(m),
        function: String::new(),
        path: Vec::new(),
        loops: Vec::new(),
        errors: Vec::new(),
    };
    for (_, f) in m.functions.iter() {
        v.function.clone_from(&f.name);
        if f.constant && !f.params.is_empty() {
            v.error("a const takes no parameters".to_owned());
        }
        if f.external.is_some() {
            continue;
        }
        let t = v.expr(&f.body);
        v.expect(t, f.ret, "body");
    }
    if v.errors.is_empty() {
        Ok(())
    } else {
        Err(v.errors)
    }
}

/// A step of the path to an error.
#[derive(Clone, Copy)]
enum Seg {
    Name(&'static str),
    Index(&'static str, usize),
    Local(Local),
}

struct Verifier<'a> {
    t: Typer<'a>,
    function: String,
    path: Vec<Seg>,
    /// The variables of the enclosing `loop`s, innermost last. Lambdas start afresh.
    loops: Vec<&'a [Local]>,
    errors: Vec<VerifyError>,
}

impl<'a> Verifier<'a> {
    fn m(&self) -> &'a Module {
        self.t.module()
    }

    fn error(&mut self, message: String) {
        let m = self.m();
        let path: Vec<String> = self
            .path
            .iter()
            .map(|s| match s {
                Seg::Name(n) => (*n).to_owned(),
                Seg::Index(n, i) => format!("{n} {i}"),
                Seg::Local(l) => format!("let {}%{}", m.locals[*l].hint, u32::from(l.into_raw())),
            })
            .collect();
        self.errors.push(VerifyError {
            function: self.function.clone(),
            path: path.join(" > "),
            message,
        });
    }

    fn name(&self, ty: TypeId) -> String {
        type_name(self.m(), self.t.types(), ty)
    }

    /// Reports `what: expected B, found A` unless `actual` may flow into `expected`.
    fn expect(&mut self, actual: TypeId, expected: TypeId, what: &str) {
        if !self.compatible(actual, expected) {
            let message = format!(
                "{what}: expected {}, found {}",
                self.name(expected),
                self.name(actual)
            );
            self.error(message);
        }
    }

    /// Whether a value of type `a` may flow into a slot of type `b` (see the module
    /// documentation).
    fn compatible(&self, a: TypeId, b: TypeId) -> bool {
        if a == b || a == TypeId::NEVER || a == TypeId::ANY || b == TypeId::ANY {
            return true;
        }
        if b == TypeId::EVIDENCE && a == TypeId::UNIT {
            return true;
        }
        let all = |xs: &[TypeId], ys: &[TypeId]| {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(&x, &y)| self.compatible(x, y))
        };
        match (self.t.ty(a), self.t.ty(b)) {
            (Type::Fn(f), Type::Fn(g)) => {
                all(&g.params, &f.params) && self.compatible(f.ret, g.ret)
            }
            (Type::List(x), Type::List(y)) => self.compatible(*x, *y),
            (Type::Tuple(xs), Type::Tuple(ys)) => all(xs, ys),
            (Type::Enum(i, xs), Type::Enum(j, ys)) => i == j && all(xs, ys),
            _ => false,
        }
    }

    /// Visits `e` with `seg` on the path and returns its type.
    fn sub(&mut self, seg: Seg, e: &'a Expr) -> TypeId {
        self.path.push(seg);
        let t = self.expr(e);
        self.path.pop();
        t
    }

    /// Visits `e` with `seg` on the path and checks it against `expected`.
    fn sub_expect(&mut self, seg: Seg, e: &'a Expr, expected: TypeId) {
        self.path.push(seg);
        let t = self.expr(e);
        self.expect(t, expected, "value");
        self.path.pop();
    }

    /// Checks arguments against parameter types.
    fn args(&mut self, args: &'a [Expr], params: &[TypeId]) {
        if args.len() != params.len() {
            self.error(format!(
                "{} arguments for {} parameters",
                args.len(),
                params.len()
            ));
        }
        for (i, arg) in args.iter().enumerate() {
            match params.get(i) {
                Some(&p) => self.sub_expect(Seg::Index("arg", i), arg, p),
                None => {
                    self.sub(Seg::Index("arg", i), arg);
                }
            }
        }
    }

    /// The type of `e`, checking it and everything inside.
    fn expr(&mut self, e: &'a Expr) -> TypeId {
        let m = self.m();
        // A `let` / `with` chain is walked in a loop, so long chains do not deepen the stack.
        let mut e = e;
        loop {
            match e {
                Expr::Let { var, value, body } => {
                    self.path.push(Seg::Local(*var));
                    let t = self.expr(value);
                    self.expect(t, m.locals[*var].ty, "value");
                    self.path.pop();
                    e = body;
                }
                Expr::With { binds, body } => {
                    for b in binds {
                        self.path.push(Seg::Local(b.var));
                        let t = self.expr(&b.value);
                        self.expect(t, m.locals[b.var].ty, "value");
                        // The release's value is discarded.
                        if let Some(r) = &b.release {
                            self.sub(Seg::Name("release"), &r.body);
                        }
                        self.path.pop();
                    }
                    e = body;
                }
                _ => return self.node(e),
            }
        }
    }

    fn node(&mut self, e: &'a Expr) -> TypeId {
        let m = self.m();
        match e {
            Expr::Let { .. } | Expr::With { .. } => self.expr(e),
            Expr::Lit(lit) => lit_type(lit),
            Expr::Var(l) => m.locals[*l].ty,
            Expr::Fn { f, ty_args } => {
                self.check_ty_args(*f, ty_args);
                self.t.fn_value(*f, ty_args)
            }
            Expr::Call {
                callee,
                args,
                fails,
                suspends,
            } => self.call(callee, args, *fails, *suspends),
            Expr::Builtin { op, ty_args, args } => {
                let info = op.info();
                if ty_args.len() != info.ty_params as usize {
                    self.error(format!(
                        "{} type arguments for @{} with {}",
                        ty_args.len(),
                        info.path(),
                        info.ty_params
                    ));
                }
                let (params, ret) = self.t.builtin_sig(*op, ty_args);
                self.args(args, &params);
                ret
            }
            Expr::Ctor {
                ctor,
                ty_args,
                args,
            } => {
                let def = &m.enums[ctor.enum_id];
                if ty_args.len() != def.params {
                    self.error(format!(
                        "{} type arguments for enum {} with {}",
                        ty_args.len(),
                        def.name,
                        def.params
                    ));
                }
                if ctor.variant >= def.variants.len() {
                    self.error(format!("no constructor {} in {}", ctor.variant, def.name));
                    return TypeId::ANY;
                }
                let fields = self.t.field_types(*ctor, ty_args);
                self.args(args, &fields);
                self.t.intern(Type::Enum(ctor.enum_id, ty_args.clone()))
            }
            Expr::Field { base, ctor, index } => {
                let b = self.sub(Seg::Name("base"), base);
                if matches!(b, TypeId::ANY | TypeId::NEVER) {
                    return b;
                }
                match self.t.field_type(b, *ctor, *index) {
                    Some(t) => t,
                    None => {
                        let message = format!(
                            "field {index} of {}.{} on a value of type {}",
                            m.enums[ctor.enum_id].name,
                            m.variant(*ctor).name,
                            self.name(b)
                        );
                        self.error(message);
                        TypeId::ANY
                    }
                }
            }
            Expr::Tuple(es) => {
                if es.len() < 2 {
                    self.error(format!("a tuple of {} elements", es.len()));
                }
                let ts = es
                    .iter()
                    .enumerate()
                    .map(|(i, e)| self.sub(Seg::Index("elem", i), e))
                    .collect();
                self.t.intern(Type::Tuple(ts))
            }
            Expr::List { elem, elems, tail } => {
                for (i, x) in elems.iter().enumerate() {
                    self.sub_expect(Seg::Index("elem", i), x, *elem);
                }
                let list = self.t.intern(Type::List(*elem));
                if let Some(t) = tail {
                    self.sub_expect(Seg::Name("tail"), t, list);
                }
                list
            }
            Expr::If { cond, then, else_ } => {
                self.sub_expect(Seg::Name("cond"), cond, TypeId::BOOL);
                let a = self.sub(Seg::Name("then"), then);
                let b = self.sub(Seg::Name("else"), else_);
                self.join(TypeId::NEVER, &[a, b])
            }
            Expr::Match { scrutinee, arms } => {
                let s = self.sub(Seg::Name("scrutinee"), scrutinee);
                let tys: Vec<TypeId> = arms
                    .iter()
                    .enumerate()
                    .map(|(i, arm)| {
                        self.path.push(Seg::Index("arm", i));
                        self.pat(&arm.pat, s);
                        let t = self.arm(arm);
                        self.path.pop();
                        t
                    })
                    .collect();
                self.join(TypeId::NEVER, &tys)
            }
            Expr::Escape { body, arms } => {
                let b = self.sub(Seg::Name("body"), body);
                let tys: Vec<TypeId> = arms
                    .iter()
                    .enumerate()
                    .map(|(i, arm)| {
                        self.path.push(Seg::Index("arm", i));
                        match &arm.pat {
                            Pat::Ctor { ctor, .. } if m.enums[ctor.enum_id].error => {
                                let ty = self.t.intern(Type::Enum(ctor.enum_id, Vec::new()));
                                self.pat(&arm.pat, ty);
                            }
                            _ => self.error("an escape arm not matching an error".to_owned()),
                        }
                        let t = self.arm(arm);
                        self.path.pop();
                        t
                    })
                    .collect();
                self.join(b, &tys)
            }
            Expr::Binary { op, ty, lhs, rhs } => {
                let a = self.sub(Seg::Name("lhs"), lhs);
                let b = self.sub(Seg::Name("rhs"), rhs);
                let structural = *ty == OpTy::Structural;
                let logical = matches!(op, BinOp::And | BinOp::Or);
                if structural && !matches!(op, BinOp::Eq | BinOp::Ne) {
                    self.error("a structural operator other than == and !=".to_owned());
                }
                if logical && *ty != OpTy::Bool {
                    self.error("&& or || not on Bool".to_owned());
                }
                if structural {
                    if !self.compatible(a, b) && !self.compatible(b, a) {
                        let message = format!("comparing {} with {}", self.name(a), self.name(b));
                        self.error(message);
                    }
                } else {
                    let operand = op_ty_type(*ty);
                    self.expect(a, operand, "lhs");
                    self.expect(b, operand, "rhs");
                }
                bin_result(*op, *ty)
            }
            Expr::Unary { op, ty, operand } => {
                let t = self.sub(Seg::Name("operand"), operand);
                match op {
                    UnOp::Neg => {
                        if !matches!(ty, OpTy::Int | OpTy::Int64 | OpTy::Float) {
                            self.error("negation of a non-number".to_owned());
                        }
                        self.expect(t, op_ty_type(*ty), "operand");
                        op_ty_type(*ty)
                    }
                    UnOp::Not => {
                        self.expect(t, TypeId::BOOL, "operand");
                        TypeId::BOOL
                    }
                }
            }
            Expr::Lambda {
                params,
                ty,
                body,
                suspends,
            } => {
                self.lambda(params, *ty, body, *suspends);
                *ty
            }
            Expr::Concat(parts) => {
                for (i, part) in parts.iter().enumerate() {
                    if let StrPart::Value(v, ty) = part {
                        self.sub_expect(Seg::Index("part", i), v, *ty);
                    }
                }
                TypeId::STRING
            }
            Expr::Loop { vars, inits, body } => {
                if vars.len() != inits.len() {
                    self.error(format!(
                        "{} initial values for {} loop variables",
                        inits.len(),
                        vars.len()
                    ));
                }
                for (i, (init, &v)) in inits.iter().zip(vars).enumerate() {
                    self.sub_expect(Seg::Index("init", i), init, m.locals[v].ty);
                }
                self.loops.push(vars);
                let t = self.sub(Seg::Name("loop"), body);
                self.loops.pop();
                t
            }
            Expr::Recur(args) => {
                match self.loops.last() {
                    Some(vars) => {
                        let tys: Vec<TypeId> = vars.iter().map(|&v| m.locals[v].ty).collect();
                        self.path.push(Seg::Name("recur"));
                        self.args(args, &tys);
                        self.path.pop();
                    }
                    None => self.error("recur outside a loop".to_owned()),
                }
                TypeId::NEVER
            }
            Expr::Assert(a) => {
                match &a.cond {
                    AssertCond::Compare { ty, lhs, rhs, .. } => {
                        self.sub_expect(Seg::Name("lhs"), lhs, *ty);
                        self.sub_expect(Seg::Name("rhs"), rhs, *ty);
                    }
                    AssertCond::Bool(c) => self.sub_expect(Seg::Name("cond"), c, TypeId::BOOL),
                }
                TypeId::UNIT
            }
            Expr::Fail(v) => {
                let t = self.sub(Seg::Name("fail"), v);
                // A type parameter may stand for one error type (spec 7.2, `or_fail[A, E]`).
                let is_error = match self.t.ty(t) {
                    Type::Enum(id, _) => m.enums[*id].error,
                    Type::Param(_) => true,
                    _ => matches!(t, TypeId::NEVER | TypeId::ANY),
                };
                if !is_error {
                    let message = format!("failing with a non-error {}", self.name(t));
                    self.error(message);
                }
                TypeId::NEVER
            }
            Expr::Use { ctx, effect } => {
                let t = self.sub(Seg::Name("use"), ctx);
                self.table(t);
                self.t.intern(Type::Handler(*effect))
            }
            Expr::Provide { base, bindings } => {
                if let Some(b) = base {
                    let t = self.sub(Seg::Name("base"), b);
                    self.table(t);
                }
                for (i, (effect, h)) in bindings.iter().enumerate() {
                    let t = self.sub(Seg::Index("binding", i), h);
                    self.binding(*effect, t, i);
                }
                TypeId::EVIDENCE
            }
            Expr::Handler { effect, ops, .. } => {
                let def = &m.effects[*effect];
                if ops.len() != def.ops.len() {
                    self.error(format!(
                        "{} operations for effect {} with {}",
                        ops.len(),
                        def.name,
                        def.ops.len()
                    ));
                }
                for (i, op) in ops.iter().enumerate() {
                    let t = self.sub(Seg::Index("op", i), op);
                    if !matches!(self.t.ty(t), Type::Fn(_)) {
                        let message = format!("operation {i} is a {}", self.name(t));
                        self.error(message);
                    }
                }
                self.t.intern(Type::Handler(*effect))
            }
            Expr::Fiber { op, args } => self.fiber(*op, args),
        }
    }

    /// Checks a guard and returns the arm body's type.
    fn arm(&mut self, arm: &'a Arm) -> TypeId {
        if let Some(g) = &arm.guard {
            self.sub_expect(Seg::Name("guard"), g, TypeId::BOOL);
        }
        self.sub(Seg::Name("body"), &arm.body)
    }

    /// The type of branches: the first that is not `Never` (`first` comes before `rest`); the
    /// others must be compatible with it.
    fn join(&mut self, first: TypeId, rest: &[TypeId]) -> TypeId {
        let mut result = first;
        for &t in rest {
            if result == TypeId::NEVER {
                result = t;
            } else if !self.compatible(t, result) && !self.compatible(result, t) {
                let message = format!(
                    "branches of types {} and {}",
                    self.name(result),
                    self.name(t)
                );
                self.error(message);
            }
        }
        result
    }

    /// A handler value of type `t` provided for `effect`. It may be the handler of another
    /// instantiation of the same effect: a call to a generic function adds the caller's handler
    /// (`State[Int]`) under the callee's key (`State[$modify.0]`, `generic_ev` in lowering).
    /// `EffectDef` does not record what it instantiates, so the names are compared (an
    /// instantiation's name is the effect's followed by `[..]`).
    fn binding(&mut self, effect: EffectId, t: TypeId, i: usize) {
        let m = self.m();
        let base = |e: EffectId| {
            let name = &m.effects[e].name;
            name.split_once('[').map_or(name.as_str(), |(b, _)| b)
        };
        let ok = match self.t.ty(t) {
            Type::Handler(h) => base(*h) == base(effect),
            _ => matches!(t, TypeId::NEVER | TypeId::ANY),
        };
        if !ok {
            let message = format!(
                "binding {i}: expected handler {}, found {}",
                m.effects[effect].name,
                self.name(t)
            );
            self.error(message);
        }
    }

    /// An evidence table where one must be (`Use::ctx`, `Provide::base`): not `()`.
    fn table(&mut self, t: TypeId) {
        if !matches!(t, TypeId::EVIDENCE | TypeId::NEVER) {
            let message = format!("expected evidence, found {}", self.name(t));
            self.error(message);
        }
    }

    fn check_ty_args(&mut self, f: FnId, ty_args: &[TypeId]) {
        let func = &self.m().functions[f];
        if ty_args.len() != func.type_params {
            self.error(format!(
                "{} type arguments for {} with {}",
                ty_args.len(),
                func.name,
                func.type_params
            ));
        }
    }

    fn call(
        &mut self,
        callee: &'a Callee,
        args: &'a [Expr],
        fails: bool,
        suspends: bool,
    ) -> TypeId {
        let m = self.m();
        match callee {
            Callee::Direct { f, ty_args } => {
                self.check_ty_args(*f, ty_args);
                let func = &m.functions[*f];
                if func.suspends != suspends {
                    self.error(format!(
                        "a call to {} marked suspends: {suspends}, but the function's is {}",
                        func.name, func.suspends
                    ));
                }
                let (params, ret) = self.t.direct_sig(*f, ty_args);
                self.args(args, &params);
                ret
            }
            Callee::Indirect(c) => {
                let ct = self.sub(Seg::Name("callee"), c);
                let Type::Fn(f) = self.t.ty(ct).clone() else {
                    if !matches!(ct, TypeId::ANY | TypeId::NEVER) {
                        let message = format!("calling a {}", self.name(ct));
                        self.error(message);
                    }
                    for (i, arg) in args.iter().enumerate() {
                        self.sub(Seg::Index("arg", i), arg);
                    }
                    return if ct == TypeId::NEVER {
                        TypeId::NEVER
                    } else {
                        TypeId::ANY
                    };
                };
                if f.fails && !fails {
                    self.error("a call to a fallible function value not marked fails".to_owned());
                }
                if f.suspends && !suspends {
                    self.error(
                        "a call to a suspending function value not marked suspends".to_owned(),
                    );
                }
                let mut params = f.params.clone();
                if f.ev {
                    params.push(TypeId::EVIDENCE);
                }
                self.args(args, &params);
                f.ret
            }
            Callee::Op {
                handler,
                effect,
                op,
            } => {
                let h = self.sub(Seg::Name("handler"), handler);
                let ty = self.t.intern(Type::Handler(*effect));
                self.expect(h, ty, "handler");
                let def = &m.effects[*effect];
                match def.ops.get(*op) {
                    Some(o) if o.suspend && !suspends => self.error(format!(
                        "a call to suspend fn {}#{} not marked suspends",
                        def.name, o.name
                    )),
                    Some(_) => {}
                    None => self.error(format!("no operation {op} in {}", def.name)),
                }
                for (i, arg) in args.iter().enumerate() {
                    self.sub(Seg::Index("arg", i), arg);
                }
                TypeId::ANY
            }
        }
    }

    fn lambda(&mut self, params: &'a [Local], ty: TypeId, body: &'a Expr, suspends: bool) {
        let m = self.m();
        let Type::Fn(f) = self.t.ty(ty).clone() else {
            let message = format!("a lambda of type {}", self.name(ty));
            self.error(message);
            return;
        };
        if suspends && !f.suspends {
            self.error("a suspending lambda whose type does not suspend".to_owned());
        }
        let declared = params.len().saturating_sub(usize::from(f.ev));
        if declared != f.params.len() || params.len() < usize::from(f.ev) {
            self.error(format!(
                "a lambda with {} parameters of type {}",
                params.len(),
                self.name(ty)
            ));
        }
        for (&p, &t) in params.iter().zip(&f.params) {
            self.expect(t, m.locals[p].ty, "parameter");
        }
        if f.ev
            && params
                .last()
                .is_some_and(|&p| m.locals[p].ty != TypeId::EVIDENCE)
        {
            self.error(
                "a lambda whose type takes evidence without an evidence parameter".to_owned(),
            );
        }
        let loops = std::mem::take(&mut self.loops);
        self.path.push(Seg::Name("lambda"));
        let t = self.expr(body);
        self.expect(t, f.ret, "body");
        self.path.pop();
        self.loops = loops;
    }

    /// Checks a pattern against the type of the value it matches.
    fn pat(&mut self, p: &'a Pat, ty: TypeId) {
        let m = self.m();
        // Under `Never` and `Any` nothing is known about the parts.
        let unknown = matches!(ty, TypeId::NEVER | TypeId::ANY);
        match p {
            Pat::Wild => {}
            Pat::Bind(l) => self.expect(ty, m.locals[*l].ty, "binding"),
            Pat::Lit(lit) => self.expect(lit_type(lit), ty, "literal pattern"),
            Pat::Ctor { ctor, fields } => {
                let fields_tys = match self.t.ty(ty) {
                    Type::Enum(id, args) if *id == ctor.enum_id => {
                        let args = args.clone();
                        Some(self.t.field_types(*ctor, &args))
                    }
                    _ if unknown => None,
                    _ => {
                        let message = format!(
                            "pattern {}.{} against {}",
                            m.enums[ctor.enum_id].name,
                            m.variant(*ctor).name,
                            self.name(ty)
                        );
                        self.error(message);
                        None
                    }
                };
                if let Some(tys) = &fields_tys
                    && tys.len() != fields.len()
                {
                    self.error(format!(
                        "{} field patterns for {} fields",
                        fields.len(),
                        tys.len()
                    ));
                }
                for (i, f) in fields.iter().enumerate() {
                    let t = fields_tys
                        .as_ref()
                        .and_then(|tys| tys.get(i).copied())
                        .unwrap_or(TypeId::ANY);
                    self.pat(f, t);
                }
            }
            Pat::Tuple(ps) => {
                let tys = match self.t.ty(ty) {
                    Type::Tuple(ts) if ts.len() == ps.len() => ts.clone(),
                    _ => {
                        if !unknown {
                            let message =
                                format!("a {}-tuple pattern against {}", ps.len(), self.name(ty));
                            self.error(message);
                        }
                        vec![TypeId::ANY; ps.len()]
                    }
                };
                for (p, t) in ps.iter().zip(tys) {
                    self.pat(p, t);
                }
            }
            Pat::List { elems, rest } => {
                let elem = match self.t.ty(ty) {
                    Type::List(e) => *e,
                    _ => {
                        if !unknown {
                            let message = format!("a list pattern against {}", self.name(ty));
                            self.error(message);
                        }
                        TypeId::ANY
                    }
                };
                for p in elems {
                    self.pat(p, elem);
                }
                if let Some(r) = rest {
                    self.pat(r, ty);
                }
            }
        }
    }

    fn fiber(&mut self, op: FiberOp, args: &'a [Expr]) -> TypeId {
        if args.len() != op.arity() {
            self.error(format!(
                "{} arguments for fiber#{} with {}",
                args.len(),
                op.name(),
                op.arity()
            ));
        }
        let tys: Vec<TypeId> = args
            .iter()
            .enumerate()
            .map(|(i, a)| self.sub(Seg::Index("arg", i), a))
            .collect();
        let first = tys.first().copied();
        match op {
            // `body: fn(scope) -> A`.
            FiberOp::Scope { .. } => {
                if let Some(t) = first {
                    let ok = match self.t.ty(t) {
                        Type::Fn(f) => f.params == [TypeId::FIBER_SCOPE],
                        _ => false,
                    };
                    if !ok {
                        let message = format!("fiber#scope of a {}", self.name(t));
                        self.error(message);
                    }
                }
            }
            // `scope`, then for fork the parameterless child.
            FiberOp::Fork | FiberOp::Deferred => {
                if let Some(t) = first {
                    self.expect(t, TypeId::FIBER_SCOPE, "scope");
                }
                if let (FiberOp::Fork, Some(&child)) = (op, tys.get(1)) {
                    let ok = match self.t.ty(child) {
                        Type::Fn(f) => f.params.is_empty() && !f.ev,
                        _ => false,
                    };
                    if !ok {
                        let message = format!("fiber#fork of a {}", self.name(child));
                        self.error(message);
                    }
                }
            }
            // `deferred: Deferred[A]`, `value: A`.
            FiberOp::Complete => {
                if let (Some(d), Some(&v)) = (first, tys.get(1)) {
                    let a = self.t.enum_arg(d, 0);
                    self.expect(v, a, "value");
                }
            }
            // `f: fn() -> A` without the evidence table.
            FiberOp::Attempt { .. } => {
                if let Some(t) = first {
                    let ok = match self.t.ty(t) {
                        Type::Fn(f) => f.params.is_empty() && !f.ev,
                        _ => matches!(t, TypeId::ANY | TypeId::NEVER),
                    };
                    if !ok {
                        let message = format!("fiber#attempt of a {}", self.name(t));
                        self.error(message);
                    }
                }
            }
            _ => {}
        }
        self.t.fiber_result(op, first)
    }
}
