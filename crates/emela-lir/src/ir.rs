//! The Flow IR data types.
//!
//! A function is a flat store: its variables, statements, statement sequences and labels live in
//! per-function arenas and refer to each other by ID, so analyses can keep their results in side
//! tables keyed by those IDs. The tree shape (which sequence a statement belongs to, which
//! sequences a statement holds) is given by the IDs in [`Seq::stmts`] and in the compound
//! statements.

use emela_core::{Builtin, CtorRef, EffectId, EnumId, External, FiberOp, TypeId};
use la_arena::{Arena, Idx};

use crate::repr::Repr;

pub type FnId = Idx<Function>;
pub type VarId = Idx<VarData>;
pub type StmtId = Idx<Stmt>;
pub type SeqId = Idx<Seq>;
pub type LabelId = Idx<LabelData>;

/// A LIR module. It owns the Core module it was made from: variables' Core types are IDs of its
/// type table, and constructors, enums and effects are those of its definitions. The Core
/// functions are not used (the LIR functions replace them).
///
/// Owned rather than borrowed: a reference would put a lifetime parameter on every type and pass
/// of the crate.
#[derive(Debug, Clone)]
pub struct Module {
    pub core: emela_core::Module,
    pub functions: Arena<Function>,
    /// Which passes have run, and so which constructs the functions may hold.
    pub stage: Stage,
    /// The generated show function of each error enum, made by `from_core` for every error enum
    /// without type parameters that the functions construct, called or not: the error's display
    /// (`emela_core::Module::error_display`) decides between the structural `show$E`, the function
    /// of its Show impl, and `name$E` (the name only) for an error that does not satisfy Show.
    /// Backends show a failure that reaches the entry with it (`error: NotFound(id: 1)`,
    /// `error: Retry`, `[dyn.error]`); which enum arrives is only known at run time.
    pub error_shows: Vec<(EnumId, FnId)>,
}

/// The stage of a LIR module. The Flow IR and the Heap IR share the data types; each pass of the
/// Heap IR forbids some constructs and allows new ones, and the verifier checks the invariants of
/// the module's stage. Later stages include the earlier ones' invariants.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Made by `from_core`: closures nested in their functions, `defect` and `protect` with
    /// implicit unwinding, no reference counting.
    Flow,
    /// Closures lifted to functions taking their record as the environment
    /// ([`Value::MakeClosure`], [`Value::EnvGet`]); no [`Value::Closure`] or [`Value::FnRef`].
    ClosuresConverted,
    /// Defects and cleanups made explicit control: calls that may raise a defect have a landing
    /// (`on_defect`), failure edges have landings ([`OnFail::Handle`]), [`Term::Unwind`] leaves a
    /// function with the pending defect, and the cleanups of `protect` are copied onto every way
    /// out of its body. No [`Stmt::Protect`], [`Term::Defect`], [`OnFail::Break`] or
    /// [`OnFail::Propagate`].
    DefectsExplicit,
    /// Suspending functions made resumable state machines: every value of a suspending function
    /// that may come back suspended has an `on_suspend` edge ([`Suspend`]) that saves the
    /// variables still needed into a heap frame, and fairness yields ([`RuntimeOp::Yield`],
    /// [`RuntimeOp::YieldPerform`]) come before suspending calls.
    StateMachines,
    /// Reference counts explicit: [`Stmt::Dup`] and [`Stmt::Drop`] make every owned reference be
    /// consumed exactly once on every path (see `heap::rc` for who owns what).
    RefCounted,
}

impl Module {
    pub fn new(core: emela_core::Module) -> Self {
        Module {
            core,
            functions: Arena::new(),
            stage: Stage::Flow,
            error_shows: Vec::new(),
        }
    }

    /// An empty function, defined later with [`Module::define`] (for mutual recursion). `name`
    /// must be unique in the module.
    pub fn declare(&mut self, name: &str) -> FnId {
        debug_assert!(
            self.functions.iter().all(|(_, f)| f.name != name),
            "duplicate function name `{name}`"
        );
        self.functions.alloc(Function::new(name))
    }

