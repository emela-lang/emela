//! Builds the Core IR from the type checker's results (HIR and `ProgramTypes`): lowering.
//!
//! The whole program (user modules, Prelude, core) becomes one IR [`Module`]. All enums are
//! allocated first ([`Lowerer::declare_enums`]); only fns reachable from the roots are lowered.
//! A fn gets an ID and is pushed on the worklist when first referenced ([`Lowerer::fn_id`]), and
//! bodies are lowered until the worklist is empty.
//!
//! Lowering lives in emela-core rather than its own crate: emela-core already depends on
//! emela-types for the builtin signatures, so a separate crate would make no dependency lighter.
//!
//! Since unreachable fns are not lowered, a construct lowering does not support yet
//! ([`LowerError::Unsupported`]) is reported only once a reachable fn uses it. Type errors are not
//! affected (the type checker checks every fn). The test `every_core_function_lowers`
//! ([`Roots::All`]) makes sure all of core and std lowers, used or not.
//!
//! The methods of [`Lowerer`] are split over the submodules by construct (`types`, `traits`,
//! `handler`, `with`, `expr`, `call`, `host` and the others). The methods that the submodules
//! all call (`expr`, `expr_ty`, `ty`, `effect_id`, the evidence and the handler functions) stay
//! here, so a submodule calls back into this module but the submodules do not call each other in
//! a circle.

use std::path::Path;

use emela_resolve::hir::{
    BinaryOp, DefId, DefKind, DefModule, ExprId, Item, LocalId, Program, Span,
};
use emela_resolve::{ModuleId, ModuleMap};
use emela_types::{Prim, ProgramTypes, Row, RowElem, Tag, Ty};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;

use crate::intrinsics::Builtin;
use crate::ir::{
    BinOp, BundledFile, Callee, CtorRef, EffectDef, EffectId, EnumId, Expr, FiberOp, FnId,
    HostExport, Lit, Local, Module, OpDef, OpTy, Pat, Type, TypeId,
};
use crate::opt::{CoreFn, CoreFns};

mod builtin_handler;
mod builtin_impls;
mod call;
mod derive_std;
mod expr;
mod fiber;
mod handler;
mod host;
mod local_fn;
mod member;
mod suspend;
mod traits;
mod types;
mod with;

use fiber::{AttemptCall, OnFailure, StartCall, TaskItem};
use local_fn::Lifted;
use suspend::Specialization;

/// Why something that passed type checking could not be lowered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// Not supported in the IR yet. `what` describes what, in English.
    Unsupported { what: String, span: Option<Span> },
    /// An external function reachable from the entry has no `@external` for the language of the
    /// build target (`[ext.decl]`). `span` is the position of the function name.
    MissingExternal {
        name: String,
        language: String,
        span: Option<Span>,
    },
    /// The file named by `@external(js, "./x.mjs", …)` in bundled core is not among the bundled
    /// host files (`LowerInput::bundled_files`). An error in the bundled sources. `path` is the
    /// specifier as written.
    MissingCoreFile { path: String, span: Option<Span> },
    /// An exported fn that may suspend, on the native target (E5202, `[target.native.export]`). `name` is
    /// the fn, or the fn with `@entry` for an export made by `@entry`. `span` is the `@export` or
    /// the `@entry`.
    SuspendingExport { name: String, span: Option<Span> },
    /// An `@export(wasm, ...)` written by a user, on the `wasm` target (`[target.wasm.external]`).
    /// `span` is the `@export` or the `@entry`.
    WasmExport { name: String, span: Option<Span> },
    /// `@entry(Http.serve)` on the native target (E5203, `[target.native.unsupported]`). `span` is the
    /// `@entry`.
    HttpServiceOnC { span: Option<Span> },
}

/// Which fns lowering starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roots {
    /// The entry module's `main` and the fns the program exposes to the host (`[ext.export]`)
    /// for the build target. The normal build.
    Exported,
    /// The entry module's `main` and its `pub` fns, except test functions and fns with a
    /// trait-bounded type parameter (no impl could be chosen for a caller that sees every type
    /// the same). Available to external backends that select public entry points instead of
    /// the host exports of a built-in target.
    Public,
    /// The test functions (`LowerInput::tests`), for `emela test`.
    Tests,
    /// Every fn and const with a body, every `@external` fn and every handler, including unused
    /// ones. For lowering tests.
    All,
}

