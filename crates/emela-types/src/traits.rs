//! Traits and impls (spec 10.1–10.3): supertraits, the table of impls, the signatures of impl
//! functions, and whether a type satisfies a trait.
//!
//! An impl is keyed by its trait and the definition of its type (`impl Pair[A: Eq]: Eq` is
//! `(Eq, Pair)`). The head has only distinct type parameters (E0328), so one key has one impl and
//! impls never overlap (spec 10.2).

use emela_resolve::hir::{DefId, DefKind, DefModule, ImplItem, Item, Program, Res, Span, TypeKind};
use la_arena::ArenaMap;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::substitute;
use crate::lower::TyTable;
use crate::row::{Row, RowElem};
use crate::scheme::Scheme;
use crate::signature::{Signature, Signatures, param_labels, with_traits};
use crate::ty::{Prim, Ty, TyParam};

/// The traits of the Prelude (spec 10.6) whose implementations for the built-in types the
/// compiler provides for now (see [`Traits::satisfies`]).
const BUILTIN_TRAITS: &[&str] = &["Eq", "Ord", "Show"];

/// What the type checker knows about traits and impls.
#[derive(Debug, Default)]
pub struct Traits {
    /// Trait → its direct supertraits (`trait Ord: Eq`). The Prelude's Ord has Eq.
    supers: FxHashMap<DefId, Vec<DefId>>,
    /// (trait, the definition of the type) → the impl.
    impls: FxHashMap<(DefId, DefId), DefId>,
    /// Registered impls → their heads.
    heads: ArenaMap<DefId, ImplHead>,
}

/// The head of an impl: `impl Pair[A: Eq, B: Eq]: Eq`.
#[derive(Debug, Clone)]
pub struct ImplHead {
    pub trait_def: DefId,
    /// The definition of the type (`Pair`). A built-in type is the Prelude's definition (`Int`).
    pub ty_def: DefId,
    /// The type applied to the impl's type parameters (`Pair[Bound(0), Bound(1)]`).
    pub self_ty: Ty,
    /// The impl's type parameters with their constraints.
    pub params: Vec<TyParam>,
}

/// A type and a trait it does not satisfy (E0326). Within a type, the innermost part that fails:
/// for `Pair[User, Int]: Eq` with `impl Pair[A: Eq, B: Eq]: Eq`, it is `User: Eq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsatisfied {
    pub ty: Ty,
    pub trait_def: DefId,
}

impl Traits {
    /// The impl of `trait_def` for the type defined by `ty_def`.
    pub fn impl_of(&self, trait_def: DefId, ty_def: DefId) -> Option<DefId> {
        self.impls.get(&(trait_def, ty_def)).copied()
    }

    /// The head of a registered impl. Impls with errors in their heads are not registered.
    pub fn head(&self, impl_def: DefId) -> Option<&ImplHead> {
        self.heads.get(impl_def)
    }

    /// The direct supertraits of a trait.
    pub fn supertraits(&self, trait_def: DefId) -> &[DefId] {
        self.supers.get(&trait_def).map_or(&[], Vec::as_slice)
    }

    /// Whether the constraint `trait_def` gives `target`: it is `target`, or `target` is one of
    /// its supertraits, transitively (spec 10.3: `[A: Ord]` allows `==`).
    pub fn implies(&self, trait_def: DefId, target: DefId) -> bool {
        let mut seen = FxHashSet::default();
        let mut stack = vec![trait_def];
        while let Some(t) = stack.pop() {
            if t == target {
                return true;
            }
            if seen.insert(t) {
                stack.extend(self.supertraits(t).iter().copied());
            }
        }
        false
    }

