//! The Core IR.
//!
//! The typed tree after desugaring; backends (JS / WASM) see only this. Everything is referred to
//! by ID: functions by `FnId`, locals by `Local`, enums by `EnumId`, types by `TypeId`. Display names (`name` /
//! `hint`) exist only to make output readable.

use la_arena::{Arena, Idx};
use rustc_hash::FxHashMap;

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
    /// Host files bundled with the compiler that the output needs (`.mjs` for JS). Present only
    /// when an external function points to a bundled file ([`HostModule::Bundled`]); then all of
    /// them are here, so relative imports between them work. Where they go in the output is up to
    /// the backend.
    pub bundled_files: Vec<BundledFile>,
    /// Every type the module mentions, each once.
    pub types: TypeTable,
    /// Trait functions (spec 10.3) → their impls. A call of a trait function is generic in `Self`,
    /// its first type argument; [`crate::mono`] replaces it with a direct call of the impl
    /// function for that type (no dictionary passing). Empty after monomorphization.
    pub dispatch: FxHashMap<FnId, Dispatch>,
    /// The Prelude's trait functions, if lowered. [`crate::mono`] calls them where `==`,
    /// interpolation and `Builtin::ShowNested` cannot use the structure (spec 10.6).
    pub traits: PreludeTraits,
    /// Every trait's impls, written and derived, indexed by [`TraitRef`]: what [`crate::mono`]
    /// needs to tell whether a type satisfies Show where the type checker does not ask (`dbg`,
    /// assert, errors reaching the entry, spec 10.6, 13.2, 14.3). Kept by monomorphization, which
    /// the C and WASM backends run again on its output.
    pub trait_impls: Vec<TraitImpls>,
    /// The Prelude's Show in [`Module::trait_impls`].
    pub show_trait: Option<TraitRef>,
    /// How a failure reaching the entry or a test shows an error enum (spec 13.3, 14.3). Filled by
    /// [`crate::mono`] for the errors the functions construct. An error not here is shown by its
    /// name (its type does not satisfy Show).
    pub error_display: FxHashMap<EnumId, ErrorDisplay>,
}

/// A trait, by its index in [`Module::trait_impls`].
pub type TraitRef = usize;

/// The impls of one trait ([`Module::trait_impls`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraitImpls {
    /// Eq, Ord or Show of the Prelude, which the compiler implements for the basic types and
    /// tuples (spec 10.6; Float has no Ord).
    pub builtin: Option<DeriveOp>,
    /// Per impl (written or derived), its type with the impl's type parameters as `Type::Param`
    /// (`Wrap[$0]`), and per type parameter the traits it is constrained with (`[A: Eq]`).
    pub impls: Vec<(TypeId, Vec<Vec<TraitRef>>)>,
}

/// How an error reaching the entry is shown ([`Module::error_display`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorDisplay {
    /// By the backend's display of the structure, which is what its Show gives (derived only).
    Structure,
    /// By calling the function, its Show impl (`fn(E) -> String`).
    Impl(FnId),
}

/// The IR functions of the Prelude's trait functions ([`Module::traits`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreludeTraits {
    pub eq: Option<FnId>,
    pub compare: Option<FnId>,
    pub show: Option<FnId>,
}

/// The impls of one trait function.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dispatch {
    /// The impl's type, with the impl's type parameters as `Type::Param` (`Pair[$0, $1]`), and the
    /// impl function. The impl function's type parameters are the impl's, then the trait
    /// function's own (all but `Self`).
    pub impls: Vec<(TypeId, FnId)>,
    /// The trait function has a default implementation: its own body, used for types whose impl
    /// does not define the function.
    pub default: bool,
    /// Impl functions that give what the structural operation gives (`==` by `OpTy::Structural`,
    /// interpolation's show): the impls of the compiler, and those of core for `List` and `Option`
    /// (spec 10.6). Other impls in core and std (Duration's Show) are written ones.
    pub structural: Vec<FnId>,
    /// For the functions of the derivable traits (`Eq.eq`, `Ord.compare`, `Show.show`), which
    /// one, and the enums that derive the trait (spec 10.5). [`crate::mono`] generates their
    /// impl functions per type.
    pub derived: Option<Derived>,
}

/// The enums deriving a trait, for one function of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub op: DeriveOp,
    pub enums: Vec<EnumId>,
}

/// The function of a derivable trait (spec 10.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeriveOp {
    /// `Eq.eq(self, other) -> Bool`.
    Eq,
    /// `Ord.compare(self, other) -> Ordering`.
    Compare,
    /// `Show.show(self) -> String`.
    Show,
}