    /// Replaces the declared function. The names must agree.
    pub fn define(&mut self, id: FnId, f: Function) {
        debug_assert_eq!(
            self.functions[id].name, f.name,
            "defining a different function"
        );
        self.functions[id] = f;
    }

    /// Declares and defines in one step.
    pub fn add(&mut self, f: Function) -> FnId {
        let id = self.declare(&f.name);
        self.functions[id] = f;
        id
    }
}

/// A function, with its own arenas.
#[derive(Debug, Clone)]
pub struct Function {
    /// Unique in the module.
    pub name: String,
    pub exported: bool,
    /// Only parameters with a representation: a parameter of type `()` or `Never` has no variable
    /// and no argument. The evidence table, if taken, is an ordinary last parameter (`ref`, Core
    /// type `Evidence`).
    pub params: Vec<VarId>,
    /// The Core type of the return value. The function returns nothing if its representation is
    /// [`Repr::Void`].
    pub ret: TypeId,
    /// Whether it may fail: `fail` and the `on_fail fail` edge are allowed only then, and direct
    /// calls to it must have a failure edge.
    pub fails: bool,
    /// Whether it may suspend (`emela_core::Function::suspends`).
    pub suspends: bool,
    /// A const's value (`emela_core::Function::constant`).
    pub constant: bool,
    /// For an external function, the host function; `body` is then `None`.
    pub external: Option<External>,
    /// For a function lifted from a closure (Heap IR), the Core types of the captured values, in
    /// the order of [`Value::MakeClosure::captures`]. Its first parameter is then the environment:
    /// the closure record itself (`ref`, with the closure's function type as its Core type),
    /// borrowed for the call. [`Value::EnvGet`] reads the captures from it.
    pub env: Option<Vec<TypeId>>,
    /// Heap IR, from [`Stage::DefectsExplicit`]: whether a call may come back with a pending
    /// defect, so that calls to it need a defect landing. External functions may (a host
    /// exception is a defect, spec 12.5).
    pub defects: bool,
    /// The body. Every path ends in a terminator other than [`Term::Yield`].
    pub body: Option<SeqId>,
    pub vars: Arena<VarData>,
    pub stmts: Arena<Stmt>,
    pub seqs: Arena<Seq>,
    pub labels: Arena<LabelData>,
}

impl Function {
    /// A function without parameters returning `()`, with no body yet.
    pub fn new(name: &str) -> Self {
        Function {
            name: name.to_owned(),
            exported: false,
            params: Vec::new(),
            ret: TypeId::UNIT,
            fails: false,
            suspends: false,
            constant: false,
            external: None,
            env: None,
            defects: false,
            body: None,
            vars: Arena::new(),
            stmts: Arena::new(),
            seqs: Arena::new(),
            labels: Arena::new(),
        }
    }
}

/// A variable. Defined exactly once (as a parameter, a statement's destination, a loop variable or
/// a closure parameter) and never assigned again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarData {
    /// A naming hint for output; need not be unique.
    pub hint: String,
    /// How the value is held. Never [`Repr::Void`]: values without a representation get no
    /// variable.
    pub repr: Repr,
    /// The Core type, an ID of the module's Core type table. `repr` is `repr_of` this type.
    pub ty: TypeId,
}

/// A label of a [`Stmt::Block`], the target of [`Term::Break`] and [`OnFail::Break`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelData {
    pub hint: String,
}

/// A sequence of statements run in order, then its terminator.
#[derive(Debug, Clone, PartialEq)]
pub struct Seq {
    pub stmts: Vec<StmtId>,
    pub term: Term,
}

