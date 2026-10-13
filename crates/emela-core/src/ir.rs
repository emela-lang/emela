//! The Core IR.
//!
//! The typed tree after desugaring; backends (JS / WASM) see only this. Everything is referred to
//! by ID: functions by `FnId`, locals by `Local`, types by `EnumId`. Display names (`name` /
//! `hint`) exist only to make output readable.

use la_arena::{Arena, Idx};

use crate::intrinsics::Builtin;

pub type FnId = Idx<Function>;
pub type EnumId = Idx<EnumDef>;
pub type Local = Idx<LocalData>;
pub type EffectId = Idx<EffectDef>;

#[derive(Debug, Clone, Default)]
pub struct Module {
    pub enums: Arena<EnumDef>,
    pub functions: Arena<Function>,
    pub locals: Arena<LocalData>,
    /// Effects (spec 8.1), used for evidence table keys and handler operation names.
    pub effects: Arena<EffectDef>,
    /// The ID of Prelude's `Option`, if added (`option_enum`).
    pub option: Option<EnumId>,
    /// Bundled JS files written under [`crate::CORE_JS_DIR`] in the output. Present only when an
    /// external function points to a bundled file.
    pub js_files: Vec<JsFile>,
}

/// A JS file shipped alongside the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsFile {
    /// Relative to [`crate::CORE_JS_DIR`], `/`-separated.
    pub path: String,
    pub contents: String,
}

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    /// `hint` is a naming hint for the output and need not be unique.
    pub fn local(&mut self, hint: &str) -> Local {
        let hint = if hint.is_empty() { "v" } else { hint };
        self.locals.alloc(LocalData {
            hint: hint.to_owned(),
        })
    }

    /// The body is filled in later with `define` (for mutual recursion). `name` must be unique in
    /// the module.
    pub fn declare(&mut self, name: &str, exported: bool) -> FnId {
        debug_assert!(
            self.functions.iter().all(|(_, f)| f.name != name),
            "duplicate function name `{name}`"
        );
        self.functions.alloc(Function {
            name: name.to_owned(),
            exported,
            params: Vec::new(),
            body: Expr::Lit(Lit::Unit),
            external: None,
            suspends: false,
            constant: false,
        })
    }

    /// Makes the function external (spec chapter 12). The body is unused.
    pub fn define_external(&mut self, f: FnId, params: Vec<Local>, external: External) {
        let func = &mut self.functions[f];
        func.params = params;
        func.external = Some(external);
    }

    /// `name` must be unique in the module.
    pub fn add_effect(&mut self, def: EffectDef) -> EffectId {
        debug_assert!(
            self.effects.iter().all(|(_, e)| e.name != def.name),
            "duplicate effect name `{}`",
            def.name
        );
        self.effects.alloc(def)
    }

    pub fn define(&mut self, f: FnId, params: Vec<Local>, body: Expr) {
        let func = &mut self.functions[f];
        func.params = params;
        func.body = body;
    }

    pub fn add_enum(&mut self, def: EnumDef) -> EnumId {
        self.enums.alloc(def)
    }

    /// Adds Prelude's `Option[A]` (`Some(A)` then `None`) and returns its ID. Later calls return
    /// the same ID.
    ///
    /// One per module, so that builtins returning Option (`List.get`, `Int.checked_add`) and the
    /// user's `Some` / `None` refer to the same definition.
    pub fn option_enum(&mut self) -> EnumId {
        if let Some(id) = self.option {
            return id;
        }
        let id = self.enums.alloc(EnumDef::option());
        self.option = Some(id);
        id
    }

    /// `Some`．
    pub fn some_ctor(&mut self) -> CtorRef {
        CtorRef {
            enum_id: self.option_enum(),
            variant: OPTION_SOME,
        }
    }

    /// `None`．
    pub fn none_ctor(&mut self) -> CtorRef {
        CtorRef {
            enum_id: self.option_enum(),
            variant: OPTION_NONE,
        }
    }

    pub fn variant(&self, ctor: CtorRef) -> &VariantDef {
        &self.enums[ctor.enum_id].variants[ctor.variant]
    }
}

#[derive(Debug, Clone)]
pub struct LocalData {
    pub hint: String,
}

