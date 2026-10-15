//! Builds the Core IR from the type checker's results (HIR and `ProgramTypes`): lowering.
//!
//! The whole program (user modules, Prelude, core) becomes one IR [`Module`]. All enums are
//! allocated first ([`Lowerer::declare_enums`]); only fns reachable from the roots are lowered.
//! A fn gets an ID and is pushed on the worklist when first referenced ([`Lowerer::fn_id`]), and
//! bodies are lowered until the worklist is empty.

use std::path::Path;

use emela_resolve::hir::{
    Arg, Arm as HirArm, BinaryOp, Block, DefId, DefKind, DefModule, ExprId, ExprKind, Fields, Item,
    Literal, LocalId, LocalKind, PatId, PatKind, Program, Res, Span, Stmt, StrPart as HirStrPart,
    UnaryOp, int_literal_value,
};
use emela_resolve::{ModuleId, ModuleMap};
use emela_types::{Prim, ProgramTypes, Row, RowElem, Tag, Ty};
use la_arena::ArenaMap;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::intrinsics::Builtin;
use crate::ir::{
    Arm, Assert, BinOp, BundledFile, Callee, CtorRef, EffectDef, EffectId, EnumDef, EnumId, Expr,
    External, ExternalBinding, FiberOp, FnId, FnSets, FnType, HostModule, HostRef, Lit, Local,
    Module, OpDef, OpTy, Pat, SetSummary, StrPart, Type, TypeId, UnOp, VariantDef, VariantFields,
    WithBind,
};
use crate::named;

mod fiber;
mod suspend;

use fiber::{AttemptCall, ForkCall, ScopeCall};
use suspend::Specialization;

/// Why something that passed type checking could not be lowered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// Not supported in the IR yet. `what` describes what, in English.
    Unsupported { what: String, span: Option<Span> },
    /// An external function reachable from the entry has no `@external` for the build target
    /// (spec 12.1). `span` is the position of the function name.
    MissingExternal {
        name: String,
        target: String,
        span: Option<Span>,
    },
    /// The file named by `@external(js, "./x.mjs", …)` in bundled core is not among the bundled
    /// host files (`LowerInput::bundled_files`). An error in the bundled sources. `path` is the
    /// specifier as written.
    MissingCoreFile { path: String, span: Option<Span> },
}

/// Which fns lowering starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roots {
    /// The fns exported by the entry module (`pub` and `main`, excluding test functions). The
    /// normal build.
    Exported,
    /// The test functions (`LowerInput::tests`), for `emela test`.
    Tests,
    /// Every fn and const with a body, including unused ones. For lowering tests.
    All,
}

#[derive(Debug)]
pub struct Lowered {
    pub module: Module,
    /// The IR function for each test function in `LowerInput::tests`, in the same order. Filled
    /// only with `Roots::Tests`.
    pub tests: Vec<FnId>,
}

pub struct LowerInput<'a> {
    pub hir: &'a Program,
    pub types: &'a ProgramTypes,
    /// Module names, used to prefix IR names.
    pub modules: &'a ModuleMap,
    /// Module sources, used to cut out the expression text shown when an assert fails (spec 13.2).
    pub sources: ArenaMap<ModuleId, &'a str>,
    /// The entry module. Its definitions' IR names get no module prefix.
    pub entry: ModuleId,
    /// `@test` fns (spec 13.1), not exported in a normal build. The caller has checked that normal
    /// code does not reference them (E0227), so a normal build never reaches them and drops them.
    pub tests: Vec<DefId>,
    /// Source file paths of modules. Used to rewrite `@external` modules naming a file
    /// (`"./ffi.mjs"`) to the file next to the declaring source (spec 12.1). Modules without a
    /// path are left alone.
    pub paths: ArenaMap<ModuleId, &'a Path>,
    /// Bundled host files (paths relative to `lib/` and contents; `crate::bundled_files`). File
    /// specifiers of core modules are looked up here instead of next to the source, become
    /// `HostModule::Bundled`, and the files are put in `Module::bundled_files`.
    pub bundled_files: &'a [(&'a str, &'a str)],
    /// The build target (`"js"`). With `Some`, checks that external functions reachable from the
    /// entry (main for `Roots::Exported`, the test functions for `Roots::Tests`) have an
    /// `@external` for that target (spec 12.1). check and the LSP pick no target, so `None`.
    pub target: Option<&'a str>,
}

/// Collects every place that cannot be lowered and returns them all in `Err`. Self tail calls
/// come out as `Expr::Loop` / `Expr::Recur` (`tail::loopify`).
pub fn lower(input: &LowerInput<'_>, roots: Roots) -> Result<Lowered, Vec<LowerError>> {
    let hir = input.hir;
    let mut cx = Lowerer {
        hir,
        types: input.types,
        modules: input.modules,
        sources: &input.sources,
        entry: input.entry,
        tests: &input.tests,
        pat_guards: Vec::new(),
        true_def: hir.prelude_def("True"),
        false_def: hir.prelude_def("False"),
        module: Module::new(),
        fns: FxHashMap::default(),
        worklist: Vec::new(),
        current_def: None,
        spec: 0,
        enums: ArenaMap::default(),
        ctors: ArenaMap::default(),
        locals: ArenaMap::default(),
        effects: FxHashMap::default(),
        handler_parts: FxHashMap::default(),
        ev: None,
        bound: Vec::new(),
        paths: &input.paths,
        bundled_files: input.bundled_files,
        uses_bundled: false,
        errors: Vec::new(),
    };
    cx.declare_enums();
    cx.define_ctors();
    for (_, items) in hir.modules.iter() {
        for &def in &items.items {
            let is_root = match (roots, hir.item(def)) {
                (Roots::Exported, Some(Item::Fn(_))) => cx.is_exported(def),
                (Roots::All, Some(Item::Fn(f))) => f.body.is_some() || f.is_external,
                (Roots::All, Some(Item::Const(_) | Item::Handler(_))) => true,
                _ => false,
            };
            if is_root {
                cx.fn_id(def, 0);
            }
        }
    }
    let tests = match roots {
        Roots::Tests => input.tests.iter().map(|&def| cx.fn_id(def, 0)).collect(),
        _ => Vec::new(),
    };
    while let Some((def, spec)) = cx.worklist.pop() {
        cx.define_fn(def, spec);
    }
    if let Some(target) = input.target {
        let entries: Vec<FnId> = match roots {
            Roots::Exported => cx.entry_main().into_iter().collect(),
            Roots::Tests => tests.clone(),
            Roots::All => Vec::new(),
        };
        cx.check_target(&entries, target);
    }
    if cx.uses_bundled {
        // If any bundled file is used, ship them all, so relative imports between them work.
        cx.module.bundled_files = input
            .bundled_files
            .iter()
            .map(|(path, contents)| BundledFile {
                path: (*path).to_owned(),
                contents: (*contents).to_owned(),
            })
            .collect();
    }
    if cx.errors.is_empty() {
        // Target-neutral, so done once here: every backend gets self tail calls as loops (16.3).
        crate::tail::loopify(&mut cx.module);
        Ok(Lowered {
            module: cx.module,
            tests,
        })
    } else {
        Err(cx.errors)
    }
}

struct Lowerer<'a> {
    hir: &'a Program,
    types: &'a ProgramTypes,
    modules: &'a ModuleMap,
    sources: &'a ArenaMap<ModuleId, &'a str>,
    entry: ModuleId,
    tests: &'a [DefId],
    /// Comparisons collected from const patterns while lowering a pattern (`x == LIMIT()`),
    /// placed at the front of the arm's guard.
    pat_guards: Vec<Expr>,
    /// Prelude's `True` and `False`, which become `Lit::Bool` in the IR.
    true_def: Option<DefId>,
    false_def: Option<DefId>,
    module: Module,
    /// HIR fns and consts, with which suspending copy (`suspend.rs`) → IR functions. A const
    /// becomes a parameterless function. Only [`Lowerer::fn_id`] adds to it.
    fns: FxHashMap<(DefId, Specialization), FnId>,
    /// Fns (and which suspending copy) that have an ID but whose bodies are not lowered yet.
    worklist: Vec<(DefId, Specialization)>,
    /// The definition whose body is being lowered, and which suspending copy. Used to decide the
    /// "may suspend" marks of lambdas and calls.
    current_def: Option<DefId>,
    spec: Specialization,
    /// HIR type / enum / error → IR enum.
    enums: ArenaMap<DefId, EnumId>,
    /// HIR constructors → IR constructors. An error declaration is itself the constructor. Bool's
    /// constructors are not here.
    ctors: ArenaMap<DefId, CtorRef>,
    /// HIR locals → IR locals. Only [`Lowerer::local`] adds to it.
    locals: ArenaMap<LocalId, Local>,
    /// Effects with their type-argument keys ([`EffectKey`]) → IR effects. Only
    /// [`Lowerer::effect_id`] adds to it.
    effects: FxHashMap<EffectKey, EffectId>,
    /// Stateful handlers → IR functions for init and release. Only [`Lowerer::handler_fns`] adds
    /// to it.
    handler_parts: FxHashMap<DefId, (FnId, Option<FnId>)>,
    /// The current evidence table (evidence passing, spec 16.5): the last argument of a function
    /// or lambda with a non-empty R, or a table extended by `with`. `None` outside functions and
    /// in functions whose R is empty.
    ev: Option<Local>,
    /// Effects and handler values provided by enclosing `with`s, outermost first. Only `with`s in
    /// the same function (a top-level fn or a handler operation), carried into lambdas. `use X`
    /// searches from the back (spec 8.3).
    bound: Vec<(EffectKey, Local)>,
    paths: &'a ArenaMap<ModuleId, &'a Path>,
    bundled_files: &'a [(&'a str, &'a str)],
    /// Whether a lowered external function pointed to a bundled host file.
    uses_bundled: bool,
    errors: Vec<LowerError>,
}

/// An evidence table key: the effect and its type arguments as a string (empty for effects without
/// type arguments). Effects with different type arguments are different effects (spec 8.1), so each
/// gets its own IR effect. A type argument `Bound(i)` is written as the owning definition's IR name
/// and the index (`$modify.0`).
///
/// Equality and hashing look only at the effect and the string. The tag and the definition it was
/// read in (`Lowerer::tag_key`) are kept to build the IR effect's type arguments
/// (`Lowerer::effect_id`).
#[derive(Clone)]
struct EffectKey {
    def: DefId,
    args: String,
    tag: Tag,
    ctx: Option<DefId>,
}

impl PartialEq for EffectKey {
    fn eq(&self, other: &Self) -> bool {
        self.def == other.def && self.args == other.args
    }
}

impl Eq for EffectKey {}

impl std::hash::Hash for EffectKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.def.hash(state);
        self.args.hash(state);
    }
}

/// A binding from a block statement.
enum Bind {
    /// `let var = value`．
    Var(Local, Expr),
    /// A match with one arm.
    Pat(Pat, Expr),
}

/// The callee of a call.
///
/// `Call` is the call's mark (`fails` is `Expr::Call::fails`, `suspends` is
/// `Expr::Call::suspends`).
enum Target {
    /// The function, its type arguments (`Callee::Direct::ty_args`), the mark, and the evidence
    /// table appended as the last argument (when calling a function with a non-empty R).
    Direct(FnId, Vec<TypeId>, Call, Option<Expr>),
    Builtin {
        op: Builtin,
        ty_args: Vec<TypeId>,
    },
    /// The constructor and the enum's type arguments.
    Ctor(CtorRef, Vec<TypeId>),
    /// A function value: an evaluated expression that can be referenced any number of times
    /// without side effects (a variable or function). `Option<Expr>` is as in `Direct`.
    Value(Expr, Call, Option<Expr>),
    /// An operation on a capability value (`log.info`). `handler` is the capability value
    /// expression.
    Op {
        handler: Expr,
        effect: EffectId,
        op: usize,
        ty_args: Vec<TypeId>,
        call: Call,
    },
    /// `State.run(initial, body)` (spec 8.8). A builtin handler provided by the runtime.
    StateRun(Box<StateRun>),
    /// A concurrency operation that passes its arguments through (spec chapter 11, `fiber.rs`).
    Fiber(FiberOp),
    /// `Fiber.scope(body)` (spec 11.2).
    Scope(Box<ScopeCall>),
    /// `s.fork(f)`（11.2）．
    Fork(Box<ForkCall>),
    /// `Fiber.attempt(f)`（11.5）．
    Attempt(Box<AttemptCall>),
}

/// What is needed to lower `State.run(initial, body)`. Locals are allocated up front.
///
/// Creates a cell holding the initial value (`$cellNew`), adds a handler value that reads and
/// writes it to the evidence table under the key `State[S]`, calls body, and returns the pair of
/// body's value and the final state. If body fails, the failure propagates as the call's failure
/// and the state is discarded.
struct StateRun {
    /// The IR effect of `State[S]`.
    effect: EffectId,
    /// Indices of the operations get, set and update in `EffectDef::ops`.
    ops: [usize; 3],
    /// The base evidence table for calling body (the current table).
    base: Option<Expr>,
    /// The mark of the body call.
    call: Call,
    /// The state type S.
    state: TypeId,
    /// The types of the operation implementations get, set and update.
    op_tys: [TypeId; 3],
    cell: Local,
    handler: Local,
    value: Local,
    f: Local,
    result: Local,
}