/// How a sequence ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// Leaves the sequence normally, giving the value of the enclosing compound statement: the
    /// arm's value for `if` and `switch`, the block's value, the loop's value (leaving the loop),
    /// the protected body's value. Without a value when the statement has no destination. Not
    /// allowed at the end of a function or closure body, and a cleanup ends with it without value.
    Yield(Option<Operand>),
    /// Returns from the enclosing function or closure. Without a value when it returns nothing.
    Return(Option<Operand>),
    /// Fails the enclosing function or closure with the error value (`ref`).
    Fail(Operand),
    /// Leaves the enclosing block with the label, giving its value.
    Break {
        label: LabelId,
        value: Option<Operand>,
    },
    /// Starts the next iteration of the innermost enclosing loop with new values for its
    /// variables. It cannot cross a `protect` or a closure: in Core the body of `with` is not in
    /// tail position either, and a closure is another body.
    Recur(Vec<Operand>),
    /// Raises a defect with the message (a string operand, since some messages, such as those of
    /// `assert`, are built at run time), unwinding through the cleanups to the caller. The
    /// second field is the location of a failed assert (`<path>:<line>:<column>`,
    /// `[test.assert]`), kept outside the message. Flow IR only: the Heap IR makes the unwinding
    /// explicit with [`RuntimeOp::Defect`] (or [`RuntimeOp::DefectAt`]) and [`Term::Unwind`].
    Defect(Operand, Option<String>),
    /// Heap IR: returns from the function with the pending defect (the caller's defect landing
    /// runs). The function's result is not given (backends return any value).
    Unwind,
    /// Control never gets here (after a call that does not return, such as `panic`).
    Unreachable,
}

/// A statement. Each optional `dst` is the variable defined by the statement, visible to the
/// statements after it in the same sequence (and the sequences nested in those). `None` when the
/// value has no representation or is unused.
///
/// A compound statement that gives a value (`if`, `switch`, `block`, `loop`, `protect`) has its
/// result variable as `dst`, and its arms end with `yield v`, rather than declaring the variable
/// first and assigning it in each arm. So every variable is defined exactly once: the verifier
/// checks definition before use by scope alone, and liveness and reference counting stay simple.
/// WASM maps it to the result of `block` and `if`, C to an assignment of a declared variable.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `dst = value`．`on_defect` (Heap IR) is the landing run instead of the rest of the sequence
    /// when the value (a call, builtin or concurrency operation that may raise a defect) comes
    /// back with a pending defect. It does not see `dst` and cannot yield. `on_suspend` (Heap IR,
    /// from [`Stage::StateMachines`], in suspending functions) is what happens when the value
    /// comes back suspended.
    Let {
        dst: Option<VarId>,
        value: Value,
        on_defect: Option<SeqId>,
        on_suspend: Option<Suspend>,
    },
    /// Runs `then` if `cond` (an `i32`) is not zero, else `else_`.
    If {
        dst: Option<VarId>,
        cond: Operand,
        then: SeqId,
        else_: SeqId,
    },
    /// Runs the case matching the scrutinee, or `default`. Without a default, a tag switch must
    /// cover every constructor; a literal switch always has a default.
    Switch {
        dst: Option<VarId>,
        on: SwitchOn,
        cases: Vec<Case>,
        default: Option<SeqId>,
    },
    /// Binds `vars` to `inits` and runs `body`; [`Term::Recur`] rebinds them and runs it again, and
    /// [`Term::Yield`] leaves the loop with its value. Made from `emela_core::Expr::Loop`.
    Loop {
        dst: Option<VarId>,
        vars: Vec<VarId>,
        inits: Vec<Operand>,
        body: SeqId,
    },
    /// Runs `body`; [`Term::Break`] with `label` from inside leaves it early. The receiver of
    /// `escape` and the target of failure edges.
    Block {
        dst: Option<VarId>,
        label: LabelId,
        body: SeqId,
    },
    /// Runs `body`, then `cleanup` however `body` is left (value, break, return, failure, defect,
    /// unwinding while suspended). `cleanup` cannot leave by any other way than its end. Made from
    /// the releases of `with` (spec 14.2).
    Protect {
        dst: Option<VarId>,
        body: SeqId,
        cleanup: SeqId,
    },
    /// Heap IR: adds a reference to the `ref` variable's value. Backends expand it to a call of
    /// the runtime (or an inline increment); static data is marked so that it is never counted.
    Dup(VarId),
    /// Heap IR: gives up a reference to the `ref` variable's value, freeing it (and dropping what
    /// it holds) when it was the last one.
    Drop(VarId),
}