#[derive(Debug, Clone)]
pub struct Function {
    /// Unique in the module; exported under this name.
    pub name: String,
    pub exported: bool,
    pub params: Vec<Local>,
    pub body: Expr,
    /// For an external function (spec chapter 12), the host function. `body` is then unused.
    pub external: Option<External>,
    /// Whether it may suspend (spec 8.1, 16.5). If true, the body may contain suspending calls
    /// (`Expr::Call::suspends`). For an external function this is `@external suspend fn`
    /// (spec 12.2), and the host function returns a value to await (a Promise in JS). Output for a
    /// non-suspending function knows nothing about suspension.
    pub suspends: bool,
    /// The value of a const (spec 5.6). Takes no arguments; the body is evaluated once, on the
    /// first call, and the value is remembered. A defect while evaluating carries the const's name
    /// (`name`), and a reference to the const while it is being evaluated is a defect.
    pub constant: bool,
}

/// The host function of an external function (spec 12.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    /// The per-target `@external` arguments, in written order.
    pub bindings: Vec<ExternalBinding>,
    /// How each parameter crosses the boundary (spec 12.4, 12.5), in `Function::params` order.
    pub params: Vec<Boundary>,
    /// How the return value crosses the boundary.
    pub ret: Boundary,
}

impl External {
    /// The binding for `target` (`js`, `wasm`), if any. If the target appears twice, the first.
    pub fn binding(&self, target: &str) -> Option<&ExternalBinding> {
        self.bindings.iter().find(|b| b.target == target)
    }
}

/// One `@external(js, "./http_ffi.mjs", "fetchText")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalBinding {
    pub target: String,
    /// The remaining arguments. In JS, one means a global reference (`"console.log"`), two mean a
    /// module and a name. A module naming a file (starting with `./` or `../`) has been rewritten
    /// by lowering to an absolute path from the declaring source's directory (spec 12.1).
    pub args: Vec<String>,
}

/// How an external function's parameters and return value cross the boundary (spec 12.4, 12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Boundary {
    /// Lone surrogates received from the host are replaced with U+FFFD.
    String,
    /// The host's return value is discarded and becomes `()`.
    Unit,
    /// A callback. Arguments the host calls it with and values returned to the host are converted
    /// by the same rules.
    Fn {
        params: Vec<Boundary>,
        ret: Box<Boundary>,
    },
    /// Passed as is.
    Plain,
}

/// An effect (spec 8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDef {
    /// Unique in the module; used as the evidence table key.
    pub name: String,
    /// In declaration order; indexed by `Callee::Op::op` and `Expr::Handler::ops`.
    pub ops: Vec<OpDef>,
}

/// An effect operation (spec 8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpDef {
    pub name: String,
    /// Whether it is a `suspend fn`. If true, the handler's implementation (a lambda in
    /// `Expr::Handler::ops`) takes resume as its last argument, and operation calls
    /// (`Callee::Op`) are suspending calls (spec 8.4, 14.5).
    pub suspend: bool,
}

impl OpDef {
    /// An `fn` operation.
    pub fn new(name: &str) -> Self {
        OpDef {
            name: name.to_owned(),
            suspend: false,
        }
    }

    /// A `suspend fn` operation.
    pub fn suspend(name: &str) -> Self {
        OpDef {
            name: name.to_owned(),
            suspend: true,
        }
    }
}

/// An enum definition. A `type` is an enum with one constructor.
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    /// The number of type parameters. `Type::Param(i)` in field types refers to the `i`th.
    pub params: usize,
    pub variants: Vec<VariantDef>,
    /// Whether it is an error declaration (spec 7.1): one constructor, no type parameters. It can
    /// be passed to `Expr::Fail` and written in `Expr::Escape` arm patterns. Backends must
    /// represent it so the error can be identified at run time.
    pub error: bool,
}

/// The index of `Some` in Prelude's `Option`.
pub const OPTION_SOME: usize = 0;
/// The index of `None` in Prelude's `Option`.
pub const OPTION_NONE: usize = 1;

impl EnumDef {
    /// Prelude's `enum Option[A] { Some(A), None }`.
    pub fn option() -> Self {
        EnumDef {
            name: "Option".to_owned(),
            params: 1,
            variants: vec![
                VariantDef::positional("Some", vec![Type::Param(0)]),
                VariantDef::unit("None"),
            ],
            error: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: VariantFields,
    /// Field types, in `fields` order. Used to build show functions (the stand-in for Show) from
    /// types.
    pub tys: Vec<Type>,
}

impl VariantDef {
    pub fn unit(name: &str) -> Self {
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Unit,
            tys: Vec::new(),
        }
    }

    pub fn positional(name: &str, tys: Vec<Type>) -> Self {
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Positional(tys.len()),
            tys,
        }
    }

