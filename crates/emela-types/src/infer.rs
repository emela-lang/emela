//! Unification, joining with `Never`, generalization and instantiation.
//!
//! Error sets and effect sets of function types are not equated by unification; they are
//! collected as inclusion constraints ([`Subset`]). A function placed where a type is expected
//! only needs its E and R to be contained in the expected E and R (spec 7.4, 9.2). The
//! constraints are solved after the body is inferred (`body/errors.rs`).

use emela_resolve::hir::{DefId, ExprId, Span};
use ena::unify::InPlaceUnificationTable;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::error::{TypeError, TypeErrorKind};
use crate::row::{Row, RowElem, RowVar, Tag};
use crate::scheme::{Scheme, param_name, set_param_name};
use crate::ty::{ParamKind, Prim, Ty, TyConId, TyParam, TyVar, TyVarValue};

pub use crate::row::SetKind;

/// An inclusion constraint `sub ⊆ sup`.
#[derive(Clone, Debug)]
pub(crate) struct Subset {
    pub(crate) sub: Row,
    pub(crate) sup: Row,
    pub(crate) origin: Origin,
    pub(crate) kind: SetKind,
}

/// Where an inclusion constraint came from; decides the diagnostic when it cannot be solved.
#[derive(Clone, Debug)]
pub(crate) enum Origin {
    /// A function value placed where a function type is expected (unification). The span is that
    /// expression's; `def` is the definition being inferred.
    Coerce {
        span: Option<Span>,
        def: Option<DefId>,
    },
    /// The E (R) of a definition's body is contained in the declared E (R).
    Body(DefId),
    /// The E of the body of a handler's `index`-th operation is contained in the E of the effect's
    /// operation declaration (spec 8.1).
    HandlerOp { handler: DefId, index: usize },
    /// The R of a handler's init and release bodies is contained in its dependencies, and the E
    /// of init in init's E set variable (spec 8.4, 9.4). Both are set variables, so this always
    /// holds.
    HandlerPart,
    /// The E of a handler's release body is empty (spec 8.4).
    Release(DefId),
    /// The R of the body of `main` or a test function is empty (spec 8.3, 13.3).
    Closed(DefId),
    /// The E (R) of a lambda's body is contained in the lambda type's E (R). Always holds.
    Lambda,
    /// The E (R) of an argument `arg` of the `member`-th item of a layer is empty
    /// (`[effect.layer]`).
    LayerArg {
        layer: DefId,
        member: usize,
        arg: ExprId,
    },
}

/// Decides the direction of inclusion for the E of function types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variance {
    /// The actual side is contained in the expected side.
    Co,
    /// The expected side is contained in the actual side (inside function parameters).
    Contra,
    /// Both directions (inside type arguments of named types).
    Inv,
}

impl Variance {
    fn flip(self) -> Self {
        match self {
            Variance::Co => Variance::Contra,
            Variance::Contra => Variance::Co,
            Variance::Inv => Variance::Inv,
        }
    }
}

/// Holds the type variable table and the collected type errors.
#[derive(Default)]
pub struct InferCtx {
    table: InPlaceUnificationTable<TyVar>,
    errors: Vec<TypeError>,
    /// Type variables of integer literals. They can only be bound to Int or Int64 (spec 2.4).
    int_vars: Vec<TyVar>,
    next_row_var: u32,
    subsets: Vec<Subset>,
    /// Constraints created during unification, moved to `subsets` once unification succeeds.
    row_buffer: Vec<(Row, Row, SetKind)>,
    /// The span and definition for constraints collected from now on.
    origin: (Option<Span>, Option<DefId>),
    /// Named error type → tag. Used to turn `RowElem::Of` into tags.
    error_tags: FxHashMap<TyConId, DefId>,
    /// Solutions of set variables. Present only after solving ([`Self::set_row_solution`]).
    row_solution: Option<FxHashMap<RowVar, Row>>,
    /// Whether the inclusion constraints are being solved ([`Self::set_solving`]).
    solving: bool,
}