/// A host file bundled with the compiler (under `lib/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledFile {
    /// Relative to `lib/`, `/`-separated (`std/clock.mjs`). [`HostModule::Bundled`] uses the same
    /// path.
    pub path: String,
    pub contents: String,
}

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    /// The structure of the type.
    pub fn ty(&self, id: TypeId) -> &Type {
        &self.types[id]
    }

    /// The ID of the type (`TypeTable::intern`).
    pub fn intern(&mut self, ty: Type) -> TypeId {
        self.types.intern(ty)
    }

    pub fn list(&mut self, elem: TypeId) -> TypeId {
        self.types.list(elem)
    }

    pub fn tuple(&mut self, elems: Vec<TypeId>) -> TypeId {
        self.types.tuple(elems)
    }

    pub fn enum_type(&mut self, id: EnumId, args: Vec<TypeId>) -> TypeId {
        self.types.enum_type(id, args)
    }

    pub fn func(&mut self, f: FnType) -> TypeId {
        self.types.func(f)
    }

    pub fn cell(&mut self, content: TypeId) -> TypeId {
        self.types.cell(content)
    }

    pub fn type_param(&mut self, i: usize) -> TypeId {
        self.types.type_param(i)
    }

    pub fn handler(&mut self, effect: EffectId) -> TypeId {
        self.types.handler(effect)
    }

    /// A function type that neither fails, suspends nor takes the evidence table.
    pub fn fn_type(&mut self, params: Vec<TypeId>, ret: TypeId) -> TypeId {
        self.types.func(FnType::pure(params, ret))
    }

    /// `hint` is a naming hint for the output and need not be unique.
    pub fn local(&mut self, hint: &str, ty: impl IntoTypeId) -> Local {
        let ty = ty.into_type_id(&mut self.types);
        let hint = if hint.is_empty() { "v" } else { hint };
        self.locals.alloc(LocalData {
            hint: hint.to_owned(),
            ty,
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
            ret: TypeId::UNIT,
            type_params: 0,
            set_params: Vec::new(),
            fails: false,
            body: Expr::Lit(Lit::Unit),
            external: None,
            suspends: false,
            constant: false,
            sets: None,
        })
    }

    /// Makes the function external (spec chapter 12). The body is unused.
    pub fn define_external(
        &mut self,
        f: FnId,
        params: Vec<Local>,
        ret: impl IntoTypeId,
        external: External,
    ) {
        let ret = ret.into_type_id(&mut self.types);
        let func = &mut self.functions[f];
        func.params = params;
        func.ret = ret;
        func.external = Some(external);
    }

    /// `name` must be unique in the module.
    /// The effect whose handler values a table key holds: the base for a label, else `effect`.
    pub fn handler_effect(&self, effect: EffectId) -> EffectId {
        self.effects[effect].label_of.unwrap_or(effect)
    }

    pub fn add_effect(&mut self, def: EffectDef) -> EffectId {
        debug_assert!(
            self.effects.iter().all(|(_, e)| e.name != def.name),
            "duplicate effect name `{}`",
            def.name
        );
        self.effects.alloc(def)
    }

    pub fn define(&mut self, f: FnId, params: Vec<Local>, ret: impl IntoTypeId, body: Expr) {
        let ret = ret.into_type_id(&mut self.types);
        let func = &mut self.functions[f];
        func.params = params;
        func.ret = ret;
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
        let a = self.types.type_param(0);
        let id = self.enums.alloc(EnumDef::option(a));
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

    /// Whether the function takes the evidence table as its last argument.
    pub fn takes_ev(&self, f: FnId) -> bool {
        self.functions[f]
            .params
            .last()
            .is_some_and(|&p| self.locals[p].ty == TypeId::EVIDENCE)
    }
}

#[derive(Debug, Clone)]
pub struct LocalData {
    pub hint: String,
    pub ty: TypeId,
}

#[derive(Debug, Clone)]
pub struct Function {
    /// Unique in the module; exported under this name.
    pub name: String,
    pub exported: bool,
    /// Their types are the locals' types (`LocalData::ty`). The evidence table, if taken, is last,
    /// and it is the only parameter of type `Type::Evidence` ([`Module::takes_ev`]).
    pub params: Vec<Local>,
    pub ret: TypeId,
    /// The number of type parameters, referred to by `Type::Param(i)` in the types of the
    /// parameters, the return value and the locals of the body. Includes set parameters (`R(f)`),
    /// which no type refers to, so that indices match the type checker's.
    pub type_params: usize,
    /// The indices of the set type parameters among `type_params` (sorted). The others are
    /// ordinary types. Monomorphization uses them to instantiate a generic root
    /// ([`crate::monomorphize_from`]).
    pub set_params: Vec<usize>,
    /// Whether a call may fail: the declared E is not empty (spec 7.2). A set type parameter
    /// counts as not empty, so an instantiation may never fail. Always false for external
    /// functions (spec 12.5) and consts (spec 5.6).
    pub fails: bool,
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
    /// The function's own E and R, when they contain set type parameters (`R(f)` of `List.map`);
    /// `None` otherwise. Then `fails`, `suspends` and whether it takes the evidence table
    /// over-approximate its instantiations, as for [`FnType::sets`], and [`crate::mono`] works out
    /// the exact ones per instantiation.
    pub sets: Option<FnSets>,
}