impl Stmt {
    /// The variable the statement defines.
    pub fn dst(&self) -> Option<VarId> {
        match self {
            Stmt::Let { dst, .. }
            | Stmt::If { dst, .. }
            | Stmt::Switch { dst, .. }
            | Stmt::Loop { dst, .. }
            | Stmt::Block { dst, .. }
            | Stmt::Protect { dst, .. } => *dst,
            Stmt::Dup(_) | Stmt::Drop(_) => None,
        }
    }
}

/// The suspension edge of a value in a suspending function (Heap IR, from
/// [`Stage::StateMachines`]): a resume point of the function's state machine.
///
/// The value (a suspending call, a waiting concurrency operation or a fairness yield) either comes
/// back as usual (fast path: nothing is allocated, the variables stay in locals), or comes back
/// suspended. Then a frame of the function is allocated with `state` and the `saved` variables
/// (moved into it), pushed onto the continuation being captured, and the function returns
/// (suspended) to its caller, which does the same up to the runtime's driver loop. When the
/// runtime resumes the frame, the function is entered again with the frame: the saved variables
/// are restored and control continues right after the value as if it had come back with the
/// resumed input (its value in `dst`, or a pending failure or defect for the landings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspend {
    /// The resume point, unique in the function (numbered from 0).
    pub state: u32,
    /// The variables saved in the frame, in slot order: every variable live after the value (on
    /// any way on, the landings included) and the references the value borrows, so that the
    /// resumed function owns exactly what the uninterrupted one owns after the value.
    pub saved: Vec<VarId>,
}

/// What a switch looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchOn {
    /// The constructor of an enum value; cases are [`CaseKey::Variant`].
    Tag(VarId),
    /// An `i32` or `i64` value; cases are [`CaseKey::Lit`] of the same representation.
    Lit(VarId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub key: CaseKey,
    pub body: SeqId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaseKey {
    /// An index into the enum's `variants`.
    Variant(usize),
    Lit(Const),
}

/// An argument: a variable or a constant (ANF).
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Var(VarId),
    Const(Const),
}

impl From<VarId> for Operand {
    fn from(v: VarId) -> Self {
        Operand::Var(v)
    }
}

impl From<Const> for Operand {
    fn from(c: Const) -> Self {
        Operand::Const(c)
    }
}

/// A constant.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Int(i32),
    Int64(i64),
    Float(f64),
    Bool(bool),
    String(String),
    /// The placeholder for `()` in a slot of a container: a constructor field, a tuple element, a
    /// list element, a cell's contents or a concurrency operation's argument whose type has no
    /// representation. It keeps the slot (so `List[()]` keeps its length and field indices stay
    /// Core's); backends store any word there. Its representation is [`Repr::Void`], so it is
    /// accepted only in such slots.
    Unit,
}

impl Const {
    pub fn repr(&self) -> Repr {
        match self {
            Const::Int(_) | Const::Bool(_) => Repr::I32,
            Const::Int64(_) => Repr::I64,
            Const::Float(_) => Repr::F64,
            Const::String(_) => Repr::Ref,
            Const::Unit => Repr::Void,
        }
    }
}