impl StateRun {
    /// Builds the `State.run` expression from positional arguments (initial value, body).
    fn apply(self, args: Vec<Expr>) -> Expr {
        let [initial, body]: [Expr; 2] = args.try_into().expect("State.run takes two arguments");
        let cell = || Expr::Var(self.cell);
        let get = || Expr::Builtin {
            op: Builtin::CellGet,
            ty_args: vec![self.state],
            args: vec![cell()],
        };
        let set = |value: Expr| Expr::Builtin {
            op: Builtin::CellSet,
            ty_args: vec![self.state],
            args: vec![cell(), value],
        };
        let lambda = |params: Vec<Local>, ty: TypeId, body: Expr| Expr::Lambda {
            params,
            ty,
            body: Box::new(body),
            suspends: false,
        };
        // The function passed to update is pure (spec 8.8), so read, call and write happen in one
        // step.
        let update = set(Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
            args: vec![get()],
            fails: false,
            suspends: false,
        });
        let mut ops = vec![None, None, None];
        let [get_ty, set_ty, update_ty] = self.op_tys;
        ops[self.ops[0]] = Some(lambda(Vec::new(), get_ty, get()));
        ops[self.ops[1]] = Some(lambda(vec![self.value], set_ty, set(Expr::Var(self.value))));
        ops[self.ops[2]] = Some(lambda(vec![self.f], update_ty, update));
        let handler = Expr::Handler {
            name: "State".to_owned(),
            effect: self.effect,
            ops: ops
                .into_iter()
                .map(|o| o.expect("three operations"))
                .collect(),
        };
        let run = Expr::Call {
            callee: Callee::Indirect(Box::new(body)),
            args: vec![Expr::Provide {
                base: self.base.map(Box::new),
                bindings: vec![(self.effect, Expr::Var(self.handler))],
            }],
            fails: self.call.fails,
            suspends: self.call.suspends,
        };
        let lets = [
            (
                self.cell,
                Expr::Builtin {
                    op: Builtin::CellNew,
                    ty_args: vec![self.state],
                    args: vec![initial],
                },
            ),
            (self.handler, handler),
            (self.result, run),
        ];
        let tuple = Expr::Tuple(vec![Expr::Var(self.result), get()]);
        lets.into_iter()
            .rev()
            .fold(tuple, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }
}

/// The mark of a call.
#[derive(Clone, Copy)]
struct Call {
    fails: bool,
    suspends: bool,
}

impl Target {
    /// Builds a call with arguments in positional order.
    fn apply(self, args: Vec<Expr>) -> Expr {
        match self {
            Target::Direct(f, ty_args, call, ev) => Expr::Call {
                callee: Callee::Direct { f, ty_args },
                args: args.into_iter().chain(ev).collect(),
                fails: call.fails,
                suspends: call.suspends,
            },
            Target::Builtin { op, ty_args } => Expr::Builtin { op, ty_args, args },
            Target::Ctor(ctor, ty_args) => Expr::Ctor {
                ctor,
                ty_args,
                args,
            },
            Target::Value(f, call, ev) => Expr::Call {
                callee: Callee::Indirect(Box::new(f)),
                args: args.into_iter().chain(ev).collect(),
                fails: call.fails,
                suspends: call.suspends,
            },
            Target::Op {
                handler,
                effect,
                op,
                ty_args,
                call,
            } => Expr::Call {
                callee: Callee::Op {
                    handler: Box::new(handler),
                    effect,
                    op,
                    ty_args,
                },
                args,
                fails: call.fails,
                suspends: call.suspends,
            },
            Target::StateRun(run) => run.apply(args),
            Target::Fiber(op) => Expr::Fiber { op, args },
            Target::Scope(scope) => scope.apply(args),
            Target::Fork(fork) => fork.apply(args),
            Target::Attempt(attempt) => attempt.apply(args),
        }
    }
}