/// The host function of an external function (spec 12.1). How the parameters and the return value
/// cross the boundary (spec 12.4, 12.5) is decided by each backend from their types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    /// The per-target `@external` arguments, in written order.
    pub bindings: Vec<ExternalBinding>,
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
    pub host: HostRef,
}

/// What an `@external` points to in the host (spec 12.1). What a name means is up to the target's
/// backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRef {
    /// One argument, `@external(js, "console.log")`: a name the host resolves on its own (a global
    /// reference in JS).
    Global(String),
    /// Two arguments, `@external(wasm, "env", "sin")`: `name` exported by `module`.
    Import { module: HostModule, name: String },
}

/// The module of [`HostRef::Import`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostModule {
    /// A module the host finds by name (`node:fs`, `env`, `wasi:http/client`).
    Named(String),
    /// A file next to a project source (written starting with `./` or `../`), as an absolute path
    /// resolved from the declaring source's directory.
    File(String),
    /// A file bundled with the compiler (written in a bundled source starting with `./` or `../`),
    /// relative to `lib/` (`std/clock.mjs`). Its contents are in [`Module::bundled_files`].
    Bundled(String),
}

/// An effect (spec 8.1). An effect with type arguments is one `EffectDef` per instantiation
/// (`State[Int]` and `State[String]` are different effects), and the types of its operations are
/// the instantiated ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDef {
    /// Unique in the module; used as the evidence table key.
    pub name: String,
    /// The IR name of the effect declaration this instantiates (`State.State` for
    /// `State.State[Int]`); the same as `name` for an effect without type parameters.
    pub decl: String,
    /// The type arguments of the instantiation, in the declaration's parameter order. Set type
    /// arguments (`Scope[E, R]`, spec 11.2) have no IR type and are the placeholder `()`. A
    /// `Type::Param(i)` is a type parameter of the function whose body uses the effect (the key
    /// `State[$modify.0]` is used only in the body of `modify`), as in that function's local types.
    pub args: Vec<TypeId>,
    /// In declaration order; indexed by `Callee::Op::op` and `Expr::Handler::ops`.
    pub ops: Vec<OpDef>,
    /// For a label (`effect AuditLog of Console`, issue #255), the effect it stands for. The label
    /// is its own key in evidence tables, but its handler values are the base's: `Use` of a label
    /// has type `Type::Handler(base)`, and a table binding of the label holds a handler of the
    /// base. `ops` are the base's.
    pub label_of: Option<EffectId>,
}

impl EffectDef {
    /// An effect without type parameters.
    pub fn new(name: &str, ops: Vec<OpDef>) -> Self {
        EffectDef {
            name: name.to_owned(),
            decl: name.to_owned(),
            args: Vec::new(),
            ops,
            label_of: None,
        }
    }
}

/// An effect operation (spec 8.1), with its types for the instantiation of its effect.
///
/// An operation may have type parameters of its own only in built-in effects (`fork[A]` and
/// `deferred[A]` of Scope, spec 8.1, 11.2). They are `Type::Param(0..type_params)` in `params` and
/// `ret`, instantiated per call by `Callee::Op::ty_args`. The type arguments of built-in effects
/// are sets (`Scope[E, R]`), so such an operation's types contain no other `Type::Param`. A
/// handler implements a generic operation for every instantiation at once, so its implementation
/// sees the operation's own type parameters as `Type::Any`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpDef {
    pub name: String,
    /// Whether it is a `suspend fn`. If true, the handler's implementation (a lambda in
    /// `Expr::Handler::ops`) takes resume (`fn(ret) -> ()`) as its last argument and returns `()`,
    /// and operation calls (`Callee::Op`) are suspending calls (spec 8.4, 14.5).
    pub suspend: bool,
    /// The number of the operation's own type parameters (see above); 0 outside built-in effects.
    pub type_params: usize,
    /// The declared parameters' types. For Scope's fork, the IR passes the child function wrapped
    /// to take the evidence table as an ordinary parameter (`fn(evidence) -> A`, see
    /// `lower/fiber.rs`), and the type says so.
    pub params: Vec<TypeId>,
    pub ret: TypeId,
    /// Whether a call may fail: the operation's E is not empty (spec 7.2).
    pub fails: bool,
}