/// The right-hand side of [`Stmt::Let`]. Arguments are operands, and values that must be
/// variables (bases, callees, closures' captures) are [`VarId`]s.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Const(Const),
    /// A numeric operation or comparison.
    Prim {
        op: PrimOp,
        ty: PrimTy,
        args: Vec<Operand>,
    },
    /// A call. Arguments are the callee's parameters that have a representation, with the
    /// evidence table last if it takes one. `on_fail` is the failure edge, present exactly when
    /// the callee may fail. `suspends` means the same as `emela_core::Expr::Call::suspends`.
    Call {
        callee: Callee,
        args: Vec<Operand>,
        suspends: bool,
        on_fail: Option<OnFail>,
    },
    /// A builtin. After monomorphisation, so without type arguments. Builtins do not fail.
    Builtin {
        op: Builtin,
        args: Vec<Operand>,
    },
    /// An enum value. Arguments are the constructor's fields in field order; a field without
    /// representation gets [`Const::Unit`].
    Make {
        ctor: CtorRef,
        args: Vec<Operand>,
    },
    /// Field `index` (in field definition order) of `base`, which must be a value of `ctor`. A
    /// field without representation has nothing to read (no destination).
    Field {
        base: VarId,
        ctor: CtorRef,
        index: usize,
    },
    /// The constructor index of an enum value (`i32`).
    Tag(VarId),
    /// A tuple. An element without representation gets [`Const::Unit`].
    Tuple(Vec<Operand>),
    /// Element `index` of a tuple.
    TupleGet {
        base: VarId,
        index: usize,
    },
    /// `[a, b]` or `[a, b, ..tail]`. Elements without representation are [`Const::Unit`].
    List {
        elems: Vec<Operand>,
        tail: Option<Operand>,
    },
    /// Whether the list is empty (`i32`).
    ListIsEmpty(VarId),
    /// The first element of a non-empty list (nothing to read, so no destination, for elements
    /// without representation).
    ListHead(VarId),
    /// The rest of a non-empty list.
    ListTail(VarId),
    /// A top-level function used as a value. Flow IR only (closure conversion makes it a
    /// [`Value::MakeClosure`] of a wrapper).
    FnRef(FnId),
    /// A closure. Its body is nested here. Flow IR only (closure conversion lifts it).
    Closure(Box<Closure>),
    /// Heap IR: a closure record `{function, captures…}`. `function` is a lifted function (its
    /// `env` lists the captures' Core types); calling the record (`call_indirect`, or `call_op`
    /// through a handler) calls `function` with the record itself as the first argument (the
    /// environment), then the call's arguments. The record is the environment: captures live in
    /// it directly, so making a closure is one allocation.
    MakeClosure {
        function: FnId,
        captures: Vec<VarId>,
    },
    /// Heap IR: capture `index` of the environment `env`, the first parameter of a lifted
    /// function.
    EnvGet {
        env: VarId,
        index: usize,
    },
    /// The current handler of the effect in the evidence table (`emela_core::Expr::Use`).
    EvLookup {
        ev: VarId,
        effect: EffectId,
    },
    /// A new evidence table: `base` (or an empty table) plus `bindings`, later bindings winning
    /// (`emela_core::Expr::Provide`).
    EvExtend {
        base: Option<VarId>,
        bindings: Vec<(EffectId, VarId)>,
    },
    /// A handler value (`emela_core::Expr::Handler`): one closure per operation of the effect, in
    /// declaration order.
    Handler {
        name: String,
        effect: EffectId,
        ops: Vec<VarId>,
    },
    /// A concurrency operation (spec chapter 11), done by the runtime. Arguments are as for
    /// `emela_core::Expr::Fiber`, with [`Const::Unit`] for one without representation. `on_fail`
    /// is present exactly when the operation may fail (`FiberOp::fails`).
    Fiber {
        op: FiberOp,
        args: Vec<Operand>,
        on_fail: Option<OnFail>,
    },
    /// Whether the caught failure `error` (`ref`) is a value of the error enum (`i32`). Used by the
    /// arms of `escape`, where failures of different error enums arrive at the same block.
    IsError {
        error: VarId,
        enum_id: EnumId,
    },
    /// The caught failure `error` seen as a value of the error enum, after [`Value::IsError`] said
    /// it is one. Defines a variable of that enum type.
    AsError {
        error: VarId,
        enum_id: EnumId,
    },
    /// An operation of the runtime that the language has no builtin for.
    Runtime {
        op: RuntimeOp,
        args: Vec<Operand>,
    },
    /// The variable's value at the destination's Core type, which differs from the variable's:
    /// a function value used at a function type with other `fails` / `suspends` flags (a smaller
    /// E or R used where a larger one is expected, spec 9.2), or a value entering or leaving
    /// `Any`. The representations must agree unless one side is `Any` (then the value is boxed or
    /// unboxed). Made where a `let` binds a value to a local of another type: the failure edge
    /// and `suspends` of a call through a function value follow the called variable's type, so
    /// the local must have its own type or the verifier disagrees. Backends whose calling
    /// conventions depend on those flags adapt the value here.
    Coerce(VarId),
}