impl<'a> Lowerer<'a> {
    /// Allocates IDs and names for every type / enum / error. Constructors are added later.
    fn declare_enums(&mut self) {
        let bool_def = self.hir.prelude_def("Bool");
        let option_def = self.hir.prelude_def("Option");
        for (_, items) in self.hir.modules.iter() {
            for &def in &items.items {
                match self.hir.item(def) {
                    // Bool is a builtin type in the IR (`TypeId::BOOL`).
                    Some(Item::Type(_) | Item::Enum(_)) if Some(def) == bool_def => {}
                    // Prelude's Option refers to the same definition as the builtins
                    // (`Module::option_enum`).
                    Some(Item::Enum(_)) if Some(def) == option_def => {
                        let id = self.module.option_enum();
                        self.enums.insert(def, id);
                    }
                    Some(Item::Type(t)) => {
                        let id = self.declare_enum(def, t.type_params.len());
                        // An `@external type` has no constructor (spec 12.3).
                        self.module.enums[id].external = t.ctor.is_none();
                        self.enums.insert(def, id);
                    }
                    Some(Item::Enum(e)) => {
                        let id = self.declare_enum(def, e.type_params.len());
                        self.enums.insert(def, id);
                    }
                    // An error defines a type and a constructor of the same name, like `type`
                    // (spec 7.1). It has no type parameters.
                    Some(Item::Error(_)) => {
                        let id = self.declare_enum(def, 0);
                        self.module.enums[id].error = true;
                        self.enums.insert(def, id);
                    }
                    // A stateful handler's value (self) is an enum with the handler's one
                    // constructor (spec 8.4).
                    Some(Item::Handler(h)) if is_stateful(h) => {
                        let id = self.declare_enum(def, 0);
                        self.enums.insert(def, id);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Adds constructors to enums and builds the constructor table (`ctors`).
    ///
    /// Constructor field types come from the type checker's constructor declarations. A
    /// constructor's type parameters are in the same order as the constructed type's, so
    /// `Ty::Bound(i)` maps directly to `Type::Param(i)`.
    fn define_ctors(&mut self) {
        // `self.module` cannot be mutated while `self.enums` is borrowed, so copy the pairs first.
        let enums: Vec<(DefId, EnumId)> = self.enums.iter().map(|(d, &e)| (d, e)).collect();
        for (def, enum_id) in enums {
            let ctors = match self.hir.item(def) {
                Some(Item::Type(t)) => t.ctor.into_iter().collect(),
                Some(Item::Enum(e)) => e.variants.clone(),
                Some(Item::Error(_)) => vec![def],
                Some(Item::Handler(h)) => h.ctor.into_iter().collect(),
                _ => Vec::new(),
            };
            for (variant, ctor) in ctors.into_iter().enumerate() {
                self.ctors.insert(ctor, CtorRef { enum_id, variant });
                // Option's constructors were added by `EnumDef::option`.
                if Some(enum_id) != self.module.option {
                    let v = self.variant_def(ctor);
                    self.module.enums[enum_id].variants.push(v);
                }
            }
        }
    }

    fn variant_def(&mut self, ctor: DefId) -> VariantDef {
        let fields = match self.hir.item(ctor) {
            Some(Item::Ctor(c)) => &c.fields,
            Some(Item::Error(e)) => &e.fields,
            _ => &Fields::None,
        };
        let fields = match fields {
            Fields::None => VariantFields::Unit,
            Fields::Positional(tys) => VariantFields::Positional(tys.len()),
            Fields::Named(fs) => {
                VariantFields::Named(fs.iter().map(|f| f.name.to_string()).collect())
            }
        };
        // A constructor without fields has the constructed type itself, not a function type.
        let types = self.types;
        let tys = match types.sigs.get(ctor).map(|sig| &sig.scheme.ty) {
            Some(Ty::Fn(f)) => f.params.iter().map(|t| self.ty(t)).collect(),
            _ => Vec::new(),
        };
        VariantDef {
            name: self.hir.defs[ctor].name.to_string(),
            fields,
            tys,
        }
    }

    /// Converts a type checker type to an IR type.
    fn ty(&mut self, ty: &Ty) -> TypeId {
        let t = match ty {
            Ty::Prim(p) => {
                return match p {
                    Prim::Int => TypeId::INT,
                    Prim::Int64 => TypeId::INT64,
                    Prim::Float => TypeId::FLOAT,
                    Prim::Bool => TypeId::BOOL,
                    Prim::String => TypeId::STRING,
                    Prim::Unit => TypeId::UNIT,
                };
            }
            Ty::Never => return TypeId::NEVER,
            // A type variable generalized in the definition being lowered is its type parameter.
            // One left undetermined (an unused `None`'s `Option[?]` and the like) can take any
            // representation. `Ty::Error` only appears when there are type errors, and then
            // lowering is not called.
            Ty::Var(v) => match self
                .current_def
                .and_then(|def| self.types.generalized_tys.get(def))
                .and_then(|vars| vars.iter().find(|(w, _)| w == v))
            {
                Some(&(_, i)) => Type::Param(i as usize),
                None => return TypeId::UNIT,
            },
            Ty::Error => return TypeId::UNIT,
            // Set type arguments of named types (`Handler[E, R]`) have no run-time representation,
            // but what they decide about calling conventions is kept.
            Ty::Set(_, row) => return self.set_ty(row),
            Ty::Cap(tag) => {
                let key = self.key_here(tag);
                Type::Handler(self.effect_id(&key))
            }
            Ty::Bound(i) => Type::Param(*i as usize),
            Ty::Fn(f) => {
                let params = f.params.iter().map(|t| self.ty(t)).collect();
                let ret = self.ty(&f.ret);
                Type::Fn(FnType {
                    params,
                    ret,
                    fails: !f.fails.is_empty(),
                    suspends: self.row_suspends(&f.uses),
                    ev: self.row_needs_ev(&f.uses),
                    sets: self.fn_sets(&f.fails, &f.uses),
                })
            }
            Ty::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.ty(t)).collect()),
            Ty::List(t) => Type::List(self.ty(t)),
            Ty::Named(con, args) => {
                let def = self.types.ty_table.def(*con);
                match self.enums.get(def) {
                    Some(&id) => Type::Enum(id, args.iter().map(|t| self.ty(t)).collect()),
                    // `self` of a stateless handler. The value is `()` (`Lowerer::path`).
                    None => return TypeId::UNIT,
                }
            }
        };
        self.module.intern(t)
    }

    /// The summary of a set inside the definition being lowered ([`SetSummary`]): set type
    /// parameters (`RowElem::Bound`, and set variables generalized by inference) are listed as
    /// `Type::Param`, and the flags come from the other elements by the same rules as the marks
    /// (a non-empty E fails, `row_needs_ev`, `row_suspends`). Error tags only make it fail and
    /// effect tags only set `ev` and `suspends`; a set variable left undetermined, whose kind is
    /// not known here, fails and needs the evidence table.
    fn set_ty(&mut self, row: &Row) -> TypeId {
        let hir = self.hir;
        let async_ = hir.prelude_def("Async");
        let mut set = SetSummary::default();
        for e in row.elems() {
            match e {
                RowElem::Bound(i) => set.params.push(self.module.type_param(*i as usize)),
                RowElem::Var(v) => match self.generalized_row_param(*v) {
                    Some(i) => set.params.push(self.module.type_param(i as usize)),
                    None => {
                        set.fails = true;
                        set.ev = true;
                    }
                },
                RowElem::Tag(t) if matches!(hir.item(t.def), Some(Item::Effect(_))) => {
                    set.ev |= Some(t.def) != async_;
                    set.suspends |= !self.is_immediate_effect(t.def);
                }
                RowElem::Tag(t) if Some(t.def) == async_ => set.suspends = true,
                RowElem::Tag(_) | RowElem::Of(_) => set.fails = true,
            }
        }
        set.params.sort();
        set.params.dedup();
        self.module.intern(Type::Set(set))
    }

    /// The type parameter that the set variable `v` was generalized to in the definition being
    /// lowered, if any.
    fn generalized_row_param(&self, v: emela_types::RowVar) -> Option<u32> {
        self.current_def
            .and_then(|def| self.types.generalized_rows.get(def))
            .and_then(|rows| rows.iter().find(|(w, _)| *w == v))
            .map(|&(_, i)| i)
    }

    /// The E and R of a function type, if they contain set type parameters (`FnType::sets`).
    fn fn_sets(&mut self, fails: &Row, uses: &Row) -> Option<FnSets> {
        let errors = self.set_ty(fails);
        let effects = self.set_ty(uses);
        let open = |t: TypeId| matches!(self.module.ty(t), Type::Set(s) if !s.params.is_empty());
        (open(errors) || open(effects)).then_some(FnSets { errors, effects })
    }

    /// The IR ID of copy `spec` of a fn. If none exists yet, allocates one and pushes it on the
    /// worklist; fn IDs are created only here. The original function (0) keeps the definition's IR
    /// name; suspending copies are named `name$suspend<bits>` (`$` never appears in Emela names,
    /// so they cannot clash). Only the original function is exported.
    fn fn_id(&mut self, def: DefId, spec: Specialization) -> FnId {
        if let Some(&id) = self.fns.get(&(def, spec)) {
            return id;
        }
        let mut name = self.ir_name(def);
        if spec != 0 {
            name = format!("{name}$suspend{spec}");
        }
        let id = self
            .module
            .declare(&name, spec == 0 && self.is_exported(def));
        self.module.functions[id].suspends = self.fn_suspends(def, spec);
        self.fns.insert((def, spec), id);
        self.worklist.push((def, spec));
        id
    }

    /// The IR ID of the fn referenced by `expr`, choosing the copy that matches the instantiation.
    fn fn_ref(&mut self, def: DefId, expr: ExprId) -> FnId {
        let spec = self.specialization_of(def, Some(expr));
        self.fn_id(def, spec)
    }

    /// Creates the IR local for an HIR local if it does not exist yet.
    fn local(&mut self, id: LocalId) -> Local {
        if let Some(&l) = self.locals.get(id) {
            return l;
        }
        let types = self.types;
        let ty = match types.locals.get(id) {
            Some(t) => self.ty(t),
            None => TypeId::UNIT,
        };
        let l = self.module.local(&self.hir.locals[id].name, ty);
        self.locals.insert(id, l);
        l
    }

    /// A fresh evidence table parameter or variable.
    fn ev_local(&mut self) -> Local {
        self.module.local("ev", TypeId::EVIDENCE)
    }

    /// The IR return type and type parameter count of a definition's declared (or generalized)
    /// type. Non-functions (consts) have no type parameters and return their own type.
    fn signature(&mut self, def: DefId) -> (TypeId, usize) {
        let types = self.types;
        match types
            .sigs
            .get(def)
            .map(|s| (&s.scheme.ty, s.scheme.params.len()))
        {
            Some((Ty::Fn(f), n)) => (self.ty(&f.ret), n),
            Some((ty, n)) => (self.ty(ty), n),
            None => (TypeId::UNIT, 0),
        }
    }

    fn define_fn(&mut self, def: DefId, spec: Specialization) {
        let hir = self.hir;
        let id = self.fns[&(def, spec)];
        // Each function starts with a fresh evidence table and `with` bindings, and fresh local
        // mappings (so the original function and suspending copies of the same definition do not
        // share locals; HIR locals belong to exactly one definition's body).
        self.ev = None;
        self.bound.clear();
        self.locals = ArenaMap::default();
        self.current_def = Some(def);
        self.spec = spec;
        let (mut params, body) = match hir.item(def) {
            // An `@external suspend fn` has Async in its declared R, so it becomes a suspending
            // function that awaits the Promise the host function returns (spec 12.2, `fn_id`).
            Some(Item::Fn(f)) if f.body.is_none() => {
                let params: Vec<Local> = f.sig.params.iter().map(|p| self.local(p.local)).collect();
                // A bodiless fn's parameters are not in `types.locals`; their types come from the
                // declared type. Backends decide how they cross the boundary from these types.
                let types = self.types;
                if let Some(Ty::Fn(sig)) = types.sigs.get(def).map(|s| &s.scheme.ty) {
                    for (&l, t) in params.iter().zip(&sig.params) {
                        self.module.locals[l].ty = self.ty(t);
                    }
                }
                let external = self.external(def, f);
                let (ret, type_params) = self.signature(def);
                self.module.define_external(id, params, ret, external);
                self.module.functions[id].type_params = type_params;
                return;
            }
            Some(Item::Fn(f)) => (
                f.sig.params.iter().map(|p| self.local(p.local)).collect(),
                f.body.expect("a fn with a body"),
            ),
            // A const's value is evaluated once, at the first reference (spec 5.6).
            Some(Item::Const(c)) => {
                self.module.functions[id].constant = true;
                (Vec::new(), c.value)
            }
            Some(Item::Handler(_)) => {
                let (params, ret, body) = self.handler(def);
                self.module.define(id, params, ret, body);
                return;
            }
            _ => unreachable!("fn_id got a definition that is not a fn, const or handler"),
        };
        if self.takes_ev(def) {
            let ev = self.ev_local();
            params.push(ev);
            self.ev = Some(ev);
        }
        let body = self.expr(body);
        let (ret, type_params) = self.signature(def);
        self.module.define(id, params, ret, body);
        let fails = self.declared_fails(def);
        let types = self.types;
        let sets = match types.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => self.fn_sets(&f.fails, &f.uses),
            _ => None,
        };
        let func = &mut self.module.functions[id];
        func.type_params = type_params;
        // A const's value cannot fail (spec 5.6).
        func.fails = !func.constant && fails;
        if !func.constant {
            func.sets = sets;
        }
    }

    /// Whether the declared (or generalized) type's E is not empty. Opaque sets (set type
    /// parameters, generalized sets) count as not empty.
    fn declared_fails(&self, def: DefId) -> bool {
        matches!(self.types.sigs.get(def).map(|s| &s.scheme.ty), Some(Ty::Fn(f)) if !f.fails.is_empty())
    }

    /// The IR type arguments of the reference `expr` to the generic definition `def`
    /// (`Callee::Direct::ty_args`): the type checker's instantiation (`ProgramTypes::type_args`),
    /// with set type parameters as summaries of the instantiated sets (`ProgramTypes::type_rows`,
    /// [`Lowerer::set_ty`]).
    ///
    /// A reference within a group being inferred is not instantiated (a self reference, or a
    /// reference to a definition whose scheme was not known yet). There the two definitions share
    /// the type variables that become type parameters, so each parameter of `def` is its generalized
    /// variable seen from the definition being lowered, and a self reference passes on its own
    /// parameters.
    fn ty_args(&mut self, def: DefId, expr: ExprId) -> Vec<TypeId> {
        let types = self.types;
        let n = types.sigs.get(def).map_or(0, |s| s.scheme.params.len());
        if n == 0 {
            return Vec::new();
        }
        let params = &types
            .sigs
            .get(def)
            .expect("a generic definition has a signature")
            .scheme
            .params;
        let is_set = |i: usize| matches!(params[i].kind, emela_types::ParamKind::Set);
        let recorded = types.type_args.get(expr).map_or(&[][..], Vec::as_slice);
        if recorded.is_empty() && self.current_def == Some(def) {
            return (0..n)
                .map(|i| {
                    let p = self.module.type_param(i);
                    if is_set(i) {
                        self.module.intern(Type::Set(SetSummary {
                            params: vec![p],
                            ..SetSummary::default()
                        }))
                    } else {
                        p
                    }
                })
                .collect();
        }
        let mut out: Vec<TypeId> = recorded.iter().map(|t| self.ty(t)).collect();
        out.resize(n, TypeId::UNIT);
        for &(v, i) in types.generalized_tys.get(def).into_iter().flatten() {
            let i = i as usize;
            if i >= recorded.len() && i < n {
                out[i] = self.ty(&Ty::Var(v));
            }
        }
        // Set type parameters get the summary of the instantiated set (`ProgramTypes::type_rows`),
        // or, for a reference within a group being inferred, of the shared set variable.
        let rows = types.type_rows.get(expr);
        let generalized = types.generalized_rows.get(def);
        for (i, slot) in out.iter_mut().enumerate() {
            if !is_set(i) {
                continue;
            }
            let row = match rows.and_then(|rows| rows.get(i)) {
                Some(row) => row.clone(),
                None => match generalized
                    .into_iter()
                    .flatten()
                    .find(|&&(_, j)| j as usize == i)
                {
                    Some(&(v, _)) => Row::var(v),
                    // Not known here: may fail and need the evidence table, and suspends as the
                    // copy called does.
                    None => {
                        let spec = self.specialization_of(def, Some(expr));
                        let suspends = spec.checked_shr(i as u32).is_some_and(|b| b & 1 != 0);
                        *slot = self.module.intern(Type::Set(SetSummary {
                            suspends,
                            ..SetSummary::unknown()
                        }));
                        continue;
                    }
                },
            };
            *slot = self.set_ty(&row);
        }
        out
    }

    /// Whether a fn takes the evidence table as its last argument: yes if the R of its declared
    /// (generalized) type has an element other than Async. Opaque sets (`R(f)`, generalized sets)
    /// also need it, to pass it on to function arguments. Async is provided by the runtime
    /// (spec 8.3, 13.3) and has no handler value, so it needs no table.
    fn takes_ev(&self, def: DefId) -> bool {
        matches!(self.types.sigs.get(def).map(|s| &s.scheme.ty), Some(Ty::Fn(f)) if self.row_needs_ev(&f.uses))
    }

    /// Whether to pass the evidence table when calling a value of function type: yes if the value
    /// type's R has an element other than Async.
    fn ty_takes_ev(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Fn(f) if self.row_needs_ev(&f.uses))
    }

    /// Whether a function with effect set `row` needs an evidence table: yes if it has an element
    /// other than Async.
    fn row_needs_ev(&self, row: &Row) -> bool {
        let async_ = self.hir.prelude_def("Async");
        row.elems()
            .iter()
            .any(|e| !matches!(e, RowElem::Tag(t) if Some(t.def) == async_))
    }

    /// The current evidence table to pass as a call's last argument, or `()` if there is none (the
    /// callee does not use it; the type checker has checked R, so calls with effects that need a
    /// table only occur where there is one).
    fn ev_arg(&self) -> Expr {
        match self.ev {
            Some(ev) => Expr::Var(ev),
            None => Expr::Lit(Lit::Unit),
        }
    }

    /// Creates the effect's IR ID if it does not exist yet.
    fn effect_id(&mut self, key: &EffectKey) -> EffectId {
        if let Some(&id) = self.effects.get(key) {
            return id;
        }
        let def = key.def;
        let decl = self.ir_name(def);
        let name = if key.args.is_empty() {
            decl.clone()
        } else {
            format!("{decl}[{}]", key.args)
        };
        // The type arguments as seen from the definition the key was read in, as in `key_ty`.
        let outer = std::mem::replace(&mut self.current_def, key.ctx);
        // Keys leave out E and R (`key_ty`), so the IR type arguments do too: set arguments are
        // the placeholder `()`, and function types keep only their flags.
        let args: Vec<TypeId> = key
            .tag
            .args
            .iter()
            .map(|t| {
                let t = self.ty(t);
                self.module.types.erase_sets(t)
            })
            .collect();
        self.current_def = outer;
        // Added before its operations, whose types might mention the effect itself.
        let id = self.module.add_effect(EffectDef {
            name,
            decl,
            args: args.clone(),
            ops: Vec::new(),
        });
        self.effects.insert(key.clone(), id);
        let op_defs = match self.hir.item(def) {
            Some(Item::Effect(e)) => e.ops.clone(),
            _ => Vec::new(),
        };
        let ops = op_defs
            .into_iter()
            .map(|op| self.op_def(def, op, &args))
            .collect();
        self.module.effects[id].ops = ops;
        id
    }

    /// The operation `op` of `effect` for the effect's IR type arguments `args`: its declared type
    /// with the effect's type parameters replaced by `args` and the operation's own (`fork[A]`,
    /// spec 8.1) renumbered from 0.
    fn op_def(&mut self, effect: DefId, op: DefId, args: &[TypeId]) -> OpDef {
        let name = self.hir.defs[op].name.to_string();
        let suspend = matches!(self.hir.item(op), Some(Item::Op(o)) if o.sig.is_suspend);
        let types = self.types;
        let Some((Ty::Fn(f), n)) = types
            .sigs
            .get(op)
            .map(|s| (&s.scheme.ty, s.scheme.params.len()))
        else {
            return OpDef {
                suspend,
                ..OpDef::new(&name, Vec::new(), TypeId::UNIT)
            };
        };
        // The scheme's parameters are the effect's, then the operation's own. A declared type has
        // no type variables, and its set parameters are read as possibly suspending (as in a
        // generic signature).
        let own = n.saturating_sub(args.len());
        let mut subst = args.to_vec();
        subst.extend((0..own).map(|i| self.module.type_param(i)));
        let outer = (self.current_def.take(), self.spec);
        self.spec = Specialization::MAX;
        let mut params: Vec<TypeId> = f.params.iter().map(|t| self.ty(t)).collect();
        let ret = self.ty(&f.ret);
        (self.current_def, self.spec) = outer;
        for p in &mut params {
            *p = self.module.types.subst(*p, &subst);
        }
        let ret = self.module.types.subst(ret, &subst);
        if self.is_fork_op(effect, op) {
            params = params.into_iter().map(|p| self.wrapped_child(p)).collect();
        }
        OpDef {
            name,
            suspend,
            type_params: own,
            params,
            ret,
            fails: !f.fails.is_empty(),
        }
    }

    /// The IR type arguments of the reference `expr` to operation `index` of `effect`
    /// (`Callee::Op::ty_args`): the type checker's instantiation of the operation's own type
    /// parameters.
    fn op_ty_args(&mut self, effect: EffectId, index: usize, expr: ExprId) -> Vec<TypeId> {
        let n = self.module.effects[effect].ops[index].type_params;
        if n == 0 {
            return Vec::new();
        }
        let types = self.types;
        let recorded = types.type_args.get(expr).map_or(&[][..], Vec::as_slice);
        let mut out: Vec<TypeId> = recorded.iter().map(|t| self.ty(t)).collect();
        out.resize(n, TypeId::UNIT);
        out
    }

    /// The evidence table key of a tag (spec 8.1). `ctx` is the definition owning type parameters
    /// (`Bound(i)`) and generalized type variables.
    fn tag_key(&self, tag: &Tag, ctx: Option<DefId>) -> EffectKey {
        let args: Vec<String> = tag.args.iter().map(|t| self.key_ty(t, ctx)).collect();
        EffectKey {
            def: tag.def,
            args: args.join(", "),
            tag: tag.clone(),
            ctx,
        }
    }

    /// The key of a tag inside the definition being lowered.
    fn key_here(&self, tag: &Tag) -> EffectKey {
        self.tag_key(tag, self.current_def)
    }

    /// The type as written in a key: IR names for named types, and the owning definition's IR name
    /// and index for type parameters. Type variables left undetermined (not generalized) are `_`
    /// (lowering treats them as `()`, so they all share one key).
    fn key_ty(&self, ty: &Ty, ctx: Option<DefId>) -> String {
        let list = |tys: &[Ty]| {
            tys.iter()
                .map(|t| self.key_ty(t, ctx))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match ty {
            Ty::Prim(p) => p.name().to_owned(),
            Ty::Never => "Never".to_owned(),
            Ty::List(e) => format!("List[{}]", self.key_ty(e, ctx)),
            Ty::Tuple(es) => format!("({})", list(es)),
            Ty::Named(id, args) => {
                let name = self.ir_name(self.types.ty_table.def(*id));
                if args.is_empty() {
                    name
                } else {
                    format!("{name}[{}]", list(args))
                }
            }
            // E and R of function types are not in the key (an approximation; see NOTES.md).
            Ty::Fn(f) => format!("fn({}) -> {}", list(&f.params), self.key_ty(&f.ret, ctx)),
            Ty::Bound(i) => match ctx {
                Some(def) => format!("${}.{i}", self.ir_name(def)),
                None => format!("${i}"),
            },
            Ty::Var(v) => ctx
                .and_then(|def| self.types.generalized_tys.get(def))
                .and_then(|vars| vars.iter().find(|(w, _)| w == v))
                .map_or_else(|| "_".to_owned(), |&(_, i)| self.key_ty(&Ty::Bound(i), ctx)),
            Ty::Set(..) | Ty::Cap(_) | Ty::Error => "_".to_owned(),
        }
    }

    /// The current handler for the key's effect: from the innermost `with` providing it in the same
    /// function, or from the evidence table (`use X`, spec 8.3). `None` if neither.
    fn lookup_effect(&mut self, key: &EffectKey) -> Option<Expr> {
        if let Some((_, h)) = self.bound.iter().rev().find(|(e, _)| e == key) {
            return Some(Expr::Var(*h));
        }
        let ev = self.ev?;
        Some(Expr::Use {
            ctx: Box::new(Expr::Var(ev)),
            effect: self.effect_id(key),
        })
    }

    /// The operation's index in `EffectDef::ops`.
    fn op_index(&self, effect: DefId, op: DefId) -> Option<usize> {
        match self.hir.item(effect) {
            Some(Item::Effect(e)) => e.ops.iter().position(|&o| o == op),
            _ => None,
        }
    }

    /// The external function's bindings. A module naming a file becomes the file next to the
    /// declaring source (spec 12.1), or the bundled file for bundled sources.
    fn external(&mut self, def: DefId, f: &emela_resolve::hir::FnItem) -> External {
        let module_id = match &self.hir.defs[def].module {
            DefModule::Source(m) => Some(*m),
            _ => None,
        };
        let is_core = module_id.is_some_and(|m| self.modules.is_core(m));
        let dir = module_id
            .and_then(|m| self.paths.get(m))
            .and_then(|p| p.parent());
        let span = self.hir.defs[def].span;
        let mut bindings = Vec::new();
        for e in &f.externals {
            let host = match e.args.as_slice() {
                [name] => HostRef::Global(name.clone()),
                [module, name] => {
                    let module = if !(module.starts_with("./") || module.starts_with("../")) {
                        HostModule::Named(module.clone())
                    } else if is_core {
                        let lib_dir = module_id
                            .map(|m| crate::lib_dir(&self.modules[m].name.to_string()))
                            .unwrap_or("core");
                        HostModule::Bundled(self.bundled_file(lib_dir, module, span))
                    } else if let Some(dir) = dir {
                        HostModule::File(normalize(&dir.join(module)))
                    } else {
                        HostModule::Named(module.clone())
                    };
                    HostRef::Import {
                        module,
                        name: name.clone(),
                    }
                }
                // The parser reported the malformed attribute (E0138).
                _ => continue,
            };
            bindings.push(ExternalBinding {
                target: e.target.to_string(),
                host,
            });
        }
        External { bindings }
    }

    /// Resolves a bundled core file specifier (`./x.mjs`) to its path relative to `lib/`
    /// (`std/x.mjs`). `spec` is relative to the source's directory under `lib/` (`lib_dir`: `std`
    /// or `core`).
    fn bundled_file(&mut self, lib_dir: &str, spec: &str, span: Option<Span>) -> String {
        let found = lib_relative(&format!("{lib_dir}/{spec}"))
            .filter(|path| self.bundled_files.iter().any(|(p, _)| p == path));
        match found {
            Some(path) => {
                self.uses_bundled = true;
                path
            }
            None => {
                self.errors.push(LowerError::MissingCoreFile {
                    path: spec.to_owned(),
                    span,
                });
                spec.to_owned()
            }
        }
    }

    /// Lowers a handler to IR functions (called from `define_fn`). Returns the parameters and body
    /// of the function that returns the handler value.
    ///
    /// A stateless handler is a function returning the handler value (operations as lambdas in
    /// declaration order). If it has dependencies (the R of operation bodies, spec 8.4), it takes
    /// the evidence table at the `with` as its last argument.
    ///
    /// A stateful handler (with fields, init or release, spec 8.4) becomes three functions. The init
    /// function (`name$init`) returns the state (self); the function returning the handler value
    /// takes the state as its first argument for the operation lambdas to capture; and the release
    /// function (`name$release`) takes the state and evaluates the release body. All three take the
    /// same evidence table as their last argument (dependencies(H) is the union of the R of init,
    /// release and operation bodies).
    fn handler(&mut self, def: DefId) -> (Vec<Local>, TypeId, Expr) {
        let hir = self.hir;
        let Some(Item::Handler(h)) = hir.item(def) else {
            unreachable!("a handler definition");
        };
        let span = hir.defs[def].span;
        let Res::Def(effect) = h.effect else {
            return (
                Vec::new(),
                TypeId::UNIT,
                self.unsupported_at("handler of an unknown effect", span),
            );
        };
        let Some(tag) = self.types.sigs.handler_effect(def) else {
            return (
                Vec::new(),
                TypeId::UNIT,
                self.unsupported_at("handler of an unknown effect", span),
            );
        };
        let effect_id = self.effect_id(&self.tag_key(tag, None));
        let ret = self.module.handler(effect_id);
        let takes_ev = self
            .types
            .handler_uses
            .get(def)
            .is_some_and(|r| self.row_needs_ev(r));
        let stateful = is_stateful(h);
        if stateful {
            self.handler_parts(def, h, takes_ev);
        }

        let mut params = Vec::new();
        if stateful {
            // Every operation's implicit self refers to this parameter.
            let state_ty = self.handler_state_ty(def, h);
            let state = self.module.local("self", state_ty);
            for op in &h.ops {
                self.locals.insert(op.self_local, state);
            }
            params.push(state);
        }
        self.ev = None;
        if takes_ev {
            let ev = self.ev_local();
            params.push(ev);
            self.ev = Some(ev);
        }
        let n = self.module.effects[effect_id].ops.len();
        let mut ops: Vec<Option<Expr>> = vec![None; n];
        for op in &h.ops {
            let index = match op.op {
                Res::Def(o) => self.op_index(effect, o),
                _ => None,
            };
            let (Some(index), Some(body)) = (index, op.body) else {
                continue;
            };
            // A suspend fn implementation takes the operation's parameters plus resume last
            // (spec 8.4). resume is an ordinary function value; calling it neither suspends nor
            // fails (its type is `fn(T) -> ()`).
            let ps: Vec<Local> = op.params.iter().map(|p| self.local(p.local)).collect();
            let lowered = self.expr(body);
            // If the body may suspend (awaiting an `@external suspend fn`, say), the implementation
            // is a generator too. Async that only calls resume later (spec 8.7) is not in the
            // body's R, so it does not suspend.
            let row_suspends = self
                .types
                .effects
                .get(body)
                .is_some_and(|r| self.row_suspends(r));
            // As in the type checker (E0417), an fn-operation implementation is not a generator
            // unless its body's R is not Immediate and it contains a suspending call. With Clock in
            // the body's R, calling only `clock.now()` does not suspend. When R is Immediate but a
            // true call occurs (inside a `with` of an immediately resuming handler), the backend
            // runs it on the spot, as in non-suspending function bodies.
            let suspends = if op.is_suspend {
                row_suspends
            } else {
                row_suspends && suspend::has_suspending_call(&lowered)
            };
            // A failure in an operation body returns to the operation's caller (as with lambdas).
            let ret = self.ty(self.expr_ty(body));
            let ty = self.module.func(FnType {
                params: ps.iter().map(|&l| self.module.locals[l].ty).collect(),
                ret,
                fails: self.types.errors.get(body).is_some_and(|r| !r.is_empty()),
                suspends,
                ev: false,
                sets: None,
            });
            ops[index] = Some(Expr::Lambda {
                params: ps,
                ty,
                body: Box::new(lowered),
                suspends,
            });
        }
        if ops.iter().any(Option::is_none) {
            // Missing operations have already been reported by the type checker (E0404).
            return (
                params,
                ret,
                self.unsupported_at("handler with missing operations", span),
            );
        }
        let ops = ops.into_iter().map(Option::unwrap).collect();
        (
            params,
            ret,
            Expr::Handler {
                name: hir.defs[def].name.to_string(),
                effect: effect_id,
                ops,
            },
        )
    }

    /// Lowers the bodies of a stateful handler's init and release functions.
    fn handler_parts(&mut self, def: DefId, h: &emela_resolve::hir::HandlerItem, takes_ev: bool) {
        let (init, release) = self.handler_fns(def);
        let ev_param = |cx: &mut Self| {
            cx.ev = None;
            takes_ev.then(|| {
                let ev = cx.ev_local();
                cx.ev = Some(ev);
                ev
            })
        };
        let state_ty = self.handler_state_ty(def, h);

        // init's value is the handler's state. For a handler without init (no fields), the state
        // is its constructor's value if it has one (`ConsoleLogger`), otherwise `()`.
        let params: Vec<Local> = ev_param(self).into_iter().collect();
        let body = match (h.init, h.ctor.and_then(|c| self.ctors.get(c).copied())) {
            (Some(body), _) => self.expr(body),
            (None, Some(ctor)) => Expr::Ctor {
                ctor,
                ty_args: Vec::new(),
                args: Vec::new(),
            },
            (None, None) => Expr::Lit(Lit::Unit),
        };
        self.module.define(init, params, state_ty, body);

        if let (Some(f), Some((self_local, body))) = (release, h.release) {
            // The state's type, which the type checker gives as the handler's type even when the
            // state is `()` (no fields and no init).
            let state = self.local(self_local);
            self.module.locals[state].ty = state_ty;
            let mut params = vec![state];
            params.extend(ev_param(self));
            // The release block's value is discarded (spec 8.4), but the function returns it as is.
            let ret = self.ty(self.expr_ty(body));
            let body = self.expr(body);
            self.module.define(f, params, ret, body);
        }
    }

    /// The type of a stateful handler's state (self, spec 8.4): init's value, else the handler's
    /// constructor value, else `()` (as built by `handler_parts`).
    fn handler_state_ty(&mut self, def: DefId, h: &emela_resolve::hir::HandlerItem) -> TypeId {
        match (h.init, self.enums.get(def)) {
            (Some(body), _) => {
                let ty = self.expr_ty(body);
                self.ty(ty)
            }
            (None, Some(&id)) if h.ctor.is_some() => self.module.enum_type(id, Vec::new()),
            _ => TypeId::UNIT,
        }
    }

    /// The IR functions of a stateful handler's init and release (if any), declared if they do not
    /// exist yet. Bodies are lowered in `define_fn` together with the function returning the
    /// handler value (`fn_id(def, 0)`).
    ///
    /// Whether they may suspend is decided by the R of the body expressions (init awaiting an
    /// `@external suspend fn`, say).
    fn handler_fns(&mut self, def: DefId) -> (FnId, Option<FnId>) {
        if let Some(&fns) = self.handler_parts.get(&def) {
            return fns;
        }
        self.fn_id(def, 0);
        let Some(Item::Handler(h)) = self.hir.item(def) else {
            unreachable!("a handler definition");
        };
        let name = self.ir_name(def);
        let suspends = |cx: &Self, body: ExprId| {
            cx.types
                .effects
                .get(body)
                .is_some_and(|r| r.tags().any(|t| !cx.is_immediate_effect(t.def)))
        };
        let init = self.module.declare(&format!("{name}$init"), false);
        self.module.functions[init].suspends = h.init.is_some_and(|b| suspends(self, b));
        self.module.functions[init].fails = self
            .types
            .handler_fails
            .get(def)
            .is_some_and(|r| !r.is_empty());
        let release = h.release.map(|(_, body)| {
            let f = self.module.declare(&format!("{name}$release"), false);
            self.module.functions[f].suspends = suspends(self, body);
            f
        });
        self.handler_parts.insert(def, (init, release));
        (init, release)
    }

    /// `with H1, ..., Hn { body }`（8.3，14.2）．
    ///
    /// Builds the handlers in the initialization order chosen by the type checker
    /// (`ProgramTypes::with_handlers`; layers expanded, handlers overridden by later ones removed)
    /// and evaluates the body with the current evidence table extended by the provided effects. A
    /// handler's dependencies are satisfied by handlers in the same `with` providing other effects;
    /// the rest, and dependencies on its own effect, by the outer table. Dependency cycles have been
    /// reported by the type checker (E0411).
    ///
    /// For a stateful handler, init's value (the state) becomes a binding without release, and the
    /// handler value built from the state a binding with release (`WithBind::managed`). Without a
    /// release function, release is `()`.
    fn with(&mut self, id: ExprId, handlers: &[(Res, Span)], body: ExprId, span: Span) -> Expr {
        let hir = self.hir;
        let Some(order) = self.types.with_handlers.get(id) else {
            return self.unsupported("with without handlers", span);
        };
        // (handler, key of the provided effect).
        let mut hs: Vec<(DefId, EffectKey)> = Vec::new();
        for &def in order {
            // Errors go at the handler as written; for handlers expanded from a layer, at the
            // `with`.
            let span = handlers
                .iter()
                .find(|(res, _)| *res == Res::Def(def))
                .map_or(span, |&(_, s)| s);
            match hir.item(def) {
                Some(Item::Handler(_)) => {
                    let Some(tag) = self.types.sigs.handler_effect(def) else {
                        return self.unsupported("handler of an unknown effect", span);
                    };
                    hs.push((def, self.tag_key(tag, None)));
                }
                _ => return self.unsupported("this kind of handler", span),
            }
        }
        let outer = self.ev;
        let mut locals: Vec<Local> = Vec::with_capacity(hs.len());
        let mut binds = Vec::new();
        for (i, (h, own)) in hs.iter().enumerate() {
            let h = *h;
            let f = self.fn_id(h, 0);
            let ctx = if self
                .types
                .handler_uses
                .get(h)
                .is_some_and(|r| self.row_needs_ev(r))
            {
                // Dependencies satisfied by handlers of other effects initialized earlier in the
                // same `with`.
                let row = self.types.handler_uses.get(h).cloned().unwrap_or_default();
                let keys: Vec<EffectKey> = row
                    .tags()
                    .map(|t| self.tag_key(t, None))
                    .filter(|y| y != own)
                    .collect();
                let bindings: Vec<(EffectId, Expr)> = keys
                    .into_iter()
                    .filter_map(|y| {
                        let j = hs[..i].iter().position(|(_, e)| *e == y)?;
                        Some((self.effect_id(&y), Expr::Var(locals[j])))
                    })
                    .collect();
                Some(if bindings.is_empty() {
                    self.ev_arg()
                } else {
                    Expr::Provide {
                        base: outer.map(|l| Box::new(Expr::Var(l))),
                        bindings,
                    }
                })
            } else {
                None
            };
            let handler_ty = self.effect_id(own);
            let handler_ty = self.module.handler(handler_ty);
            let l = self.module.local("h", handler_ty);
            locals.push(l);
            let Some(Item::Handler(item)) = hir.item(h) else {
                unreachable!("a handler definition");
            };
            if !is_stateful(item) {
                binds.push(WithBind::new(
                    l,
                    direct(f, ctx.into_iter().collect(), false),
                ));
                continue;
            }
            // A stateful handler uses the table three times, so bind it to a variable.
            let ctx = ctx.map(|c| match c {
                Expr::Var(_) | Expr::Lit(_) => c,
                c => {
                    let v = self.ev_local();
                    binds.push(WithBind::new(v, c));
                    Expr::Var(v)
                }
            });
            let (init, release) = self.handler_fns(h);
            let state_ty = self.handler_state_ty(h, item);
            let state = self.module.local("self", state_ty);
            let init_fails = self
                .types
                .handler_fails
                .get(h)
                .is_some_and(|r| !r.is_empty());
            binds.push(WithBind::new(
                state,
                direct_marked(
                    &self.module,
                    init,
                    ctx.clone().into_iter().collect(),
                    init_fails,
                ),
            ));
            let with_state = |ctx: &Option<Expr>| {
                std::iter::once(Expr::Var(state))
                    .chain(ctx.clone())
                    .collect::<Vec<_>>()
            };
            let release = match release {
                Some(r) => direct_marked(&self.module, r, with_state(&ctx), false),
                None => Expr::Lit(Lit::Unit),
            };
            binds.push(WithBind::managed(
                l,
                direct(f, with_state(&ctx), false),
                &hir.defs[h].name,
                release,
            ));
        }
        let ctx = self.ev_local();
        let depth = self.bound.len();
        for ((_, effect), &l) in hs.iter().zip(&locals) {
            self.bound.push((effect.clone(), l));
        }
        self.ev = Some(ctx);
        let body = self.expr(body);
        self.ev = outer;
        self.bound.truncate(depth);
        // Not built if the body does not use the table (when `use` only refers to this `with`'s
        // handlers directly).
        if mentions(&body, ctx) {
            let bindings = hs
                .iter()
                .zip(&locals)
                .map(|((_, effect), &l)| (self.effect_id(effect), Expr::Var(l)))
                .collect();
            binds.push(WithBind::new(
                ctx,
                Expr::Provide {
                    base: outer.map(|l| Box::new(Expr::Var(l))),
                    bindings,
                },
            ));
        }
        Expr::With {
            binds,
            body: Box::new(body),
        }
    }

    /// `use X` (spec 8.2, 8.3). The handler of the innermost `with` providing X in the same
    /// function, or else from the evidence table passed by the caller. For effects with type
    /// arguments, the type arguments must match too (spec 8.1).
    fn use_(&mut self, id: ExprId, span: Span) -> Expr {
        let Some(Ty::Cap(tag)) = self.types.exprs.get(id) else {
            return self.unsupported("unresolved effect", span);
        };
        let key = self.key_here(tag);
        match self.lookup_effect(&key) {
            Some(e) => e,
            // Unreachable once type checking has checked R.
            None => self.unsupported("`use` without a provided effect", span),
        }
    }

    /// A lambda. If its type's R is not empty, it takes the evidence table as its last argument.
    /// Bindings of enclosing `with`s are added to the received table for the body (`use X` looks
    /// at enclosing `with`s first, spec 8.3).
    fn lambda(&mut self, id: ExprId, params: &[emela_resolve::hir::Param], body: ExprId) -> Expr {
        let mut ps: Vec<Local> = params.iter().map(|p| self.local(p.local)).collect();
        let suspends = self.ty_suspends(self.expr_ty(id));
        let ty = self.ty(self.expr_ty(id));
        if !self.ty_takes_ev(self.expr_ty(id)) {
            return Expr::Lambda {
                params: ps,
                ty,
                body: Box::new(self.expr(body)),
                suspends,
            };
        }
        let outer = self.ev;
        let p = self.ev_local();
        ps.push(p);
        let (ctx, prelude) = if self.bound.is_empty() {
            (p, None)
        } else {
            let c = self.ev_local();
            let bindings = self
                .bound
                .clone()
                .into_iter()
                .map(|(e, h)| (self.effect_id(&e), Expr::Var(h)))
                .collect();
            (
                c,
                Some(Expr::Provide {
                    base: Some(Box::new(Expr::Var(p))),
                    bindings,
                }),
            )
        };
        self.ev = Some(ctx);
        let mut body = self.expr(body);
        self.ev = outer;
        if let Some(value) = prelude.filter(|_| mentions(&body, ctx)) {
            body = Expr::Let {
                var: ctx,
                value: Box::new(value),
                body: Box::new(body),
            };
        }
        Expr::Lambda {
            params: ps,
            ty,
            body: Box::new(body),
            suspends,
        }
    }

    /// An operation on a capability value used as a value without calling it
    /// (`List.each(xs, log.info)`). Becomes a lambda that just calls it. The capability value is
    /// evaluated at the reference.
    fn op_value(&mut self, id: ExprId, base: ExprId, op: DefId, span: Span) -> Expr {
        let Some(Ty::Cap(tag)) = self.types.exprs.get(base).cloned() else {
            return self.unsupported("effect operation", span);
        };
        let Some(index) = self.op_index(tag.def, op) else {
            return self.unsupported("effect operation", span);
        };
        let effect_id = self.effect_id(&self.key_here(&tag));
        let fails = self.callee_fails(id);
        let suspends = self.module.effects[effect_id].ops[index].suspend;
        let (param_tys, ev) = match self.expr_ty(id) {
            Ty::Fn(f) => (f.params.clone(), self.row_needs_ev(&f.uses)),
            _ => unreachable!("an operation reference has a function type"),
        };
        let param_tys: Vec<TypeId> = param_tys.iter().map(|t| self.ty(t)).collect();
        let ty = self.ty(self.expr_ty(id));
        let handler = self.expr(base);
        let h_ty = self.module.handler(effect_id);
        let h = self.module.local("h", h_ty);
        // Scope's fork wraps the child function, as at a call (`fiber.rs`).
        if self.is_fork_op(tag.def, op) {
            let op_ty = self.expr_ty(id).clone();
            let Some(fork) = self.fork_call(Expr::Var(h), &tag, &op_ty, id) else {
                return self.unsupported("fork", span);
            };
            let x_ty = param_tys.first().copied().unwrap_or(TypeId::UNIT);
            let x = self.module.local("x", x_ty);
            // The evidence table, if the type takes one, is unused: operation calls take no table
            // (`Callee::Op`).
            let mut params = vec![x];
            if ev {
                params.push(self.ev_local());
            }
            return Expr::Let {
                var: h,
                value: Box::new(handler),
                body: Box::new(Expr::Lambda {
                    params,
                    ty,
                    body: Box::new(fork.apply(vec![Expr::Var(x)])),
                    suspends: false,
                }),
            };
        }
        let mut params: Vec<Local> = param_tys
            .into_iter()
            .map(|t| self.module.local("x", t))
            .collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        if ev {
            params.push(self.ev_local());
        }
        Expr::Let {
            var: h,
            value: Box::new(handler),
            body: Box::new(Expr::Lambda {
                params,
                ty,
                body: Box::new(Expr::Call {
                    callee: Callee::Op {
                        handler: Box::new(Expr::Var(h)),
                        effect: effect_id,
                        op: index,
                        ty_args: self.op_ty_args(effect_id, index, id),
                    },
                    args,
                    fails,
                    suspends,
                }),
                suspends,
            }),
        }
    }

    /// The IR function of the entry module's `main`.
    fn entry_main(&self) -> Option<FnId> {
        let items = self.hir.modules.get(self.entry)?;
        items.items.iter().find_map(|&def| {
            let data = &self.hir.defs[def];
            (data.name == "main" && data.kind == DefKind::Fn)
                .then(|| self.fns.get(&(def, 0)).copied())
                .flatten()
        })
    }

    /// Reports external functions reachable from `entries` that lack an `@external` for `target`
    /// (spec 12.1). Functions called from operation bodies of handlers in `with`s are followed too
    /// (since the function building the handler value is called).
    fn check_target(&mut self, entries: &[FnId], target: &str) {
        let missing = unannotated_reachable(&self.module, entries, target);
        let mut defs: Vec<DefId> = missing
            .into_iter()
            .map(|f| {
                self.fns
                    .iter()
                    .find(|(_, id)| **id == f)
                    .map(|(&(d, _), _)| d)
                    .expect("IR functions are made from HIR definitions")
            })
            .collect();
        // In order of declaration position.
        defs.sort_by_key(|&d| {
            self.hir.defs[d]
                .span
                .map(|s| (s.module.into_raw().into_u32(), s.range.start()))
        });
        for def in defs {
            self.errors.push(LowerError::MissingExternal {
                name: self.hir.defs[def].name.to_string(),
                target: target.to_owned(),
                span: self.hir.defs[def].span,
            });
        }
    }

    /// An expression not supported yet. Records a diagnostic and returns `()` to keep going.
    fn unsupported(&mut self, what: &str, span: Span) -> Expr {
        self.report(what, span);
        Expr::Lit(Lit::Unit)
    }

    /// [`Lowerer::unsupported`] where the position may be missing.
    fn unsupported_at(&mut self, what: &str, span: Option<Span>) -> Expr {
        self.errors.push(LowerError::Unsupported {
            what: what.to_owned(),
            span,
        });
        Expr::Lit(Lit::Unit)
    }

    fn report(&mut self, what: &str, span: Span) {
        self.errors.push(LowerError::Unsupported {
            what: what.to_owned(),
            span: Some(span),
        });
    }

    /// The type checker has typed every expression.
    fn expr_ty(&self, id: ExprId) -> &'a Ty {
        &self.types.exprs[id]
    }

    fn expr(&mut self, id: ExprId) -> Expr {
        // `self.hir` is `&'a Program`, so a copy lets us read the HIR without borrowing `self`.
        let hir = self.hir;
        let e = &hir.exprs[id];
        match &e.kind {
            ExprKind::Lit(lit) => self.lit(id, lit),
            ExprKind::Path(res) => self.path(id, *res, e.span),
            ExprKind::Call { callee, args } => {
                let written = args.iter().map(|a| a.value).collect();
                self.call(id, *callee, written)
            }
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                // `x |> f(a)` is `f(x, a)`, and `x |> f` is `f(x)` (spec 6.7).
                let (callee, mut written) = match &hir.exprs[*rhs].kind {
                    ExprKind::Call { callee, args } => {
                        (*callee, args.iter().map(|a| a.value).collect())
                    }
                    _ => (*rhs, Vec::new()),
                };
                written.insert(0, *lhs);
                self.call(id, callee, written)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, *lhs, *rhs, e.span),
            ExprKind::Unary { op, operand } => {
                let (op, ty) = match op {
                    UnaryOp::Neg => (UnOp::Neg, self.op_ty(*operand)),
                    UnaryOp::Not => (UnOp::Not, Some(OpTy::Bool)),
                };
                let Some(ty) = ty else {
                    return self.unsupported("negation of this type", e.span);
                };
                Expr::Unary {
                    op,
                    ty,
                    operand: Box::new(self.expr(*operand)),
                }
            }
            ExprKind::Unit => Expr::Lit(Lit::Unit),
            ExprKind::Tuple(es) => Expr::Tuple(es.iter().map(|&e| self.expr(e)).collect()),
            ExprKind::List { elems, rest } => Expr::List {
                elem: match self.expr_ty(id) {
                    Ty::List(t) => self.ty(t),
                    _ => unreachable!("a list expression has a list type"),
                },
                elems: elems.iter().map(|&e| self.expr(e)).collect(),
                tail: rest.map(|r| Box::new(self.expr(r))),
            },
            ExprKind::Block(block) => self.block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => Expr::If {
                cond: Box::new(self.expr(*cond)),
                then: Box::new(self.expr(*then_branch)),
                else_: Box::new(match else_branch {
                    Some(b) => self.expr(*b),
                    None => Expr::Lit(Lit::Unit),
                }),
            },
            ExprKind::Missing => self.unsupported("missing expression", e.span),
            ExprKind::Interpolated(parts) => self.interpolated(parts),
            ExprKind::Field { base, .. } => self.field(id, *base, e.span),
            ExprKind::Match { scrutinee, arms } => Expr::Match {
                scrutinee: Box::new(self.expr(*scrutinee)),
                arms: arms.iter().map(|arm| self.arm(arm)).collect(),
            },
            ExprKind::Lambda { params, body } => self.lambda(id, params, *body),
            ExprKind::Assert(cond) => self.assert(*cond),
            ExprKind::Update { ctor, fields, base } => {
                self.update(id, *ctor, fields, *base, e.span)
            }
            ExprKind::Escape { expr, arms } => Expr::Escape {
                body: Box::new(self.expr(*expr)),
                arms: arms.iter().map(|arm| self.arm(arm)).collect(),
            },
            ExprKind::Fail(inner) => Expr::Fail(Box::new(self.expr(*inner))),
            ExprKind::Use { .. } => self.use_(id, e.span),
            ExprKind::With { handlers, body } => self.with(id, handlers, *body, e.span),
        }
    }