    pub fn named(name: &str, fields: Vec<(&str, Type)>) -> Self {
        let (names, tys) = fields.into_iter().map(|(n, t)| (n.to_owned(), t)).unzip();
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Named(names),
            tys,
        }
    }
}

/// The type of an IR value. Used for showing (the stand-in for Show) and for builtins whose
/// behavior depends on the type.
///
/// The type checker's `emela_types::Ty` rewritten with IR names (`EnumId`). No type variables.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    Int64,
    Float,
    Bool,
    String,
    Unit,
    Never,
    /// Two or more elements.
    Tuple(Vec<Type>),
    List(Box<Type>),
    /// An enum / type application. The argument count is `EnumDef::params`.
    Enum(EnumId, Vec<Type>),
    /// A type parameter of the enclosing `EnumDef`. Occurs only in field types.
    Param(usize),
    /// A function type. It cannot be shown (spec 10.6), so it carries nothing.
    Fn,
}

impl Type {
    pub fn list(elem: Type) -> Type {
        Type::List(Box::new(elem))
    }

    /// Replaces `Type::Param(i)` with `args[i]`.
    pub fn subst(&self, args: &[Type]) -> Type {
        match self {
            Type::Param(i) => args[*i].clone(),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| t.subst(args)).collect()),
            Type::List(t) => Type::list(t.subst(args)),
            Type::Enum(id, ts) => Type::Enum(*id, ts.iter().map(|t| t.subst(args)).collect()),
            _ => self.clone(),
        }
    }
}

/// The three forms of variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantFields {
    /// `Red`
    Unit,
    /// `Some(T)`. The value is the field count.
    Positional(usize),
    /// `Point { x: Int, y: Int }`. In definition order.
    Named(Vec<String>),
}