#[derive(Debug)]
pub struct Lowered {
    pub module: Module,
    /// The IR function for each test function in `LowerInput::tests`, in the same order. Filled
    /// only with `Roots::Tests`.
    pub tests: Vec<FnId>,
    /// What the Core IR optimisation passes changed (`opt::run`), and what the δ-only stage after
    /// monomorphisation changed (`opt::delta_after_monomorphization`). The two stages share
    /// [`crate::opt::Stats::delta`]; [`crate::opt::Stats::passes`] counts only the sweeps of
    /// `opt::run`. No change counter fires with `lower_with(_, _, false)`, or while no pass has
    /// fired.
    pub stats: crate::opt::Stats,
}

pub struct LowerInput<'a> {
    pub hir: &'a Program,
    pub types: &'a ProgramTypes,
    /// Module names, used to prefix IR names.
    pub modules: &'a ModuleMap,
    /// Module sources, used to cut out the expression text shown when an assert fails (spec 13.2).
    pub sources: ArenaMap<ModuleId, &'a str>,
    /// The entry module. Its definitions' IR names get no module prefix. `None` for the tests of
    /// a project with neither `src/main.emel` nor `src/lib.emel`.
    pub entry: Option<ModuleId>,
    /// `@test` fns (spec 13.1), not exported in a normal build. The caller has checked that normal
    /// code does not reference them (E2204), so a normal build never reaches them and drops them.
    pub tests: Vec<DefId>,
    /// Source file paths of modules. Used to rewrite `@external` modules naming a file
    /// (`"./ffi.mjs"`) to the file next to the declaring source (spec 12.1). Modules without a
    /// path are left alone.
    pub paths: ArenaMap<ModuleId, &'a Path>,
    /// Source file paths of modules as diagnostics show them (relative to the project directory
    /// when under it). Used for the location of an assert (`[test.assert]`). An assert in a
    /// module without one has no location.
    pub shown_paths: ArenaMap<ModuleId, String>,
    /// Bundled host files (paths relative to `lib/` and contents; `crate::bundled_files`). File
    /// specifiers of core modules are looked up here instead of next to the source, become
    /// `HostModule::Bundled`, and the files are put in `Module::bundled_files`.
    pub bundled_files: &'a [(&'a str, &'a str)],
    /// The language of the build target (`"js"`, `"wasm"` or `"c"`, `[ext.decl]`): the
    /// `@external` and `@export` annotations of this language take effect. With `Some` and `Roots::Exported` or `Roots::Tests`, the result
    /// is monomorphized ([`crate::monomorphize_traced`]) from the roots (the exported functions, a
    /// generic one instantiated at `Any`; or the test functions, `Lowered::tests` then naming
    /// their copies), and then external functions reachable from the entry (main, or the test
    /// functions) are checked for an `@external` for that language (spec 12.1), on the monomorphized
    /// IR and only when lowering found no other error. check and the LSP do not lower; lowering
    /// tests pick no target, so `None` keeps the polymorphic IR.
    pub language: Option<&'a str>,
}

/// Collects every place that cannot be lowered and returns them all in `Err`. Self tail calls
/// come out as `Expr::Loop` / `Expr::Recur` (`tail::loopify`).
///
/// The Core IR optimisation passes run, `optimize` permitting, after self tail call loopification
/// and before the IR is checked and monomorphized (`opt::run`); all targets get them. After
/// monomorphisation one δ-only stage runs on the monomorphic module
/// (`opt::delta_after_monomorphization`), because a callee that is effect-polymorphic before
/// monomorphisation (such as a higher-order wrapper) is concrete afterwards; see
/// `opt::reduce::delta`. Both stages share `opt::Stats::delta`.
pub fn lower(input: &LowerInput<'_>, roots: Roots) -> Result<Lowered, Vec<LowerError>> {
    lower_with(input, roots, true)
}