    fn lit(&mut self, id: ExprId, lit: &Literal) -> Expr {
        Expr::Lit(self.lit_value(self.expr_ty(id), lit))
    }

    /// The type (Int or Int64) decides an integer's width. The range was checked by the type
    /// checker (E0311).
    fn lit_value(&self, ty: &Ty, lit: &Literal) -> Lit {
        match lit {
            Literal::Int(text) => {
                let n = int_literal_value(text)
                    .expect("an integer whose range the type checker checked");
                if *ty == Ty::INT64 {
                    Lit::Int64(n as i64)
                } else {
                    Lit::Int(n as i32)
                }
            }
            Literal::Float(text) => Lit::Float(text.parse().expect("a float checked by the lexer")),
            Literal::String(s) => Lit::String(s.clone()),
        }
    }

    /// A name reference outside a call (a reference as a value).
    fn path(&mut self, id: ExprId, res: Res, span: Span) -> Expr {
        let def = match res {
            // `self` of a stateless handler has no value (the function building the handler value
            // does not take one).
            Res::Local(l)
                if self.hir.locals[l].kind == LocalKind::SelfParam
                    && self.locals.get(l).is_none() =>
            {
                return Expr::Lit(Lit::Unit);
            }
            Res::Local(l) => return Expr::Var(self.local(l)),
            Res::Def(def) => def,
            Res::Err => return self.unsupported("unresolved name", span),
        };
        if Some(def) == self.true_def {
            return Expr::Lit(Lit::Bool(true));
        }
        if Some(def) == self.false_def {
            return Expr::Lit(Lit::Bool(false));
        }
        match self.hir.item(def) {
            // A generic fn with effects with type arguments in its R must add keys at every call,
            // so it is wrapped.
            Some(Item::Fn(f)) if f.body.is_some() && self.has_generic_tags(def) => {
                self.wrap_in_lambda(id, span)
            }
            Some(Item::Fn(f)) if f.body.is_some() => self.fn_value(def, id),
            // Builtins and constructors with fields are not IR values, so they are wrapped in a
            // lambda.
            Some(Item::Fn(f)) if f.is_intrinsic => self.wrap_in_lambda(id, span),
            Some(Item::Fn(_)) => self.fn_value(def, id),
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) if self.module.variant(ctor).fields.is_empty() => Expr::Ctor {
                    ctor,
                    ty_args: self.ty_args(def, id),
                    args: Vec::new(),
                },
                _ => self.wrap_in_lambda(id, span),
            },
            // A const's value cannot fail (spec 5.6).
            Some(Item::Const(_)) => Expr::Call {
                callee: Callee::Direct {
                    f: self.fn_id(def, 0),
                    ty_args: self.ty_args(def, id),
                },
                args: Vec::new(),
                fails: false,
                suspends: false,
            },
            _ => self.unsupported("reference to this kind of definition", span),
        }
    }

    /// The fn `def` referenced by `expr` as a value.
    fn fn_value(&mut self, def: DefId, expr: ExprId) -> Expr {
        Expr::Fn {
            f: self.fn_ref(def, expr),
            ty_args: self.ty_args(def, expr),
        }
    }

    /// `written` are the arguments in written order (for a pipe, the left side first). Each
    /// argument's position comes from the parameter order arranged by the type checker
    /// (`ProgramTypes::calls`).
    fn call(&mut self, id: ExprId, callee: ExprId, written: Vec<ExprId>) -> Expr {
        let span = self.hir.exprs[id].span;
        let slots = &self.types.calls[id];
        let positions: Vec<usize> = written
            .iter()
            .map(|&arg| {
                slots
                    .iter()
                    .position(|s| *s == Some(arg))
                    .expect("the type checker placed each argument at a parameter position")
            })
            .collect();

        // Resolve the callee before the arguments (evaluation is left to right, spec 14.1).
        let target = match self.callee(callee, span) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let args: Vec<(usize, Expr, TypeId)> = positions
            .into_iter()
            .zip(written)
            .map(|(pos, arg)| {
                let ty = self.expr_ty(arg);
                let ty = self.ty(ty);
                (pos, self.expr(arg), ty)
            })
            .collect();
        let (target, binding) = match target {
            Target::Value(f, call, ev) if !matches!(f, Expr::Var(_) | Expr::Fn { .. }) => {
                // Reordering arguments and binding them with `let` would evaluate them before the
                // callee, so bind the callee first.
                let ty = self.expr_ty(callee);
                let ty = self.ty(ty);
                let l = self.module.local("f", ty);
                (Target::Value(Expr::Var(l), call, ev), Some((l, f)))
            }
            Target::Op {
                handler,
                effect,
                op,
                ty_args,
                call,
            } if !matches!(handler, Expr::Var(_) | Expr::Use { .. }) => {
                // An expression that only looks up the evidence table (`(use X).op()`) has no
                // side effects, so it is not bound.
                let h_ty = self.module.handler(effect);
                let l = self.module.local("h", h_ty);
                let t = Target::Op {
                    handler: Expr::Var(l),
                    effect,
                    op,
                    ty_args,
                    call,
                };
                (t, Some((l, handler)))
            }
            t => (t, None),
        };
        let call = named::in_written_order(&mut self.module, args, |args| target.apply(args));
        match binding {
            Some((var, value)) => Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(call),
            },
            None => call,
        }
    }

    /// Turns a reference (`id`) to a builtin or constructor into a lambda that just calls it:
    /// `List.map(xs, Some)` becomes `List.map(xs, fn(x) { Some(x) })`. The arity comes from the
    /// reference's type.
    ///
    /// If the reference type's R needs an evidence table, the lambda takes the table as its last
    /// argument and uses it as the caller's table (`State.run`, and generic fns with effects with
    /// type arguments in their R).
    fn wrap_in_lambda(&mut self, id: ExprId, span: Span) -> Expr {
        let ev = self.ty_takes_ev(self.expr_ty(id)).then(|| self.ev_local());
        let (outer, bound) = (self.ev, std::mem::take(&mut self.bound));
        if ev.is_some() {
            self.ev = ev;
        }
        let target = self.callee(id, span);
        self.ev = outer;
        self.bound = bound;
        let target = match target {
            Ok(t) => t,
            Err(e) => return e,
        };
        let param_tys = match self.expr_ty(id) {
            Ty::Fn(f) => &f.params,
            _ => unreachable!("a reference to a callable definition has a function type"),
        };
        let mut params: Vec<Local> = param_tys
            .iter()
            .map(|t| {
                let ty = self.ty(t);
                self.module.local("x", ty)
            })
            .collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        params.extend(ev);
        Expr::Lambda {
            params,
            ty: self.ty(self.expr_ty(id)),
            body: Box::new(target.apply(args)),
            suspends: self.ty_suspends(self.expr_ty(id)),
        }
    }

    /// If it cannot be lowered, records a diagnostic and puts a substitute expression in `Err`.
    fn callee(&mut self, callee: ExprId, span: Span) -> Result<Target, Expr> {
        let hir = self.hir;
        // A call to a function value is decided by the callee expression's type.
        let call = Call {
            fails: self.callee_fails(callee),
            suspends: self.ty_suspends(self.expr_ty(callee)),
        };
        let ev = self
            .ty_takes_ev(self.expr_ty(callee))
            .then(|| self.ev_arg());
        // An operation on a capability value (`log.info(...)`).
        if let ExprKind::Field { base, .. } = &hir.exprs[callee].kind
            && let Some(&op) = self.types.ops.get(callee)
            && let Some(Ty::Cap(effect)) = self.types.exprs.get(*base).cloned()
            && let Some(index) = self.op_index(effect.def, op)
        {
            let tag = effect;
            let effect = self.effect_id(&self.key_here(&tag));
            let handler = self.expr(*base);
            // Scope's fork wraps the child function (`fiber.rs`).
            if self.is_fork_op(tag.def, op) {
                let op_ty = self.expr_ty(callee).clone();
                return match self.fork_call(handler, &tag, &op_ty, callee) {
                    Some(fork) => Ok(Target::Fork(Box::new(fork))),
                    None => Err(self.unsupported("fork", span)),
                };
            }
            // Calls to suspend fn operations may suspend (spec 8.1); fn operation bodies do not
            // (E0417).
            let call = Call {
                suspends: self.module.effects[effect].ops[index].suspend,
                ..call
            };
            return Ok(Target::Op {
                handler,
                effect,
                op: index,
                ty_args: self.op_ty_args(effect, index, callee),
                call,
            });
        }
        let ExprKind::Path(Res::Def(def)) = hir.exprs[callee].kind else {
            return Ok(Target::Value(self.expr(callee), call, ev));
        };
        match hir.item(def) {
            Some(Item::Fn(f)) if f.body.is_some() => {
                let ev = self
                    .takes_ev(def)
                    .then(|| self.generic_ev(def, callee, self.ev_arg()));
                let f = self.fn_ref(def, callee);
                let ty_args = self.ty_args(def, callee);
                Ok(Target::Direct(f, ty_args, self.direct_call(f, call), ev))
            }
            Some(Item::Fn(f)) if f.is_intrinsic && self.is_state_run(def) => self
                .state_run(callee, span)
                .map(|r| Target::StateRun(Box::new(r))),
            Some(Item::Fn(f)) if f.is_intrinsic => {
                if let Some(target) = self.fiber_target(def, callee, call, span) {
                    return target;
                }
                let data = &hir.defs[def];
                let module = match &data.module {
                    DefModule::Source(m) => self.modules[*m].name.to_string(),
                    _ => String::new(),
                };
                let Some(op) = Builtin::lookup_intrinsic(&module, &data.name) else {
                    return Err(self.unsupported(&format!("{module}.{}", data.name), span));
                };
                let ty_args = match self.types.type_args.get(callee) {
                    Some(tys) => tys.iter().map(|t| self.ty(t)).collect(),
                    None => Vec::new(),
                };
                Ok(Target::Builtin { op, ty_args })
            }
            // External functions do not fail (spec 12.5), and their R is empty or only Async
            // (spec 12.2), so no table is passed.
            Some(Item::Fn(_)) => {
                let f = self.fn_ref(def, callee);
                let ty_args = self.ty_args(def, callee);
                let call = Call {
                    fails: false,
                    ..call
                };
                Ok(Target::Direct(f, ty_args, self.direct_call(f, call), None))
            }
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) => Ok(Target::Ctor(ctor, self.ty_args(def, callee))),
                None => Err(self.unsupported("this constructor", span)),
            },
            _ => Ok(Target::Value(self.expr(callee), call, ev)),
        }
    }

    /// Whether the definition's declared R has tags with type parameters (`S` of `State[S]`).
    fn has_generic_tags(&self, def: DefId) -> bool {
        !self.generic_tags(def).is_empty()
    }

    /// Tags in the declared R that contain type parameters.
    fn generic_tags(&self, def: DefId) -> Vec<Tag> {
        match self.types.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => f
                .uses
                .tags()
                .filter(|t| t.args.iter().any(has_bound))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The evidence table for calling the generic definition `def` (spec 8.1). The callee's body
    /// looks up keys written with its own type parameters (`State[$modify.0]`), so the handler for
    /// the caller's instantiated key (`State[Int]`) is added to the current table `ev` under that
    /// key too. The instantiated type arguments are the `type_args` of the referencing expression
    /// `callee`.
    fn generic_ev(&mut self, def: DefId, callee: ExprId, ev: Expr) -> Expr {
        let tags = self.generic_tags(def);
        let Some(args) = self.types.type_args.get(callee).cloned() else {
            // Monomorphic references within a group (recursion) do not instantiate type
            // parameters, so the keys work as is.
            return ev;
        };
        let mut bindings = Vec::new();
        for tag in tags {
            let inner = self.tag_key(&tag, Some(def));
            let instance = tag.map_args(|t| emela_types::substitute_bound(t, &args));
            let outer = self.key_here(&instance);
            if inner == outer {
                continue;
            }
            if let Some(h) = self.lookup_effect(&outer) {
                bindings.push((self.effect_id(&inner), h));
            }
        }
        if bindings.is_empty() {
            return ev;
        }
        Expr::Provide {
            base: match ev {
                Expr::Lit(Lit::Unit) => None,
                ev => Some(Box::new(ev)),
            },
            bindings,
        }
    }

    /// Whether it is the bundled core's `State.run` (spec 8.8).
    fn is_state_run(&self, def: DefId) -> bool {
        let data = &self.hir.defs[def];
        let module = match &data.module {
            DefModule::Source(m) => self.modules[*m].name.to_string(),
            _ => return false,
        };
        crate::intrinsics::is_runtime_handler(&module, &data.name)
    }

    /// Prepares lowering a `State.run` call. `callee` is the expression referencing `State.run`.
    fn state_run(&mut self, callee: ExprId, span: Span) -> Result<StateRun, Expr> {
        let Ty::Fn(f) = self.expr_ty(callee).clone() else {
            return Err(self.unsupported("State.run", span));
        };
        let Some(Ty::Fn(body)) = f.params.get(1) else {
            return Err(self.unsupported("State.run", span));
        };
        // The State[S] in body's R: the State effect of the same module as State.run.
        let module = &self.hir.defs[self.hir_def_of(callee)].module;
        let Some(tag) = body
            .uses
            .tags()
            .find(|t| {
                let d = &self.hir.defs[t.def];
                d.kind == DefKind::Effect && d.name == "State" && d.module == *module
            })
            .cloned()
        else {
            return Err(self.unsupported("State.run", span));
        };
        let effect = self.effect_id(&self.key_here(&tag));
        let index = |name: &str| {
            self.module.effects[effect]
                .ops
                .iter()
                .position(|o| o.name == name)
        };
        let (Some(get), Some(set), Some(update)) = (index("get"), index("set"), index("update"))
        else {
            return Err(self.unsupported("State.run", span));
        };
        let call = Call {
            fails: !body.fails.is_empty(),
            suspends: self.row_suspends(&body.uses),
        };
        let base = self.ev.map(Expr::Var);
        // `State.run(initial: S, body: fn() -> A use { State[S] }) -> (A, S)` (spec 8.8).
        let state = match f.params.first() {
            Some(s) => self.ty(s),
            None => TypeId::UNIT,
        };
        let result = self.ty(&body.ret);
        let update_ty = self.module.func(FnType {
            params: vec![state],
            ret: state,
            fails: false,
            suspends: false,
            ev: false,
            sets: None,
        });
        let cell_ty = self.module.cell(state);
        let handler_ty = self.module.handler(effect);
        let op_tys = [
            self.module.fn_type(Vec::new(), state),
            self.module.fn_type(vec![state], TypeId::UNIT),
            self.module.fn_type(vec![update_ty], TypeId::UNIT),
        ];
        Ok(StateRun {
            effect,
            ops: [get, set, update],
            base,
            call,
            state,
            op_tys,
            cell: self.module.local("cell", cell_ty),
            handler: self.module.local("state", handler_ty),
            value: self.module.local("value", state),
            f: self.module.local("f", update_ty),
            result: self.module.local("result", result),
        })
    }

    /// The definition of a referencing expression.
    fn hir_def_of(&self, expr: ExprId) -> DefId {
        match self.hir.exprs[expr].kind {
            ExprKind::Path(Res::Def(def)) => def,
            _ => unreachable!("a definition reference"),
        }
    }

    /// The mark for a direct call of `f`. Whether it suspends matches the function's mark
    /// (`Expr::Call`).
    fn direct_call(&self, f: FnId, call: Call) -> Call {
        Call {
            suspends: self.module.functions[f].suspends,
            ..call
        }
    }

    /// Whether the callee may fail: true if the E of the callee expression's function type (as
    /// solved by the type checker) is not empty. Opaque sets, such as set type parameters and
    /// generalized set variables, are not empty, so they count as true. Builtins and constructors
    /// have an empty E.
    fn callee_fails(&self, callee: ExprId) -> bool {
        matches!(self.expr_ty(callee), Ty::Fn(f) if !f.fails.is_empty())
    }

    fn binary(&mut self, op: BinaryOp, lhs: ExprId, rhs: ExprId, span: Span) -> Expr {
        let operand = self.op_ty(lhs);
        let (op, ty) = match op {
            BinaryOp::Add => (BinOp::Add, operand),
            BinaryOp::Sub => (BinOp::Sub, operand),
            BinaryOp::Mul => (BinOp::Mul, operand),
            BinaryOp::Div => (BinOp::Div, operand),
            BinaryOp::Rem => (BinOp::Rem, operand),
            // Equality of non-primitive values compares structure.
            BinaryOp::Eq => (BinOp::Eq, operand.or(Some(OpTy::Structural))),
            BinaryOp::Ne => (BinOp::Ne, operand.or(Some(OpTy::Structural))),
            // Ordering (Ord) of non-primitive values waits for traits.
            BinaryOp::Lt => (BinOp::Lt, operand),
            BinaryOp::Le => (BinOp::Le, operand),
            BinaryOp::Gt => (BinOp::Gt, operand),
            BinaryOp::Ge => (BinOp::Ge, operand),
            BinaryOp::And => (BinOp::And, Some(OpTy::Bool)),
            BinaryOp::Or => (BinOp::Or, Some(OpTy::Bool)),
            BinaryOp::Pipe => unreachable!("a pipe is lowered as a call"),
        };
        let Some(ty) = ty else {
            return self.unsupported("this operator on this type", span);
        };
        Expr::Binary {
            op,
            ty,
            lhs: Box::new(self.expr(lhs)),
            rhs: Box::new(self.expr(rhs)),
        }
    }

    /// `None` unless it is a primitive type.
    fn op_ty(&self, operand: ExprId) -> Option<OpTy> {
        prim_op_ty(self.expr_ty(operand))
    }

    /// `assert expr` (spec 13.2). A comparison becomes the form that shows both sides
    /// (`Assert::compare`). If the sides' type cannot be shown (it contains type parameters or
    /// functions), it falls back to the form that shows no values (`Assert::bool`).
    fn assert(&mut self, cond: ExprId) -> Expr {
        let hir = self.hir;
        let c = &hir.exprs[cond];
        let text = self.source_text(c.span);
        if let ExprKind::Binary { op, lhs, rhs } = &c.kind
            && let Some(bin) = comparison(*op)
        {
            let ty = self.ty(self.expr_ty(*lhs));
            let op_ty = match (self.op_ty(*lhs), bin) {
                (Some(t), _) => Some(t),
                (None, BinOp::Eq | BinOp::Ne) => Some(OpTy::Structural),
                (None, _) => None,
            };
            if let Some(op_ty) = op_ty
                && unshowable(&self.module, ty).is_none()
            {
                let (lhs, rhs) = (self.expr(*lhs), self.expr(*rhs));
                return Assert::compare(bin, op_ty, ty, lhs, rhs, text);
            }
        }
        let cond = self.expr(cond);
        Assert::bool(cond, text)
    }

    /// The source text; empty for positions without a source.
    ///
    /// The return value borrows the source (`'a`), not `self`, so `self.expr(..)` (`&mut self`) can
    /// be called while holding the text.
    fn source_text(&self, span: Span) -> &'a str {
        let range = usize::from(span.range.start())..usize::from(span.range.end());
        let sources: &'a ArenaMap<ModuleId, &'a str> = self.sources;
        sources
            .get(span.module)
            .and_then(|src| src.get(range))
            .unwrap_or_default()
    }

    /// A field access. Only single-constructor types (type, error) have fields, so the constructor
    /// is that type's only one. The index was looked up by the type checker
    /// (`ProgramTypes::fields`).
    fn field(&mut self, id: ExprId, base: ExprId, span: Span) -> Expr {
        // Missing from the table means a capability operation (`log.info`) used as a value.
        let Some(&index) = self.types.fields.get(id) else {
            return match self.types.ops.get(id) {
                Some(&op) => self.op_value(id, base, op, span),
                None => self.unsupported("effect operation", span),
            };
        };
        let Ty::Named(con, _) = self.expr_ty(base) else {
            unreachable!("a value with a field has a named type");
        };
        let ctor = CtorRef {
            enum_id: self.enums[self.types.ty_table.def(*con)],
            variant: 0,
        };
        Expr::Field {
            base: Box::new(self.expr(base)),
            ctor,
            index,
        }
    }

    /// A record update `C(f: e, ..base)` (spec 5.3). Written fields are evaluated in written order,
    /// and `..base` last (spec 14.1). Unwritten fields are taken from `base`.
    fn update(
        &mut self,
        id: ExprId,
        ctor: ExprId,
        fields: &[Arg],
        base: ExprId,
        span: Span,
    ) -> Expr {
        let ExprKind::Path(Res::Def(def)) = self.hir.exprs[ctor].kind else {
            unreachable!("the constructor of a record update is a constructor reference");
        };
        // The constructor table has every constructor (including stateful handlers').
        let Some(&ctor) = self.ctors.get(def) else {
            return self.unsupported("record update of this constructor", span);
        };
        let slots = &self.types.calls[id];
        let mut args: Vec<Option<Expr>> = vec![None; slots.len()];
        let mut lets = Vec::new();
        for field in fields {
            let pos = slots
                .iter()
                .position(|s| *s == Some(field.value))
                .expect("the type checker placed each field at a position");
            let value = self.expr(field.value);
            args[pos] = Some(if is_atomic(&value) {
                value
            } else {
                let ty = self.expr_ty(field.value);
                let ty = self.ty(ty);
                let l = self.module.local("arg", ty);
                lets.push((l, value));
                Expr::Var(l)
            });
        }
        let base_ty = self.expr_ty(base);
        let base_ty = self.ty(base_ty);
        let b = self.module.local("base", base_ty);
        let base = self.expr(base);
        lets.push((b, base));
        let args = args
            .into_iter()
            .enumerate()
            .map(|(index, arg)| {
                arg.unwrap_or_else(|| Expr::Field {
                    base: Box::new(Expr::Var(b)),
                    ctor,
                    index,
                })
            })
            .collect();
        // The update has the constructed type, whose arguments are the enum's.
        let ty_args = match self.expr_ty(id) {
            Ty::Named(_, tys) => tys.iter().map(|t| self.ty(t)).collect(),
            _ => Vec::new(),
        };
        let value = Expr::Ctor {
            ctor,
            ty_args,
            args,
        };
        lets.into_iter()
            .rev()
            .fold(value, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }

    /// String interpolation. Embedded values carry their types (backends build show functions from
    /// them).
    fn interpolated(&mut self, parts: &[HirStrPart]) -> Expr {
        let parts = parts
            .iter()
            .map(|part| match part {
                HirStrPart::Text(s) => StrPart::Lit(s.clone()),
                HirStrPart::Interp(e) => {
                    let ty = self.ty(self.expr_ty(*e));
                    if let Some(what) = unshowable(&self.module, ty) {
                        self.report(what, self.hir.exprs[*e].span);
                    }
                    StrPart::Value(self.expr(*e), ty)
                }
            })
            .collect();
        Expr::Concat(parts)
    }

    /// A block: statements become nested `let`s, and statements whose value is unused are bound to
    /// a discard variable. Bindings with patterns other than a variable or `_` (`(a, b) = pair`)
    /// become a one-arm match with the remaining statements in its body. The type checker has
    /// checked that binding patterns always succeed (E0317).
    fn block(&mut self, block: &Block) -> Expr {
        let hir = self.hir;
        let mut binds = Vec::new();
        for stmt in &block.stmts {
            let (pat, value) = match stmt {
                Stmt::Let { pat, value, .. } => (Some(*pat), *value),
                Stmt::Expr(e) => (None, *e),
            };
            let value_ty = self.expr_ty(value);
            let value_ty = self.ty(value_ty);
            let value = self.expr(value);
            let bind = match pat.map(|p| (p, &hir.pats[p].kind)) {
                None | Some((_, PatKind::Wild)) => {
                    Bind::Var(self.module.local("_", value_ty), value)
                }
                Some((_, PatKind::Bind(l))) => Bind::Var(self.local(*l), value),
                Some((p, _)) => {
                    let pat = self.pat(p);
                    // Bindings that always succeed (E0317) never have const patterns.
                    if !std::mem::take(&mut self.pat_guards).is_empty() {
                        self.report("const pattern in binding", hir.pats[p].span);
                    }
                    Bind::Pat(pat, value)
                }
            };
            binds.push(bind);
        }
        let tail = match block.tail {
            Some(e) => self.expr(e),
            None => Expr::Lit(Lit::Unit),
        };
        binds.into_iter().rev().fold(tail, |body, bind| match bind {
            Bind::Var(var, value) => Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            },
            Bind::Pat(pat, value) => Expr::Match {
                scrutinee: Box::new(value),
                arms: vec![Arm {
                    pat,
                    guard: None,
                    body,
                }],
            },
        })
    }

    /// A match arm. Comparisons for const patterns become a guard evaluated before the written
    /// guard.
    fn arm(&mut self, arm: &HirArm) -> Arm {
        let pat = self.pat(arm.pat);
        let mut guards = std::mem::take(&mut self.pat_guards);
        if let Some(g) = arm.guard {
            guards.push(self.expr(g));
        }
        Arm {
            pat,
            guard: guards.into_iter().reduce(|a, b| Expr::Binary {
                op: BinOp::And,
                ty: OpTy::Bool,
                lhs: Box::new(a),
                rhs: Box::new(b),
            }),
            body: self.expr(arm.body),
        }
    }

    /// Const patterns push their comparisons onto `pat_guards` (the caller turns them into a
    /// guard).
    fn pat(&mut self, id: PatId) -> Pat {
        let hir = self.hir;
        let p = &hir.pats[id];
        match &p.kind {
            PatKind::Wild => Pat::Wild,
            PatKind::Bind(l) => Pat::Bind(self.local(*l)),
            PatKind::Lit(lit) => Pat::Lit(self.lit_value(&self.types.pats[id], lit)),
            PatKind::Unit => Pat::Lit(Lit::Unit),
            PatKind::Tuple(ps) => Pat::Tuple(ps.iter().map(|&p| self.pat(p)).collect()),
            PatKind::List { elems, rest } => Pat::List {
                elems: elems.iter().map(|&p| self.pat(p)).collect(),
                rest: rest.map(|r| {
                    Box::new(match r.binding {
                        Some(l) => Pat::Bind(self.local(l)),
                        None => Pat::Wild,
                    })
                }),
            },
            PatKind::Ctor {
                res: Res::Def(def), ..
            } => {
                if Some(*def) == self.true_def {
                    return Pat::Lit(Lit::Bool(true));
                }
                if Some(*def) == self.false_def {
                    return Pat::Lit(Lit::Bool(false));
                }
                let Some(&ctor) = self.ctors.get(*def) else {
                    self.report("this constructor pattern", p.span);
                    return Pat::Wild;
                };
                // The type checker reordered fields by position; omitted fields are `None`.
                // Constructor patterns without parentheses (`Empty`) are not in the table.
                let n = self.module.variant(ctor).fields.len();
                let fields = match self.types.pat_fields.get(id) {
                    Some(fs) => fs
                        .iter()
                        .map(|f| match f {
                            Some(f) => self.pat(*f),
                            None => Pat::Wild,
                        })
                        .collect(),
                    None => vec![Pat::Wild; n],
                };
                Pat::Ctor { ctor, fields }
            }
            // A const pattern binds a fresh variable and compares it with the const's value in a
            // guard.
            PatKind::Const(Res::Def(def)) => {
                let types = self.types;
                let x_ty = self.ty(&types.pats[id]);
                let x = self.module.local("c", x_ty);
                let ty = prim_op_ty(&types.pats[id]).unwrap_or(OpTy::Structural);
                let value = Expr::Call {
                    callee: Callee::Direct {
                        f: self.fn_id(*def, 0),
                        ty_args: self.const_ty_args(*def, &types.pats[id]),
                    },
                    args: Vec::new(),
                    fails: false,
                    suspends: false,
                };
                self.pat_guards.push(Expr::Binary {
                    op: BinOp::Eq,
                    ty,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(value),
                });
                Pat::Bind(x)
            }
            PatKind::Ctor { .. } | PatKind::Const(_) | PatKind::Missing => {
                self.report("this pattern", p.span);
                Pat::Wild
            }
        }
    }

    /// The type arguments of the const `def` compared with a value of type `ty` (a const pattern
    /// has no reference expression with recorded type arguments): read off by matching the
    /// const's declared type against `ty`.
    fn const_ty_args(&mut self, def: DefId, ty: &Ty) -> Vec<TypeId> {
        let types = self.types;
        let Some(sig) = types.sigs.get(def) else {
            return Vec::new();
        };
        let mut found: Vec<Option<&Ty>> = vec![None; sig.scheme.params.len()];
        match_bound(&sig.scheme.ty, ty, &mut found);
        found
            .into_iter()
            .map(|t| t.map_or(TypeId::UNIT, |t| self.ty(t)))
            .collect()
    }

    fn declare_enum(&mut self, def: DefId, params: usize) -> EnumId {
        let name = self.ir_name(def);
        self.module.add_enum(EnumDef {
            name,
            params,
            variants: Vec::new(),
            error: false,
            external: false,
        })
    }

    /// The name in the IR: definitions of the entry module keep their names; others get the
    /// module name (`List.map`).
    fn ir_name(&self, def: DefId) -> String {
        let data = &self.hir.defs[def];
        match &data.module {
            DefModule::Source(m) if *m == self.entry => data.name.to_string(),
            DefModule::Source(m) => format!("{}.{}", self.modules[*m].name, data.name),
            DefModule::Prelude => data.name.to_string(),
            DefModule::Builtin(m) => format!("{m}.{}", data.name),
        }
    }

    /// Whether to export in JS: the entry module's `pub` fns and `main` (called by the entry
    /// module). Test functions are excluded (for `emela test`, the test runner exports them).
    fn is_exported(&self, def: DefId) -> bool {
        let data = &self.hir.defs[def];
        data.kind == DefKind::Fn
            && data.module == DefModule::Source(self.entry)
            && (data.is_pub || data.name == "main")
            && !self.tests.contains(&def)
    }
}

