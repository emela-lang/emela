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
//!
//! # Lowering policy
//!
//! What needs analysis goes to `opt`; what the syntax alone tells us goes to the module for that
//! syntax. This conversion knows the syntax: it does not look inside a callee, analyse effects or
//! use counts, or rewrite a pattern by the shape of its arms. Those are optimizations, and they do
//! not belong here.
//!
//! The choices made for the performance of the code it produces are few, each in the module named
//! after the syntax that needs it, documented and unit-tested where it is made:
//!
//! | Choice | Where | Why |
//! |---|---|---|
//! | Adjacent texts merged, empty strings dropped | [`concat`] | the structure of interpolation |
//! | `append` associated to the right (#606) | [`concat`] | a growing string is copied once per later piece, not once per piece |
//! | Pattern decision trees, reads cached per path | [`pat`] | no arm body is converted twice |
//! | Show and equality generated per type | [`derive`] | structural display and equality |
//! | Values of `()` and `Never` dropped | [`expr`] | the lowering of types without representation |
//! | Self tail calls as loops | `emela_core::tail` | target-independent normalization above this conversion |
//!
//! The shape this conversion produces is fixed by snapshots: an optimization must not change it.
//!
//! # Modules
//!
//! - [`expr`]: the traversal of expressions and the statements for each kind.
//! - [`value`]: values, their types, calls, closures and concurrency operations.
//! - [`control`]: sequences, `if` / loops / `protect`, failures and releases.
//! - [`concat`], [`pat`], [`derive`], [`captures`]: the lowerings of one syntax each.

mod captures;
mod concat;
mod control;
mod derive;
mod expr;
mod pat;
mod value;

use emela_core::{Expr, Lit, Local, TypeId};
use la_arena::Arena;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dump::core_ty;
use crate::ir::{FnId, Function, LabelId, Module, Operand, StmtId, Term, VarId};

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
    // Every error enum that the functions construct gets its show function, for a failure
    // reaching the entry (spec 14.3): errors of different enums arrive there, so the backend
    // picks the function by the value's enum at run time. An error whose type satisfies Show is
    // shown by it (`Module::error_display`: the structure, or the impl's function); any other
    // by its name only.
    let mut error_shows = Vec::new();
    let mut made = FxHashSet::default();
    for body in &bodies {
        constructed(body, &mut made);
    }
    let mut error_enums: Vec<emela_core::EnumId> = made
        .into_iter()
        .filter(|&id| {
            let def = &sh.core.enums[id];
            def.error && def.params == 0
        })
        .collect();
    error_enums.sort_by_key(|&id| ix(id));
    for id in error_enums {
        let ty = sh.core.enum_type(id, Vec::new());
        let f = match sh.core.error_display.get(&id) {
            Some(emela_core::ErrorDisplay::Structure) => sh.derived(Derive::Show, ty),
            Some(&emela_core::ErrorDisplay::Impl(f)) => sh.fn_map[ix(f)],
            None => sh.derived(Derive::ErrorName, ty),
        };
        error_shows.push((id, f));
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
        error_shows,
    })
}

/// The enums whose constructors `e` uses.
fn constructed(e: &Expr, out: &mut FxHashSet<emela_core::EnumId>) {
    if let Expr::Ctor { ctor, .. } = e {
        out.insert(ctor.enum_id);
    }
    e.for_each_child(&mut |c| constructed(c, out));
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
    /// `name$E(x: E) -> String`: the name of an error that does not satisfy Show, shown in place
    /// of its value at the entry (`[dyn.error]`).
    ErrorName,
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
            Derive::ErrorName => "name",
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
}