impl InferCtx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_var(&mut self) -> TyVar {
        self.table.new_key(TyVarValue(None))
    }

    pub fn new_var(&mut self) -> Ty {
        Ty::Var(self.fresh_var())
    }

    pub fn new_row_var(&mut self) -> RowVar {
        let v = RowVar(self.next_row_var);
        self.next_row_var += 1;
        v
    }

    pub(crate) fn set_error_tags(&mut self, tags: FxHashMap<TyConId, DefId>) {
        self.error_tags = tags;
    }

    pub(crate) fn error_tag(&self, con: TyConId) -> Option<DefId> {
        self.error_tags.get(&con).copied()
    }

    /// The span for inclusion constraints that unification collects from now on.
    pub(crate) fn set_origin(&mut self, span: Option<Span>, def: Option<DefId>) {
        self.origin = (span, def);
    }

    /// Adds an inclusion constraint on error sets.
    pub(crate) fn add_subset(&mut self, sub: Row, sup: Row, origin: Origin) {
        self.add_subset_of(SetKind::Errors, sub, sup, origin);
    }

    pub(crate) fn add_subset_of(&mut self, kind: SetKind, sub: Row, sup: Row, origin: Origin) {
        if sub.is_empty() || sub == sup {
            return;
        }
        // While solving, comparing tags with set type arguments adds the same constraints again
        // on every round (`body/errors.rs`); keep one of each so the rounds end.
        if self.solving
            && self
                .subsets
                .iter()
                .any(|c| c.kind == kind && c.sub == sub && c.sup == sup)
        {
            return;
        }
        self.subsets.push(Subset {
            sub,
            sup,
            origin,
            kind,
        });
    }

    /// Forgets the solution of a previous round of solving.
    pub(crate) fn clear_row_solution(&mut self) {
        self.row_solution = None;
    }

    /// While set, constraints that are already recorded are not added again.
    pub(crate) fn set_solving(&mut self, solving: bool) {
        self.solving = solving;
    }

    pub(crate) fn subsets(&self) -> &[Subset] {
        &self.subsets
    }

    /// After this, [`Self::resolve`] replaces set variables with their solutions. Set variables
    /// without a solution become empty.
    pub(crate) fn set_row_solution(&mut self, solution: FxHashMap<RowVar, Row>) {
        self.row_solution = Some(solution);
    }

    /// A type variable for an integer literal, bindable only to Int or Int64. Type variables
    /// unified with it inherit the same constraint.
    pub fn new_int_var(&mut self) -> Ty {
        let v = self.fresh_var();
        self.int_vars.push(v);
        Ty::Var(v)
    }

    /// Whether `v` is an integer-literal type variable (or unified with one).
    pub fn is_int_var(&mut self, v: TyVar) -> bool {
        let int_vars = std::mem::take(&mut self.int_vars);
        let found = int_vars.iter().any(|&w| self.table.unioned(v, w));
        self.int_vars = int_vars;
        found
    }

    /// Follows bound type variables until the head is not a bound type variable. Unbound type
    /// variables are normalized to their representative.
    pub fn shallow_resolve(&mut self, ty: &Ty) -> Ty {
        let mut ty = ty.clone();
        while let Ty::Var(v) = ty {
            match self.table.probe_value(v).0 {
                Some(bound) => ty = bound,
                None => return Ty::Var(self.table.find(v)),
            }
        }
        ty
    }

    /// Follows every bound type variable in the type.
    ///
    /// Error types inside error sets (`RowElem::Of`) become tags if they are named error types;
    /// anything other than type parameters and type variables is dropped (reported separately).
    /// After set variables are solved, they are replaced by their solutions.
    pub fn resolve(&mut self, ty: &Ty) -> Ty {
        match self.shallow_resolve(ty) {
            Ty::Fn(f) => Ty::func_sets(
                f.params.iter().map(|p| self.resolve(p)).collect::<Vec<_>>(),
                self.resolve(&f.ret),
                self.resolve_row(&f.fails),
                self.resolve_row(&f.uses),
            ),
            Ty::Set(kind, row) => Ty::Set(kind, self.resolve_row(&row)),
            ty => ty.map_children(|child| self.resolve(child)),
        }
    }

    /// Normalizes the types inside a set like [`Self::resolve`].
    pub fn resolve_row(&mut self, row: &Row) -> Row {
        row.flat_map(|e| match e {
            RowElem::Of(ty) => match self.resolve(ty) {
                Ty::Named(con, _) => match self.error_tag(con) {
                    Some(def) => Row::tag(def),
                    None => Row::empty(),
                },
                ty @ (Ty::Bound(_) | Ty::Var(_)) => Row::from_elems([RowElem::Of(ty)]),
                _ => Row::empty(),
            },
            // Type arguments of tags in a solution may have been fixed by unification after
            // solving, so follow them (spec 8.1).
            RowElem::Var(v) => match &self.row_solution {
                Some(solution) => {
                    let row = solution.get(v).cloned().unwrap_or_default();
                    row.map_tys(|t| self.resolve(t))
                }
                None => Row::var(*v),
            },
            RowElem::Tag(t) => Row::of_tag(t.map_args(|a| self.resolve(a))),
            e => Row::from_elems([e.clone()]),
        })
    }

    /// Whether two tags are the same effect (or error) (spec 8.1). Effects with different type
    /// arguments are different tags, so the definitions and type arguments are compared. If the
    /// type arguments are not determined yet, tries unification and treats them as the same if it
    /// succeeds, binding the type variables (deciding the `S` of `State[S]` in
    /// `State.run(initial, body)` from the body's R). Otherwise changes nothing and returns false.
    ///
    /// `a` is on the containing side (the declared or provided set) and `b` on the contained side
    /// (the required one). Set type arguments (the `E` and `R` of `Scope[E, R]`, spec 11.2) only
    /// need `b`'s sets to be contained in `a`'s: a scope whose children may fail with more errors
    /// or use more effects serves code that forks fewer. The constraints are recorded and solved
    /// with the others.
    pub(crate) fn same_tag(&mut self, a: &Tag, b: &Tag) -> bool {
        if a.def != b.def || a.args.len() != b.args.len() {
            return false;
        }
        if a.args.is_empty() {
            return true;
        }
        let snapshot = self.table.snapshot();
        self.row_buffer.clear();
        let ok = a.args.iter().zip(&b.args).all(|(x, y)| {
            let variance = match (x, y) {
                (Ty::Set(..), Ty::Set(..)) => Variance::Co,
                _ => Variance::Inv,
            };
            self.unify_inner(x, y, variance).is_ok()
        });
        if ok {
            self.table.commit(snapshot);
            let origin = Origin::Coerce {
                span: self.origin.0,
                def: self.origin.1,
            };
            for (sub, sup, kind) in std::mem::take(&mut self.row_buffer) {
                self.add_subset_of(kind, sub, sup, origin.clone());
            }
        } else {
            self.row_buffer.clear();
            self.table.rollback_to(snapshot);
        }
        ok
    }

    /// Whether `row` has a tag that is the same as `tag` ([`Self::same_tag`]). If `row` has
    /// several tags of the same effect, the first one that unifies is taken (an approximation).
    pub(crate) fn row_has_tag(&mut self, row: &Row, tag: &Tag) -> bool {
        let tags: Vec<Tag> = row.tags().cloned().collect();
        tags.iter().any(|t| t == tag) || tags.iter().any(|t| self.same_tag(t, tag))
    }

    /// On failure, the table is restored to its state before the call.
    ///
    /// Error sets of function types become constraints that the actual side is contained in the
    /// expected side (reversed inside parameters, both directions inside type arguments of named
    /// types). Constraints are kept only if unification succeeds.
    pub fn unify(&mut self, expected: &Ty, actual: &Ty) -> Result<(), TypeError> {
        let snapshot = self.table.snapshot();
        self.row_buffer.clear();
        match self.unify_inner(expected, actual, Variance::Co) {
            Ok(()) => {
                self.table.commit(snapshot);
                let origin = Origin::Coerce {
                    span: self.origin.0,
                    def: self.origin.1,
                };
                for (sub, sup, kind) in std::mem::take(&mut self.row_buffer) {
                    self.add_subset_of(kind, sub, sup, origin.clone());
                }
                Ok(())
            }
            Err(kind) => {
                self.row_buffer.clear();
                // Roll back before resolving so partially bound type variables do not garble the
                // display.
                self.table.rollback_to(snapshot);
                let kind = match kind {
                    TypeErrorKind::Mismatch { expected, actual } => TypeErrorKind::Mismatch {
                        expected: self.resolve_for_report(&expected),
                        actual: self.resolve_for_report(&actual),
                    },
                    TypeErrorKind::InfiniteType { var, ty } => TypeErrorKind::InfiniteType {
                        var,
                        ty: self.resolve_for_report(&ty),
                    },
                };
                Err(TypeError {
                    expected: self.resolve_for_report(expected),
                    actual: self.resolve_for_report(actual),
                    kind: Box::new(kind),
                })
            }
        }
    }

    /// The form for error reports. Undetermined integer-literal type variables are shown as
    /// `Int`, since that is what they default to. Unsolved set variables are omitted, since they
    /// play no part in explaining a type mismatch.
    fn resolve_for_report(&mut self, ty: &Ty) -> Ty {
        match self.shallow_resolve(ty) {
            Ty::Var(v) if self.is_int_var(v) => Ty::INT,
            Ty::Fn(f) => {
                let known = |row: &Row| {
                    Row::from_elems(
                        row.elems()
                            .iter()
                            .filter(|e| !matches!(e, RowElem::Var(_)))
                            .cloned(),
                    )
                };
                let fails = known(&f.fails);
                let uses = known(&f.uses).map_tys(|t| self.resolve_for_report(t));
                Ty::func_sets(
                    f.params
                        .iter()
                        .map(|p| self.resolve_for_report(p))
                        .collect::<Vec<_>>(),
                    self.resolve_for_report(&f.ret),
                    fails.map_tys(|t| self.resolve_for_report(t)),
                    uses,
                )
            }
            Ty::Set(kind, row) => Ty::Set(
                kind,
                Row::from_elems(
                    row.elems()
                        .iter()
                        .filter(|e| !matches!(e, RowElem::Var(_)))
                        .cloned(),
                ),
            ),
            ty => ty.map_children(|child| self.resolve_for_report(child)),
        }
    }

    /// Records a `unify` failure as an error and continues. Returns `true` if unification
    /// succeeded.
    pub fn expect(&mut self, expected: &Ty, actual: &Ty) -> bool {
        self.unify(expected, actual)
            .map_err(|e| self.errors.push(e))
            .is_ok()
    }

    /// Places `actual` where `expected` is expected. A `Never` fits any type.
    pub fn coerce(&mut self, expected: &Ty, actual: &Ty) -> Result<(), TypeError> {
        if self.shallow_resolve(actual) == Ty::Never {
            return Ok(());
        }
        self.unify(expected, actual)
    }

    /// The common type of two branches. If one side is `Never`, the other's type. An unbound type
    /// variable joined with `Never` stays unbound rather than being bound to `Never`: `Never`
    /// unifies only with `Never`, so binding it would reject the type a later branch gives it.
    ///
    /// On failure, reports `a` as the expected type and `b` as the actual type.
    ///
    /// If both are function types, the error and effect sets become fresh set variables that
    /// contain both E and both R.
    pub fn join(&mut self, a: &Ty, b: &Ty) -> Result<Ty, TypeError> {
        let a = self.shallow_resolve(a);
        let b = self.shallow_resolve(b);
        match (&a, &b) {
            (Ty::Never, _) => Ok(b),
            (_, Ty::Never) => Ok(a),
            (Ty::Fn(f), Ty::Fn(g)) if f.params.len() == g.params.len() => {
                let e = self.new_row_var();
                let r = self.new_row_var();
                let joined =
                    Ty::func_sets(f.params.clone(), f.ret.clone(), Row::var(e), Row::var(r));
                self.unify(&joined, &a)?;
                self.unify(&joined, &b).map_err(|mut e| {
                    e.expected = self.resolve_for_report(&a);
                    e
                })?;
                Ok(joined)
            }
            _ => {
                self.unify(&a, &b)?;
                Ok(a)
            }
        }
    }

    /// Records a `join` failure as an error, taking `a` as the overall type.
    pub fn join_or_report(&mut self, a: &Ty, b: &Ty) -> Ty {
        match self.join(a, b) {
            Ok(ty) => ty,
            Err(e) => {
                self.errors.push(e);
                a.clone()
            }
        }
    }

    pub fn report(&mut self, error: TypeError) {
        self.errors.push(error);
    }

    pub fn errors(&self) -> &[TypeError] {
        &self.errors
    }

    pub fn take_errors(&mut self) -> Vec<TypeError> {
        std::mem::take(&mut self.errors)
    }

    fn unify_inner(
        &mut self,
        expected: &Ty,
        actual: &Ty,
        variance: Variance,
    ) -> Result<(), TypeErrorKind> {
        let e = self.shallow_resolve(expected);
        let a = self.shallow_resolve(actual);
        let mismatch = |e: &Ty, a: &Ty| TypeErrorKind::Mismatch {
            expected: e.clone(),
            actual: a.clone(),
        };
        match (&e, &a) {
            (Ty::Var(x), Ty::Var(y)) => {
                let int = self.is_int_var(*x) || self.is_int_var(*y);
                self.table.union(*x, *y);
                if int {
                    let root = self.table.find(*x);
                    self.int_vars.push(root);
                }
                Ok(())
            }
            (Ty::Var(v), t) | (t, Ty::Var(v)) => {
                if self.is_int_var(*v)
                    && !matches!(t, Ty::Prim(Prim::Int | Prim::Int64) | Ty::Error)
                {
                    return Err(mismatch(&e, &a));
                }
                self.bind(*v, t)
            }
            (Ty::Error, _) | (_, Ty::Error) => Ok(()),
            (Ty::Prim(p), Ty::Prim(q)) if p == q => Ok(()),
            (Ty::Never, Ty::Never) => Ok(()),
            (Ty::Bound(i), Ty::Bound(j)) if i == j => Ok(()),
            // Capability values match only with the same effect and type arguments (spec 8.1).
            (Ty::Cap(x), Ty::Cap(y)) if x.def == y.def && x.args.len() == y.args.len() => {
                self.unify_all(&x.args, &y.args, Variance::Inv)
            }
            // Set type arguments of a named type. Being inside type arguments, inclusion is
            // usually both ways.
            (Ty::Set(k, x), Ty::Set(l, y)) if k == l => {
                self.buffer_rows(*k, x, y, variance);
                Ok(())
            }
            (Ty::Fn(f), Ty::Fn(g)) if f.params.len() == g.params.len() => {
                for (p, q) in f.params.iter().zip(&g.params) {
                    self.unify_inner(p, q, variance.flip())?;
                }
                self.unify_inner(&f.ret, &g.ret, variance)?;
                self.buffer_rows(SetKind::Errors, &f.fails, &g.fails, variance);
                self.buffer_rows(SetKind::Effects, &f.uses, &g.uses, variance);
                Ok(())
            }
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
                self.unify_all(xs, ys, variance)
            }
            // Where a type argument is used is not tracked, so both directions.
            (Ty::Named(c, xs), Ty::Named(d, ys)) if c == d && xs.len() == ys.len() => {
                self.unify_all(xs, ys, Variance::Inv)
            }
            // Values are immutable, so list and tuple elements keep the outer direction.
            (Ty::List(x), Ty::List(y)) => self.unify_inner(x, y, variance),
            _ => Err(mismatch(&e, &a)),
        }
    }

    /// Collects inclusion constraints on a function type's sets. `expected` is the expected side.
    fn buffer_rows(&mut self, kind: SetKind, expected: &Row, actual: &Row, variance: Variance) {
        if expected == actual {
            return;
        }
        let (e, a) = (expected.clone(), actual.clone());
        match variance {
            Variance::Co => self.row_buffer.push((a, e, kind)),
            Variance::Contra => self.row_buffer.push((e, a, kind)),
            Variance::Inv => {
                self.row_buffer.push((a.clone(), e.clone(), kind));
                self.row_buffer.push((e, a, kind));
            }
        }
    }

    fn unify_all(&mut self, xs: &[Ty], ys: &[Ty], variance: Variance) -> Result<(), TypeErrorKind> {
        for (x, y) in xs.iter().zip(ys) {
            self.unify_inner(x, y, variance)?;
        }
        Ok(())
    }

    /// Binds the unbound type variable `v` to `ty`, rejecting an infinite type if `v` occurs in
    /// `ty`.
    fn bind(&mut self, v: TyVar, ty: &Ty) -> Result<(), TypeErrorKind> {
        if self.occurs(v, ty) {
            return Err(TypeErrorKind::InfiniteType {
                var: v,
                ty: ty.clone(),
            });
        }
        self.table.union_value(v, TyVarValue(Some(ty.clone())));
        Ok(())
    }

    fn occurs(&mut self, v: TyVar, ty: &Ty) -> bool {
        match self.shallow_resolve(ty) {
            Ty::Var(w) => self.table.unioned(v, w),
            ty => {
                let mut found = false;
                ty.for_each_child(|child| found = found || self.occurs(v, child));
                found
            }
        }
    }

    /// The unbound type variables (representatives) in the type, in order of first occurrence,
    /// without duplicates.
    pub fn free_vars(&mut self, ty: &Ty) -> Vec<TyVar> {
        let mut out = Vec::new();
        self.collect_free_vars(ty, &mut out);
        out
    }

    fn collect_free_vars(&mut self, ty: &Ty, out: &mut Vec<TyVar>) {
        match self.shallow_resolve(ty) {
            Ty::Var(v) => {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            ty => ty.for_each_child(|child| self.collect_free_vars(child, out)),
        }
    }

    /// Quantifies the type variables of `ty` not in `env_vars` (those occurring in the
    /// environment).
    ///
    /// The caller passes the environment's variables instead of the context tracking levels:
    /// only top-level definitions are generalized, so level-based generalization would buy
    /// nothing.
    ///
    /// Type parameters are named `A`, `B`, ... in order of occurrence, without constraints.
    pub fn generalize(&mut self, ty: &Ty, env_vars: &[TyVar]) -> Scheme {
        self.generalize_with(ty, Vec::new(), env_vars)
    }

    /// Quantifies like [`Self::generalize`], appending the quantified variables after `existing`
    /// (the type parameters written in the declaration, `Ty::Bound(i)` inside `ty`). Names are
    /// chosen not to clash with the written ones.
    ///
    /// Set variables left in the type (after [`Self::resolve`]) are also quantified, as set type
    /// parameters named `E`, `E1`, `E2`, ... for error sets and `R`, `R1`, ... for effect sets,
    /// placed after the type variables.
    pub fn generalize_with(
        &mut self,
        ty: &Ty,
        existing: Vec<TyParam>,
        env_vars: &[TyVar],
    ) -> Scheme {
        self.generalize_bounded(ty, existing, env_vars, &Default::default())
            .0
    }

    /// Quantifies like [`Self::generalize_with`], adding an `Immediate` bound to the parameters
    /// that replace set variables in `immediate` (spec 9.5). Also returns the quantified set
    /// variables and the indices of the parameters replacing them.
    pub(crate) fn generalize_bounded(
        &mut self,
        ty: &Ty,
        existing: Vec<TyParam>,
        env_vars: &[TyVar],
        immediate: &rustc_hash::FxHashSet<RowVar>,
    ) -> (Scheme, Indices<RowVar>, Indices<TyVar>) {
        // If an environment type variable is bound, the type variables it reaches also belong to
        // the environment.
        let mut env = Vec::new();
        for v in env_vars {
            self.collect_free_vars(&Ty::Var(*v), &mut env);
        }
        let mut map = FxHashMap::default();
        let mut params = existing;
        let mut next_name = 0;
        for v in self.free_vars(ty) {
            if !env.contains(&v) {
                map.insert(v, params.len() as u32);
                let name = loop {
                    let name = param_name(next_name);
                    next_name += 1;
                    if !params.iter().any(|p| p.name == name) {
                        break name;
                    }
                };
                params.push(TyParam::new(SmolStr::new(name)));
            }
        }
        let ty = self.resolve(ty);
        let mut row_map = FxHashMap::default();
        let mut generalized = Vec::new();
        let mut next_name = [0, 0];
        for (v, kind) in free_row_vars(&ty) {
            row_map.insert(v, params.len() as u32);
            generalized.push((v, params.len() as u32));
            let (prefix, next_name) = match kind {
                SetKind::Errors => ("E", &mut next_name[0]),
                SetKind::Effects => ("R", &mut next_name[1]),
            };
            let name = loop {
                let name = set_param_name(prefix, *next_name);
                *next_name += 1;
                if !params.iter().any(|p| p.name == name) {
                    break name;
                }
            };
            let mut param = TyParam::set(SmolStr::new(name));
            if immediate.contains(&v) {
                param.bounds.push(SmolStr::new(crate::lower::IMMEDIATE));
            }
            params.push(param);
        }
        let mut tys: Vec<(TyVar, u32)> = map.iter().map(|(&v, &i)| (v, i)).collect();
        tys.sort_by_key(|&(_, i)| i);
        (
            Scheme::new(params, replace_vars(&ty, &map, &row_map)),
            generalized,
            tys,
        )
    }

    /// Replaces each type parameter of the scheme with a fresh type variable (a fresh set
    /// variable for set type parameters).
    pub fn instantiate(&mut self, scheme: &Scheme) -> Ty {
        self.instantiate_with_args(scheme).0
    }

    /// Instantiates like [`Self::instantiate`], also returning the type variables used as type
    /// arguments. Set type parameter positions get an unused type variable, so the list lines up
    /// with the declaration's type parameters.
    pub fn instantiate_with_args(&mut self, scheme: &Scheme) -> (Ty, Vec<Ty>) {
        let (tys, rows) = self.fresh_args(&scheme.params);
        (substitute(&scheme.ty, &tys, &rows), tys)
    }

    /// A fresh type variable and set variable per type parameter. Type parameters get an empty
    /// set; set type parameters get an unused type variable.
    pub(crate) fn fresh_args(&mut self, params: &[TyParam]) -> (Vec<Ty>, Vec<Row>) {
        let mut tys = Vec::new();
        let mut rows = Vec::new();
        for p in params {
            tys.push(self.new_var());
            rows.push(match p.kind {
                ParamKind::Type => Row::empty(),
                ParamKind::Set => Row::var(self.new_row_var()),
            });
        }
        (tys, rows)
    }
}