/// Collapses `.` and `..` in a path textually (symbolic links are not resolved).
fn normalize(path: &Path) -> String {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out.to_string_lossy().into_owned()
}

/// Turns a specifier relative to `lib/` (`./a/../b.mjs`) into a path from `lib/` with `.` and `..`
/// collapsed (`b.mjs`). `None` if it leaves `lib/`.
fn lib_relative(spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Whether the handler is stateful (has fields, init or release; spec 8.4, 8.6).
fn is_stateful(h: &emela_resolve::hir::HandlerItem) -> bool {
    !h.fields.is_empty() || h.init.is_some() || h.release.is_some()
}

/// A direct call to the non-suspending, non-generic function `f`.
fn direct(f: FnId, args: Vec<Expr>, fails: bool) -> Expr {
    Expr::Call {
        callee: Callee::Direct {
            f,
            ty_args: Vec::new(),
        },
        args,
        fails,
        suspends: false,
    }
}

/// A direct call to the non-generic function `f`, marked as suspending if the function is.
fn direct_marked(m: &Module, f: FnId, args: Vec<Expr>, fails: bool) -> Expr {
    Expr::Call {
        callee: Callee::Direct {
            f,
            ty_args: Vec::new(),
        },
        args,
        fails,
        suspends: m.functions[f].suspends,
    }
}

/// Whether the expression refers to local `l`.
fn mentions(e: &Expr, l: Local) -> bool {
    if let Expr::Var(v) = e {
        return *v == l;
    }
    let mut found = false;
    e.for_each_child(&mut |c| found = found || mentions(c, l));
    found
}

/// Pushes the top-level functions the expression refers to (as values and direct calls) onto
/// `out`.
/// External functions without an `@external` for `target` that are reachable from `entries`
/// (spec 12.1). A handler operation's implementation (a lambda in `Expr::Handler`) is followed only
/// once the operation is called or used as a value somewhere reachable; operations of effects with
/// different type arguments count as the same operation of the same declaration.
fn unannotated_reachable(m: &Module, entries: &[FnId], target: &str) -> Vec<FnId> {
    #[derive(Default)]
    struct Walk<'m> {
        fns: Vec<FnId>,
        /// (effect declaration, operation index) called or used as a value.
        used: FxHashSet<(&'m str, usize)>,
        /// Implementations not followed yet, by (effect declaration, operation index).
        pending: FxHashMap<(&'m str, usize), Vec<&'m Expr>>,
        /// Bodies to walk: implementations whose operation became used.
        ready: Vec<&'m Expr>,
    }
    impl<'m> Walk<'m> {
        fn expr(&mut self, m: &'m Module, e: &'m Expr) {
            match e {
                Expr::Fn { f, .. }
                | Expr::Call {
                    callee: Callee::Direct { f, .. },
                    ..
                } => self.fns.push(*f),
                Expr::Call {
                    callee: Callee::Op { effect, op, .. },
                    ..
                } => {
                    let key = (m.effects[*effect].decl.as_str(), *op);
                    if self.used.insert(key)
                        && let Some(bodies) = self.pending.remove(&key)
                    {
                        self.ready.extend(bodies);
                    }
                }
                Expr::Handler { effect, ops, .. } => {
                    let decl = m.effects[*effect].decl.as_str();
                    for (i, op) in ops.iter().enumerate() {
                        if self.used.contains(&(decl, i)) {
                            self.ready.push(op);
                        } else {
                            self.pending.entry((decl, i)).or_default().push(op);
                        }
                    }
                    return;
                }
                _ => {}
            }
            e.for_each_child(&mut |c| self.expr(m, c));
        }
    }

    let mut seen = vec![false; m.functions.len()];
    let mut walk = Walk {
        fns: entries.to_vec(),
        ..Walk::default()
    };
    let mut missing = Vec::new();
    loop {
        if let Some(body) = walk.ready.pop() {
            walk.expr(m, body);
            continue;
        }
        let Some(f) = walk.fns.pop() else { break };
        let i = f.into_raw().into_u32() as usize;
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let func = &m.functions[f];
        if let Some(ext) = &func.external {
            if ext.binding(target).is_none() {
                missing.push(f);
            }
            continue;
        }
        walk.expr(m, &func.body);
    }
    missing
}

/// `None` unless it is a primitive type.
fn prim_op_ty(ty: &Ty) -> Option<OpTy> {
    match ty {
        Ty::Prim(Prim::Int) => Some(OpTy::Int),
        Ty::Prim(Prim::Int64) => Some(OpTy::Int64),
        Ty::Prim(Prim::Float) => Some(OpTy::Float),
        Ty::Prim(Prim::Bool) => Some(OpTy::Bool),
        Ty::Prim(Prim::String) => Some(OpTy::String),
        _ => None,
    }
}

fn comparison(op: BinaryOp) -> Option<BinOp> {
    Some(match op {
        BinaryOp::Eq => BinOp::Eq,
        BinaryOp::Ne => BinOp::Ne,
        BinaryOp::Lt => BinOp::Lt,
        BinaryOp::Le => BinOp::Le,
        BinaryOp::Gt => BinOp::Gt,
        BinaryOp::Ge => BinOp::Ge,
        _ => return None,
    })
}

/// Evaluating it has no effect, so reordering it is unobservable.
fn is_atomic(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } | Expr::Lambda { .. }
    )
}