    /// Whether `ty` satisfies `trait_def`. `env` are the type parameters `Ty::Bound(i)` stands
    /// for, with their constraints. On failure, the part of the type that does not satisfy which
    /// trait.
    ///
    /// - A type parameter satisfies the traits its constraints imply.
    /// - A type with an impl satisfies it if its type arguments satisfy the impl's constraints.
    ///   A `derive` counts as the impl it will generate, with the same constraint on every type
    ///   parameter (spec 10.5).
    /// - The built-in types (primitives, tuples, `List`, and the Prelude's enums) satisfy Eq, Ord
    ///   and Show when their elements do, except Float, which is not Ord (spec 10.6). This stands
    ///   in for their impls until they are written.
    /// - Undetermined types (type variables) and broken types are not reported.
    pub fn satisfies(
        &self,
        program: &Program,
        ty_table: &TyTable,
        ty: &Ty,
        trait_def: DefId,
        env: &[TyParam],
    ) -> Result<(), Unsatisfied> {
        let fail = || {
            Err(Unsatisfied {
                ty: ty.clone(),
                trait_def,
            })
        };
        let all = |tys: &[Ty], traits: &dyn Fn(usize) -> Vec<DefId>| {
            for (i, t) in tys.iter().enumerate() {
                for tr in traits(i) {
                    self.satisfies(program, ty_table, t, tr, env)?;
                }
            }
            Ok(())
        };
        match ty {
            Ty::Error | Ty::Never | Ty::Var(_) | Ty::Set(..) | Ty::Cap(_) => return Ok(()),
            Ty::Bound(i) => {
                let given = env
                    .get(*i as usize)
                    .map_or(&[][..], |p| p.traits.as_slice());
                if given.iter().any(|&t| self.implies(t, trait_def)) {
                    return Ok(());
                }
                return fail();
            }
            Ty::Fn(_) => return fail(),
            _ => {}
        }
        let args: Vec<Ty> = match ty {
            Ty::Named(_, args) | Ty::Tuple(args) => args.clone(),
            Ty::List(elem) => vec![(**elem).clone()],
            _ => Vec::new(),
        };
        if let Some(def) = head_def(program, ty_table, ty) {
            if let Some(head) = self.impl_of(trait_def, def).and_then(|i| self.head(i)) {
                return all(&args, &|i| {
                    head.params.get(i).map_or(Vec::new(), |p| p.traits.clone())
                });
            }
            if derives(program, def, trait_def) {
                return all(&args, &|_| vec![trait_def]);
            }
        }
        let builtin_trait = program.defs[trait_def].module == DefModule::Prelude
            && BUILTIN_TRAITS.contains(&program.defs[trait_def].name.as_str());
        let builtin_ty = match ty {
            Ty::Prim(Prim::Float) => program.defs[trait_def].name != "Ord",
            Ty::Prim(_) | Ty::Tuple(_) | Ty::List(_) => true,
            Ty::Named(id, _) => program.defs[ty_table.def(*id)].module == DefModule::Prelude,
            _ => false,
        };
        if builtin_trait && builtin_ty {
            return all(&args, &|_| vec![trait_def]);
        }
        fail()
    }
}

/// The definition naming a type's head, the key of impls: the type's definition, or the
/// Prelude's definition of a built-in type. `None` for tuples, functions, `()` and types that are
/// not values.
pub fn head_def(program: &Program, ty_table: &TyTable, ty: &Ty) -> Option<DefId> {
    match ty {
        Ty::Named(id, _) => Some(ty_table.def(*id)),
        Ty::Prim(Prim::Unit) => None,
        Ty::Prim(p) => program.prelude_def(p.name()),
        Ty::List(_) => program.prelude_def("List"),
        _ => None,
    }
}

/// Whether the type `ty_def` lists `trait_def` in its `derive`.
fn derives(program: &Program, ty_def: DefId, trait_def: DefId) -> bool {
    let derives = match program.item(ty_def) {
        Some(Item::Type(t)) => &t.derives,
        Some(Item::Enum(e)) => &e.derives,
        _ => return false,
    };
    derives.iter().any(|&(res, _)| res == Res::Def(trait_def))
}