impl OpDef {
    /// An `fn` operation that does not fail.
    pub fn new(name: &str, params: Vec<TypeId>, ret: TypeId) -> Self {
        OpDef {
            name: name.to_owned(),
            suspend: false,
            type_params: 0,
            params,
            ret,
            fails: false,
        }
    }

    /// A `suspend fn` operation that does not fail.
    pub fn suspend(name: &str, params: Vec<TypeId>, ret: TypeId) -> Self {
        OpDef {
            suspend: true,
            ..OpDef::new(name, params, ret)
        }
    }

    /// The same operation, failing.
    pub fn failing(self) -> Self {
        OpDef {
            fails: true,
            ..self
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
    /// Whether it is an `@external type` (spec 12.3): an opaque host value with no constructors,
    /// which Emela only passes around (`Std.Fiber.Fiber` is one too, made by the runtime). Backends
    /// represent its values as host handles. An enum without constructors that is not external
    /// has no values.
    pub external: bool,
}

/// The index of `Some` in Prelude's `Option`.
pub const OPTION_SOME: usize = 0;
/// The index of `None` in Prelude's `Option`.
pub const OPTION_NONE: usize = 1;

impl EnumDef {
    /// Prelude's `enum Option[A] { Some(A), None }`. `a` is `Type::Param(0)`.
    pub fn option(a: TypeId) -> Self {
        EnumDef {
            name: "Option".to_owned(),
            params: 1,
            variants: vec![
                VariantDef::positional("Some", vec![a]),
                VariantDef::unit("None"),
            ],
            error: false,
            external: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: VariantFields,
    /// Field types, in `fields` order. Used to build show functions (the stand-in for Show) from
    /// types.
    pub tys: Vec<TypeId>,
}

impl VariantDef {
    pub fn unit(name: &str) -> Self {
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Unit,
            tys: Vec::new(),
        }
    }

    pub fn positional(name: &str, tys: Vec<TypeId>) -> Self {
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Positional(tys.len()),
            tys,
        }
    }

    pub fn named(name: &str, fields: Vec<(&str, TypeId)>) -> Self {
        let (names, tys) = fields.into_iter().map(|(n, t)| (n.to_owned(), t)).unzip();
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Named(names),
            tys,
        }
    }
}

/// The type of an IR value, as an index into the module's type table ([`Module::types`]). Types are
/// hash-consed: equal types get equal IDs, so comparing IDs compares types. The structure is read
/// with [`Module::ty`].
///
/// The types without components have fixed IDs (`TypeId::INT`, ...), usable without the module.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(u32);

impl TypeId {
    pub const INT: TypeId = TypeId(0);
    pub const INT64: TypeId = TypeId(1);
    pub const FLOAT: TypeId = TypeId(2);
    pub const BOOL: TypeId = TypeId(3);
    pub const STRING: TypeId = TypeId(4);
    pub const UNIT: TypeId = TypeId(5);
    pub const NEVER: TypeId = TypeId(6);
    pub const EVIDENCE: TypeId = TypeId(7);
    pub const FIBER_SCOPE: TypeId = TypeId(8);
    pub const ANY: TypeId = TypeId(9);

    /// The position in the type table.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Debug for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypeId({})", self.0)
    }
}

/// The structure of an IR type, with components as [`TypeId`]s. Used for showing (the stand-in
/// for Show), for builtins whose behavior depends on the type, and by backends that need value
/// representations.
///
/// The type checker's `emela_types::Ty` rewritten with IR names (`EnumId`, `EffectId`). No type
/// variables: a type variable left undetermined (an unused `None`'s `Option[?]`) becomes `Unit`,
/// as any representation behaves the same.
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
    Tuple(Vec<TypeId>),
    List(TypeId),
    /// An enum / type application. The argument count is `EnumDef::params`.
    Enum(EnumId, Vec<TypeId>),
    /// The `i`th type parameter of the enclosing definition: the `EnumDef` in field types, the
    /// function otherwise.
    Param(usize),
    /// A function value (spec 10.6: cannot be shown).
    Fn(FnType),
    /// A capability value (spec 8.2): the handler value of the effect, returned by `use X`
    /// (`Expr::Use`). Cannot be shown or compared.
    Handler(EffectId),
    /// An evidence table (`Expr::Provide`, spec 16.5).
    Evidence,
    /// A mutable cell holding a value of the type (the state of `State.run`, `Builtin::CellNew`).
    Cell(TypeId),
    /// The run-time scope that `FiberOp::Scope` passes to its body and `FiberOp::Fork` and
    /// `FiberOp::Deferred` take (spec 11.2). Not Scope's capability value, which is a `Handler`.
    FiberScope,
    /// A value whose type differs per use and is not tracked: inside Scope's handler, the child
    /// function that fork receives (the `A` of `fork[A]`, spec 11.2) and the handles that
    /// `FiberOp::Fork` and `FiberOp::Deferred` return. Only built-in effects have generic
    /// operations (spec 8.1), and lowering builds their handlers. Backends give it a uniform
    /// representation. A handler's implementation of a generic operation sees the operation's own
    /// type parameters as `Any` too (`OpDef`).
    ///
    /// [`crate::verify`] accepts `Any` where any type is expected and the other way round.
    Any,
    /// What decides calling conventions about an error or effect set: the argument of a set type
    /// parameter (`R(f)` of `List.map`) in `ty_args` and in enum type arguments, and the E and R
    /// of a function type with set type parameters ([`FnSets`]). Not the type of a value.
    Set(SetSummary),
}