/// Generalized variables and the indices of the type parameters that replaced them.
pub(crate) type Indices<V> = Vec<(V, u32)>;

/// The set variables in the type and their kinds, in order of first occurrence, without
/// duplicates.
pub(crate) fn free_row_vars(ty: &Ty) -> Vec<(RowVar, SetKind)> {
    let mut out = Vec::new();
    collect_row_vars(ty, &mut out);
    out
}

fn collect_row_vars(ty: &Ty, out: &mut Vec<(RowVar, SetKind)>) {
    let rows: Vec<(&Row, SetKind)> = match ty {
        Ty::Fn(f) => vec![(&f.fails, SetKind::Errors), (&f.uses, SetKind::Effects)],
        Ty::Set(kind, row) => vec![(row, *kind)],
        _ => Vec::new(),
    };
    for (row, kind) in rows {
        for v in row.vars() {
            if !out.iter().any(|(w, _)| *w == v) {
                out.push((v, kind));
            }
        }
    }
    ty.for_each_child(|child| collect_row_vars(child, out));
}

fn replace_vars(ty: &Ty, map: &FxHashMap<TyVar, u32>, rows: &FxHashMap<RowVar, u32>) -> Ty {
    let replace_row = |row: &Row| {
        row.flat_map(|e| match e {
            RowElem::Var(v) => match rows.get(v) {
                Some(i) => Row::from_elems([RowElem::Bound(*i)]),
                None => Row::var(*v),
            },
            RowElem::Of(t) => Row::from_elems([RowElem::Of(replace_vars(t, map, rows))]),
            RowElem::Tag(t) => Row::of_tag(t.map_args(|a| replace_vars(a, map, rows))),
            e => Row::from_elems([e.clone()]),
        })
    };
    match ty {
        Ty::Var(v) => map.get(v).map_or_else(|| ty.clone(), |i| Ty::Bound(*i)),
        Ty::Set(kind, row) => Ty::Set(*kind, replace_row(row)),
        Ty::Fn(f) => Ty::func_sets(
            f.params
                .iter()
                .map(|p| replace_vars(p, map, rows))
                .collect::<Vec<_>>(),
            replace_vars(&f.ret, map, rows),
            replace_row(&f.fails),
            replace_row(&f.uses),
        ),
        _ => ty.map_children(|child| replace_vars(child, map, rows)),
    }
}

