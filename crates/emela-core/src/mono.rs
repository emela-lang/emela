//! Monomorphization: a Core IR → Core IR pass that makes every function monomorphic. Lowering runs
//! it for build, run and test (`LowerInput::target`), so every backend gets monomorphic IR there.
//!
//! Starting from the roots, each function is copied once per instantiation it is used with
//! (worklist-driven, deduplicated by an interned key): the key is the function and its type
//! arguments with the type parameters of the caller already substituted, so it is fully concrete.
//! Type arguments of set type parameters (`R(f)` of `List.map`) are [`SetSummary`]s, so the key
//! includes whether the instantiating sets fail, suspend and need the evidence table. Inside a
//! copy those are known, so function types become exact ([`subst_type`]): a function value is
//! called with the evidence table exactly when it takes one. Polymorphic recursion does not exist
//! (spec 9.3), so the worklist ends.
//!
//! The pass works in two steps per expression, kept apart so that more can be added to each:
//!
//! 1. Types are instantiated ([`Subst`]): `Type::Param` in locals, return types, `ty_args`, lambda
//!    and list types, and the effects of capability types and evidence table keys that mention
//!    type parameters (`State[$modify.0]` becomes `State[Int]`).
//! 2. Call targets are rewritten ([`Mono::target`]): a direct call or a function value with type
//!    arguments becomes the copy for them. A trait function (`Module::dispatch`) becomes the copy
//!    of the impl function for the type its `Self` is instantiated with, or of its own default
//!    implementation, which is thus copied per type (spec 10.3). The function of a derived impl
//!    is generated for the type ([`derive`], spec 10.5).
//!
//! What changes besides types, because flags became exact:
//!
//! - A copy whose own R no longer needs the evidence table drops its evidence parameter, and calls
//!   to it drop the argument. Likewise lambdas and calls of function values.
//! - `fails` of a copy, and the marks of calls to it, come from the instantiated E.
//! - Evidence table keys added for a call to a generic function (`generic_ev` in lowering) belong
//!   to the callee, so they are instantiated with the call's type arguments.
//!
//! Enums are not copied: their definitions stay generic, and their uses carry concrete type
//! arguments (`Type::Enum(id, args)`), from which backends compute field representations. Effects
//! are rebuilt: the output has each effect instance that is used once, keyed like lowering's
//! evidence table keys (the declaration and the type arguments without E and R).
//!
//! The keys, the worklist and the substitution do not depend on the shape of the Core tree, so
//! that the pass can later run on another IR (a flow IR) as well.
//!
//! # Why JS is monomorphized too
//!
//! Traits are then resolved in one place, here, and JS needs no dictionary passing of its own.
//! The C and WASM backends monomorphize the IR they receive again; on monomorphic IR that changes
//! neither names nor shapes (it starts from roots without type parameters and follows only calls
//! without type arguments).
//!
//! Suspending copies from lowering (`name$suspend<bits>`) overlap with what set summaries say, but
//! stay: the bits that pick a copy and the `suspends` flag of a summary come from the same set, so
//! they cannot disagree, and since the key includes the summaries each copy is its own
//! instantiation.
//!
//! # Cost
//!
//! Measured on 2026-10-15 (release builds). On a generated program of 4000 functions, the part
//! reachable from main (1005 functions) took 6 ms; with every function as a root (4139, 35
//! generic, giving 4153) 23 ms, and `verify_monomorphic` 8 ms, against 1.3–1.5 s for type
//! checking, lowering and verification. For `emela build` of generated generic-heavy programs, the
//! JS output grew 4.6–5.1 times (shortening the names still leaves 4.3–4.8: it is the number of
//! copies, as each instantiation of `List.map` also copies `fold`, `reverse` and `reverse_onto`)
//! and the time 1.1–1.3 times; the programs in `examples/` changed by 0.97–1.14 times.

mod debug;
mod derive;

use std::collections::VecDeque;

use la_arena::ArenaMap;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dump::type_name;
use crate::intrinsics::Builtin;
use crate::ir::*;
use crate::typing::Typer;

/// Monomorphizes the module from its exported functions, a generic one instantiated at `Any`
/// ([`monomorphize_traced`]).
pub fn monomorphize(m: &Module) -> Module {
    monomorphize_traced(m, &exported(m)).module
}

/// The exported functions of `m`, the roots of a normal build.
pub(crate) fn exported(m: &Module) -> Vec<FnId> {
    m.functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(id, _)| id)
        .collect()
}

/// Monomorphizes the module from `roots` (the entry, test functions, every function for tests).
/// Returns the new module and, per root, its function in it.
///
/// A generic root that takes no arguments (a test or `main` whose inferred type has type
/// parameters, such as a test that joins a child fiber that only panics) is instantiated with
/// `Never` for its ordinary type parameters and the empty set for its set type parameters (see
/// `root_type_args`); the copy is not exported. A generic root that takes arguments has no sound
/// default instantiation (a caller could pass any type) and gives `None`: the caller reports it.
pub fn monomorphize_from(m: &Module, roots: &[FnId]) -> (Module, Vec<Option<FnId>>) {
    let mut mono = Mono::new(m);
    let out_roots = roots
        .iter()
        .map(|&f| {
            let args = root_type_args(&mut mono.out, m, f)?;
            Some(mono.target(f, &args).f)
        })
        .collect();
    mono.run();
    (mono.out, out_roots)
}

/// What [`monomorphize_traced`] returns.
pub struct Monomorphized {
    pub module: Module,
    /// Per root, its function in `module`.
    pub roots: Vec<FnId>,
    /// Per function of `module`, the function of the input it copies.
    pub origins: ArenaMap<FnId, FnId>,
    /// Trait functions called for a type without an impl, as `Show.show` and the type's name.
    /// The type checker accepts only types with an impl, so this guards against the two
    /// disagreeing: such a call reaches the trait function's body, which panics. A generic root
    /// instantiated at `Any` is an exported fn, whose parameters and return value are boundary
    /// types (E0436), so it gets no value of a type parameter to call a trait function on.
    pub missing_impls: Vec<(String, String)>,
}

/// Monomorphizes the module from `roots` (the exported functions or the test functions; lowering
/// runs it for build, run and test). Also tells which input function each output function copies,
/// so that what is found in the output can be reported at the source (E0412 in lowering).
///
/// Unlike [`monomorphize_from`], a generic root is instantiated whether or not it takes arguments.
/// It has no single instantiation, so it is instantiated for callers that see every value the same
/// way (the host, for an exported fn with a type parameter its boundary types do not use):
/// `Type::Any` for each type parameter, and for each set type parameter a set that fails and needs
/// the evidence table but does not suspend, the flags polymorphic lowering gives such a function.
/// The copy keeps the original name, whether it is exported and its host names.
pub fn monomorphize_traced(m: &Module, roots: &[FnId]) -> Monomorphized {
    let mut mono = Mono::new(m);
    let out_roots = roots.iter().map(|&f| mono.root(f)).collect();
    mono.run();
    Monomorphized {
        module: mono.out,
        roots: out_roots,
        origins: mono.origins,
        missing_impls: mono.missing_impls,
    }
}

/// The type arguments the root `f` is instantiated with, in `out`'s table: `Never` for an ordinary
/// type parameter and the empty set for a set type parameter. `None` for a generic function that
/// takes arguments (other than the evidence table).
///
/// Sound for a root that takes no arguments and that nothing calls (the entry, a test), by
/// parametricity. The body cannot make a value of an ordinary type parameter, and no argument
/// brings one in, so no value of it exists at run time: it is the type of a result that never
/// arrives (joining a child that only panics), which is what `Never` says, and `Never` may also
/// stand for an error type (`fail` of a value that never arrives). A set type parameter stands for
/// elements that only callers would add: what the body itself fails with or uses is already among
/// the set's other elements, so the empty set (neither fails, suspends nor needs the evidence
/// table) leaves the exact flags.
fn root_type_args(out: &mut Module, m: &Module, f: FnId) -> Option<Vec<TypeId>> {
    let def = &m.functions[f];
    if def.type_params == 0 {
        return Some(Vec::new());
    }
    let inputs = def.params.len() - usize::from(m.takes_ev(f));
    if inputs > 0 {
        return None;
    }
    let empty = out.intern(Type::Set(SetSummary::default()));
    let args = (0..def.type_params)
        .map(|i| {
            if def.set_params.contains(&i) {
                empty
            } else {
                TypeId::NEVER
            }
        })
        .collect();
    Some(args)
}