/// An error or effect set reduced to what decides how a function with it is called
/// ([`Type::Set`]). The flags are those of the set's elements other than set type parameters,
/// which are listed in `params` (as `Type::Param`, sorted, each once). With `params` empty, the
/// set is fully known.
///
/// The flags are worked out without knowing whether the set is an E or an R, so only the ones that
/// matter for its position are read: `fails` for an E, `suspends` and `ev` for an R.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SetSummary {
    /// The set is not empty (as an E: a call may fail).
    pub fails: bool,
    /// As an R: a call may suspend (not Immediate, spec 9.5).
    pub suspends: bool,
    /// As an R: it has an element other than Async, so the function takes the evidence table.
    pub ev: bool,
    pub params: Vec<TypeId>,
}

impl SetSummary {
    /// The summary of a set whose elements are all unknown: every flag set. What replacing a set
    /// type parameter with something other than a [`Type::Set`] (the `()` placeholder of effect
    /// type arguments) gives.
    pub fn unknown() -> Self {
        SetSummary {
            fails: true,
            suspends: true,
            ev: true,
            params: Vec::new(),
        }
    }

    /// Adds the elements of `other` (flags and parameters; `params` may need sorting afterwards).
    pub fn merge(&mut self, other: &SetSummary) {
        self.fails |= other.fails;
        self.suspends |= other.suspends;
        self.ev |= other.ev;
        self.params.extend_from_slice(&other.params);
    }
}

/// The E and R of a function type that contain set type parameters, as [`Type::Set`]s
/// ([`FnType::sets`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FnSets {
    pub errors: TypeId,
    pub effects: TypeId,
}

/// A function type, with what decides how a value of it is called.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FnType {
    /// The declared parameters, without the evidence table.
    pub params: Vec<TypeId>,
    pub ret: TypeId,
    /// Whether a call may fail (the E is not empty, spec 7.2); the same as `Expr::Call::fails`.
    pub fails: bool,
    /// Whether a call may suspend (the R is not Immediate, spec 9.5); the same as
    /// `Expr::Call::suspends` for `Callee::Indirect`.
    pub suspends: bool,
    /// Whether the function takes the evidence table as its last argument (the R has effects
    /// other than Async, spec 16.5).
    pub ev: bool,
    /// The E and R when they contain set type parameters of the enclosing definition (`R(f)`),
    /// `None` otherwise. Then the flags above over-approximate every instantiation (a set type
    /// parameter counts as failing and needing the evidence table; whether it may suspend follows
    /// the suspending copy being lowered), and substituting known sets for the parameters
    /// ([`subst_type`]) makes them exact.
    pub sets: Option<FnSets>,
}

impl FnType {
    /// A function type that neither fails, suspends nor takes the evidence table.
    pub fn pure(params: Vec<TypeId>, ret: TypeId) -> Self {
        FnType {
            params,
            ret,
            fails: false,
            suspends: false,
            ev: false,
            sets: None,
        }
    }
}

/// The types of a module, each stored once ([`Module::types`]).
#[derive(Debug, Clone)]
pub struct TypeTable {
    types: Vec<Type>,
    /// Whether the type contains a `Type::Param`, by ID (so that `subst` can skip it).
    has_param: Vec<bool>,
    ids: FxHashMap<Type, TypeId>,
}

impl Default for TypeTable {
    fn default() -> Self {
        let mut table = TypeTable {
            types: Vec::new(),
            has_param: Vec::new(),
            ids: FxHashMap::default(),
        };
        // In the order of the `TypeId` constants.
        for t in [
            Type::Int,
            Type::Int64,
            Type::Float,
            Type::Bool,
            Type::String,
            Type::Unit,
            Type::Never,
            Type::Evidence,
            Type::FiberScope,
            Type::Any,
        ] {
            table.intern(t);
        }
        debug_assert_eq!(table.types[TypeId::ANY.index()], Type::Any);
        table
    }
}