impl VariantFields {
    pub fn len(&self) -> usize {
        match self {
            VariantFields::Unit => 0,
            VariantFields::Positional(n) => *n,
            VariantFields::Named(names) => names.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CtorRef {
    pub enum_id: EnumId,
    /// An index into `EnumDef::variants`.
    pub variant: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    /// 32-bit signed.
    Int(i32),
    Int64(i64),
    /// binary64．
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Lit),
    Var(Local),
    /// A top-level function used as a value.
    Fn(FnId),
    /// `let var = value; body`．
    Let {
        var: Local,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// Arguments are in positional order; use `named::call_in_written_order` to reorder named
    /// arguments. `fails` tells whether the callee may fail (the function type's E is not empty,
    /// spec 7.2). If false, the result need not be checked; if true, it does not always fail.
    ///
    /// `suspends` tells whether the call may suspend (spec 8.1, 14.5). Inside a non-suspending
    /// function, a true call occurs only inside a `with` of a handler that resumes immediately
    /// (spec 8.7). Backends run it to completion on the spot, and if it is not resumed, treat it as
    /// unwound while suspended. Rules per callee:
    ///
    /// - `Callee::Direct`: must equal the called function's `Function::suspends`.
    /// - `Callee::Indirect`: true if the R of the callee expression's type may suspend. When true,
    ///   the function value actually called may or may not suspend; when false, it must not
    ///   suspend.
    /// - `Callee::Op`: must be true for a suspend fn operation (`OpDef::suspend`). True for an fn
    ///   operation means, as with `Indirect`, the implementation may or may not suspend.
    Call {
        callee: Callee,
        args: Vec<Expr>,
        fails: bool,
        suspends: bool,
    },
    /// A builtin call (`intrinsics`). `ty_args` are the concrete type arguments, used only by
    /// builtins whose behavior depends on the type, such as `dbg` (the count is
    /// `Builtin::info().ty_params`).
    Builtin {
        op: Builtin,
        ty_args: Vec<Type>,
        args: Vec<Expr>,
    },
    /// Arguments are in field definition order.
    Ctor {
        ctor: CtorRef,
        args: Vec<Expr>,
    },
    /// `base` must be a value of `ctor` (intended for `type` values).
    Field {
        base: Box<Expr>,
        ctor: CtorRef,
        index: usize,
    },
    /// Two or more elements; `()` is `Lit::Unit`.
    Tuple(Vec<Expr>),
    /// `[a, b]` or `[a, b, ..tail]`.
    List {
        elems: Vec<Expr>,
        tail: Option<Box<Expr>>,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    /// Evaluates the scrutinee once and tries arms top to bottom. A defect if no arm matches.
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<Arm>,
    },
    Binary {
        op: BinOp,
        ty: OpTy,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnOp,
        ty: OpTy,
        operand: Box<Expr>,
    },
    /// Free variables are captured as is (capture sets are computed at the WASM stage).
    /// `suspends` means the same as `Function::suspends`.
    Lambda {
        params: Vec<Local>,
        body: Box<Expr>,
        suspends: bool,
    },
    /// String interpolation `"a#{x}b"`. Parts are evaluated left to right and concatenated.
    Concat(Vec<StrPart>),
    /// Made by self tail call loopification (`tail::loopify`). Initializes `vars` with `inits`
    /// and evaluates `body`. `vars` are bound afresh on each iteration.
    Loop {
        vars: Vec<Local>,
        inits: Vec<Expr>,
        body: Box<Expr>,
    },
    /// Continues with the next iteration of the enclosing `Loop`. Occurs only in tail position of
    /// a `Loop` body.
    Recur(Vec<Expr>),
    /// `assert expr` (spec 13.2). The value is `()`; a defect if false.
    Assert(Box<Assert>),
    /// `fail e` (spec 7.2). Evaluates `e` and ends the current computation with its value (an error
    /// enum value). The failure propagates outward until the innermost catching `Escape` or the
    /// function's caller. The type is Never.
    Fail(Box<Expr>),
    /// `body escape { arms }` (spec 7.3). Catches failures raised while evaluating `body`.
    ///
    /// Arm patterns are error constructors (`Pat::Ctor`). Arms for the failed error are tried top
    /// to bottom, and the body of the first arm whose pattern and guard match is the escape's
    /// value. Errors named in no arm propagate outward. An error named in an arm that matches no
    /// arm is a defect (as with match). Failures inside arms propagate outward. `body` is not in
    /// tail position (spec 6.8).
    Escape {
        body: Box<Expr>,
        arms: Vec<Arm>,
    },
    /// Looks up the current handler of the effect in the evidence table `ctx` (`use X`, spec 8.2).
    ///
    /// The evidence table maps effects to handler values and is passed as the last argument of
    /// functions and lambdas whose R is not empty (evidence passing, spec 16.5).
    Use {
        ctx: Box<Expr>,
        effect: EffectId,
    },
    /// A new table: `base` plus `bindings`. `base` is unchanged, and later bindings of the same
    /// effect win. With `base` of `None`, bindings are added to an empty table.
    Provide {
        base: Option<Box<Expr>>,
        bindings: Vec<(EffectId, Expr)>,
    },
    /// A stateless handler value (spec 8.4). `ops` are lambdas in the effect's operation
    /// declaration order.
    ///
    /// Lambdas for suspend fn operations (`OpDef::suspend`) take one extra argument, resume, after
    /// the declared parameters. resume is a function value taking one value and returning `()`;
    /// calling it resumes the suspended caller (spec 8.4, 14.5). It may be called at most once; a
    /// second call is a defect. `name` is the handler's name, used in that defect's message.
    Handler {
        name: String,
        effect: EffectId,
        ops: Vec<Expr>,
    },
    /// `with` (spec 8.3, 14.2). Evaluates and binds `binds` in order (handler values and evidence
    /// tables), then evaluates `body`. Same meaning as `Let`, but `body` is not in tail position
    /// (spec 6.8).
    ///
    /// For a binding with release (`WithBind::release`, spec 8.4), once bound, its release is
    /// evaluated however the `with` ends (value, failure, defect, unwinding while suspended).
    /// Releases run in reverse binding order. If evaluating a binding's value fails, later bindings
    /// and `body` are not evaluated; only the releases bound so far run, in reverse, before the
    /// failure propagates (spec 14.2). A defect in a release is merged into the `with`'s result by
    /// the rules of spec 14.4, and all remaining releases are still evaluated.
    With {
        binds: Vec<WithBind>,
        body: Box<Expr>,
    },
    /// A concurrency operation (spec chapter 11). The number and meaning of `args` depend on the
    /// `FiberOp` (`FiberOp::arity`), as do whether it may suspend (`FiberOp::suspends`) and fail
    /// (`FiberOp::fails`). Suspending operations occur only in bodies of suspending functions and
    /// lambdas (they require Async). `FiberOp::Attempt` depends on `f` and follows the same rule as
    /// `Expr::Call::suspends`.
    Fiber {
        op: FiberOp,
        args: Vec<Expr>,
    },
}

impl Expr {
    /// Applies `f` to every direct subexpression (not patterns), not necessarily in evaluation
    /// order.
    pub fn for_each_child(&self, f: &mut impl FnMut(&Expr)) {
        match self {
            Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) => {}
            Expr::Let { value, body, .. } => {
                f(value);
                f(body);
            }
            Expr::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct(_) => {}
                    Callee::Indirect(c) | Callee::Op { handler: c, .. } => f(c),
                }
                args.iter().for_each(f);
            }
            Expr::Builtin { args, .. }
            | Expr::Ctor { args, .. }
            | Expr::Tuple(args)
            | Expr::Recur(args)
            | Expr::Handler { ops: args, .. } => args.iter().for_each(f),
            Expr::Field { base, .. } | Expr::Use { ctx: base, .. } => f(base),
            Expr::List { elems, tail } => {
                elems.iter().for_each(&mut *f);
                if let Some(t) = tail {
                    f(t);
                }
            }
            Expr::If { cond, then, else_ } => {
                f(cond);
                f(then);
                f(else_);
            }
            Expr::Match {
                scrutinee: body,
                arms,
            }
            | Expr::Escape { body, arms } => {
                f(body);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        f(g);
                    }
                    f(&arm.body);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            Expr::Unary { operand: e, .. } | Expr::Lambda { body: e, .. } | Expr::Fail(e) => f(e),
            Expr::Concat(parts) => {
                for p in parts {
                    if let StrPart::Value(v, _) = p {
                        f(v);
                    }
                }
            }
            Expr::Loop { inits, body, .. } => {
                inits.iter().for_each(&mut *f);
                f(body);
            }
            Expr::Assert(a) => match &a.cond {
                AssertCond::Compare { lhs, rhs, .. } => {
                    f(lhs);
                    f(rhs);
                }
                AssertCond::Bool(e) => f(e),
            },
            Expr::Provide { base, bindings } => {
                if let Some(b) = base {
                    f(b);
                }
                for (_, h) in bindings {
                    f(h);
                }
            }
            Expr::With { binds, body } => {
                for b in binds {
                    f(&b.value);
                    if let Some(r) = &b.release {
                        f(&r.body);
                    }
                }
                f(body);
            }
            Expr::Fiber { args, .. } => args.iter().for_each(f),
        }
    }
}