/// A copy of a function for one instantiation.
#[derive(Clone)]
struct Spec {
    old: FnId,
    args: Vec<TypeId>,
    new: FnId,
}

/// How to call a copy ([`Mono::target`]).
#[derive(Clone, Copy)]
struct Target {
    f: FnId,
    /// The original took the evidence table but the copy does not.
    drops_ev: bool,
    fails: bool,
    suspends: bool,
}

/// What an old local became in the copy being defined.
#[derive(Clone, Copy)]
enum NewLocal {
    Local(Local),
    /// A dropped evidence parameter: its uses become `()`.
    Unit,
}

struct Mono<'a> {
    m: &'a Module,
    out: Module,
    /// Types of old expressions (the callees of indirect calls).
    typer: Typer<'a>,
    /// Copies by function and type arguments (the key).
    specs: FxHashMap<(FnId, Box<[TypeId]>), Target>,
    worklist: VecDeque<Spec>,
    /// Output functions → the input functions they copy.
    origins: ArenaMap<FnId, FnId>,
    names: FxHashSet<String>,
    /// Effect instances in the output, by declaration and erased type arguments.
    effects: FxHashMap<(String, Vec<TypeId>), EffectId>,
    /// Old effects without type parameters → their instance in the output.
    plain_effects: FxHashMap<EffectId, EffectId>,
    /// Whether a type of the output table mentions a capability type, by ID (`None`: not known
    /// yet).
    has_handler: Vec<Option<bool>>,
    /// Types imported from `typer`'s extension of the old table.
    imported: FxHashMap<TypeId, TypeId>,
    /// Per copy being defined: its type arguments, old locals → new, and substituted types.
    args: Vec<TypeId>,
    locals: FxHashMap<Local, NewLocal>,
    memo: FxHashMap<TypeId, TypeId>,
    /// The variable types of the enclosing loops in the copy being defined, innermost last.
    loops: Vec<Vec<TypeId>>,
    /// See [`Monomorphized::missing_impls`].
    missing_impls: Vec<(String, String)>,
    /// Derived impl functions whose bodies are not generated yet ([`Mono::derived_target`]).
    derived_work: Vec<derive::DerivedSpec>,
    /// Error enums whose display ([`Mono::error_display`]) is decided.
    error_seen: FxHashSet<EnumId>,
    /// The functions showing values of a type that does not satisfy Show for `dbg`
    /// ([`Mono::debug_val`]), by type.
    debug_fns: FxHashMap<TypeId, FnId>,
}

impl<'a> Mono<'a> {
    fn new(m: &'a Module) -> Self {
        let out = Module {
            enums: m.enums.clone(),
            option: m.option,
            bundled_files: m.bundled_files.clone(),
            types: m.types.clone(),
            // For a later pass on the output (the C and WASM backends monomorphize again).
            trait_impls: m.trait_impls.clone(),
            show_trait: m.show_trait,
            http_service: m.http_service,
            ..Module::default()
        };
        Mono {
            m,
            out,
            typer: Typer::new(m),
            specs: FxHashMap::default(),
            worklist: VecDeque::new(),
            origins: ArenaMap::default(),
            names: FxHashSet::default(),
            effects: FxHashMap::default(),
            plain_effects: FxHashMap::default(),
            has_handler: Vec::new(),
            imported: FxHashMap::default(),
            args: Vec::new(),
            locals: FxHashMap::default(),
            memo: FxHashMap::default(),
            loops: Vec::new(),
            missing_impls: Vec::new(),
            derived_work: Vec::new(),
            error_seen: FxHashSet::default(),
            debug_fns: FxHashMap::default(),
        }
    }

    /// Defines copies and derived impl functions until none is left.
    fn run(&mut self) {
        loop {
            if let Some(spec) = self.worklist.pop_front() {
                self.define(spec);
            } else if let Some(d) = self.derived_work.pop() {
                self.define_derived(d);
            } else {
                break;
            }
        }
    }

    // ---- Step 1: types ----

    /// `ty` of the old module with the current copy's type arguments substituted.
    fn ty(&mut self, ty: TypeId) -> TypeId {
        if let Some(&t) = self.memo.get(&ty) {
            return t;
        }
        let args = std::mem::take(&mut self.args);
        let t = subst_type(
            &mut Subst {
                mono: self,
                args: &args,
            },
            ty,
            &args,
        );
        self.args = args;
        self.memo.insert(ty, t);
        t
    }

    fn tys(&mut self, tys: &[TypeId]) -> Vec<TypeId> {
        tys.iter().map(|&t| self.ty(t)).collect()
    }

    /// The effect `e` of the old module seen with type arguments `args` (those of the function the
    /// effect's type parameters belong to).
    fn effect(&mut self, e: EffectId, args: &[TypeId]) -> EffectId {
        if let Some(&id) = self.plain_effects.get(&e) {
            return id;
        }
        let def = &self.m.effects[e];
        let new_args: Vec<TypeId> = def
            .args
            .iter()
            .map(|&t| subst_type(&mut Subst { mono: self, args }, t, args))
            .collect();
        let key_args: Vec<TypeId> = new_args
            .iter()
            .map(|&t| self.out.types.erase_sets(t))
            .collect();
        let key = (def.decl.clone(), key_args);
        let generic = def.args.iter().any(|&t| self.m.types.has_param(t));
        if let Some(&id) = self.effects.get(&key) {
            if !generic {
                self.plain_effects.insert(e, id);
            }
            return id;
        }
        let mut name = if generic {
            let names: Vec<String> = new_args
                .iter()
                .map(|&t| type_name(&self.out, &self.out.types, t))
                .collect();
            format!("{}[{}]", def.decl, names.join(", "))
        } else {
            def.name.clone()
        };
        while self.out.effects.iter().any(|(_, d)| d.name == name) {
            name.push('\'');
        }
        // Added before its operations, whose types might mention the effect itself.
        let label_of = def.label_of;
        let id = self.out.add_effect(EffectDef {
            name,
            decl: def.decl.clone(),
            args: new_args,
            ops: Vec::new(),
            label_of: None,
        });
        self.effects.insert(key, id);
        if !generic {
            self.plain_effects.insert(e, id);
        }
        let ops = def
            .ops
            .iter()
            .map(|op| {
                // An operation's own type parameters (only in built-in effects) stay.
                let a: &[TypeId] = if op.type_params == 0 { args } else { &[] };
                let mut s = Subst {
                    mono: self,
                    args: a,
                };
                OpDef {
                    params: op
                        .params
                        .iter()
                        .map(|&t| subst_type(&mut s, t, a))
                        .collect(),
                    ret: subst_type(&mut s, op.ret, a),
                    ..op.clone()
                }
            })
            .collect();
        self.out.effects[id].ops = ops;
        // The base of a label has no type parameters (E0429 and its restriction).
        self.out.effects[id].label_of = label_of.map(|b| self.effect(b, &[]));
        id
    }

    /// The effect `e` inside the copy being defined.
    fn effect_here(&mut self, e: EffectId) -> EffectId {
        let args = std::mem::take(&mut self.args);
        let id = self.effect(e, &args);
        self.args = args;
        id
    }

    /// Whether the output type `ty` mentions a capability type.
    fn has_handler(&mut self, ty: TypeId) -> bool {
        let i = ty.index();
        if self.has_handler.len() <= i {
            self.has_handler
                .resize(self.out.types.len().max(i + 1), None);
        }
        if let Some(b) = self.has_handler[i] {
            return b;
        }
        let b = match self.out.types[ty].clone() {
            Type::Handler(_) => true,
            Type::Tuple(ts) | Type::Enum(_, ts) => ts.iter().any(|&t| self.has_handler(t)),
            Type::List(t) | Type::Cell(t) => self.has_handler(t),
            Type::Fn(f) => f.params.iter().any(|&t| self.has_handler(t)) || self.has_handler(f.ret),
            _ => false,
        };
        self.has_handler[i] = Some(b);
        b
    }