impl Value {
    /// Applies `f` to every variable the value uses, in operand order. A closure uses its
    /// captures (not the variables of its body).
    pub fn for_each_var(&self, mut f: impl FnMut(VarId)) {
        let ops = |os: &[Operand], f: &mut dyn FnMut(VarId)| {
            for o in os {
                if let Operand::Var(v) = o {
                    f(*v);
                }
            }
        };
        match self {
            Value::Const(_) | Value::FnRef(_) => {}
            Value::Prim { args, .. }
            | Value::Builtin { args, .. }
            | Value::Make { args, .. }
            | Value::Tuple(args)
            | Value::Fiber { args, .. }
            | Value::Runtime { args, .. } => ops(args, &mut f),
            Value::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct(_) => {}
                    Callee::Indirect(v) | Callee::Op { handler: v, .. } => f(*v),
                }
                ops(args, &mut f);
            }
            Value::Field { base: v, .. }
            | Value::Tag(v)
            | Value::TupleGet { base: v, .. }
            | Value::ListIsEmpty(v)
            | Value::ListHead(v)
            | Value::ListTail(v)
            | Value::EvLookup { ev: v, .. }
            | Value::IsError { error: v, .. }
            | Value::AsError { error: v, .. }
            | Value::Coerce(v) => f(*v),
            Value::List { elems, tail } => {
                ops(elems, &mut f);
                ops(tail.as_slice(), &mut f);
            }
            Value::Closure(c) => c.captures.iter().for_each(|&v| f(v)),
            Value::MakeClosure { captures, .. } => captures.iter().for_each(|&v| f(v)),
            Value::EnvGet { env, .. } => f(*env),
            Value::EvExtend { base, bindings } => {
                base.iter().for_each(|&v| f(v));
                bindings.iter().for_each(|&(_, v)| f(v));
            }
            Value::Handler { ops: hs, .. } => hs.iter().for_each(|&v| f(v)),
        }
    }
}

impl Value {
    /// Applies `f` to every variable the value uses, in operand order, letting it replace them
    /// (the same variables as [`Value::for_each_var`]).
    pub fn for_each_var_mut(&mut self, mut f: impl FnMut(&mut VarId)) {
        let ops = |os: &mut [Operand], f: &mut dyn FnMut(&mut VarId)| {
            for o in os {
                if let Operand::Var(v) = o {
                    f(v);
                }
            }
        };
        match self {
            Value::Const(_) | Value::FnRef(_) => {}
            Value::Prim { args, .. }
            | Value::Builtin { args, .. }
            | Value::Make { args, .. }
            | Value::Tuple(args)
            | Value::Fiber { args, .. }
            | Value::Runtime { args, .. } => ops(args, &mut f),
            Value::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct(_) => {}
                    Callee::Indirect(v) | Callee::Op { handler: v, .. } => f(v),
                }
                ops(args, &mut f);
            }
            Value::Field { base: v, .. }
            | Value::Tag(v)
            | Value::TupleGet { base: v, .. }
            | Value::ListIsEmpty(v)
            | Value::ListHead(v)
            | Value::ListTail(v)
            | Value::EvLookup { ev: v, .. }
            | Value::IsError { error: v, .. }
            | Value::AsError { error: v, .. }
            | Value::Coerce(v) => f(v),
            Value::List { elems, tail } => {
                ops(elems, &mut f);
                if let Some(t) = tail {
                    ops(std::slice::from_mut(t), &mut f);
                }
            }
            Value::Closure(c) => c.captures.iter_mut().for_each(&mut f),
            Value::MakeClosure { captures, .. } => captures.iter_mut().for_each(&mut f),
            Value::EnvGet { env, .. } => f(env),
            Value::EvExtend { base, bindings } => {
                base.iter_mut().for_each(&mut f);
                bindings.iter_mut().for_each(|(_, v)| f(v));
            }
            Value::Handler { ops: hs, .. } => hs.iter_mut().for_each(&mut f),
        }
    }
}