/// Concurrency operations (spec chapter 11). The `args` of `Expr::Fiber` are in the order given
/// for each variant.
///
/// Scopes (the `Scope` capability value), fiber handles (`Fiber[A]`), Deferreds and causes
/// (`Cause[E]`) are opaque values chosen by the backend. Capability values may be put in evidence
/// tables with `Expr::Provide`. Exit (`Std.Fiber`) and CauseKind (`Std.Cause`) are represented like
/// user enums, with constructor order given by `EXIT_SUCCESS`, `EXIT_FAILURE` and `CAUSE_FAIL`,
/// `CAUSE_DIE`, `CAUSE_INTERRUPT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberOp {
    /// `Fiber.scope(body)` (spec 11.2). `[body]`, where `body` is a function value taking the
    /// scope capability value. When the body returns a value, the remaining children are
    /// interrupted, and the value is returned after all children finish. The first failure (Fail,
    /// Die) of a child or the body is the scope's result. `fails` tells whether the body's (and
    /// children's) E is not empty.
    Scope { fails: bool },
    /// `s.fork(f)` (spec 11.2). `[scope, f]`, where `f` is a parameterless function value run as
    /// a child fiber. The value is the fiber handle. Not a suspension point (the child starts when
    /// the parent next suspends).
    Fork,
    /// `s.deferred()` (spec 11.7). `[scope]`. The value is an empty Deferred.
    Deferred,
    /// `Fiber.join(h)` (spec 11.5). `[fiber]`. The child's value; a defect if the child was
    /// interrupted.
    Join,
    /// `Fiber.await(h)` (spec 11.5). `[fiber]`. A value of the `exit` enum (`Exit[A, {}]`).
    Await { exit: EnumId },
    /// `Fiber.interrupt(h)` (spec 11.4). `[fiber]`. Interrupts the child and waits until it has
    /// unwound. The value is `()`.
    Interrupt,
    /// `Fiber.attempt(f)` (spec 11.5). `[f]`. Calls `f` on the spot and turns Fail and Die into
    /// `Failure` of the `exit` enum. Interrupt is not caught. `suspends` tells whether `f` may
    /// suspend (the same rule as `Callee::Indirect` of `Expr::Call::suspends`).
    Attempt { exit: EnumId, suspends: bool },
    /// `Deferred.complete(d, value)` (spec 11.7). `[deferred, value]`. The value is a Bool. Not a
    /// suspension point.
    Complete,
    /// `Deferred.await(d)` (spec 11.7). `[deferred]`. Waits until a value is set.
    DeferredAwait,
    /// `Deferred.poll(d)` (spec 11.7). `[deferred]`. The value is Prelude's Option
    /// (`Module::option_enum`).
    Poll,
    /// `Cause.kind(c)` (spec 11.5). `[cause]`. The value is of the `kind` enum (CauseKind).
    CauseKind { kind: EnumId },
    /// `Cause.to_string(c)` (spec 11.5). `[cause]`. `error: …`, `defect: …` or `interrupted`.
    CauseToString,
    /// `Cause.raise(c)` (spec 11.5). `[cause]`. Fail becomes a failure, Die a defect, and Interrupt
    /// unwinds the current fiber's interruption. The type is Never, but it may fail.
    CauseRaise,
}

