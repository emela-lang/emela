//! Connects the exhaustiveness check ([`crate::exhaustiveness`]) to the results of type
//! inference.
//!
//! Converts HIR patterns into the checker's patterns and implements the table that looks up
//! constructor shapes from types. The check runs after inference of a definition group has
//! settled the types ([`crate::body`]).

use emela_resolve::hir::{
    Arg, Arm, DefId, ExprId, ExprKind, Item, Literal, PatId, PatKind, Program, Res, UnaryOp,
    int_literal_value,
};
use smol_str::SmolStr;

use crate::exhaustiveness::{
    self, CtorTable, Lit, MatchCheck, Pat, TyShape, VariantShape, check_match,
};
use crate::infer::substitute_bound;
use crate::lower::TyTable;
use crate::signature::Signatures;
use crate::ty::{Prim, Ty};

/// Types are passed with unification bindings followed.
pub(crate) struct Shapes<'a> {
    pub(crate) program: &'a Program,
    pub(crate) ty_table: &'a TyTable,
    pub(crate) sigs: &'a Signatures,
}

impl CtorTable for Shapes<'_> {
    type Ty = Ty;

    fn shape(&self, ty: &Ty) -> TyShape<Ty> {
        match ty {
            Ty::Prim(Prim::Bool) => match self.program.prelude_def("Bool") {
                Some(bool_def) => TyShape::Enum(self.variants(bool_def, &[])),
                None => TyShape::Opaque,
            },
            Ty::Prim(Prim::Unit) => TyShape::Tuple(Vec::new()),
            Ty::Tuple(elems) => TyShape::Tuple(elems.clone()),
            Ty::List(elem) => TyShape::List((**elem).clone()),
            Ty::Named(id, args) => {
                let def = self.ty_table.def(*id);
                match self.program.item(def) {
                    Some(Item::Enum(_)) => TyShape::Enum(self.variants(def, args)),
                    Some(Item::Type(item)) => match item.ctor {
                        Some(ctor) => TyShape::Enum(vec![self.variant(ctor, args)]),
                        None => TyShape::Opaque,
                    },
                    Some(Item::Error(_)) => TyShape::Enum(vec![self.variant(def, args)]),
                    Some(Item::Handler(item)) => match item.ctor {
                        Some(ctor) => TyShape::Enum(vec![self.variant(ctor, args)]),
                        None => TyShape::Opaque,
                    },
                    _ => TyShape::Opaque,
                }
            }
            _ => TyShape::Opaque,
        }
    }
}

