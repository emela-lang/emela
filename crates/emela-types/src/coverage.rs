//! Connects the exhaustiveness check ([`crate::exhaustiveness`]) to the results of type
//! inference.
//!
//! Converts HIR patterns into the checker's patterns and implements the table that looks up
//! constructor shapes from types. The check runs after inference of a definition group has
//! settled the types ([`crate::body`]).

use emela_resolve::hir::{
    Arm, DefId, ExprKind, Item, Literal, PatId, PatKind, Program, Res, UnaryOp,
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
            .map(|arm| exhaustiveness::Arm {
                pat: self.pat(arm.pat, ty),
                guarded: arm.guard.is_some(),
            })
            .collect();
        check_match(self, ty, &arms)
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
    /// that the check does not break down.
    pub(crate) fn pat(&self, pat: PatId, ty: &Ty) -> Pat {
        let program = self.program;
        let shape = self.shape(ty);
        match (&program.pats[pat].kind, shape) {
            (PatKind::Wild | PatKind::Bind(_) | PatKind::Missing, _) => Pat::Wild,
            (PatKind::Lit(lit), TyShape::Opaque) => Pat::Lit(lit_value(lit, false)),
            (PatKind::Const(Res::Def(def)), TyShape::Opaque) => Pat::Lit(self.const_value(*def)),
            (PatKind::Unit, TyShape::Tuple(tys)) if tys.is_empty() => Pat::Tuple(Vec::new()),
            (PatKind::Tuple(elems), TyShape::Tuple(tys)) if elems.len() == tys.len() => Pat::Tuple(
                elems
                    .iter()
                    .zip(&tys)
                    .map(|(&p, ty)| self.pat(p, ty))
                    .collect(),
            ),
            (PatKind::List { elems, rest }, TyShape::List(elem_ty)) => Pat::List {
                prefix: elems.iter().map(|&p| self.pat(p, &elem_ty)).collect(),
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
                        fields.push((i, self.pat(arg.pat, field_ty)));
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

    /// The value of a const pattern: the literal's value if it is a literal (or a negative
    /// numeric literal). Other expressions cannot be evaluated at compile time yet, so they
    /// become a value that matches only that const.
    fn const_value(&self, def: DefId) -> Lit {
        let program = self.program;
        if let Some(Item::Const(item)) = program.item(def) {
            match &program.exprs[item.value].kind {
                ExprKind::Lit(lit) => return lit_value(lit, false),
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand,
                } => {
                    if let ExprKind::Lit(lit @ (Literal::Int(_) | Literal::Float(_))) =
                        &program.exprs[*operand].kind
                    {
                        return lit_value(lit, true);
                    }
                }
                _ => {}
            }
        }
        // A value distinct from every literal: it starts with a character that cannot occur in
        // a string value.
        Lit::Str(SmolStr::new(format!("\u{0}const {}", def.into_raw())))
    }
}

/// Out-of-range integers (reported by the type checker) become values that overlap nothing.
fn lit_value(lit: &Literal, negative: bool) -> Lit {
    match lit {
        Literal::Int(text) => {
            let (digits, radix) = if let Some(hex) = text.strip_prefix("0x") {
                (hex, 16)
            } else if let Some(bin) = text.strip_prefix("0b") {
                (bin, 2)
            } else {
                (text.as_str(), 10)
            };
            match i64::from_str_radix(digits, radix) {
                Ok(n) => Lit::Int(if negative { -n } else { n }),
                Err(_) => Lit::Str(SmolStr::new(format!("\u{0}int {text}"))),
            }
        }
        Literal::Float(text) => match text.parse::<f64>() {
            Ok(x) => Lit::Float(if negative { -x } else { x }),
            Err(_) => Lit::Str(SmolStr::new(format!("\u{0}float {text}"))),
        },
        Literal::String(s) => Lit::Str(SmolStr::new(s)),
    }
}