/// The index of `Success(A)` in `Exit`.
pub const EXIT_SUCCESS: usize = 0;
/// The index of `Failure(Cause[E])` in `Exit`.
pub const EXIT_FAILURE: usize = 1;
/// The index of `Fail` in `CauseKind`.
pub const CAUSE_FAIL: usize = 0;
/// The index of `Die` in `CauseKind`.
pub const CAUSE_DIE: usize = 1;
/// The index of `Interrupt` in `CauseKind`.
pub const CAUSE_INTERRUPT: usize = 2;

impl FiberOp {
    pub fn arity(self) -> usize {
        match self {
            FiberOp::Fork | FiberOp::Complete => 2,
            _ => 1,
        }
    }

    pub fn suspends(self) -> bool {
        match self {
            FiberOp::Scope { .. }
            | FiberOp::Join
            | FiberOp::Await { .. }
            | FiberOp::Interrupt
            | FiberOp::DeferredAwait => true,
            FiberOp::Attempt { suspends, .. } => suspends,
            FiberOp::Fork
            | FiberOp::Deferred
            | FiberOp::Complete
            | FiberOp::Poll
            | FiberOp::CauseKind { .. }
            | FiberOp::CauseToString
            | FiberOp::CauseRaise => false,
        }
    }

    /// Whether it may fail (whether the result must be checked).
    pub fn fails(self) -> bool {
        match self {
            FiberOp::Scope { fails } => fails,
            FiberOp::CauseRaise => true,
            _ => false,
        }
    }

    /// The name in dumps.
    pub fn name(self) -> &'static str {
        match self {
            FiberOp::Scope { .. } => "scope",
            FiberOp::Fork => "fork",
            FiberOp::Deferred => "deferred",
            FiberOp::Join => "join",
            FiberOp::Await { .. } => "await",
            FiberOp::Interrupt => "interrupt",
            FiberOp::Attempt { .. } => "attempt",
            FiberOp::Complete => "complete",
            FiberOp::DeferredAwait => "deferred_await",
            FiberOp::Poll => "poll",
            FiberOp::CauseKind { .. } => "cause_kind",
            FiberOp::CauseToString => "cause_to_string",
            FiberOp::CauseRaise => "cause_raise",
        }
    }
}

/// One binding of `Expr::With`.
#[derive(Debug, Clone, PartialEq)]
pub struct WithBind {
    pub var: Local,
    pub value: Expr,
    /// If the bound value is a handler value with init and release (spec 8.4), its cleanup. `None`
    /// means the same as `Let`.
    pub release: Option<Release>,
}