/// Runtime operations that Core has no builtin for, made by the conversion from Core.
///
/// They live here rather than as Core builtins because a Core builtin also needs an `@intrinsic`
/// declaration in the bundled sources and an implementation in the JS backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeOp {
    /// `(String) -> String`: a string shown inside a value or by `dbg` (spec 10.6), in `"` with
    /// the escapes of `$showString` in the JS runtime.
    ShowString,
    /// `(String) -> ()`: writes the line to stderr (`dbg`).
    DebugPrint,
    /// `(handler, String) -> ()`: marks the handler value released, so that each of its operations
    /// is the defect `handler <name> was used after release` from then on (spec 8.6). The string
    /// is the handler's name.
    Retire,
    /// `(String) -> ()`: makes the message the pending defect. If a defect is already pending (a
    /// release failing while unwinding, spec 14.4), the message becomes a note of it instead.
    /// Heap IR; a [`Term::Unwind`] (through the cleanups) follows.
    Defect,
    /// `(String, String) -> ()`: [`RuntimeOp::Defect`] for a failed assert, whose defect carries
    /// the location (the second string, a constant) outside its message (`[test.assert]`). The
    /// location stays with the defect when a note is added to its message. Heap IR.
    DefectAt,
    /// `() -> ref`: takes the pending defect out, so that a cleanup runs with no defect pending.
    /// Heap IR.
    DefectTake,
    /// `(ref) -> ()`: puts back a defect taken by [`RuntimeOp::DefectTake`] as the pending one.
    /// A defect raised meanwhile (by the cleanup) becomes a note of it (spec 14.4: the first
    /// defect is the result). Consumes the argument. Heap IR.
    DefectResume,
    /// `() -> ()`: marks the start of a release that may suspend: the current fiber is not
    /// interrupted until [`RuntimeOp::Unhold`] (spec 11.4).
    Hold,
    /// `() -> ()`: the end of a release started with [`RuntimeOp::Hold`].
    Unhold,
    /// `() -> ()`: a fairness yield before a suspending call (spec 11.1). Spends one unit of the
    /// running fiber's budget; when it runs out, the fiber suspends and goes to the end of the
    /// queue (and a pending interruption is delivered instead, as a defect). Heap IR, in
    /// suspending functions only.
    Yield,
    /// `() -> ()`: [`RuntimeOp::Yield`] before a call of a `suspend fn` operation, which also
    /// delivers a pending interruption (spec 11.4: the call is a suspension point). Heap IR.
    YieldPerform,
}

impl RuntimeOp {
    pub fn name(self) -> &'static str {
        match self {
            RuntimeOp::ShowString => "show_string",
            RuntimeOp::DebugPrint => "debug_print",
            RuntimeOp::Retire => "retire",
            RuntimeOp::Defect => "defect",
            RuntimeOp::DefectAt => "defect_at",
            RuntimeOp::DefectTake => "defect_take",
            RuntimeOp::DefectResume => "defect_resume",
            RuntimeOp::Hold => "hold",
            RuntimeOp::Unhold => "unhold",
            RuntimeOp::Yield => "yield",
            RuntimeOp::YieldPerform => "yield_perform",
        }
    }

    /// The representations of the arguments and of the result.
    pub fn sig(self) -> (&'static [Repr], Repr) {
        match self {
            RuntimeOp::ShowString => (&[Repr::Ref], Repr::Ref),
            RuntimeOp::DebugPrint => (&[Repr::Ref], Repr::Void),
            RuntimeOp::Retire | RuntimeOp::DefectAt => (&[Repr::Ref, Repr::Ref], Repr::Void),
            RuntimeOp::Defect | RuntimeOp::DefectResume => (&[Repr::Ref], Repr::Void),
            RuntimeOp::DefectTake => (&[], Repr::Ref),
            RuntimeOp::Hold | RuntimeOp::Unhold | RuntimeOp::Yield | RuntimeOp::YieldPerform => {
                (&[], Repr::Void)
            }
        }
    }
}