/// [`lower`] with the Core IR optimisation passes switched on or off. Lowering itself is the same
/// either way: `optimize` only reaches `opt::run`, so a build with `--no-opt` differs from the
/// default only by what the passes change.
pub fn lower_with(
    input: &LowerInput<'_>,
    roots: Roots,
    optimize: bool,
) -> Result<Lowered, Vec<LowerError>> {
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
        layer_args: FxHashMap::default(),
        layer_arg_work: Vec::new(),
        current_def: None,
        spec: 0,
        enums: ArenaMap::default(),
        ctors: ArenaMap::default(),
        locals: ArenaMap::default(),
        effects: FxHashMap::default(),
        handler_parts: FxHashMap::default(),
        ev: None,
        opened_cell: None,
        self_cap: None,
        with_cells: Vec::new(),
        bound: Vec::new(),
        paths: &input.paths,
        shown_paths: &input.shown_paths,
        bundled_files: input.bundled_files,
        uses_bundled: false,
        errors: Vec::new(),
        prelude_trait_fns: FxHashMap::default(),
        host_exports: Vec::new(),
        roots,
        local_fns: ArenaMap::default(),
        fn_name: String::new(),
        derive_std: derive_std::Lookups::default(),
        core_fns: FxHashMap::default(),
    };
    if roots == Roots::Exported {
        cx.collect_host_exports(input.language);
        cx.module.http_service = emela_types::is_http_service(hir);
    }
    cx.declare_enums();
    cx.define_ctors();
    for (_, items) in hir.modules.iter() {
        for &def in &items.items {
            let is_root = match (roots, hir.item(def)) {
                (Roots::Exported, Some(Item::Fn(_))) => cx.is_main(def),
                (Roots::Public, Some(Item::Fn(_))) => cx.is_public_root(def),
                (Roots::All, Some(Item::Fn(f))) => f.body.is_some() || f.is_external,
                (Roots::All, Some(Item::Const(_))) => true,
                // A built-in handler has no functions (`lower/builtin_handler.rs`).
                (Roots::All, Some(Item::Handler(h))) => !h.is_intrinsic,
                _ => false,
            };
            if is_root {
                cx.fn_id(def, 0);
            }
        }
    }
    for i in 0..cx.host_exports.len() {
        cx.fn_id(cx.host_exports[i].def, 0);
    }
    let tests = match roots {
        Roots::Tests => input.tests.iter().map(|&def| cx.fn_id(def, 0)).collect(),
        _ => Vec::new(),
    };
    let exit_main = match roots {
        Roots::Exported | Roots::Public => cx.exit_code_main(),
        Roots::Tests | Roots::All => None,
    };
    loop {
        if let Some((def, spec)) = cx.worklist.pop() {
            cx.define_fn(def, spec);
        } else if let Some((layer, arg)) = cx.layer_arg_work.pop() {
            cx.define_layer_arg(layer, arg);
        } else {
            break;
        }
    }
    if let Some(exit_main) = exit_main {
        cx.define_exit_code_main(exit_main);
    }
    cx.builtin_impls();
    cx.trait_impls();
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
    if !cx.errors.is_empty() {
        return Err(cx.errors);
    }
    // Target-neutral, so done once here: every backend gets self tail calls as loops (16.3).
    crate::tail::loopify(&mut cx.module);
    // The Core IR optimisations also run for every target, before the IR is verified and
    // monomorphized. They preserve evaluation order, effects, failures, `with` releases and drop
    // points.
    let mut stats = crate::opt::run(&mut cx.module, &cx.core_fns, optimize);
    let (Some(target), Roots::Exported | Roots::Public | Roots::Tests) = (input.language, roots)
    else {
        return Ok(Lowered {
            module: cx.module,
            tests,
            stats,
        });
    };
    // Ill-typed IR is a lowering bug, not an error in the program. Checked before
    // monomorphization, which assumes well-typed IR.
    #[cfg(debug_assertions)]
    if let Err(errors) = crate::verify(&cx.module) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!(
            "lowering produced ill-typed IR (a compiler bug):\n{}",
            lines.join("\n")
        );
    }
    let mono_roots = match roots {
        Roots::Tests => tests.clone(),
        _ => crate::mono::exported(&cx.module),
    };
    let mut mono = crate::mono::monomorphize_traced(&cx.module, &mono_roots);
    // Until derive generates impls, the type checker counts a `derive` as an impl
    // (`emela_types::Traits::satisfies`), so a call can reach a type without one.
    for (f, ty) in &mono.missing_impls {
        cx.errors.push(LowerError::Unsupported {
            what: format!("`{f}` for `{ty}`, which has no impl"),
            span: None,
        });
    }
    // Every root of a normal build (main and the exported fns) is an entry (spec 12.1).
    let entries: Vec<FnId> = mono.roots.clone();
    cx.check_target(&mono, &entries, target);
    if !cx.errors.is_empty() {
        return Err(cx.errors);
    }
    // The stage after monomorphisation: δ only, on the monomorphic module, with the identity map
    // rebuilt through `origins`. This is where an effect-polymorphic wrapper (`fn apply(f, x) {
    // f(x) }`) becomes a candidate; δ left it alone before monomorphisation because of its set
    // type parameters. The rule keeps its budget and counter; see `opt::reduce::delta`.
    if optimize {
        let core_fns = post_mono_core_fns(&mono.origins, &cx.core_fns);
        stats.merge(crate::opt::delta_after_monomorphization(
            &mut mono.module,
            &core_fns,
        ));
    }
    let tests = match roots {
        Roots::Tests => mono.roots,
        _ => Vec::new(),
    };
    Ok(Lowered {
        module: mono.module,
        tests,
        stats,
    })
}