impl Shapes<'_> {
    /// Checks the arms (of `match` or `escape`) applied to a value of type `ty`.
    pub(crate) fn check_arms(&self, ty: &Ty, arms: &[Arm]) -> MatchCheck {
        let arms: Vec<exhaustiveness::Arm> = arms
            .iter()
            .map(|arm| self.arm(arm.pat, ty, arm.guard.is_some()))
            .collect();
        check_match(self, ty, &arms)
    }

    /// The checker's arm for the pattern `pat` applied to a value of type `ty`. An arm whose
    /// pattern has a const that is not made only of literals and constructors is treated like a
    /// guarded arm (spec 5.6, 6.5): such a const stands for any value of its type, and the arm
    /// is neither counted for exhaustiveness nor covers the arms below it.
    pub(crate) fn arm(&self, pat: PatId, ty: &Ty, guarded: bool) -> exhaustiveness::Arm {
        let mut computed = false;
        let pat = self.pat(pat, ty, &mut computed);
        exhaustiveness::Arm {
            pat,
            guarded: guarded || computed,
        }
    }

    /// In declaration order.
    fn variants(&self, enum_def: DefId, args: &[Ty]) -> Vec<VariantShape<Ty>> {
        match self.program.item(enum_def) {
            Some(Item::Enum(item)) => item
                .variants
                .iter()
                .map(|&v| self.variant(v, args))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Field types come from the constructor's declared type, with type arguments substituted.
    fn variant(&self, ctor: DefId, args: &[Ty]) -> VariantShape<Ty> {
        let fields = match self.sigs.get(ctor).map(|sig| &sig.scheme.ty) {
            Some(Ty::Fn(f)) => f.params.iter().map(|p| substitute_bound(p, args)).collect(),
            _ => Vec::new(),
        };
        VariantShape {
            name: self.program.defs[ctor].name.clone(),
            fields,
        }
    }

    /// Parts that do not fit the type (the type checker has reported them) become wildcards so
    /// that the check does not break down. A const whose value cannot be known
    /// ([`Shapes::const_pat`]) becomes a wildcard and sets `computed`.
    fn pat(&self, pat: PatId, ty: &Ty, computed: &mut bool) -> Pat {
        let program = self.program;
        let shape = self.shape(ty);
        match (&program.pats[pat].kind, shape) {
            (PatKind::Wild | PatKind::Bind(_) | PatKind::Missing, _) => Pat::Wild,
            (PatKind::Lit(lit), TyShape::Opaque) => Pat::Lit(lit_value(lit, false)),
            (PatKind::Const(Res::Def(def)), _) => match self.const_pat(*def, ty, &mut Vec::new()) {
                Some(p) => p,
                None => {
                    *computed = true;
                    Pat::Wild
                }
            },
            (PatKind::Unit, TyShape::Tuple(tys)) if tys.is_empty() => Pat::Tuple(Vec::new()),
            (PatKind::Tuple(elems), TyShape::Tuple(tys)) if elems.len() == tys.len() => Pat::Tuple(
                elems
                    .iter()
                    .zip(&tys)
                    .map(|(&p, ty)| self.pat(p, ty, computed))
                    .collect(),
            ),
            (PatKind::List { elems, rest }, TyShape::List(elem_ty)) => Pat::List {
                prefix: elems
                    .iter()
                    .map(|&p| self.pat(p, &elem_ty, computed))
                    .collect(),
                rest: rest.is_some(),
            },
            (
                PatKind::Ctor {
                    res: Res::Def(ctor),
                    args,
                    ..
                },
                TyShape::Enum(variants),
            ) => {
                let Some(variant) = self.variant_index(*ctor, ty) else {
                    return Pat::Wild;
                };
                let field_tys = &variants[variant].fields;
                let labels = self.sigs.get(*ctor).map(|sig| sig.labels.as_slice());
                let mut fields = Vec::new();
                let mut next_positional = 0;
                for arg in args.iter().flatten() {
                    let position = match &arg.label {
                        None => {
                            let i = next_positional;
                            next_positional += 1;
                            Some(i)
                        }
                        Some(label) => labels.and_then(|labels| {
                            labels.iter().position(|l| l.as_ref() == Some(&label.name))
                        }),
                    };
                    // Fields that could not be placed have been reported by the type checker.
                    if let Some(i) = position
                        && let Some(field_ty) = field_tys.get(i)
                    {
                        fields.push((i, self.pat(arg.pat, field_ty, computed)));
                    }
                }
                Pat::Variant { variant, fields }
            }
            _ => Pat::Wild,
        }
    }

    /// The type definition (type, enum, error, or the Bool enum).
    fn ty_def(&self, ty: &Ty) -> Option<DefId> {
        match ty {
            Ty::Named(id, _) => Some(self.ty_table.def(*id)),
            Ty::Prim(Prim::Bool) => self.program.prelude_def("Bool"),
            _ => None,
        }
    }

    /// `None` if `ctor` does not construct `ty`.
    fn variant_index(&self, ctor: DefId, ty: &Ty) -> Option<usize> {
        let ty_def = self.ty_def(ty)?;
        match self.program.item(ctor) {
            // An error is its own constructor.
            Some(Item::Error(_)) => (ctor == ty_def).then_some(0),
            Some(Item::Ctor(item)) if item.owner == ty_def => match self.program.item(ty_def) {
                Some(Item::Enum(e)) => e.variants.iter().position(|&v| v == ctor),
                _ => Some(0),
            },
            _ => None,
        }
    }

    /// The value of the const `def` as a pattern for a value of type `ty`, if its value is made
    /// only of literals, negated numeric literals, constructors, tuples, lists and other such
    /// consts (spec 5.6). `None` otherwise, and for a const that refers to itself (`seen` holds the
    /// consts being expanded; E0231 has been reported).
    fn const_pat(&self, def: DefId, ty: &Ty, seen: &mut Vec<DefId>) -> Option<Pat> {
        let Some(Item::Const(item)) = self.program.item(def) else {
            return None;
        };
        if seen.contains(&def) {
            return None;
        }
        seen.push(def);
        let pat = self.value_pat(item.value, ty, seen);
        seen.pop();
        pat
    }

    /// The value of the expression `expr` as a pattern ([`Shapes::const_pat`]).
    fn value_pat(&self, expr: ExprId, ty: &Ty, seen: &mut Vec<DefId>) -> Option<Pat> {
        let program = self.program;
        match (&program.exprs[expr].kind, self.shape(ty)) {
            (ExprKind::Lit(lit), TyShape::Opaque) => Some(Pat::Lit(lit_value(lit, false))),
            (
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand,
                },
                TyShape::Opaque,
            ) => match &program.exprs[*operand].kind {
                ExprKind::Lit(lit @ (Literal::Int(_) | Literal::Float(_))) => {
                    Some(Pat::Lit(lit_value(lit, true)))
                }
                _ => None,
            },
            (ExprKind::Unit, TyShape::Tuple(tys)) if tys.is_empty() => Some(Pat::Tuple(Vec::new())),
            (ExprKind::Tuple(elems), TyShape::Tuple(tys)) if elems.len() == tys.len() => elems
                .iter()
                .zip(&tys)
                .map(|(&e, ty)| self.value_pat(e, ty, seen))
                .collect::<Option<_>>()
                .map(Pat::Tuple),
            (ExprKind::List { elems, rest }, TyShape::List(elem_ty)) => {
                let mut prefix: Vec<Pat> = elems
                    .iter()
                    .map(|&e| self.value_pat(e, &elem_ty, seen))
                    .collect::<Option<_>>()?;
                // `[x, ..REST]` with a known list `REST`.
                if let Some(rest) = rest {
                    match self.value_pat(*rest, ty, seen)? {
                        Pat::List {
                            prefix: tail,
                            rest: false,
                        } => prefix.extend(tail),
                        _ => return None,
                    }
                }
                Some(Pat::List {
                    prefix,
                    rest: false,
                })
            }
            (ExprKind::Path(Res::Def(def)), _) => match program.item(*def) {
                Some(Item::Const(_)) => self.const_pat(*def, ty, seen),
                _ => self.ctor_pat(*def, &[], ty, seen),
            },
            (ExprKind::Call { callee, args }, _) => match &program.exprs[*callee].kind {
                ExprKind::Path(Res::Def(ctor)) => self.ctor_pat(*ctor, args, ty, seen),
                _ => None,
            },
            _ => None,
        }
    }

    /// The constructor `ctor` applied to `args` as a pattern. `None` unless `ctor` constructs
    /// `ty` and every field is given a known value.
    fn ctor_pat(&self, ctor: DefId, args: &[Arg], ty: &Ty, seen: &mut Vec<DefId>) -> Option<Pat> {
        let TyShape::Enum(variants) = self.shape(ty) else {
            return None;
        };
        let variant = self.variant_index(ctor, ty)?;
        let field_tys = &variants[variant].fields;
        if args.len() != field_tys.len() {
            return None;
        }
        let labels = self.sigs.get(ctor).map(|sig| sig.labels.as_slice());
        let mut fields: Vec<(usize, Pat)> = Vec::new();
        let mut next_positional = 0;
        for arg in args {
            let i = match &arg.label {
                None => {
                    let i = next_positional;
                    next_positional += 1;
                    i
                }
                Some(label) => labels?
                    .iter()
                    .position(|l| l.as_ref() == Some(&label.name))?,
            };
            if fields.iter().any(|(j, _)| *j == i) {
                return None;
            }
            fields.push((i, self.value_pat(arg.value, field_tys.get(i)?, seen)?));
        }
        Some(Pat::Variant { variant, fields })
    }
}

/// Out-of-range integers (reported by the type checker) that still fit in i64 are compared as
/// values, as a positive `0xFFFF_FFFF` written for an Int is, so two arms with the same such value
/// are still redundant. Only those beyond i64 become values that overlap nothing.
///
/// `negative` is for a const's unary minus; a pattern's literal carries its own `-` in its text
/// (spec 6.5). The value is read with `int_literal_value`, the same function the range check and
/// lowering use, so the three agree.
fn lit_value(lit: &Literal, negative: bool) -> Lit {
    match lit {
        Literal::Int(text) => {
            let n = int_literal_value(text).map(|n| if negative { -n } else { n });
            match n.and_then(|n| i64::try_from(n).ok()) {
                Some(n) => Lit::Int(n),
                None => Lit::Str(SmolStr::new(format!("\u{0}int {text}"))),
            }
        }
        Literal::Float(text) => match text.parse::<f64>() {
            Ok(x) => Lit::Float(if negative { -x } else { x }),
            Err(_) => Lit::Str(SmolStr::new(format!("\u{0}float {text}"))),
        },
        Literal::String(s) => Lit::Str(SmolStr::new(s)),
    }
}