/// Replaces `Ty::Bound(i)` with `args[i]`; used to substitute type arguments of named types. Set
/// type parameters (`RowElem::Bound(i)`) are replaced when `args[i]` is a set (`Ty::Set`), and
/// left as is otherwise.
pub fn substitute_bound(ty: &Ty, args: &[Ty]) -> Ty {
    let rows: Vec<Row> = args
        .iter()
        .enumerate()
        .map(|(i, a)| match a {
            Ty::Set(_, row) => row.clone(),
            _ => Row::from_elems([RowElem::Bound(i as u32)]),
        })
        .collect();
    substitute(ty, args, &rows)
}

/// Replaces `Ty::Bound(i)` with `tys[i]` and `RowElem::Bound(i)` in sets with `rows[i]`.
/// Out-of-range indices are left as is.
pub(crate) fn substitute(ty: &Ty, tys: &[Ty], rows: &[Row]) -> Ty {
    match ty {
        Ty::Bound(i) => tys.get(*i as usize).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Fn(f) => Ty::func_sets(
            f.params
                .iter()
                .map(|p| substitute(p, tys, rows))
                .collect::<Vec<_>>(),
            substitute(&f.ret, tys, rows),
            substitute_row(&f.fails, tys, rows),
            substitute_row(&f.uses, tys, rows),
        ),
        Ty::Set(kind, row) => Ty::Set(*kind, substitute_row(row, tys, rows)),
        _ => ty.map_children(|child| substitute(child, tys, rows)),
    }
}

/// Replaces type parameters inside a set like [`substitute`].
pub(crate) fn substitute_row(row: &Row, tys: &[Ty], rows: &[Row]) -> Row {
    row.flat_map(|e| match e {
        RowElem::Bound(i) => rows
            .get(*i as usize)
            .cloned()
            .unwrap_or_else(|| Row::from_elems([e.clone()])),
        RowElem::Of(t) => Row::from_elems([RowElem::Of(substitute(t, tys, rows))]),
        RowElem::Tag(t) => Row::of_tag(t.map_args(|a| substitute(a, tys, rows))),
        e => Row::from_elems([e.clone()]),
    })
}