/// The [`CoreFns`] map of the monomorphic module: every output function whose input lowering
/// recorded as a bundled function. Monomorphisation gives each function a new id, so the identity
/// is followed through [`Monomorphized::origins`] (output function → the input function it copies)
/// and lowering's map (input function → [`CoreFn`]). Without the rebuild, δ would treat a
/// monomorphised `List.fold` or `List.reverse` as an ordinary function and copy it into every
/// caller.
fn post_mono_core_fns(origins: &ArenaMap<FnId, FnId>, core_fns: &CoreFns) -> CoreFns {
    origins
        .iter()
        .filter_map(|(new, &old)| core_fns.get(&old).map(|&kind| (new, kind)))
        .collect()
}

struct Lowerer<'a> {
    hir: &'a Program,
    types: &'a ProgramTypes,
    modules: &'a ModuleMap,
    sources: &'a ArenaMap<ModuleId, &'a str>,
    entry: Option<ModuleId>,
    tests: &'a [DefId],
    /// Comparisons collected from const patterns while lowering a pattern (`x == LIMIT()`),
    /// placed at the front of the arm's guard.
    pat_guards: Vec<Expr>,
    /// Prelude's `True` and `False`, which become `Lit::Bool` in the IR.
    true_def: Option<DefId>,
    false_def: Option<DefId>,
    module: Module,
    /// HIR fns and consts, with which suspending copy (`suspend.rs`) → IR functions. A const
    /// becomes a parameterless function. Only [`Lowerer::fn_id`] adds to it, so that every
    /// reference also puts the fn on the worklist.
    fns: FxHashMap<(DefId, Specialization), FnId>,
    /// Fns (and which suspending copy) that have an ID but whose bodies are not lowered yet.
    worklist: Vec<(DefId, Specialization)>,
    /// The arguments of init written in layers → IR functions computing them
    /// ([`Lowerer::layer_arg`]). Only that function adds to it.
    layer_args: FxHashMap<ExprId, FnId>,
    /// Arguments in `layer_args` (with their layer) whose bodies are not lowered yet.
    layer_arg_work: Vec<(DefId, ExprId)>,
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
    /// Inside the init of a handler that opens handler members, the cell that keeps the release of
    /// what it opened, and whether that may suspend (`lower/member.rs`).
    opened_cell: Option<(Local, bool)>,
    /// Inside the operation bodies and the release of a handler: the handler, and the state and
    /// evidence table that the function making its value takes. `self` passed as the capability
    /// of the handler calls that function (`[effect.handler]`).
    self_cap: Option<(DefId, Option<Local>, Option<Local>)>,
    /// The cells of the `with`s inside an init that release the handler members opened through
    /// their handlers ([`emela_types::ProgramTypes::member_open_withs`]).
    with_cells: Vec<(ExprId, (Local, bool))>,
    /// Effects and handler values provided by enclosing `with`s, outermost first. Only `with`s in
    /// the same function: a top-level fn, a handler operation, or the body of a lambda or a local
    /// fn, which starts empty, since the side that calls it provides its R (`[effect.with]`).
    /// `use X` searches from the back (spec 8.3).
    bound: Vec<(EffectKey, Local)>,
    paths: &'a ArenaMap<ModuleId, &'a Path>,
    shown_paths: &'a ArenaMap<ModuleId, String>,
    bundled_files: &'a [(&'a str, &'a str)],
    /// Whether a lowered external function pointed to a bundled host file.
    uses_bundled: bool,
    errors: Vec<LowerError>,
    /// The Prelude's trait functions by trait and name, once looked up.
    prelude_trait_fns: FxHashMap<(String, String), Option<DefId>>,
    /// The fns exposed to the host for the build target, with their names (`[ext.export]`), in
    /// the order of `emela_types::host_exports`. Filled only for `Roots::Exported`.
    host_exports: Vec<Exposed>,
    roots: Roots,
    /// Local fns of the function being lowered → the functions they were lifted to
    /// (`local_fn.rs`). Only [`Lowerer::local_fn`] adds to it.
    local_fns: ArenaMap<LocalId, Lifted>,
    /// The IR name of the function being lowered, which names the local fns lifted from it.
    fn_name: String,
    /// What the derived impls of std traits look up, once (`derive_std.rs`).
    derive_std: derive_std::Lookups,
    /// The bundled core functions a Core IR pass recognises by identity (`opt::CoreFn`), by IR
    /// function. [`Lowerer::fn_id`] records every copy, `$suspend` ones too. Passed to
    /// [`crate::opt::run`]; not stored in the module, which monomorphization rebuilds.
    core_fns: FxHashMap<FnId, CoreFn>,
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
    /// A local fn lifted to a function of the module (`local_fn.rs`): a direct call whose
    /// arguments start with `captures`. `ev` is as in `Direct`.
    Lifted {
        f: FnId,
        ty_args: Vec<TypeId>,
        call: Call,
        captures: Vec<Expr>,
        ev: Option<Expr>,
    },
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
    /// `tasks.start(f)` (`[task.group]`, `[task.background]`).
    Start(Box<StartCall>),
    /// `Exit.attempt(f)`（11.5）．
    Attempt(Box<AttemptCall>),
}