impl TypeTable {
    /// The ID of the type, adding it if new. Components must be IDs of this table.
    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(&id) = self.ids.get(&ty) {
            return id;
        }
        let id = TypeId(u32::try_from(self.types.len()).expect("too many types"));
        let has_param = match &ty {
            Type::Param(_) => true,
            Type::Tuple(ts) | Type::Enum(_, ts) => ts.iter().any(|&t| self.has_param(t)),
            Type::List(t) | Type::Cell(t) => self.has_param(*t),
            Type::Fn(f) => {
                f.params.iter().any(|&t| self.has_param(t))
                    || self.has_param(f.ret)
                    || f.sets.is_some()
            }
            Type::Set(s) => !s.params.is_empty(),
            _ => false,
        };
        self.ids.insert(ty.clone(), id);
        self.types.push(ty);
        self.has_param.push(has_param);
        id
    }

    /// The ID of the type if the table has it.
    pub fn get(&self, ty: &Type) -> Option<TypeId> {
        self.ids.get(ty).copied()
    }

    /// Every type, in the order of their IDs.
    pub fn iter(&self) -> impl Iterator<Item = &Type> {
        self.types.iter()
    }

    /// The number of distinct types.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn list(&mut self, elem: TypeId) -> TypeId {
        self.intern(Type::List(elem))
    }

    pub fn tuple(&mut self, elems: Vec<TypeId>) -> TypeId {
        self.intern(Type::Tuple(elems))
    }

    pub fn enum_type(&mut self, id: EnumId, args: Vec<TypeId>) -> TypeId {
        self.intern(Type::Enum(id, args))
    }

    pub fn func(&mut self, f: FnType) -> TypeId {
        self.intern(Type::Fn(f))
    }

    pub fn cell(&mut self, content: TypeId) -> TypeId {
        self.intern(Type::Cell(content))
    }

    pub fn type_param(&mut self, i: usize) -> TypeId {
        self.intern(Type::Param(i))
    }

    pub fn handler(&mut self, effect: EffectId) -> TypeId {
        self.intern(Type::Handler(effect))
    }

    /// Whether the type contains a `Type::Param`.
    pub fn has_param(&self, ty: TypeId) -> bool {
        self.has_param[ty.index()]
    }

    /// Replaces `Type::Param(i)` with `args[i]` ([`subst_type`]).
    pub fn subst(&mut self, ty: TypeId, args: &[TypeId]) -> TypeId {
        subst_type(self, ty, args)
    }

    /// The type with every [`Type::Set`] inside replaced by `()` and every [`FnType::sets`]
    /// dropped (keeping the flags). For evidence table keys, which leave out E and R.
    pub fn erase_sets(&mut self, ty: TypeId) -> TypeId {
        let new = match self[ty].clone() {
            Type::Set(_) => return TypeId::UNIT,
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|&t| self.erase_sets(t)).collect()),
            Type::List(t) => Type::List(self.erase_sets(t)),
            Type::Enum(id, ts) => Type::Enum(id, ts.iter().map(|&t| self.erase_sets(t)).collect()),
            Type::Cell(t) => Type::Cell(self.erase_sets(t)),
            Type::Fn(f) => Type::Fn(FnType {
                params: f.params.iter().map(|&t| self.erase_sets(t)).collect(),
                ret: self.erase_sets(f.ret),
                sets: None,
                ..f
            }),
            _ => return ty,
        };
        self.intern(new)
    }
}

/// A type table that types can be added to, for [`subst_type`]: [`TypeTable`] itself, the
/// verifier's [`crate::Typer`], and monomorphization, which also maps capability types.
pub trait TypeStore {
    fn table(&self) -> &TypeTable;
    fn intern_type(&mut self, ty: Type) -> TypeId;
    /// Whether `ty` may change under substitution. By default, whether it contains a
    /// `Type::Param`.
    fn may_change(&mut self, ty: TypeId) -> bool {
        self.table().has_param(ty)
    }
    /// The effect that a capability type `Type::Handler(effect)` becomes. By default, the same.
    fn map_effect(&mut self, effect: EffectId) -> EffectId {
        effect
    }
}

impl TypeStore for TypeTable {
    fn table(&self) -> &TypeTable {
        self
    }

    fn intern_type(&mut self, ty: Type) -> TypeId {
        self.intern(ty)
    }
}