/// A closure value. It shares the function's arenas. Its body may use the variables of the
/// enclosing bodies only if they are listed in `captures`. Its Core type is that of the
/// statement's destination, which gives its parameters, `suspends` and `fails`.
#[derive(Debug, Clone, PartialEq)]
pub struct Closure {
    /// With the evidence table last if it takes one (the closure's Core type says).
    pub params: Vec<VarId>,
    /// The variables of the enclosing bodies the body uses, each once, in first-use order.
    pub captures: Vec<VarId>,
    pub body: SeqId,
    pub suspends: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    /// A top-level function.
    Direct(FnId),
    /// A function value (`ref`).
    Indirect(VarId),
    /// Operation `op` (an index into `EffectDef::ops`) of the handler value `handler`.
    Op {
        handler: VarId,
        effect: EffectId,
        op: usize,
    },
}

/// Where a failing call goes. The error value (`ref`) goes with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnFail {
    /// Leaves the enclosing block with the label, giving the error as its value. Flow IR only.
    Break(LabelId),
    /// Fails the enclosing function or closure (as [`Term::Fail`]). Flow IR only.
    Propagate,
    /// Heap IR: binds the error to `error` and runs the landing `body`, which cannot yield (it
    /// breaks, fails, returns or unwinds). Each failing call has its own landing, so the
    /// reference counting pass can release what differs between them.
    Handle { error: VarId, body: SeqId },
}

/// Numeric operations and comparisons. Comparisons give an `i32` (0 or 1); the others give the
/// operand type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimOp {
    Add,
    Sub,
    Mul,
    /// For `i32` and `i64`, Int's division (spec 16.1): a defect when dividing by zero.
    Div,
    Rem,
    /// Unary.
    Neg,
    /// Unary; the logical not of an `i32` truth value (Bool).
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl PrimOp {
    pub fn arity(self) -> usize {
        match self {
            PrimOp::Neg | PrimOp::Not => 1,
            _ => 2,
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            PrimOp::Eq | PrimOp::Ne | PrimOp::Lt | PrimOp::Le | PrimOp::Gt | PrimOp::Ge
        )
    }

    pub fn symbol(self) -> &'static str {
        match self {
            PrimOp::Add => "+",
            PrimOp::Sub => "-",
            PrimOp::Mul => "*",
            PrimOp::Div => "/",
            PrimOp::Rem => "%",
            PrimOp::Neg => "-",
            PrimOp::Not => "!",
            PrimOp::Eq => "==",
            PrimOp::Ne => "!=",
            PrimOp::Lt => "<",
            PrimOp::Le => "<=",
            PrimOp::Gt => ">",
            PrimOp::Ge => ">=",
        }
    }
}

/// The operand type of a [`PrimOp`]: a representation, plus strings, which are compared by
/// contents (only comparisons).
///
/// Structural equality is not a primitive: like showing, it becomes calls of functions generated
/// per type (`eq$Point`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimTy {
    I32,
    I64,
    F64,
    String,
}

impl PrimTy {
    pub fn repr(self) -> Repr {
        match self {
            PrimTy::I32 => Repr::I32,
            PrimTy::I64 => Repr::I64,
            PrimTy::F64 => Repr::F64,
            PrimTy::String => Repr::Ref,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PrimTy::I32 => "i32",
            PrimTy::I64 => "i64",
            PrimTy::F64 => "f64",
            PrimTy::String => "string",
        }
    }
}