/// What is needed to lower `State.run(initial, body)`. Locals are allocated up front.
///
/// Creates a cell holding the initial value (`$cellNew`), adds a handler value that reads and
/// writes it to the evidence table under the key `State[S]`, calls body, and returns the pair of
/// body's value and the final state. If body fails, the failure propagates as the call's failure
/// and the state is discarded. If body suspends, so does the call; the state stays in the cell, so
/// it carries over when body resumes.
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
        // step. Being a function value it takes the evidence table, which is empty for it.
        let update = set(Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
            args: vec![get(), Expr::Lit(Lit::Unit)],
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
#[derive(Debug, Clone, Copy)]
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
            Target::Lifted {
                f,
                ty_args,
                call,
                captures,
                ev,
            } => Expr::Call {
                callee: Callee::Direct { f, ty_args },
                args: captures.into_iter().chain(args).chain(ev).collect(),
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
            Target::Start(start) => start.apply(args),
            Target::Attempt(attempt) => attempt.apply(args),
        }
    }
}

impl<'a> Lowerer<'a> {
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
        if let Some(kind) = self.core_fn(def) {
            self.core_fns.insert(id, kind);
        }
        self.module.functions[id].suspends = self.fn_suspends(def, spec);
        if spec == 0
            && let Some(exposed) = self.host_exports.iter().find(|e| e.def == def)
        {
            self.module.functions[id].host_exports = exposed.exports.clone();
        }
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
    /// The return type, the number of type parameters and the indices of the set ones.
    fn signature(&mut self, def: DefId) -> (TypeId, usize, Vec<usize>) {
        let types = self.types;
        let Some(sig) = types.sigs.get(def) else {
            return (TypeId::UNIT, 0, Vec::new());
        };
        let params = &sig.scheme.params;
        let sets = (0..params.len())
            .filter(|&i| matches!(params[i].kind, emela_types::ParamKind::Set))
            .collect();
        let ret = match &sig.scheme.ty {
            Ty::Fn(f) => self.ty(&f.ret),
            ty => self.ty(ty),
        };
        (ret, params.len(), sets)
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
        self.local_fns = ArenaMap::default();
        self.fn_name = self.module.functions[id].name.clone();
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
                let (ret, type_params, set_params) = self.signature(def);
                self.module.define_external(id, params, ret, external);
                self.module.functions[id].type_params = type_params;
                self.module.functions[id].set_params = set_params;
                return;
            }
            Some(Item::Fn(f)) => (
                f.sig.params.iter().map(|p| self.local(p.local)).collect(),
                Some(f.body.expect("a fn with a body")),
            ),
            // A const's value is evaluated once, at the first reference (spec 5.6). It is not
            // inlined at its uses: that would bind the locals of its expression more than once.
            Some(Item::Const(c)) => {
                self.module.functions[id].constant = true;
                (Vec::new(), Some(c.value))
            }
            Some(Item::Handler(_)) => {
                let (params, ret, body) = self.handler(def);
                self.module.define(id, params, ret, body);
                self.module.functions[id].type_params = self.handler_type_params(def);
                return;
            }
            // A trait function is generic in `Self`; monomorphization replaces calls of it with
            // calls of the impl functions ([`Lowerer::dispatch`]). With a default implementation,
            // its body is used for impls that do not define it.
            Some(Item::TraitFn(f)) if f.body.is_some() => {
                self.dispatch(def, spec, true);
                (
                    f.sig.params.iter().map(|p| self.local(p.local)).collect(),
                    f.body,
                )
            }
            _ if hir.defs[def].kind == DefKind::TraitFn => {
                self.dispatch(def, spec, false);
                (self.declared_params(def), None)
            }
            _ => match hir.impl_fn(def) {
                Some((_, f)) => (
                    f.params.iter().map(|p| self.local(p.local)).collect(),
                    Some(f.body.expect("an impl function has a body (E1209)")),
                ),
                None => unreachable!(
                    "fn_id got a definition that is not a fn, const, handler, trait function or \
                     impl function"
                ),
            },
        };
        if self.takes_ev(def) {
            let ev = self.ev_local();
            params.push(ev);
            self.ev = Some(ev);
        }
        let body = match body {
            Some(body) => self.expr(body),
            // Without a default implementation the body is never called: monomorphization calls
            // an impl function instead, and reports a type without one.
            None => Expr::Builtin {
                op: Builtin::Panic,
                ty_args: Vec::new(),
                args: vec![Expr::Lit(Lit::String(format!(
                    "no impl of `{}`",
                    self.ir_name(def)
                )))],
            },
        };
        let (ret, type_params, set_params) = self.signature(def);
        self.module.define(id, params, ret, body);
        let fails = self.declared_fails(def);
        let types = self.types;
        let sets = match types.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => self.fn_sets(&f.fails, &f.uses),
            _ => None,
        };
        let func = &mut self.module.functions[id];
        func.type_params = type_params;
        func.set_params = set_params;
        // A const's value cannot fail (spec 5.6).
        func.fails = !func.constant && fails;
        if !func.constant {
            func.sets = sets;
        }
    }

    /// Whether a fn takes the evidence table as its last argument: yes if the R of its declared
    /// (generalized) type has an element other than Async. Opaque sets (`R(f)`, generalized sets)
    /// also need it, to pass it on to function arguments. Async is provided by the runtime
    /// (spec 8.3, 13.3) and has no handler value, so it needs no table.
    fn takes_ev(&self, def: DefId) -> bool {
        matches!(self.types.sigs.get(def).map(|s| &s.scheme.ty), Some(Ty::Fn(f)) if self.row_needs_ev(&f.uses))
    }

    /// Whether to pass the evidence table when calling a value of function type: always, whatever
    /// its R. A function value with a smaller R can be used as one with a larger R, also inside a
    /// list or a tuple (spec 9.4), so every function value takes the table and a value never has
    /// to be rewrapped to change its calling convention (#717). Top-level fns called directly
    /// still take it only when their R needs it ([`Lowerer::takes_ev`]); monomorphization
    /// references one used as a value through an entry that takes it (`Mono::ev_entry`).
    fn ty_takes_ev(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Fn(_))
    }

    /// Whether a function with effect set `row` needs an evidence table: yes if it has an element
    /// other than a host effect or External, which the host provides without evidence
    /// (`[effect.host]`, `[effect.external]`).
    fn row_needs_ev(&self, row: &Row) -> bool {
        row.elems()
            .iter()
            .any(|e| !matches!(e, RowElem::Tag(t) if self.hir.is_provided_by_host(t.def)))
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
            label_of: None,
            display: self.capability_display(def),
        });
        self.effects.insert(key.clone(), id);
        // A label has its base's operations, typed with the base's type arguments
        // (`effect Hits of State[Int]`), and its handler values are the base's (issue #255). The
        // base's arguments are concrete: a label has no type parameters.
        let types = self.types;
        let mut op_args = args;
        let mut owner = def;
        if let Some(base) = types.sigs.label_base(def) {
            let base_key = self.tag_key(base, None);
            let base_id = self.effect_id(&base_key);
            self.module.effects[id].label_of = Some(base_id);
            op_args = self.module.effects[base_id].args.clone();
            owner = base.def;
        }
        let mut ops: Vec<OpDef> = self
            .hir
            .effect_ops(def)
            .to_vec()
            .into_iter()
            .map(|op| self.op_def(owner, op, &op_args))
            .collect();
        // Handler members follow the operations (`lower/member.rs`).
        let members = self.hir.effect_members(def).to_vec();
        if !members.is_empty() {
            let base = types.sigs.base_of(&key.tag).clone();
            for m in members {
                let op = self.member_def(m, &base, key.ctx);
                ops.push(op);
            }
        }
        // The effect of a built-in handler gets `$cell` last (`lower/builtin_handler.rs`).
        let display = self.module.effects[id].display;
        let args = self.module.effects[id].args.clone();
        ops.extend(self.cell_op(display, &args));
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
        if self.is_start_op(effect, op) {
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
                let def = self.types.ty_table.def(*id);
                let name = self.ir_name(def);
                let visible = self
                    .types
                    .ty_table
                    .decl_caps(def)
                    .map_or(args.len(), |c| c.visible);
                let args = &args[..visible];
                if args.is_empty() {
                    name
                } else {
                    format!("{name}[{}]", list(args))
                }
            }
            // E and R of function types are not in the key. An approximation: two `State`s whose
            // type arguments are function types differing only in E or R share a key.
            Ty::Fn(f) => format!("fn({}) -> {}", list(&f.params), self.key_ty(&f.ret, ctx)),
            Ty::Bound(i) => match ctx {
                Some(def) => format!("${}.{i}", self.ir_name(def)),
                None => format!("${i}"),
            },
            Ty::Var(v) => ctx
                .and_then(|def| self.types.generalized_tys.get(def))
                .and_then(|vars| vars.iter().find(|(w, _)| w == v))
                .map_or_else(|| "_".to_owned(), |&(_, i)| self.key_ty(&Ty::Bound(i), ctx)),
            // The effects of a capability type are in the key, so that `State[use Name]` and
            // `State[use Count]` are different effects (issue #705).
            Ty::Cap(tags, _) => {
                let effects: Vec<String> = tags
                    .iter()
                    .map(|t| {
                        let name = self.ir_name(t.def);
                        if t.args.is_empty() {
                            name
                        } else {
                            format!("{name}[{}]", list(&t.args))
                        }
                    })
                    .collect();
                format!("use {}", effects.join(" + "))
            }
            Ty::Set(..) | Ty::Error => "_".to_owned(),
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
        self.hir.effect_ops(effect).iter().position(|&o| o == op)
    }

    /// The type of a stateful handler's state (self, spec 8.4): init's value, else the handler's
    /// constructor value, else `()` (as built by `handler_parts`).
    ///
    /// The type parameters of a handler with them are `Type::Param(i)`, as in its functions
    /// ([`Self::handler_type_params`]).
    fn handler_state_ty(&mut self, def: DefId, h: &emela_resolve::hir::HandlerItem) -> TypeId {
        match (h.init, self.enums.get(def).copied()) {
            (Some(body), _) => {
                let ty = self.expr_ty(body);
                self.ty(ty)
            }
            (None, Some(id)) if h.ctor.is_some() => {
                let args = self.own_ty_args(def);
                self.module.enum_type(id, args)
            }
            _ => TypeId::UNIT,
        }
    }

    /// The number of type parameters of the IR functions of a handler, and of the enum of its
    /// state: its own (`[effect.handler]`), or for an inner handler those of its outer handler,
    /// which it is read with. Monomorphization copies them per item (`[effect.with]`).
    ///
    /// The hidden capability sets of its fields (`[type.rules]`) come after them in its type and
    /// have no IR type, so they are not counted.
    fn handler_type_params(&self, def: DefId) -> usize {
        let table = &self.types.ty_table;
        match table.decl_caps(def) {
            Some(caps) => caps.visible,
            None => table.con(def).map_or(0, |c| table.cons[c].params.len()),
        }
    }

    /// `Type::Param(0..n)` for the type parameters of a handler's functions: the type arguments
    /// with which the outer handler's functions call those of an inner handler.
    pub(super) fn own_ty_args(&mut self, def: DefId) -> Vec<TypeId> {
        (0..self.handler_type_params(def))
            .map(|i| self.module.intern(Type::Param(i)))
            .collect()
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
        // The init and the release of an inner handler of a handler member without `suspend` do
        // not suspend (E4303, `[effect.decl]`).
        let plain = member::implements_plain_member(self.hir, h);
        let suspends = |cx: &Self, body: ExprId| {
            !plain
                && cx
                    .types
                    .effects
                    .get(body)
                    .is_some_and(|r| r.effect_tags().any(|t| !cx.is_immediate_effect(t.def)))
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

    /// The effect among `tags` that declares the operation or handler member `op` (a label's
    /// are its base's).
    fn op_tag(&self, tags: &[Tag], op: DefId) -> Option<Tag> {
        let parent = self.hir.defs[op].parent?;
        tags.iter()
            .find(|t| t.def == parent || self.types.sigs.base_of(t).def == parent)
            .cloned()
    }

    /// The handler of the effect that declares `op` in a capability `value` of `tags`
    /// (`[effect.use]`): `value` itself for one effect, else its element in the tuple of
    /// handlers (`Ty::Cap`).
    pub(super) fn cap_table(
        &mut self,
        value: Expr,
        tags: &[Tag],
        op: DefId,
    ) -> Option<(Expr, Tag)> {
        let tag = self.op_tag(tags, op)?;
        if tags.len() == 1 {
            return Some((value, tag));
        }
        let k = tags.iter().position(|t| *t == tag)?;
        let id = self.effect_id(&self.key_here(&tag));
        let id = self.module.handler_effect(id);
        let ty = self.module.handler(id);
        let table = member::tuple_project(value, k, tags.len(), ty, &mut self.module);
        Some((table, tag))
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

    /// Whether it is a builtin effect (`@intrinsic effect`).
    fn is_intrinsic_effect(&self, effect: DefId) -> bool {
        matches!(self.hir.item(effect), Some(Item::Effect(e)) if e.is_intrinsic)
    }

    /// The type checker has typed every expression.
    fn expr_ty(&self, id: ExprId) -> &'a Ty {
        &self.types.exprs[id]
    }

    fn expr(&mut self, id: ExprId) -> Expr {
        let value = self.expr_kind(id);
        match self.types.narrowings.get(id) {
            Some(wanted) => self.narrow(id, value, &wanted.clone()),
            None => value,
        }
    }

    /// The bundled function `def` is, if a Core IR pass matches it by identity ([`CoreFn`]).
    ///
    /// Only a definition in a bundled module counts (`ModuleMap::is_core` is true for the std
    /// sources too), so a user's `Option.or` never matches. Every IR copy (the `$suspend` ones)
    /// gets the same answer; [`Lowerer::fn_id`] records each.
    fn core_fn(&self, def: DefId) -> Option<CoreFn> {
        let DefModule::Source(module) = self.hir.defs[def].module else {
            return None;
        };
        if !self.modules.is_core(module) {
            return None;
        }
        let module = self.modules[module].name.to_string();
        let name = self.hir.defs[def].name.as_str();
        match (module.as_str(), name) {
            ("Option", "or") => Some(CoreFn::OptionOr),
            ("List", "fold") => Some(CoreFn::ListFold),
            ("List", "map") => Some(CoreFn::ListMap),
            ("List", "filter") => Some(CoreFn::ListFilter),
            ("List", "range") => Some(CoreFn::ListRange),
            ("List", "reverse") => Some(CoreFn::ListReverse),
            // The other collection functions of the core `List` module.
            ("List", _) => Some(CoreFn::Collection),
            // The bundled std functions the array pipeline laws match (D4/PR-031). The std modules
            // are bundled (`is_core` is true for them too: `add_core` adds both the core and the
            // std sources), and their names are reserved, so this is the bundled function and not
            // a user's.
            ("Std.Array", "map") => Some(CoreFn::ArrayMap),
            ("Std.Array", "fold") => Some(CoreFn::ArrayFold),
            _ => None,
        }
    }

    /// The name in the IR: definitions of the entry module keep their names; others get the
    /// module name (`List.map`).
    ///
    /// A trait function is named after its trait (`Show.show`), an impl function after its type
    /// and trait (`User: Show.show`).
    ///
    /// Enums get the prefix too, because backends derive names from them (show functions such as
    /// `$show$User`, the JS names of constructors without fields). In JS `.` becomes `_`, which
    /// cannot clash: module names contain no `_` (`[module.name]`) and function names start with a
    /// lower-case letter (`[lex.name]`).
    fn ir_name(&self, def: DefId) -> String {
        let hir = self.hir;
        let data = &hir.defs[def];
        let name = match data.kind {
            DefKind::TraitFn => match data.parent {
                Some(t) => format!("{}.{}", hir.defs[t].name, data.name),
                None => data.name.to_string(),
            },
            DefKind::ImplFn => {
                let head = data
                    .parent
                    .and_then(|i| self.types.traits.head(i))
                    .map(|h| (h.ty_def, h.trait_def));
                match head {
                    Some((ty, t)) => {
                        format!("{}: {}.{}", hir.defs[ty].name, hir.defs[t].name, data.name)
                    }
                    None => data.name.to_string(),
                }
            }
            // An inner handler is named after its outer handler (`SystemFiles.Open`).
            DefKind::Handler => {
                let mut name = data.name.to_string();
                let mut parent = data.parent;
                while let Some(p) = parent {
                    name = format!("{}.{name}", hir.defs[p].name);
                    parent = hir.defs[p].parent;
                }
                name
            }
            _ => data.name.to_string(),
        };
        match &data.module {
            DefModule::Source(m) if Some(*m) == self.entry => name,
            DefModule::Source(m) => format!("{}.{name}", self.modules[*m].name),
            DefModule::Prelude => name,
            DefModule::Builtin(m) => format!("{m}.{name}"),
        }
    }
}

/// A fn exposed to the host, with its names for the build target ([`Lowerer::host_exports`]).
struct Exposed {
    def: DefId,
    exports: Vec<HostExport>,
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

/// `call`, a direct call made by [`direct`] or [`direct_marked`], with the type arguments
/// `ty_args` of a generic function (the functions of a handler with type parameters,
/// `[effect.with]`).
pub(super) fn generic_call(mut call: Expr, ty_args: &[TypeId]) -> Expr {
    if let Expr::Call {
        callee: Callee::Direct { ty_args: args, .. },
        ..
    } = &mut call
    {
        *args = ty_args.to_vec();
    }
    call
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