/// Replaces `Type::Param(i)` with `args[i]` in `ty`. A parameter without an argument is left as
/// is (only in malformed IR, which [`crate::verify`] reports).
///
/// Set type parameters are replaced inside [`Type::Set`]s: a parameter whose argument is a set
/// summary is merged into it, and one whose argument is something else (the `()` placeholder) is
/// read as an unknown set ([`SetSummary::unknown`]). A function type whose E and R become fully
/// known gets exact flags from them and no [`FnType::sets`]; one still with set type parameters
/// keeps its flags, raised by what became known ([`fn_with_sets`]).
pub fn subst_type<S: TypeStore + ?Sized>(s: &mut S, ty: TypeId, args: &[TypeId]) -> TypeId {
    if !s.may_change(ty) {
        return ty;
    }
    let new = match s.table()[ty].clone() {
        Type::Param(i) => return args.get(i).copied().unwrap_or(ty),
        Type::Tuple(ts) => Type::Tuple(subst_all(s, &ts, args)),
        Type::List(t) => Type::List(subst_type(s, t, args)),
        Type::Enum(id, ts) => Type::Enum(id, subst_all(s, &ts, args)),
        Type::Cell(t) => Type::Cell(subst_type(s, t, args)),
        Type::Handler(e) => Type::Handler(s.map_effect(e)),
        Type::Set(set) => Type::Set(subst_set(s.table(), &set, args)),
        Type::Fn(f) => {
            let params = subst_all(s, &f.params, args);
            let ret = subst_type(s, f.ret, args);
            match f.sets {
                None => Type::Fn(FnType { params, ret, ..f }),
                Some(sets) => {
                    let errors = subst_type(s, sets.errors, args);
                    let effects = subst_type(s, sets.effects, args);
                    Type::Fn(fn_with_sets(s.table(), params, ret, &f, errors, effects))
                }
            }
        }
        _ => return ty,
    };
    s.intern_type(new)
}

fn subst_all<S: TypeStore + ?Sized>(s: &mut S, tys: &[TypeId], args: &[TypeId]) -> Vec<TypeId> {
    tys.iter().map(|&t| subst_type(s, t, args)).collect()
}

fn subst_set(types: &TypeTable, set: &SetSummary, args: &[TypeId]) -> SetSummary {
    let mut out = SetSummary {
        params: Vec::new(),
        ..set.clone()
    };
    for &p in &set.params {
        let arg = match types[p] {
            Type::Param(i) => args.get(i).copied().unwrap_or(p),
            _ => p,
        };
        match &types[arg] {
            Type::Set(a) => out.merge(a),
            Type::Param(_) => out.params.push(arg),
            _ => out.merge(&SetSummary::unknown()),
        }
    }
    out.params.sort();
    out.params.dedup();
    out
}

/// A function type like `old` with parameters `params`, return type `ret`, E `errors` and R
/// `effects` (both `Type::Set`). If neither set has set type parameters left, the flags come from
/// them; otherwise `old`'s flags are kept, raised by the known parts.
pub fn fn_with_sets(
    types: &TypeTable,
    params: Vec<TypeId>,
    ret: TypeId,
    old: &FnType,
    errors: TypeId,
    effects: TypeId,
) -> FnType {
    let (Type::Set(e), Type::Set(r)) = (&types[errors], &types[effects]) else {
        unreachable!("the E and R of a function type are sets");
    };
    if e.params.is_empty() && r.params.is_empty() {
        FnType {
            params,
            ret,
            fails: e.fails,
            suspends: r.suspends,
            ev: r.ev,
            sets: None,
        }
    } else {
        FnType {
            params,
            ret,
            fails: old.fails || e.fails,
            suspends: old.suspends || r.suspends,
            ev: old.ev || r.ev,
            sets: Some(FnSets { errors, effects }),
        }
    }
}

/// A type given by ID, or by structure and interned where it is used (`Module::local`,
/// `Module::define`).
pub trait IntoTypeId {
    fn into_type_id(self, types: &mut TypeTable) -> TypeId;
}

impl IntoTypeId for TypeId {
    fn into_type_id(self, _: &mut TypeTable) -> TypeId {
        self
    }
}

impl IntoTypeId for Type {
    fn into_type_id(self, types: &mut TypeTable) -> TypeId {
        types.intern(self)
    }
}

impl std::ops::Index<TypeId> for TypeTable {
    type Output = Type;