    /// A type of the old typer's table (which extends the old module's) in the output table.
    fn import(&mut self, ty: TypeId) -> TypeId {
        if ty.index() < self.m.types.len() {
            return ty;
        }
        if let Some(&t) = self.imported.get(&ty) {
            return t;
        }
        let new = match self.typer.ty(ty).clone() {
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|&t| self.import(t)).collect()),
            Type::List(t) => Type::List(self.import(t)),
            Type::Enum(id, ts) => Type::Enum(id, ts.iter().map(|&t| self.import(t)).collect()),
            Type::Cell(t) => Type::Cell(self.import(t)),
            Type::Set(s) => Type::Set(SetSummary {
                params: s.params.iter().map(|&t| self.import(t)).collect(),
                ..s
            }),
            Type::Fn(f) => Type::Fn(FnType {
                params: f.params.iter().map(|&t| self.import(t)).collect(),
                ret: self.import(f.ret),
                sets: f.sets.map(|s| FnSets {
                    errors: self.import(s.errors),
                    effects: self.import(s.effects),
                }),
                ..f
            }),
            t => t,
        };
        let t = self.out.intern(new);
        self.imported.insert(ty, t);
        t
    }

    // ---- Step 2: call targets ----

    /// The copy of the root `f`: its only instantiation, or for a generic `f` the one at
    /// [`Mono::uniform_args`], named and exported like `f`.
    fn root(&mut self, f: FnId) -> FnId {
        let old = &self.m.functions[f];
        if old.type_params == 0 {
            return self.target(f, &[]).f;
        }
        let args = self.uniform_args(f);
        let new = self.target(f, &args).f;
        let (name, exported, host_exports) =
            (old.name.clone(), old.exported, old.host_exports.clone());
        // Other copies of `f` have type arguments in their names, so the plain name is free.
        self.names.insert(name.clone());
        let copy = &mut self.out.functions[new];
        copy.name = name;
        copy.exported = exported;
        copy.host_exports = host_exports;
        new
    }

    /// Type arguments for a generic root ([`monomorphize_from`]): `Any`, or for a set type
    /// parameter a set that fails and needs the evidence table but does not suspend. That matches
    /// how lowering's original copy calls the functions it is given; a suspending copy
    /// (`List.map$suspend8`) is never a root.
    fn uniform_args(&mut self, f: FnId) -> Vec<TypeId> {
        let old = &self.m.functions[f];
        let types = &self.m.types;
        let mut sets = FxHashSet::default();
        for &p in &old.params {
            set_params(types, self.m.locals[p].ty, &mut sets);
        }
        set_params(types, old.ret, &mut sets);
        if let Some(s) = old.sets {
            set_params(types, s.errors, &mut sets);
            set_params(types, s.effects, &mut sets);
        }
        let set = self.out.intern(Type::Set(SetSummary {
            fails: true,
            suspends: false,
            ev: true,
            params: Vec::new(),
        }));
        (0..old.type_params)
            .map(|i| if sets.contains(&i) { set } else { TypeId::ANY })
            .collect()
    }

    /// The copy of `f` for the (already substituted) type arguments `args`, created on first use.
    /// For a trait function, the copy of the impl function for `Self` (`args[0]`), or of the
    /// default implementation when the impl does not define the function.
    fn target(&mut self, f: FnId, args: &[TypeId]) -> Target {
        if let Some(&t) = self.specs.get(&(f, args.into())) {
            return t;
        }
        if let Some(dispatch) = self.m.dispatch.get(&f) {
            if let Some((impl_fn, mut impl_args)) = self.pick_impl(f, dispatch, args) {
                impl_args.extend_from_slice(&args[1..]);
                let t = self.target(impl_fn, &impl_args);
                self.specs.insert((f, args.into()), t);
                return t;
            }
            if let Some(t) = self.derived_target(f, args) {
                self.specs.insert((f, args.into()), t);
                return t;
            }
            if !dispatch.default {
                let ty = type_name(&self.out, &self.out.types, args[0]);
                let missing = (self.m.functions[f].name.clone(), ty);
                if !self.missing_impls.contains(&missing) {
                    self.missing_impls.push(missing);
                }
            }
        }
        let old = &self.m.functions[f];
        let old_ev = self.m.takes_ev(f);
        // The function's own E and R decide `fails` and the evidence table exactly once the set
        // type parameters are known.
        let (fails, ev) = match old.sets {
            Some(sets) => {
                let mut s = Subst { mono: self, args };
                let errors = subst_type(&mut s, sets.errors, args);
                let effects = subst_type(&mut s, sets.effects, args);
                let (Type::Set(e), Type::Set(r)) =
                    (&self.out.types[errors], &self.out.types[effects])
                else {
                    unreachable!("a function's E and R are sets");
                };
                (old.fails && e.fails, old_ev && r.ev)
            }
            None => (old.fails, old_ev),
        };
        let mut name = old.name.clone();
        if !args.is_empty() {
            let names: Vec<String> = args
                .iter()
                .map(|&t| type_name(&self.out, &self.out.types, t))
                .collect();
            name = format!("{name}[{}]", names.join(", "));
        }
        if !self.names.insert(name.clone()) {
            let mut i = 2;
            while !self.names.insert(format!("{name}#{i}")) {
                i += 1;
            }
            name = format!("{name}#{i}");
        }
        // The signature is made here, so that calls and references can be adapted to it before
        // the body is defined.
        let drops_ev = old_ev && !ev;
        let kept = old.params.len() - usize::from(drops_ev);
        let params = old.params[..kept]
            .iter()
            .map(|&p| {
                let data = &self.m.locals[p];
                let ty = subst_type(&mut Subst { mono: self, args }, data.ty, args);
                self.out.local(&data.hint, ty)
            })
            .collect();
        let ret = subst_type(&mut Subst { mono: self, args }, old.ret, args);
        let new = self.out.functions.alloc(Function {
            name,
            exported: old.exported && args.is_empty(),
            host_exports: if args.is_empty() {
                old.host_exports.clone()
            } else {
                Vec::new()
            },
            params,
            ret,
            type_params: 0,
            set_params: Vec::new(),
            fails,
            body: Expr::Lit(Lit::Unit),
            external: old.external.clone(),
            suspends: old.suspends,
            constant: old.constant,
            sets: None,
        });
        self.origins.insert(new, f);
        let t = Target {
            f: new,
            drops_ev,
            fails,
            suspends: old.suspends,
        };
        self.specs.insert((f, args.into()), t);
        self.worklist.push_back(Spec {
            old: f,
            args: args.to_vec(),
            new,
        });
        t
    }

    /// The impl function of trait function `f` for `Self` = `args[0]`, and the impl's type
    /// arguments: what its type parameters stand for in `Self` (`Int` and `String` in
    /// `Pair[$0, $1]` for `Pair[Int, String]`).
    fn pick_impl(
        &self,
        f: FnId,
        dispatch: &Dispatch,
        args: &[TypeId],
    ) -> Option<(FnId, Vec<TypeId>)> {
        let self_ty = *args.first()?;
        // The trait function's type parameters are `Self` and its own; the impl function's are
        // the impl's and the same own ones.
        let own = self.m.functions[f].type_params.saturating_sub(1);
        dispatch.impls.iter().find_map(|&(pattern, impl_fn)| {
            let n = self.m.functions[impl_fn].type_params.saturating_sub(own);
            let mut bound = vec![None; n];
            self.bind_head(pattern, self_ty, &mut bound).then(|| {
                let args = bound
                    .into_iter()
                    .map(|t| t.unwrap_or(TypeId::UNIT))
                    .collect();
                (impl_fn, args)
            })
        })
    }

    /// Matches the impl's type `pattern` (of the input, with the impl's type parameters) against
    /// the type `actual` (of the output), binding the type parameters in `bound`.
    fn bind_head(&self, pattern: TypeId, actual: TypeId, bound: &mut [Option<TypeId>]) -> bool {
        match (self.m.ty(pattern), self.out.ty(actual)) {
            (Type::Param(i), _) => {
                if let Some(slot) = bound.get_mut(*i) {
                    *slot = Some(actual);
                }
                true
            }
            (Type::Enum(a, ps), Type::Enum(b, xs)) => {
                a == b
                    && ps.len() == xs.len()
                    && ps
                        .iter()
                        .zip(xs)
                        .all(|(&p, &x)| self.bind_head(p, x, bound))
            }
            (Type::List(p), Type::List(x)) => self.bind_head(*p, *x, bound),
            (Type::Tuple(ps), Type::Tuple(xs)) => {
                ps.len() == xs.len()
                    && ps
                        .iter()
                        .zip(xs)
                        .all(|(&p, &x)| self.bind_head(p, x, bound))
            }
            // The output table extends the input's, so a type without parameters has one ID.
            _ => pattern == actual,
        }
    }

    /// Defines the body of a copy.
    fn define(&mut self, spec: Spec) {
        let m = self.m;
        let old = &m.functions[spec.old];
        self.args = spec.args;
        self.locals.clear();
        self.memo.clear();
        let new = &self.out.functions[spec.new];
        for (i, &p) in old.params.iter().enumerate() {
            let n = match new.params.get(i) {
                Some(&l) => NewLocal::Local(l),
                // The dropped evidence parameter.
                None => NewLocal::Unit,
            };
            self.locals.insert(p, n);
        }
        if old.external.is_some() {
            return;
        }
        let ret = new.ret;
        let body = self.expr(&old.body);
        let body = self.coerce(&old.body, body, ret);
        self.out.functions[spec.new].body = body;
    }

    // ---- Adapting values to their slots ----

    /// `new` (the rewritten `old`) adapted to flow into a slot of type `slot`. Needed where the
    /// value's calling convention differs from the slot's:
    ///
    /// - `()` passed where there is no evidence table becomes an empty table;
    /// - a function value without the evidence table flowing into a slot whose function type takes
    ///   it (the type checker lets a function with a smaller R be used as one with a larger R:
    ///   lambdas, spec 9.4, and references to function values) is wrapped in a lambda that takes
    ///   the table and drops it. Parameters and results that are themselves such functions are
    ///   adapted the same way, the other way round for parameters.
    ///
    /// Branches (`if`, `match`, `escape`) and the bodies of `let`, `with` and `loop` are adapted
    /// separately, as each may produce a different value.
    ///
    /// JS ignores extra arguments, so it would run without this, but WASM's `call_indirect`
    /// traps on a wrong argument count.
    ///
    /// A function inside an enum's type arguments or a list's element type cannot be adapted
    /// (`List[fn() -> Int]` used as a `List[fn() -> Int ev]`). That does not happen today: the
    /// type checker compares the type arguments of named types for equality, and list literals
    /// adapt element by element. [`crate::verify_monomorphic`] would catch it. Function types that
    /// lowering builds by hand (the fiber wrappers, `State.run`'s operations, handler operation
    /// implementations) have no `sets`, so their flags stay approximate; none takes the evidence
    /// table, so the argument count is right anyway.
    fn coerce(&mut self, old: &'a Expr, new: Expr, slot: TypeId) -> Expr {
        let slot_is_fn = matches!(self.out.ty(slot), Type::Fn(_));
        if !slot_is_fn && slot != TypeId::EVIDENCE {
            return new;
        }
        match (old, new) {
            (
                Expr::If {
                    then: ot,
                    else_: oe,
                    ..
                },
                Expr::If { cond, then, else_ },
            ) => Expr::If {
                cond,
                then: Box::new(self.coerce(ot, *then, slot)),
                else_: Box::new(self.coerce(oe, *else_, slot)),
            },
            (Expr::Match { arms: oa, .. }, Expr::Match { scrutinee, arms }) => Expr::Match {
                scrutinee,
                arms: self.coerce_arms(oa, arms, slot),
            },
            (Expr::Escape { body: ob, arms: oa }, Expr::Escape { body, arms }) => Expr::Escape {
                body: Box::new(self.coerce(ob, *body, slot)),
                arms: self.coerce_arms(oa, arms, slot),
            },
            (Expr::Let { body: ob, .. }, Expr::Let { var, value, body }) => Expr::Let {
                var,
                value,
                body: Box::new(self.coerce(ob, *body, slot)),
            },
            (Expr::With { body: ob, .. }, Expr::With { binds, body }) => Expr::With {
                binds,
                body: Box::new(self.coerce(ob, *body, slot)),
            },
            (Expr::Loop { body: ob, .. }, Expr::Loop { vars, inits, body }) => Expr::Loop {
                vars,
                inits,
                body: Box::new(self.coerce(ob, *body, slot)),
            },
            (_, Expr::Lit(Lit::Unit)) if slot == TypeId::EVIDENCE => Expr::Provide {
                base: None,
                bindings: Vec::new(),
            },
            (old, new) => {
                let from = self.value_ty(old, &new);
                if self.needs_adapter(from, slot) {
                    self.adapter(new, from, slot)
                } else {
                    new
                }
            }
        }
    }

    fn coerce_arms(&mut self, old: &'a [Arm], new: Vec<Arm>, slot: TypeId) -> Vec<Arm> {
        old.iter()
            .zip(new)
            .map(|(o, n)| Arm {
                body: self.coerce(&o.body, n.body, slot),
                ..n
            })
            .collect()
    }

    /// The type of the rewritten expression `new` (of `old`).
    fn value_ty(&mut self, old: &'a Expr, new: &Expr) -> TypeId {
        match new {
            Expr::Var(l) => self.out.locals[*l].ty,
            Expr::Lambda { ty, .. } => *ty,
            Expr::Lit(lit) => crate::typing::lit_type(lit),
            Expr::Fn { f: id, .. } => {
                let ev = self.out.takes_ev(*id);
                let f = &self.out.functions[*id];
                let declared = &f.params[..f.params.len() - usize::from(ev)];
                let ty = FnType {
                    params: declared.iter().map(|&p| self.out.locals[p].ty).collect(),
                    ret: f.ret,
                    fails: f.fails,
                    suspends: f.suspends,
                    ev,
                    sets: None,
                };
                self.out.func(ty)
            }
            _ => {
                let t = self.typer.type_of(old);
                let t = self.import(t);
                self.ty(t)
            }
        }
    }

    /// Whether a function value of type `from` needs an adapter to be used as a `to`.
    fn needs_adapter(&self, from: TypeId, to: TypeId) -> bool {
        if from == to {
            return false;
        }
        match (self.out.ty(from), self.out.ty(to)) {
            (Type::Fn(a), Type::Fn(b)) => {
                (b.ev && !a.ev)
                    || a.params.len() == b.params.len()
                        && (a
                            .params
                            .iter()
                            .zip(&b.params)
                            .any(|(&p, &q)| self.needs_adapter(q, p))
                            || self.needs_adapter(a.ret, b.ret))
            }
            _ => false,
        }
    }

    /// `fn(xs.., ev) { adapt(v(adapt(xs).., [ev])) }` with `v` bound to `value`, of type `to`.
    fn adapter(&mut self, value: Expr, from: TypeId, to: TypeId) -> Expr {
        let (Type::Fn(a), Type::Fn(b)) = (self.out.ty(from).clone(), self.out.ty(to).clone())
        else {
            unreachable!("adapters are made between function types");
        };
        let v = self.out.local("f", from);
        let mut params: Vec<Local> = b.params.iter().map(|&t| self.out.local("x", t)).collect();
        let mut args: Vec<Expr> = params
            .iter()
            .zip(&a.params)
            .map(|(&p, &want)| {
                let have = self.out.locals[p].ty;
                let x = Expr::Var(p);
                if self.needs_adapter(have, want) {
                    self.adapter(x, have, want)
                } else {
                    x
                }
            })
            .collect();
        if b.ev {
            let ev = self.out.local("ev", TypeId::EVIDENCE);
            params.push(ev);
            if a.ev {
                args.push(Expr::Var(ev));
            }
        }
        let call = Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(v))),
            args,
            fails: a.fails,
            suspends: a.suspends,
        };
        let body = if self.needs_adapter(a.ret, b.ret) {
            self.adapter(call, a.ret, b.ret)
        } else {
            call
        };
        Expr::Let {
            var: v,
            value: Box::new(value),
            body: Box::new(Expr::Lambda {
                params,
                ty: to,
                body: Box::new(body),
                suspends: a.suspends,
            }),
        }
    }

    // ---- The Core tree ----

    /// A new local for an old one being bound.
    fn bind(&mut self, l: Local) -> Local {
        let data = &self.m.locals[l];
        let ty = self.ty(data.ty);
        let new = self.out.local(&data.hint, ty);
        self.locals.insert(l, NewLocal::Local(new));
        new
    }

    /// The new local of an old one (bound earlier, or bound here if not).
    fn local(&mut self, l: Local) -> NewLocal {
        match self.locals.get(&l) {
            Some(&n) => n,
            None => NewLocal::Local(self.bind(l)),
        }
    }

    fn exprs(&mut self, es: &'a [Expr]) -> Vec<Expr> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    /// Rewrites values flowing into slots of types `slots` ([`Mono::coerce`]); values past the end
    /// of `slots` are only rewritten.
    fn values(&mut self, es: &'a [Expr], slots: &[TypeId]) -> Vec<Expr> {
        es.iter()
            .enumerate()
            .map(|(i, e)| {
                let new = self.expr(e);
                match slots.get(i) {
                    Some(&slot) => self.coerce(e, new, slot),
                    None => new,
                }
            })
            .collect()
    }

    /// Rewrites a value flowing into a slot of type `slot`.
    fn value(&mut self, e: &'a Expr, slot: TypeId) -> Expr {
        let new = self.expr(e);
        self.coerce(e, new, slot)
    }

    fn local_tys(&self, ls: &[Local]) -> Vec<TypeId> {
        ls.iter().map(|&l| self.out.locals[l].ty).collect()
    }

    fn expr(&mut self, e: &'a Expr) -> Expr {
        match e {
            Expr::Lit(lit) => Expr::Lit(lit.clone()),
            Expr::Var(l) => match self.local(*l) {
                NewLocal::Local(n) => Expr::Var(n),
                NewLocal::Unit => Expr::Lit(Lit::Unit),
            },
            Expr::Fn { f, ty_args } => {
                let args = self.tys(ty_args);
                Expr::Fn {
                    f: self.target(*f, &args).f,
                    ty_args: Vec::new(),
                }
            }
            Expr::Let { .. } => {
                // Walked in a loop, so long chains do not deepen the stack.
                let mut binds = Vec::new();
                let mut e = e;
                while let Expr::Let { var, value, body } = e {
                    let var = self.bind(*var);
                    let slot = self.out.locals[var].ty;
                    binds.push((var, self.value(value, slot)));
                    e = body;
                }
                let mut out = self.expr(e);
                for (var, value) in binds.into_iter().rev() {
                    out = Expr::Let {
                        var,
                        value: Box::new(value),
                        body: Box::new(out),
                    };
                }
                out
            }
            Expr::Call {
                callee,
                args,
                fails,
                suspends,
            } => self.call(callee, args, *fails, *suspends),
            Expr::Builtin {
                op: Builtin::ShowNested,
                ty_args,
                args,
            } => self.show_nested(ty_args, args),
            Expr::Builtin {
                op: op @ (Builtin::Dbg | Builtin::Inspect),
                ty_args,
                args,
            } => self.inspect(*op, ty_args, args),
            Expr::Builtin { op, ty_args, args } => Expr::Builtin {
                op: *op,
                ty_args: self.tys(ty_args),
                args: self.exprs(args),
            },
            Expr::Ctor {
                ctor,
                ty_args,
                args,
            } => {
                let ty_args = self.tys(ty_args);
                let fields = self.out.variant(*ctor).tys.clone();
                let slots: Vec<TypeId> = fields
                    .iter()
                    .map(|&t| self.out.types.subst(t, &ty_args))
                    .collect();
                Expr::Ctor {
                    ctor: *ctor,
                    args: self.values(args, &slots),
                    ty_args,
                }
            }
            Expr::Field { base, ctor, index } => Expr::Field {
                base: Box::new(self.expr(base)),
                ctor: *ctor,
                index: *index,
            },
            Expr::Tuple(es) => Expr::Tuple(self.exprs(es)),
            Expr::List { elem, elems, tail } => {
                let elem = self.ty(*elem);
                let elems = self.values(elems, &vec![elem; elems.len()]);
                Expr::List {
                    elem,
                    elems,
                    tail: tail.as_ref().map(|t| Box::new(self.expr(t))),
                }
            }
            Expr::If { cond, then, else_ } => Expr::If {
                cond: Box::new(self.expr(cond)),
                then: Box::new(self.expr(then)),
                else_: Box::new(self.expr(else_)),
            },
            Expr::Match { scrutinee, arms } => Expr::Match {
                scrutinee: Box::new(self.expr(scrutinee)),
                arms: self.arms(arms),
            },
            Expr::Binary {
                op: op @ (BinOp::Eq | BinOp::Ne),
                ty: OpTy::Structural,
                lhs,
                rhs,
            } => {
                let t = self.typer.type_of(lhs);
                let t = self.import(t);
                let t = self.ty(t);
                let (a, b) = (self.expr(lhs), self.expr(rhs));
                self.structural_eq(*op, t, a, b)
            }
            Expr::Binary { op, ty, lhs, rhs } => Expr::Binary {
                op: *op,
                ty: *ty,
                lhs: Box::new(self.expr(lhs)),
                rhs: Box::new(self.expr(rhs)),
            },
            Expr::Unary { op, ty, operand } => Expr::Unary {
                op: *op,
                ty: *ty,
                operand: Box::new(self.expr(operand)),
            },
            Expr::Lambda {
                params,
                ty,
                body,
                suspends,
            } => {
                let new_ty = self.ty(*ty);
                // A lambda whose R turned out not to need the evidence table drops its parameter.
                let drops_ev = matches!(
                    (self.m.ty(*ty), self.out.ty(new_ty)),
                    (Type::Fn(a), Type::Fn(b)) if a.ev && !b.ev
                );
                let mut ps = Vec::new();
                for (i, &p) in params.iter().enumerate() {
                    if drops_ev && i + 1 == params.len() {
                        self.locals.insert(p, NewLocal::Unit);
                    } else {
                        ps.push(self.bind(p));
                    }
                }
                let ret = match self.out.ty(new_ty) {
                    Type::Fn(f) => f.ret,
                    _ => TypeId::ANY,
                };
                let loops = std::mem::take(&mut self.loops);
                let body = self.value(body, ret);
                self.loops = loops;
                Expr::Lambda {
                    params: ps,
                    ty: new_ty,
                    body: Box::new(body),
                    suspends: *suspends,
                }
            }
            Expr::Concat(parts) => Expr::Concat(
                parts
                    .iter()
                    .map(|p| match p {
                        StrPart::Lit(s) => StrPart::Lit(s.clone()),
                        StrPart::Value(v, ty) => {
                            let ty = self.ty(*ty);
                            let v = self.expr(v);
                            self.show_part(v, ty)
                        }
                    })
                    .collect(),
            ),
            Expr::Loop { vars, inits, body } => {
                let new_vars: Vec<Local> = vars.iter().map(|&v| self.bind(v)).collect();
                let tys = self.local_tys(&new_vars);
                let inits = self.values(inits, &tys);
                self.loops.push(tys);
                let body = self.expr(body);
                self.loops.pop();
                Expr::Loop {
                    vars: new_vars,
                    inits,
                    body: Box::new(body),
                }
            }
            Expr::Recur(args) => {
                let tys = self.loops.last().cloned().unwrap_or_default();
                Expr::Recur(self.values(args, &tys))
            }
            Expr::Assert(a) => {
                let cond = match &a.cond {
                    AssertCond::Compare {
                        op,
                        op_ty,
                        ty,
                        lhs,
                        rhs,
                    } => {
                        let ty = self.ty(*ty);
                        let (lhs, rhs) = (self.expr(lhs), self.expr(rhs));
                        let compare = |m: &mut Self, l: Expr, r: Expr| {
                            if *op_ty == OpTy::Structural {
                                m.structural_eq(*op, ty, l, r)
                            } else {
                                Expr::Binary {
                                    op: *op,
                                    ty: *op_ty,
                                    lhs: Box::new(l),
                                    rhs: Box::new(r),
                                }
                            }
                        };
                        // The sides are shown only if their type satisfies Show (spec 13.2).
                        if !self.shows(ty) {
                            AssertCond::Bool(compare(self, lhs, rhs))
                        } else if let Some(show) =
                            self.m.traits.show.filter(|_| self.written_show(ty))
                        {
                            // Through the impl: the sides are bound first and shown if it fails.
                            let l = self.out.local("left", ty);
                            let r = self.out.local("right", ty);
                            let test = compare(self, Expr::Var(l), Expr::Var(r));
                            let failed = self.assert_failed(show, a, *op, ty, l, r);
                            return assert_bound(test, failed, (l, lhs), (r, rhs));
                        } else if *op_ty == OpTy::Structural
                            && let Some(eq) = self.m.traits.eq
                            && !self.structural(eq, ty, &mut Vec::new())
                        {
                            // Compared through Eq's impl and shown by the backend.
                            let l = self.out.local("left", ty);
                            let r = self.out.local("right", ty);
                            let decide = self.structural_eq(*op, ty, Expr::Var(l), Expr::Var(r));
                            let shown = Assert::shown(decide, *op, ty, (l, lhs), (r, rhs), &a.text);
                            return Assert::at(shown, a.location.clone());
                        } else {
                            AssertCond::Compare {
                                op: *op,
                                op_ty: *op_ty,
                                ty,
                                lhs,
                                rhs,
                            }
                        }
                    }
                    AssertCond::Shown {
                        cond,
                        op,
                        ty,
                        lhs,
                        rhs,
                    } => {
                        let ty = self.ty(*ty);
                        let cond = self.expr(cond);
                        let (lhs, rhs) = (self.expr(lhs), self.expr(rhs));
                        if !self.shows(ty) {
                            AssertCond::Bool(cond)
                        } else if let Some(show) =
                            self.m.traits.show.filter(|_| self.written_show(ty))
                            // The sides are variables bound before the assert (`Assert::shown`).
                            && let (Expr::Var(l), Expr::Var(r)) = (&lhs, &rhs)
                        {
                            let failed = self.assert_failed(show, a, *op, ty, *l, *r);
                            return Expr::If {
                                cond: Box::new(cond),
                                then: Box::new(Expr::Lit(Lit::Unit)),
                                else_: Box::new(failed),
                            };
                        } else {
                            AssertCond::Shown {
                                cond,
                                op: *op,
                                ty,
                                lhs,
                                rhs,
                            }
                        }
                    }
                    AssertCond::Bool(c) => AssertCond::Bool(self.expr(c)),
                };
                Expr::Assert(Box::new(Assert {
                    cond,
                    text: a.text.clone(),
                    location: a.location.clone(),
                }))
            }
            Expr::Fail(v) => {
                let ty = self.typer.type_of(v);
                let ty = self.import(ty);
                let ty = self.ty(ty);
                self.error_display(ty);
                Expr::Fail(Box::new(self.expr(v)))
            }
            Expr::Escape { body, arms } => Expr::Escape {
                body: Box::new(self.expr(body)),
                arms: self.arms(arms),
            },
            Expr::Use { ctx, effect } => Expr::Use {
                ctx: Box::new(self.expr(ctx)),
                effect: self.effect_here(*effect),
            },
            Expr::Provide { base, bindings } => self.provide(base.as_deref(), bindings, None),
            Expr::Handler { name, effect, ops } => Expr::Handler {
                name: name.clone(),
                effect: self.effect_here(*effect),
                ops: self.exprs(ops),
            },
            Expr::With { binds, body } => {
                let binds = binds
                    .iter()
                    .map(|b| {
                        let var = self.bind(b.var);
                        let slot = self.out.locals[var].ty;
                        let value = self.value(&b.value, slot);
                        WithBind {
                            var,
                            value,
                            release: b.release.as_ref().map(|r| Release {
                                handler: r.handler.clone(),
                                body: self.expr(&r.body),
                            }),
                        }
                    })
                    .collect();
                Expr::With {
                    binds,
                    body: Box::new(self.expr(body)),
                }
            }
            Expr::Fiber { op, args } => Expr::Fiber {
                op: *op,
                args: self.exprs(args),
            },
        }
    }

    /// An evidence table extension. Keys mentioning type parameters belong to the function being
    /// defined, except in the table passed to a generic function (`callee` is then its type
    /// arguments), whose keys are the callee's (`generic_ev` in lowering).
    fn provide(
        &mut self,
        base: Option<&'a Expr>,
        bindings: &'a [(EffectId, Expr)],
        callee: Option<&[TypeId]>,
    ) -> Expr {
        // A base that was a dropped evidence parameter is no table at all.
        let base = base
            .map(|b| self.expr(b))
            .filter(|b| !matches!(b, Expr::Lit(Lit::Unit)));
        let bindings: Vec<(EffectId, Expr)> = bindings
            .iter()
            .map(|(e, h)| {
                let key = match callee {
                    Some(args) => self.effect(*e, args),
                    None => self.effect_here(*e),
                };
                (key, self.expr(h))
            })
            // Once instantiated, the callee's key is often the caller's (`State[$modify.0]` and
            // `State[Int]`), and binding the handler looked up from the base under its own key
            // changes nothing.
            .filter(|(key, h)| {
                !matches!((h, &base), (Expr::Use { ctx, effect }, Some(b)) if effect == key && **ctx == *b)
            })
            .collect();
        match base {
            Some(b) if bindings.is_empty() => b,
            base => Expr::Provide {
                base: base.map(Box::new),
                bindings,
            },
        }
    }

    /// `a == b` (or `!=`) on values of type `ty`, compared by structure (`OpTy::Structural`) when
    /// that gives what Eq gives, otherwise through `Eq.eq` (spec 10.3, 10.6). A non-structural
    /// `op_ty` is kept as is.
    fn structural_eq(&mut self, op: BinOp, ty: TypeId, a: Expr, b: Expr) -> Expr {
        let binary = |a: Expr, b: Expr| Expr::Binary {
            op,
            ty: OpTy::Structural,
            lhs: Box::new(a),
            rhs: Box::new(b),
        };
        let Some(eq) = self.m.traits.eq else {
            return binary(a, b);
        };
        if !matches!(op, BinOp::Eq | BinOp::Ne) || self.structural(eq, ty, &mut Vec::new()) {
            return binary(a, b);
        }
        let call = self.trait_call(eq, ty, vec![a, b]);
        if op == BinOp::Ne {
            Expr::Unary {
                op: UnOp::Not,
                ty: OpTy::Bool,
                operand: Box::new(call),
            }
        } else {
            call
        }
    }

    /// An interpolated value of type `ty`: shown by structure when that gives what Show gives,
    /// otherwise through `Show.show` (spec 10.3, 10.6).
    fn show_part(&mut self, v: Expr, ty: TypeId) -> StrPart {
        if let Some(show) = self.m.traits.show
            && !self.structural(show, ty, &mut Vec::new())
        {
            return StrPart::Value(self.trait_call(show, ty, vec![v]), TypeId::STRING);
        }
        StrPart::Value(v, ty)
    }

    /// A call of the Prelude's trait function `f` (pure, without the evidence table) for `Self` =
    /// `ty`.
    fn trait_call(&mut self, f: FnId, ty: TypeId, args: Vec<Expr>) -> Expr {
        let t = self.target(f, &[ty]);
        Expr::Call {
            callee: Callee::Direct {
                f: t.f,
                ty_args: Vec::new(),
            },
            args,
            fails: t.fails,
            suspends: t.suspends,
        }
    }

    /// Whether the trait function `f` (Eq.eq or Show.show) on `ty` gives what the structural
    /// operation gives (spec 10.6: types whose impls are all derived may be compared and shown by
    /// their structure). True for the impls the compiler and core provide
    /// (`Dispatch::structural`) and for derived impls (`mono/derive.rs`), when every part is so
    /// too. A type with an impl written in source is not. `seen` breaks cycles of
    /// recursive types.
    fn structural(&mut self, f: FnId, ty: TypeId, seen: &mut Vec<TypeId>) -> bool {
        if seen.contains(&ty) {
            return true;
        }
        let parts: Vec<TypeId> = match self.out.ty(ty).clone() {
            Type::Int
            | Type::Int64
            | Type::Float
            | Type::Bool
            | Type::String
            | Type::Unit
            | Type::Never
            | Type::Any => return true,
            Type::List(t) => vec![t],
            Type::Tuple(ts) => ts,
            Type::Enum(id, args) => {
                if self.out.enums[id].external {
                    return false;
                }
                let mut parts = Vec::new();
                for v in 0..self.out.enums[id].variants.len() {
                    for t in self.out.enums[id].variants[v].tys.clone() {
                        parts.push(self.out.types.subst(t, &args));
                    }
                }
                parts
            }
            _ => return false,
        };
        if let Some(dispatch) = self.m.dispatch.get(&f)
            && let Some((impl_fn, _)) = self.pick_impl(f, dispatch, &[ty])
            && !dispatch.structural.contains(&impl_fn)
        {
            return false;
        }
        seen.push(ty);
        let ok = parts.into_iter().all(|t| self.structural(f, t, seen));
        seen.pop();
        ok
    }

    /// `dbg(x)` and `inspect(x)` (spec 10.6). A type whose display reaches no written Show impl
    /// ([`Mono::written_show`]) keeps the builtin, which shows the structure: what its Show gives,
    /// and for `dbg` of a type without Show, its constructed form. Otherwise `inspect` (which the
    /// type checker gives only types with Show) calls `Show.show`, and `dbg` prints with
    /// `Builtin::DebugPrint` the text of `Show.show`, or for a type that does not satisfy Show its
    /// constructed form with the parts that do shown through Show ([`Mono::debug_val`]).
    fn inspect(&mut self, op: Builtin, ty_args: &'a [TypeId], args: &'a [Expr]) -> Expr {
        let ty_args = self.tys(ty_args);
        let mut args = self.exprs(args);
        let ty = ty_args[0];
        let Some(show) = self.m.traits.show.filter(|_| self.written_show(ty)) else {
            return Expr::Builtin { op, ty_args, args };
        };
        let value = args.remove(0);
        if op == Builtin::Inspect {
            return self.trait_call(show, ty, vec![value]);
        }
        let v = self.out.local("value", ty);
        let text = if self.shows(ty) {
            self.trait_call(show, ty, vec![Expr::Var(v)])
        } else {
            let part = self.debug_val(ty, v);
            Expr::Concat(vec![part])
        };
        let shown = self.out.local("shown", TypeId::UNIT);
        let print = Expr::Builtin {
            op: Builtin::DebugPrint,
            ty_args: Vec::new(),
            args: vec![text],
        };
        Expr::Let {
            var: v,
            value: Box::new(value),
            body: Box::new(Expr::Let {
                var: shown,
                value: Box::new(print),
                body: Box::new(Expr::Var(v)),
            }),
        }
    }

    /// Whether `ty` satisfies Show (spec 10.6), what the debug display of `dbg`, assert and errors
    /// reaching the entry asks without the type checker.
    fn shows(&self, ty: TypeId) -> bool {
        self.m
            .show_trait
            .is_some_and(|show| self.satisfies(show, ty, &mut Vec::new()))
    }

    /// Whether `ty` satisfies the trait `tr`: a written or derived impl for its head whose type
    /// parameters' constraints its type arguments satisfy (`impl Wrap[A: Eq]: Show` needs the
    /// argument's Eq), or the compiler's impls of Eq, Ord and Show for the basic types and tuples
    /// (spec 10.2, 10.5, 10.6). Function and capability types satisfy none. `seen` takes a
    /// recursive type to satisfy the trait inside itself.
    fn satisfies(&self, tr: TraitRef, ty: TypeId, seen: &mut Vec<(TraitRef, TypeId)>) -> bool {
        if seen.contains(&(tr, ty)) {
            return true;
        }
        let traits = &self.m.trait_impls[tr];
        let t = self.out.ty(ty);
        match t {
            Type::Never | Type::Any | Type::Set(_) => return true,
            Type::Fn(_)
            | Type::Handler(_)
            | Type::Evidence
            | Type::Cell(_)
            | Type::FiberScope
            | Type::Param(_) => return false,
            _ => {}
        }
        for (pattern, bounds) in &traits.impls {
            let mut bound = vec![None; bounds.len()];
            if !self.bind_head(*pattern, ty, &mut bound) {
                continue;
            }
            seen.push((tr, ty));
            let ok = bound.iter().zip(bounds).all(|(arg, traits)| {
                arg.is_none_or(|arg| traits.iter().all(|&b| self.satisfies(b, arg, seen)))
            });
            seen.pop();
            return ok;
        }
        let Some(op) = traits.builtin else {
            return false;
        };
        match t {
            Type::Float => op != DeriveOp::Compare,
            Type::Int | Type::Int64 | Type::Bool | Type::String | Type::Unit => true,
            Type::Tuple(ts) => {
                seen.push((tr, ty));
                let ok = ts.iter().all(|&t| self.satisfies(tr, t, seen));
                seen.pop();
                ok
            }
            _ => false,
        }
    }

    /// Whether the debug display of `ty` reaches a written Show impl (one not in
    /// `Dispatch::structural`): the type or a part of it (a field, an element) has one. Without
    /// one, the backends' display of the structure is the debug display (spec 10.6).
    fn written_show(&mut self, ty: TypeId) -> bool {
        let Some(show) = self.m.traits.show else {
            return false;
        };
        self.written_show_in(show, ty, &mut Vec::new())
    }

    fn written_show_in(&mut self, show: FnId, ty: TypeId, seen: &mut Vec<TypeId>) -> bool {
        if seen.contains(&ty) {
            return false;
        }
        let parts: Vec<TypeId> = match self.out.ty(ty).clone() {
            Type::List(t) => vec![t],
            Type::Tuple(ts) => ts,
            Type::Enum(id, args) => {
                if let Some(dispatch) = self.m.dispatch.get(&show)
                    && let Some((impl_fn, _)) = self.pick_impl(show, dispatch, &[ty])
                    && !dispatch.structural.contains(&impl_fn)
                {
                    return true;
                }
                let mut parts = Vec::new();
                for v in 0..self.out.enums[id].variants.len() {
                    for t in self.out.enums[id].variants[v].tys.clone() {
                        parts.push(self.out.types.subst(t, &args));
                    }
                }
                parts
            }
            _ => return false,
        };
        seen.push(ty);
        let found = parts
            .into_iter()
            .any(|t| self.written_show_in(show, t, seen));
        seen.pop();
        found
    }

    /// Records how a failure with an error of type `ty` is shown if it reaches the entry or a
    /// test (spec 14.3): by its Show, through the impl if written, or else by its name (not
    /// recorded).
    fn error_display(&mut self, ty: TypeId) {
        let Type::Enum(id, _) = *self.out.ty(ty) else {
            return;
        };
        if !self.out.enums[id].error || !self.error_seen.insert(id) {
            return;
        }
        // Already decided by an earlier pass, whose module this is.
        if let Some(&display) = self.m.error_display.get(&id) {
            let display = match display {
                ErrorDisplay::Impl(f) => ErrorDisplay::Impl(self.target(f, &[]).f),
                ErrorDisplay::Structure => ErrorDisplay::Structure,
            };
            self.out.error_display.insert(id, display);
            return;
        }
        if !self.shows(ty) {
            return;
        }
        let display = match self.m.traits.show.filter(|_| self.written_show(ty)) {
            Some(show) => ErrorDisplay::Impl(self.target(show, &[ty]).f),
            None => ErrorDisplay::Structure,
        };
        self.out.error_display.insert(id, display);
    }

    /// The defect of a failed comparison assert, showing the sides `l` and `r` through Show
    /// (spec 13.2), in the backends' message.
    fn assert_failed(
        &mut self,
        show: FnId,
        a: &Assert,
        op: BinOp,
        ty: TypeId,
        l: Local,
        r: Local,
    ) -> Expr {
        let head = if a.text.is_empty() {
            format!("left {} right", comparison_text(op))
        } else {
            a.text.clone()
        };
        let message = Expr::Concat(vec![
            StrPart::Lit(format!("assertion failed: {head}\n  left: ")),
            StrPart::Value(
                self.trait_call(show, ty, vec![Expr::Var(l)]),
                TypeId::STRING,
            ),
            StrPart::Lit("\n right: ".to_owned()),
            StrPart::Value(
                self.trait_call(show, ty, vec![Expr::Var(r)]),
                TypeId::STRING,
            ),
        ]);
        // The location stays outside the message (`[test.assert]`).
        let (op, args) = match &a.location {
            Some(location) => (
                Builtin::AssertFail,
                vec![message, Expr::Lit(Lit::String(location.clone()))],
            ),
            None => (Builtin::Panic, vec![message]),
        };
        Expr::Builtin {
            op,
            ty_args: Vec::new(),
            args,
        }
    }

    /// `String.show_nested(x)` (spec 10.6): `x` quoted for a String, otherwise `Show.show(x)`.
    fn show_nested(&mut self, ty_args: &'a [TypeId], args: &'a [Expr]) -> Expr {
        let ty = self.tys(ty_args)[0];
        let args = self.exprs(args);
        let show = self
            .m
            .traits
            .show
            .expect("lowering lowers Show.show for show_nested");
        if ty == TypeId::STRING {
            return Expr::Builtin {
                op: Builtin::ShowString,
                ty_args: Vec::new(),
                args,
            };
        }
        let t = self.target(show, &[ty]);
        Expr::Call {
            callee: Callee::Direct {
                f: t.f,
                ty_args: Vec::new(),
            },
            args,
            fails: t.fails,
            suspends: t.suspends,
        }
    }

    fn call(&mut self, callee: &'a Callee, args: &'a [Expr], fails: bool, suspends: bool) -> Expr {
        match callee {
            Callee::Direct { f, ty_args } => {
                let ty_args = self.tys(ty_args);
                let t = self.target(*f, &ty_args);
                let old_ev = self.m.takes_ev(*f);
                let slots = self.local_tys(&self.out.functions[t.f].params);
                let mut new_args = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let is_ev = old_ev && i + 1 == args.len();
                    if is_ev && t.drops_ev {
                        continue;
                    }
                    let new = match a {
                        Expr::Provide { base, bindings } if is_ev && !ty_args.is_empty() => {
                            self.provide(base.as_deref(), bindings, Some(&ty_args))
                        }
                        a => self.expr(a),
                    };
                    new_args.push(match slots.get(i) {
                        Some(&slot) => self.coerce(a, new, slot),
                        None => new,
                    });
                }
                Expr::Call {
                    callee: Callee::Direct {
                        f: t.f,
                        ty_args: Vec::new(),
                    },
                    args: new_args,
                    fails: t.fails,
                    suspends: t.suspends,
                }
            }
            Callee::Indirect(c) => {
                let old_ty = self.typer.type_of(c);
                let old_ty = self.import(old_ty);
                let new_ty = self.ty(old_ty);
                let (old_ev, new) = match (self.out.ty(old_ty), self.out.ty(new_ty)) {
                    (Type::Fn(a), Type::Fn(b)) => (a.ev, Some(b.clone())),
                    _ => (false, None),
                };
                let callee = Callee::Indirect(Box::new(self.expr(c)));
                let (new_args, fails, suspends) = match new {
                    Some(b) => {
                        let mut slots = b.params.clone();
                        if b.ev {
                            slots.push(TypeId::EVIDENCE);
                        }
                        // The evidence table is passed exactly when the value takes it.
                        let args = if old_ev && !b.ev {
                            &args[..args.len() - 1]
                        } else {
                            args
                        };
                        (
                            self.values(args, &slots),
                            fails && b.fails,
                            suspends && b.suspends,
                        )
                    }
                    None => (self.exprs(args), fails, suspends),
                };
                Expr::Call {
                    callee,
                    args: new_args,
                    fails,
                    suspends,
                }
            }
            Callee::Op {
                handler,
                effect,
                op,
                ty_args,
            } => {
                let handler = Box::new(self.expr(handler));
                let effect = self.effect_here(*effect);
                let ty_args = self.tys(ty_args);
                let slots: Vec<TypeId> = match self.out.effects[effect].ops.get(*op) {
                    Some(o) => o.params.clone(),
                    None => Vec::new(),
                };
                let slots: Vec<TypeId> = slots
                    .iter()
                    .map(|&t| self.out.types.subst(t, &ty_args))
                    .collect();
                Expr::Call {
                    args: self.values(args, &slots),
                    callee: Callee::Op {
                        handler,
                        effect,
                        op: *op,
                        ty_args,
                    },
                    fails,
                    suspends,
                }
            }
        }
    }

    fn arms(&mut self, arms: &'a [Arm]) -> Vec<Arm> {
        arms.iter()
            .map(|arm| Arm {
                pat: self.pat(&arm.pat),
                guard: arm.guard.as_ref().map(|g| self.expr(g)),
                body: self.expr(&arm.body),
            })
            .collect()
    }

    fn pat(&mut self, p: &Pat) -> Pat {
        match p {
            Pat::Wild => Pat::Wild,
            Pat::Bind(l) => Pat::Bind(self.bind(*l)),
            Pat::Lit(lit) => Pat::Lit(lit.clone()),
            Pat::Ctor { ctor, fields } => Pat::Ctor {
                ctor: *ctor,
                fields: fields.iter().map(|f| self.pat(f)).collect(),
            },
            Pat::Tuple(ps) => Pat::Tuple(ps.iter().map(|q| self.pat(q)).collect()),
            Pat::List { elems, rest } => Pat::List {
                elems: elems.iter().map(|q| self.pat(q)).collect(),
                rest: rest.as_ref().map(|r| Box::new(self.pat(r))),
            },
        }
    }
}