/// Collects supertraits and impls, adds the signatures of impl functions to `sigs`, and checks
/// supertrait cycles (E0329), impl heads (one impl per type and trait, E0330; the orphan rule,
/// E0331), impl function parameter counts (E0332) and supertrait impls (E0326).
pub(crate) fn collect(
    program: &Program,
    ty_table: &TyTable,
    sigs: &mut Signatures,
    diags: &mut Vec<Diagnostic>,
) -> Traits {
    let mut traits = Traits::default();
    for (def, data) in program.defs.iter() {
        if data.kind != DefKind::Trait {
            continue;
        }
        let supers: Vec<DefId> = match program.item(def) {
            Some(Item::Trait(t)) => t
                .supertraits
                .iter()
                .filter_map(|&(res, _)| match res {
                    Res::Def(s) if program.defs[s].kind == DefKind::Trait => Some(s),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        traits.supers.insert(def, supers);
    }
    // The Prelude's traits are built in (`emela_resolve::builtins`): only Ord has a supertrait.
    if let (Some(ord), Some(eq)) = (program.prelude_def("Ord"), program.prelude_def("Eq"))
        && traits.supertraits(ord).is_empty()
    {
        traits.supers.insert(ord, vec![eq]);
    }
    check_cycles(program, &mut traits, diags);

    let impls: Vec<(DefId, &ImplItem)> = program
        .defs
        .iter()
        .filter_map(|(def, _)| match program.item(def) {
            Some(Item::Impl(item)) => Some((def, item)),
            _ => None,
        })
        .collect();
    for &(def, item) in &impls {
        register(program, ty_table, &mut traits, sigs, def, item, diags);
    }
    // Supertraits are checked once every impl is known.
    for &(def, item) in &impls {
        let Some(head) = traits.head(def) else {
            continue;
        };
        for &s in traits.supertraits(head.trait_def) {
            if let Err(e) = traits.satisfies(program, ty_table, &head.self_ty, s, &head.params) {
                diags.push(Diagnostic {
                    span: head_span(program, def, item),
                    kind: not_implemented(program, ty_table, &e, &head.params),
                });
            }
        }
    }
    traits
}

/// E0326 for `e`, with `Bound(i)` named after `params`.
pub(crate) fn not_implemented(
    program: &Program,
    ty_table: &TyTable,
    e: &Unsatisfied,
    params: &[TyParam],
) -> DiagnosticKind {
    DiagnosticKind::NotImplemented {
        ty: e.ty.display_with(&ty_table.cons, params).to_string().into(),
        trait_name: program.defs[e.trait_def].name.clone(),
    }
}

/// The type before `:`, or the whole impl.
fn head_span(program: &Program, def: DefId, item: &ImplItem) -> Option<Span> {
    item.self_ty
        .and_then(|t| program.types[t].span)
        .or(program.defs[def].span)
}

/// Reports each cycle of supertraits once, at the trait of the cycle defined first (E0329).
fn check_cycles(program: &Program, traits: &mut Traits, diags: &mut Vec<Diagnostic>) {
    let mut order: Vec<DefId> = traits.supers.keys().copied().collect();
    order.sort_by_key(|d| d.into_raw());
    let index: FxHashMap<DefId, usize> = order.iter().enumerate().map(|(i, &d)| (d, i)).collect();
    let mut reported: FxHashSet<DefId> = FxHashSet::default();
    for &start in &order {
        if reported.contains(&start) {
            continue;
        }
        let Some(cycle) = cycle_through(traits, start) else {
            continue;
        };
        reported.extend(cycle.iter().copied());
        // Start at the trait defined first, so the same cycle reads the same from every trait.
        let first = (0..cycle.len())
            .min_by_key(|&i| index.get(&cycle[i]).copied().unwrap_or(usize::MAX))
            .unwrap_or(0);
        let mut names: Vec<SmolStr> = cycle[first..]
            .iter()
            .chain(&cycle[..first])
            .map(|&t| program.defs[t].name.clone())
            .collect();
        names.push(names[0].clone());
        diags.push(Diagnostic {
            span: program.defs[cycle[first]].span,
            kind: DiagnosticKind::SupertraitCycle { cycle: names },
        });
    }
    // Break the cycles so following supertraits always ends.
    for t in reported {
        if let Some(supers) = traits.supers.get_mut(&t) {
            supers.clear();
        }
    }
}

/// A path of supertraits from `start` back to itself, if any (`start` first, once).
fn cycle_through(traits: &Traits, start: DefId) -> Option<Vec<DefId>> {
    fn go(
        traits: &Traits,
        t: DefId,
        start: DefId,
        path: &mut Vec<DefId>,
        seen: &mut FxHashSet<DefId>,
    ) -> bool {
        for &s in traits.supertraits(t) {
            if s == start {
                return true;
            }
            if seen.insert(s) {
                path.push(s);
                if go(traits, s, start, path, seen) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }
    let mut path = vec![start];
    go(traits, start, start, &mut path, &mut FxHashSet::default()).then_some(path)
}

/// Registers an impl and gives its functions their signatures: the trait function's type with
/// `Self` replaced by the impl's type. The impl's type parameters come first, then the trait
/// function's own.
fn register(
    program: &Program,
    ty_table: &TyTable,
    traits: &mut Traits,
    sigs: &mut Signatures,
    def: DefId,
    item: &ImplItem,
    diags: &mut Vec<Diagnostic>,
) {
    if item.malformed_head {
        return;
    }
    let Res::Def(trait_def) = item.trait_ref else {
        return;
    };
    if program.defs[trait_def].kind != DefKind::Trait {
        return;
    }
    let Some(self_ref) = item.self_ty else {
        return;
    };
    let ty_def = match &program.types[self_ref].kind {
        TypeKind::Path {
            res: Res::Def(d), ..
        } => *d,
        _ => return,
    };
    let span = head_span(program, def, item);
    let param_defs: Vec<DefId> = item.type_params.iter().map(|tp| tp.def).collect();
    let self_ty = ty_table.lower(program, &param_defs, self_ref, diags);
    if self_ty == Ty::Error {
        return;
    }
    let params: Vec<TyParam> = item
        .type_params
        .iter()
        .map(|tp| {
            let mut p = TyParam::new(program.defs[tp.def].name.clone());
            with_traits(program, &mut p, tp);
            p
        })
        .collect();
    let ty_name = || SmolStr::new(self_ty.display_with(&ty_table.cons, &params).to_string());
    let trait_name = program.defs[trait_def].name.clone();

    // An impl belongs with its type or its trait (spec 10.2).
    let module = &program.defs[def].module;
    if *module != program.defs[ty_def].module && *module != program.defs[trait_def].module {
        diags.push(Diagnostic {
            span,
            kind: DiagnosticKind::OrphanImpl {
                ty: program.defs[ty_def].name.clone(),
                trait_name: trait_name.clone(),
            },
        });
    }
    if traits.impl_of(trait_def, ty_def).is_some() {
        diags.push(Diagnostic {
            span,
            kind: DiagnosticKind::DuplicateImpl {
                ty: ty_name(),
                trait_name,
            },
        });
        return;
    }
    traits.impls.insert((trait_def, ty_def), def);

    let n = params.len() as u32;
    for f in &item.fns {
        let Res::Def(trait_fn) = f.trait_fn else {
            continue;
        };
        let Some(sig) = sigs.get(trait_fn) else {
            continue;
        };
        let Ty::Fn(fn_ty) = &sig.scheme.ty else {
            continue;
        };
        if fn_ty.params.len() != f.params.len() {
            diags.push(Diagnostic {
                span: Some(f.name.span),
                kind: DiagnosticKind::ImplFnParamCount {
                    name: f.name.name.clone(),
                    trait_name: program.defs[trait_def].name.clone(),
                    expected: fn_ty.params.len(),
                    found: f.params.len(),
                },
            });
        }
        // `Self` (the trait function's `Bound(0)`) becomes the impl's type; the trait function's
        // own type parameters move after the impl's.
        let own = sig.scheme.params.len().saturating_sub(1) as u32;
        let tys: Vec<Ty> = std::iter::once(self_ty.clone())
            .chain((0..own).map(|k| Ty::Bound(n + k)))
            .collect();
        let rows: Vec<Row> = std::iter::once(Row::empty())
            .chain((0..own).map(|k| Row::from_elems([RowElem::Bound(n + k)])))
            .collect();
        let ty = substitute(&sig.scheme.ty, &tys, &rows);
        let scheme_params = params
            .iter()
            .cloned()
            .chain(sig.scheme.params.iter().skip(1).cloned())
            .collect();
        sigs.insert(
            f.def,
            Signature {
                scheme: Scheme::new(scheme_params, ty),
                labels: param_labels(program, &f.params),
            },
        );
    }
    traits.heads.insert(
        def,
        ImplHead {
            trait_def,
            ty_def,
            self_ty,
            params,
        },
    );
}