    fn index(&self, id: TypeId) -> &Type {
        &self.types[id.index()]
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
    /// A top-level function used as a value. `ty_args` instantiate its type parameters, as in
    /// `Callee::Direct`.
    Fn {
        f: FnId,
        ty_args: Vec<TypeId>,
    },
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
    /// A builtin call (`intrinsics`). `ty_args` instantiate the builtin's type parameters (the
    /// count is `Builtin::info().ty_params`): `Sig::Param(i)` in its parameter and return types is
    /// `ty_args[i]`, except for the cell builtins, whose cell types a `Sig` cannot write
    /// (`Builtin::CellNew` and the others take the content type). Builtins whose behavior depends
    /// on the type, such as `dbg`, use them too.
    Builtin {
        op: Builtin,
        ty_args: Vec<TypeId>,
        args: Vec<Expr>,
    },
    /// Arguments are in field definition order. `ty_args` are the enum's type arguments (the count
    /// is `EnumDef::params`), so the value's type is known without its arguments (`None` of
    /// `Option[Int]`).
    Ctor {
        ctor: CtorRef,
        ty_args: Vec<TypeId>,
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
    /// `[a, b]` or `[a, b, ..tail]`. `elem` is the element type (the value is a `List[elem]`, and
    /// `tail` too).
    List {
        elem: TypeId,
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
    ///
    /// `ty` is the lambda's type, a `Type::Fn`: its `params` are the types of `params` without the
    /// evidence table, which is the last parameter exactly when `ev` is set. If `suspends` is
    /// true, so is the type's (the converse need not hold: a type may say a call may suspend when
    /// this lambda does not, as for an fn-operation implementation).
    Lambda {
        params: Vec<Local>,
        ty: TypeId,
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
    pub fn for_each_child<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        match self {
            Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } => {}
            Expr::Let { value, body, .. } => {
                f(value);
                f(body);
            }
            Expr::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct { .. } => {}
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
            Expr::List { elems, tail, .. } => {
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
                AssertCond::Shown { cond, lhs, rhs, .. } => {
                    f(cond);
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
        ty: TypeId,
        lhs: Expr,
        rhs: Expr,
    },
    /// A comparison decided by `cond`, which calls an impl (`Eq.eq`, `Ord.compare`, spec 10.3).
    /// `lhs` and `rhs` are the sides, already evaluated into variables (`Expr::Var`) before the
    /// assert, so evaluating them again has no effect; if `cond` is false, both are shown as with
    /// `Compare`. `op` is the comparison written in the source, for the message.
    Shown {
        cond: Expr,
        op: BinOp,
        ty: TypeId,
        lhs: Expr,
        rhs: Expr,
    },
    /// Not a comparison; only the expression's value is checked.
    Bool(Expr),
}

impl Assert {
    /// A comparison assert.
    pub fn compare(op: BinOp, op_ty: OpTy, ty: TypeId, lhs: Expr, rhs: Expr, text: &str) -> Expr {
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

    /// A comparison decided by `cond` ([`AssertCond::Shown`]): `let a = lhs; let b = rhs;
    /// assert(cond)`, showing `a` and `b` if it fails. `cond` refers to `a` and `b`.
    pub fn shown(
        cond: Expr,
        op: BinOp,
        ty: TypeId,
        (a, lhs): (Local, Expr),
        (b, rhs): (Local, Expr),
        text: &str,
    ) -> Expr {
        let assert = Expr::Assert(Box::new(Assert {
            cond: AssertCond::Shown {
                cond,
                op,
                ty,
                lhs: Expr::Var(a),
                rhs: Expr::Var(b),
            },
            text: text.to_owned(),
        }));
        Expr::Let {
            var: a,
            value: Box::new(lhs),
            body: Box::new(Expr::Let {
                var: b,
                value: Box::new(rhs),
                body: Box::new(assert),
            }),
        }
    }

    /// Applies `f` to subexpressions in evaluation order.
    pub fn for_each_operand_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match &mut self.cond {
            AssertCond::Compare { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            AssertCond::Shown { cond, lhs, rhs, .. } => {
                f(cond);
                f(lhs);
                f(rhs);
            }
            AssertCond::Bool(e) => f(e),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    /// A direct call to a top-level function. `ty_args` instantiate the function's type
    /// parameters (`Type::Param(i)` in its parameter and return types becomes `ty_args[i]`); the
    /// count is `Function::type_params`. A set type parameter (`R(f)`, spec 9.2) has no IR type of
    /// values; its position holds the summary of the instantiating set ([`Type::Set`]), which
    /// monomorphization uses to make calling conventions exact.
    Direct { f: FnId, ty_args: Vec<TypeId> },
    /// A call to a closure value.
    Indirect(Box<Expr>),
    /// A call to an operation of the handler value `handler` (`log.info(...)`, spec 8.2). `op` is
    /// an index into `EffectDef::ops`. No evidence table is passed (the operation body's effects
    /// were fixed by `with`). `ty_args` instantiate the operation's own type parameters (the count
    /// is `OpDef::type_params`; empty except for the generic operations of built-in effects, as in
    /// `s.fork[Int](..)`).
    Op {
        handler: Box<Expr>,
        effect: EffectId,
        op: usize,
        ty_args: Vec<TypeId>,
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
    Value(Expr, TypeId),
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
