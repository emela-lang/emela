//! The impls of the Prelude's Eq, Ord and Show for the primitive types and tuples, which the
//! compiler provides (spec 10.6). `List` and `Option` have theirs in the core sources.
//!
//! They are added to `Module::dispatch` like impls written in source, once every function is
//! lowered: a tuple impl is made for each length of tuple in the module's types. Monomorphization
//! only makes types from these by substitution, so no other length can reach a trait function.
//!
//! - Eq is `==` on the primitive types (IEEE 754 for Float), and element by element on tuples.
//! - Ord is `compare` from `<` and `==` on Int, Int64 and String, `False < True` on Bool, and
//!   lexicographic on tuples. Float has none.
//! - Show is the show of interpolation on the primitive types, the String itself for a String,
//!   and `(1, "a")` on tuples, with each element shown through its own Show and a String element
//!   quoted (`Builtin::ShowNested`).

use std::collections::BTreeSet;

use crate::intrinsics::Builtin;
use crate::ir::{
    Arm, BinOp, Callee, CtorRef, Expr, FnId, Lit, Local, Module, OpTy, Pat, PreludeTraits, StrPart,
    Type, TypeId,
};

/// The primitive types with their operand types and names. Float is not Ord.
const PRIMS: &[(TypeId, OpTy, &str)] = &[
    (TypeId::INT, OpTy::Int, "Int"),
    (TypeId::INT64, OpTy::Int64, "Int64"),
    (TypeId::FLOAT, OpTy::Float, "Float"),
    (TypeId::BOOL, OpTy::Bool, "Bool"),
    (TypeId::STRING, OpTy::String, "String"),
];

/// Adds the built-in impls of the lowered trait functions to `Module::dispatch`.
pub(super) fn add(m: &mut Module, fns: &PreludeTraits) {
    let lengths: BTreeSet<usize> = m
        .types
        .iter()
        .filter_map(|t| match t {
            Type::Tuple(ts) => Some(ts.len()),
            _ => None,
        })
        .collect();
    let mut b = Builder { m };
    if let Some(eq) = fns.eq {
        for &(ty, op, name) in PRIMS {
            let f = b.prim_eq(ty, op, name);
            b.register(eq, ty, f);
        }
        let f = b.unit(TypeId::BOOL, "Eq.eq", Expr::Lit(Lit::Bool(true)));
        b.register(eq, TypeId::UNIT, f);
        for &n in &lengths {
            let (ty, f) = b.tuple_eq(eq, n);
            b.register(eq, ty, f);
        }
    }
    if let Some(compare) = fns.compare {
        let ordering = b.m.functions[compare].ret;
        for &(ty, op, name) in PRIMS {
            if ty != TypeId::FLOAT {
                let f = b.prim_compare(ordering, ty, op, name);
                b.register(compare, ty, f);
            }
        }
        let equal = b.ordering(ordering, "Equal");
        let f = b.unit(ordering, "Ord.compare", equal);
        b.register(compare, TypeId::UNIT, f);
        for &n in &lengths {
            let (ty, f) = b.tuple_compare(compare, ordering, n);
            b.register(compare, ty, f);
        }
    }
    if let Some(show) = fns.show {
        for &(ty, _, name) in PRIMS {
            let f = b.prim_show(ty, name);
            b.register(show, ty, f);
        }
        let f = b.function(
            &["x"],
            TypeId::UNIT,
            TypeId::STRING,
            0,
            "(): Show.show",
            |_, _| Expr::Lit(Lit::String("()".to_owned())),
        );
        b.register(show, TypeId::UNIT, f);
        for &n in &lengths {
            let (ty, f) = b.tuple_show(n);
            b.register(show, ty, f);
        }
    }
}

struct Builder<'a> {
    m: &'a mut Module,
}