/// Why the type cannot be shown in interpolation, if it cannot. Type parameter values need Show
/// dictionary passing or monomorphization (traits come in 0.21). Functions cannot be shown
/// (spec 10.6), but the Show check comes with traits.
fn unshowable(m: &Module, ty: TypeId) -> Option<&'static str> {
    match m.ty(ty) {
        Type::Param(_) => Some("showing a value of a type parameter"),
        Type::Fn(_) => Some("showing a function"),
        Type::Handler(_) => Some("showing a capability value"),
        Type::Tuple(ts) | Type::Enum(_, ts) => ts.iter().find_map(|&t| unshowable(m, t)),
        Type::List(t) => unshowable(m, *t),
        _ => None,
    }
}

/// Records in `out[i]` what `Ty::Bound(i)` in `pattern` corresponds to in `actual`.
fn match_bound<'t>(pattern: &Ty, actual: &'t Ty, out: &mut [Option<&'t Ty>]) {
    let all = |xs: &[Ty], ys: &'t [Ty], out: &mut [Option<&'t Ty>]| {
        for (x, y) in xs.iter().zip(ys) {
            match_bound(x, y, out);
        }
    };
    match (pattern, actual) {
        (Ty::Bound(i), _) => {
            if let Some(slot) = out.get_mut(*i as usize) {
                slot.get_or_insert(actual);
            }
        }
        (Ty::List(x), Ty::List(y)) => match_bound(x, y, out),
        (Ty::Tuple(xs), Ty::Tuple(ys)) | (Ty::Named(_, xs), Ty::Named(_, ys)) => all(xs, ys, out),
        (Ty::Fn(f), Ty::Fn(g)) => {
            all(&f.params, &g.params, out);
            match_bound(&f.ret, &g.ret, out);
        }
        _ => {}
    }
}

/// Whether the type contains type parameters (`Ty::Bound`).
fn has_bound(ty: &Ty) -> bool {
    let mut found = matches!(ty, Ty::Bound(_));
    ty.for_each_child(|c| found |= has_bound(c));
    found
}