/// The type table of the output during substitution with `args`: capability types of effects
/// that mention type parameters become their instances.
struct Subst<'m, 'a, 's> {
    mono: &'m mut Mono<'a>,
    args: &'s [TypeId],
}

impl TypeStore for Subst<'_, '_, '_> {
    fn table(&self) -> &TypeTable {
        &self.mono.out.types
    }

    fn intern_type(&mut self, ty: Type) -> TypeId {
        self.mono.out.intern(ty)
    }

    fn may_change(&mut self, ty: TypeId) -> bool {
        self.mono.out.types.has_param(ty) || self.mono.has_handler(ty)
    }

    fn map_effect(&mut self, effect: EffectId) -> EffectId {
        let args = self.args;
        self.mono.effect(effect, args)
    }
}

/// Adds the indices of the set type parameters mentioned in `ty` to `out`.
fn set_params(types: &TypeTable, ty: TypeId, out: &mut FxHashSet<usize>) {
    if !types.has_param(ty) {
        return;
    }
    match &types[ty] {
        Type::Set(s) => out.extend(s.params.iter().filter_map(|&p| match types[p] {
            Type::Param(i) => Some(i),
            _ => None,
        })),
        Type::Tuple(ts) | Type::Enum(_, ts) => {
            for &t in ts {
                set_params(types, t, out);
            }
        }
        Type::List(t) | Type::Cell(t) => set_params(types, *t, out),
        Type::Fn(f) => {
            for &t in &f.params {
                set_params(types, t, out);
            }
            set_params(types, f.ret, out);
            if let Some(s) = f.sets {
                set_params(types, s.errors, out);
                set_params(types, s.effects, out);
            }
        }
        _ => {}
    }
}

/// `let l = lhs; let r = rhs; if test { () } else { failed }`: a comparison assert whose sides
/// are shown through Show if it fails (spec 13.2). Each side is evaluated once, left first.
fn assert_bound(
    test: Expr,
    failed: Expr,
    (l, lhs): (Local, Expr),
    (r, rhs): (Local, Expr),
) -> Expr {
    Expr::Let {
        var: l,
        value: Box::new(lhs),
        body: Box::new(Expr::Let {
            var: r,
            value: Box::new(rhs),
            body: Box::new(Expr::If {
                cond: Box::new(test),
                then: Box::new(Expr::Lit(Lit::Unit)),
                else_: Box::new(failed),
            }),
        }),
    }
}

/// The comparison as written in an assert's message (`left == right`).
fn comparison_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        _ => unreachable!("{op:?} is not an assert comparison"),
    }
}