impl Builder<'_> {
    fn register(&mut self, trait_fn: FnId, ty: TypeId, f: FnId) {
        if let Some(d) = self.m.dispatch.get_mut(&trait_fn) {
            d.impls.push((ty, f));
            d.structural.push(f);
        }
    }

    /// A function with parameters named `params`, all of type `ty`, and the body `body` makes
    /// from them. The name is made unique (a user type may be named `Int`).
    fn function(
        &mut self,
        params: &[&str],
        ty: TypeId,
        ret: TypeId,
        type_params: usize,
        name: &str,
        body: impl FnOnce(&mut Self, &[Local]) -> Expr,
    ) -> FnId {
        let mut unique = name.to_owned();
        let mut i = 2;
        while self.m.functions.iter().any(|(_, f)| f.name == unique) {
            unique = format!("{name}#{i}");
            i += 1;
        }
        let f = self.m.declare(&unique, false);
        let params: Vec<Local> = params.iter().map(|p| self.m.local(p, ty)).collect();
        let body = body(self, &params);
        self.m.define(f, params, ret, body);
        self.m.functions[f].type_params = type_params;
        f
    }

    /// An impl of Eq or Ord for `()`, whose body does not look at its arguments.
    fn unit(&mut self, ret: TypeId, trait_fn: &str, body: Expr) -> FnId {
        self.function(
            &["self", "other"],
            TypeId::UNIT,
            ret,
            0,
            &format!("(): {trait_fn}"),
            |_, _| body,
        )
    }

    fn ordering(&mut self, ordering: TypeId, variant: &str) -> Expr {
        let Type::Enum(enum_id, _) = self.m.types[ordering] else {
            unreachable!("Ord.compare returns Ordering");
        };
        let variant = self.m.enums[enum_id]
            .variants
            .iter()
            .position(|v| v.name == variant)
            .expect("a variant of Ordering");
        Expr::Ctor {
            ctor: CtorRef { enum_id, variant },
            ty_args: Vec::new(),
            args: Vec::new(),
        }
    }

    fn prim_eq(&mut self, ty: TypeId, op: OpTy, name: &str) -> FnId {
        self.function(
            &["self", "other"],
            ty,
            TypeId::BOOL,
            0,
            &format!("{name}: Eq.eq"),
            |_, p| binary(BinOp::Eq, op, Expr::Var(p[0]), Expr::Var(p[1])),
        )
    }

    /// `if a == b { Equal } else if a < b { Less } else { Greater }`; for Bool, `b` instead of
    /// `a < b` (`False < True`).
    fn prim_compare(&mut self, ordering: TypeId, ty: TypeId, op: OpTy, name: &str) -> FnId {
        self.function(
            &["self", "other"],
            ty,
            ordering,
            0,
            &format!("{name}: Ord.compare"),
            |b, p| {
                let (a, c) = (Expr::Var(p[0]), Expr::Var(p[1]));
                let less = if op == OpTy::Bool {
                    c.clone()
                } else {
                    binary(BinOp::Lt, op, a.clone(), c.clone())
                };
                let (eq, lt, gt) = (
                    b.ordering(ordering, "Equal"),
                    b.ordering(ordering, "Less"),
                    b.ordering(ordering, "Greater"),
                );
                if_(binary(BinOp::Eq, op, a, c), eq, if_(less, lt, gt))
            },
        )
    }

    fn prim_show(&mut self, ty: TypeId, name: &str) -> FnId {
        self.function(
            &["self"],
            ty,
            TypeId::STRING,
            0,
            &format!("{name}: Show.show"),
            |_, p| {
                if ty == TypeId::STRING {
                    Expr::Var(p[0])
                } else {
                    Expr::Concat(vec![StrPart::Value(Expr::Var(p[0]), ty)])
                }
            },
        )
    }

    /// The tuple type of length `n` with the impl's type parameters (`($0, $1)`).
    fn tuple(&mut self, n: usize) -> (TypeId, Vec<TypeId>) {
        let elems: Vec<TypeId> = (0..n).map(|i| self.m.types.type_param(i)).collect();
        (self.m.tuple(elems.clone()), elems)
    }

    fn tuple_name(n: usize) -> String {
        let names: Vec<String> = (0..n).map(param_name).collect();
        format!("({})", names.join(", "))
    }

    /// Binds the elements of `value` (a tuple of `elems`) and evaluates `body` with them.
    fn destructure(
        &mut self,
        value: Local,
        elems: &[TypeId],
        hint: &str,
        body: impl FnOnce(&mut Self, &[Local]) -> Expr,
    ) -> Expr {
        let locals: Vec<Local> = elems
            .iter()
            .enumerate()
            .map(|(i, &t)| self.m.local(&format!("{hint}{i}"), t))
            .collect();
        let body = body(self, &locals);
        Expr::Match {
            scrutinee: Box::new(Expr::Var(value)),
            arms: vec![Arm {
                pat: Pat::Tuple(locals.iter().map(|&l| Pat::Bind(l)).collect()),
                guard: None,
                body,
            }],
        }
    }

    /// `Eq.eq(a0, b0) && Eq.eq(a1, b1) && ...`.
    fn tuple_eq(&mut self, eq: FnId, n: usize) -> (TypeId, FnId) {
        let (ty, elems) = self.tuple(n);
        let name = format!("{}: Eq.eq", Self::tuple_name(n));
        let f = self.function(&["self", "other"], ty, TypeId::BOOL, n, &name, |b, p| {
            let (x, y) = (p[0], p[1]);
            b.destructure(x, &elems, "a", |b, xs| {
                b.destructure(y, &elems, "b", |b, ys| {
                    let mut out: Option<Expr> = None;
                    for i in (0..n).rev() {
                        let c = b.call(eq, elems[i], vec![Expr::Var(xs[i]), Expr::Var(ys[i])]);
                        out = Some(match out {
                            None => c,
                            Some(rest) => binary(BinOp::And, OpTy::Bool, c, rest),
                        });
                    }
                    out.unwrap_or(Expr::Lit(Lit::Bool(true)))
                })
            })
        });
        (ty, f)
    }

    /// `Ord.compare(a0, b0) match { Equal -> Ord.compare(a1, b1) ..., order -> order }`.
    fn tuple_compare(&mut self, compare: FnId, ordering: TypeId, n: usize) -> (TypeId, FnId) {
        let (ty, elems) = self.tuple(n);
        let name = format!("{}: Ord.compare", Self::tuple_name(n));
        let f = self.function(&["self", "other"], ty, ordering, n, &name, |b, p| {
            let (x, y) = (p[0], p[1]);
            b.destructure(x, &elems, "a", |b, xs| {
                b.destructure(y, &elems, "b", |b, ys| {
                    let mut out: Option<Expr> = None;
                    for i in (0..n).rev() {
                        let c = b.call(compare, elems[i], vec![Expr::Var(xs[i]), Expr::Var(ys[i])]);
                        out = Some(match out {
                            None => c,
                            Some(rest) => {
                                let Expr::Ctor { ctor, .. } = b.ordering(ordering, "Equal") else {
                                    unreachable!()
                                };
                                let order = b.m.local("order", ordering);
                                Expr::Match {
                                    scrutinee: Box::new(c),
                                    arms: vec![
                                        Arm {
                                            pat: Pat::Ctor {
                                                ctor,
                                                fields: Vec::new(),
                                            },
                                            guard: None,
                                            body: rest,
                                        },
                                        Arm {
                                            pat: Pat::Bind(order),
                                            guard: None,
                                            body: Expr::Var(order),
                                        },
                                    ],
                                }
                            }
                        });
                    }
                    out.expect("a tuple has two or more elements")
                })
            })
        });
        (ty, f)
    }

    /// `"(#{a0}, #{a1})"`, each element shown as inside a value (`Builtin::ShowNested`).
    fn tuple_show(&mut self, n: usize) -> (TypeId, FnId) {
        let (ty, elems) = self.tuple(n);
        let name = format!("{}: Show.show", Self::tuple_name(n));
        let f = self.function(&["self"], ty, TypeId::STRING, n, &name, |b, p| {
            b.destructure(p[0], &elems, "a", |_, xs| {
                let mut parts = vec![StrPart::Lit("(".to_owned())];
                for (i, (&x, &t)) in xs.iter().zip(&elems).enumerate() {
                    if i > 0 {
                        parts.push(StrPart::Lit(", ".to_owned()));
                    }
                    let shown = Expr::Builtin {
                        op: Builtin::ShowNested,
                        ty_args: vec![t],
                        args: vec![Expr::Var(x)],
                    };
                    parts.push(StrPart::Value(shown, TypeId::STRING));
                }
                parts.push(StrPart::Lit(")".to_owned()));
                Expr::Concat(parts)
            })
        });
        (ty, f)
    }

    /// A call of the trait function `f` with `Self` = `ty`. The Prelude's trait functions are
    /// pure: they neither fail nor suspend, and take no evidence table.
    fn call(&self, f: FnId, ty: TypeId, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Callee::Direct {
                f,
                ty_args: vec![ty],
            },
            args,
            fails: false,
            suspends: false,
        }
    }
}

fn param_name(i: usize) -> String {
    char::from(b'A' + (i % 26) as u8).to_string()
}

fn binary(op: BinOp, ty: OpTy, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Binary {
        op,
        ty,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn if_(cond: Expr, then: Expr, else_: Expr) -> Expr {
    Expr::If {
        cond: Box::new(cond),
        then: Box::new(then),
        else_: Box::new(else_),
    }
}
