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
//!    arguments becomes the copy for them. Trait method calls (0.21) are meant to be resolved here
//!    too, after their types are known.
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

use std::collections::VecDeque;

use la_arena::ArenaMap;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dump::type_name;
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

/// Monomorphizes the module from `roots` (test functions, every function for tests). Returns the
/// new module and, per root, its function in it (`None` for a generic root, which is skipped).
pub fn monomorphize_from(m: &Module, roots: &[FnId]) -> (Module, Vec<Option<FnId>>) {
    let mut mono = Mono::new(m);
    let out_roots = roots
        .iter()
        .map(|&f| (m.functions[f].type_params == 0).then(|| mono.target(f, &[]).f))
        .collect();
    while let Some(spec) = mono.worklist.pop_front() {
        mono.define(spec);
    }
    (mono.out, out_roots)
}

/// What [`monomorphize_traced`] returns.
pub struct Monomorphized {
    pub module: Module,
    /// Per root, its function in `module`.
    pub roots: Vec<FnId>,
    /// Per function of `module`, the function of the input it copies.
    pub origins: ArenaMap<FnId, FnId>,
}

/// Monomorphizes the module from `roots` (the exported functions or the test functions; lowering
/// runs it for build, run and test). Also tells which input function each output function copies,
/// so that what is found in the output can be reported at the source (E0412 in lowering).
///
/// Unlike [`monomorphize_from`], a generic root is not skipped. It has no single instantiation, so
/// it is instantiated for callers that see every value the same way (JS, to which the entry
/// module's generic `pub` fns are exported): `Type::Any` for each type parameter, and for each set
/// type parameter a set that fails and needs the evidence table but does not suspend, the flags
/// polymorphic lowering gives such a function. The copy keeps the original name and whether it is
/// exported.
pub fn monomorphize_traced(m: &Module, roots: &[FnId]) -> Monomorphized {
    let mut mono = Mono::new(m);
    let out_roots = roots.iter().map(|&f| mono.root(f)).collect();
    while let Some(spec) = mono.worklist.pop_front() {
        mono.define(spec);
    }
    Monomorphized {
        module: mono.out,
        roots: out_roots,
        origins: mono.origins,
    }
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
}

impl<'a> Mono<'a> {
    fn new(m: &'a Module) -> Self {
        let out = Module {
            enums: m.enums.clone(),
            option: m.option,
            bundled_files: m.bundled_files.clone(),
            types: m.types.clone(),
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
        let id = self.out.add_effect(EffectDef {
            name,
            decl: def.decl.clone(),
            args: new_args,
            ops: Vec::new(),
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
        let (name, exported) = (old.name.clone(), old.exported);
        // Other copies of `f` have type arguments in their names, so the plain name is free.
        self.names.insert(name.clone());
        let copy = &mut self.out.functions[new];
        copy.name = name;
        copy.exported = exported;
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
    fn target(&mut self, f: FnId, args: &[TypeId]) -> Target {
        if let Some(&t) = self.specs.get(&(f, args.into())) {
            return t;
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
            params,
            ret,
            type_params: 0,
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
                        StrPart::Value(v, ty) => StrPart::Value(self.expr(v), self.ty(*ty)),
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
                    } => AssertCond::Compare {
                        op: *op,
                        op_ty: *op_ty,
                        ty: self.ty(*ty),
                        lhs: self.expr(lhs),
                        rhs: self.expr(rhs),
                    },
                    AssertCond::Bool(c) => AssertCond::Bool(self.expr(c)),
                };
                Expr::Assert(Box::new(Assert {
                    cond,
                    text: a.text.clone(),
                }))
            }
            Expr::Fail(v) => Expr::Fail(Box::new(self.expr(v))),
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