impl WithBind {
    /// A binding without release (stateless handler values and evidence tables).
    pub fn new(var: Local, value: Expr) -> Self {
        WithBind {
            var,
            value,
            release: None,
        }
    }

    /// A binding of a handler value with init and release. `value` is the init (it may fail or
    /// suspend).
    pub fn managed(var: Local, value: Expr, handler: &str, release: Expr) -> Self {
        WithBind {
            var,
            value,
            release: Some(Release {
                handler: handler.to_owned(),
                body: release,
            }),
        }
    }
}

/// The cleanup when leaving a `with` (spec 8.4, 14.2).
///
/// After evaluating `body` and discarding its value, the bound handler value is marked released.
/// Calling its operations afterwards is a defect (`handler <name> was used after release`,
/// spec 8.6). `body` may refer to the variables of the same `with` up to and including this
/// binding. `body` cannot fail (release's E is empty, spec 8.4) but may suspend.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// The handler's name (as in source), used in the use-after-release defect message.
    pub handler: String,
    pub body: Expr,
}

/// `assert expr`.
#[derive(Debug, Clone, PartialEq)]
pub struct Assert {
    pub cond: AssertCond,
    /// The source text of the expression (`user.name == "alice"`), shown on failure. May be empty.
    pub text: String,
}

/// The shape of an assert expression. The compiler looks at it and, for a comparison, shows both
/// sides (spec 13.2).
#[derive(Debug, Clone, PartialEq)]
pub enum AssertCond {
    /// `lhs op rhs`, where `op` is one of `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`. Each side is
    /// evaluated once, left first, and compared; if false, both sides are shown in a defect.
    /// `op_ty` is the comparison's meaning (as in `Expr::Binary`), and `ty` is the sides' type
    /// (used to build show functions).
    Compare {
        op: BinOp,
        op_ty: OpTy,
        ty: Type,
        lhs: Expr,
        rhs: Expr,
    },
    /// Not a comparison; only the expression's value is checked.
    Bool(Expr),
}

impl Assert {
    /// A comparison assert.
    pub fn compare(op: BinOp, op_ty: OpTy, ty: Type, lhs: Expr, rhs: Expr, text: &str) -> Expr {
        Expr::Assert(Box::new(Assert {
            cond: AssertCond::Compare {
                op,
                op_ty,
                ty,
                lhs,
                rhs,
            },
            text: text.to_owned(),
        }))
    }

    /// A non-comparison assert.
    pub fn bool(cond: Expr, text: &str) -> Expr {
        Expr::Assert(Box::new(Assert {
            cond: AssertCond::Bool(cond),
            text: text.to_owned(),
        }))
    }

    /// Applies `f` to subexpressions in evaluation order.
    pub fn for_each_operand_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match &mut self.cond {
            AssertCond::Compare { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            AssertCond::Bool(e) => f(e),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    /// A direct call to a top-level function.
    Direct(FnId),
    /// A call to a closure value.
    Indirect(Box<Expr>),
    /// A call to an operation of the handler value `handler` (`log.info(...)`, spec 8.2). `op` is
    /// an index into `EffectDef::ops`. No evidence table is passed (the operation body's effects
    /// were fixed by `with`).
    Op {
        handler: Box<Expr>,
        effect: EffectId,
        op: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pat: Pat,
    /// Evaluated after the pattern matches.
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pat {
    Wild,
    Bind(Local),
    Lit(Lit),
    /// `fields` are in field definition order, with omitted fields filled with `Wild`.
    Ctor {
        ctor: CtorRef,
        fields: Vec<Pat>,
    },
    Tuple(Vec<Pat>),
    /// `[]`，`[x, y]`，`[x, ..rest]`，`[x, ..]`．
    /// With `rest` of `None`, the length is exactly `elems.len()`.
    List {
        elems: Vec<Pat>,
        rest: Option<Box<Pat>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// Embeds the value as a string, using the show function determined by its type (the stand-in
    /// for Show; traits come in 0.21). The type must be concrete, without `Type::Param`.
    Value(Expr, Type),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// Short-circuiting.
    And,
    /// Short-circuiting.
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// The operand type of an operation, used by backends to pick the meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpTy {
    Int,
    Int64,
    Float,
    Bool,
    String,
    /// Only for structural equality (`==` / `!=`).
    Structural,
}
