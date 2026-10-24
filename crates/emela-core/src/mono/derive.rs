//! Derived impls (spec 10.5): the impl functions of `derive Eq, Ord, Show`, generated per type
//! once monomorphization knows it.
//!
//! - Eq and Show of a type that reaches no written impl of the trait (only derived impls and the
//!   built-in types) compare and show the value's structure as a whole (`Expr::Binary` with
//!   `OpTy::Structural`, `StrPart::Value`): the result is the same (spec 10.6), and backends
//!   already do it.
//! - Otherwise, and always for Ord, the function goes field by field. Basic types are compared
//!   and shown on the spot; tuples are taken apart; other types go through the trait function,
//!   which picks their impl (written or derived) like any call.
//! - Derived Ord orders constructors in declaration order, then fields from the first.
//! - Derived Show writes the value as constructed in source (`Circle(radius: 1.0)`): a String
//!   field is quoted (`Builtin::ShowString`).

use super::{Mono, Subst, Target};
use crate::Builtin;
use crate::dump::type_name;
use crate::ir::*;

/// A derived impl function waiting for its body.
pub(super) struct DerivedSpec {
    /// The trait function (`Eq.eq`), whose `Module::dispatch` lists the impls.
    trait_fn: FnId,
    op: DeriveOp,
    /// `Self`, an enum.
    ty: TypeId,
    new: FnId,
}

impl Mono<'_> {
    /// The impl function that `derive` generates for trait function `f` and `Self` = `args[0]`,
    /// if `Self` is an enum deriving the trait. Created on first use; the body comes later
    /// ([`Mono::define_derived`]).
    pub(super) fn derived_target(&mut self, f: FnId, args: &[TypeId]) -> Option<Target> {
        let derived = self.m.dispatch.get(&f)?.derived.as_ref()?;
        let ty = *args.first()?;
        let Type::Enum(id, _) = self.out.ty(ty) else {
            return None;
        };
        if !derived.enums.contains(id) {
            return None;
        }
        let op = derived.op;
        let names: &[&str] = match op {
            DeriveOp::Show => &["self"],
            DeriveOp::Eq | DeriveOp::Compare => &["self", "other"],
        };
        let params = names.iter().map(|n| self.out.local(n, ty)).collect();
        let ret = match op {
            DeriveOp::Eq => TypeId::BOOL,
            DeriveOp::Show => TypeId::STRING,
            // Ordering, which has no type parameters.
            DeriveOp::Compare => self.m.functions[f].ret,
        };
        // Named like a written impl's function (`Point: Eq.eq`).
        let ty_name = type_name(&self.out, &self.out.types, ty);
        let name = self.fresh_name(format!("{ty_name}: {}", self.m.functions[f].name));
        let new = self.out.functions.alloc(Function {
            name,
            exported: false,
            host_exports: Vec::new(),
            params,
            ret,
            type_params: 0,
            set_params: Vec::new(),
            fails: false,
            body: Expr::Lit(Lit::Unit),
            external: None,
            suspends: false,
            constant: false,
            sets: None,
        });
        self.origins.insert(new, f);
        self.derived_work.push(DerivedSpec {
            trait_fn: f,
            op,
            ty,
            new,
        });
        Some(Target {
            f: new,
            drops_ev: false,
            fails: false,
            suspends: false,
        })
    }

    /// `name`, or with `#2`, `#3`, ... if taken.
    pub(super) fn fresh_name(&mut self, name: String) -> String {
        if self.names.insert(name.clone()) {
            return name;
        }
        let mut i = 2;
        while !self.names.insert(format!("{name}#{i}")) {
            i += 1;
        }
        format!("{name}#{i}")
    }

    /// Defines the body of a derived impl function.
    pub(super) fn define_derived(&mut self, d: DerivedSpec) {
        let params = self.out.functions[d.new].params.clone();
        let body = match d.op {
            DeriveOp::Eq if self.structural(d.trait_fn, d.ty, &mut Vec::new()) => Expr::Binary {
                op: BinOp::Eq,
                ty: OpTy::Structural,
                lhs: Box::new(Expr::Var(params[0])),
                rhs: Box::new(Expr::Var(params[1])),
            },
            DeriveOp::Eq => self.eq_enum(d.trait_fn, d.ty, params[0], params[1]),
            DeriveOp::Show if self.structural(d.trait_fn, d.ty, &mut Vec::new()) => {
                Expr::Concat(vec![StrPart::Value(Expr::Var(params[0]), d.ty)])
            }
            DeriveOp::Show => self.show_enum(d.trait_fn, d.ty, params[0]),
            DeriveOp::Compare => {
                let ret = self.out.functions[d.new].ret;
                let Type::Enum(ordering, _) = *self.out.ty(ret) else {
                    unreachable!("Ord.compare returns Ordering");
                };
                let cx = Cmp {
                    f: d.trait_fn,
                    ordering,
                };
                self.compare_enum(&cx, d.ty, params[0], params[1])
            }
        };
        self.out.functions[d.new].body = body;
    }

    /// A field's type (of an enum definition, with its type parameters) for the type arguments
    /// `args`.
    fn field_ty(&mut self, field: TypeId, args: &[TypeId]) -> TypeId {
        subst_type(&mut Subst { mono: self, args }, field, args)
    }

    /// The constructors of the enum `ty` with their field types for its type arguments.
    pub(super) fn variants(&mut self, ty: TypeId) -> (EnumId, Vec<Vec<TypeId>>) {
        let Type::Enum(id, args) = self.out.ty(ty).clone() else {
            unreachable!("derived impls are for enums");
        };
        let fields: Vec<Vec<TypeId>> = self.out.enums[id]
            .variants
            .iter()
            .map(|v| v.tys.clone())
            .collect();
        let fields = fields
            .into_iter()
            .map(|tys| tys.into_iter().map(|t| self.field_ty(t, &args)).collect())
            .collect();
        (id, fields)
    }

    /// A call of the trait function `f` for `Self` = `ty`: its impl, written or derived.
    fn call_trait(&mut self, f: FnId, ty: TypeId, args: Vec<Expr>) -> Expr {
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

    /// New locals for the fields `tys`.
    pub(super) fn binds(&mut self, hint: &str, tys: &[TypeId]) -> Vec<Local> {
        tys.iter().map(|&t| self.out.local(hint, t)).collect()
    }

    // ---- Eq ----

    /// Same constructor, and equal fields.
    fn eq_enum(&mut self, f: FnId, ty: TypeId, a: Local, b: Local) -> Expr {
        let (id, variants) = self.variants(ty);
        let many = variants.len() > 1;
        let mut arms = Vec::with_capacity(variants.len());
        for (k, tys) in variants.into_iter().enumerate() {
            let ctor = CtorRef {
                enum_id: id,
                variant: k,
            };
            let xs = self.binds("x", &tys);
            let ys = self.binds("y", &tys);
            let eqs: Vec<Expr> = tys
                .iter()
                .zip(xs.iter().zip(&ys))
                .map(|(&t, (&x, &y))| self.eq_val(f, t, x, y))
                .collect();
            let mut inner = vec![Arm {
                pat: ctor_pat(ctor, &ys),
                guard: None,
                body: and_all(eqs),
            }];
            if many {
                inner.push(Arm {
                    pat: Pat::Wild,
                    guard: None,
                    body: Expr::Lit(Lit::Bool(false)),
                });
            }
            arms.push(Arm {
                pat: ctor_pat(ctor, &xs),
                guard: None,
                body: Expr::Match {
                    scrutinee: Box::new(Expr::Var(b)),
                    arms: inner,
                },
            });
        }
        if arms.is_empty() {
            // No constructors: compared by the backend like any value of such a type.
            return Expr::Binary {
                op: BinOp::Eq,
                ty: OpTy::Structural,
                lhs: Box::new(Expr::Var(a)),
                rhs: Box::new(Expr::Var(b)),
            };
        }
        Expr::Match {
            scrutinee: Box::new(Expr::Var(a)),
            arms,
        }
    }

    /// Whether `x` and `y` (of type `ty`) are equal.
    fn eq_val(&mut self, f: FnId, ty: TypeId, x: Local, y: Local) -> Expr {
        let bin = |op_ty| Expr::Binary {
            op: BinOp::Eq,
            ty: op_ty,
            lhs: Box::new(Expr::Var(x)),
            rhs: Box::new(Expr::Var(y)),
        };
        match self.out.ty(ty).clone() {
            Type::Unit | Type::Never => Expr::Lit(Lit::Bool(true)),
            t if prim_op(&t).is_some() => bin(prim_op(&t).expect("checked")),
            _ if self.structural(f, ty, &mut Vec::new()) => bin(OpTy::Structural),
            Type::Tuple(ts) => {
                let xs = self.binds("x", &ts);
                let ys = self.binds("y", &ts);
                let eqs = ts
                    .iter()
                    .zip(xs.iter().zip(&ys))
                    .map(|(&t, (&x, &y))| self.eq_val(f, t, x, y))
                    .collect();
                take_apart(x, &xs, take_apart(y, &ys, and_all(eqs)))
            }
            _ => self.call_trait(f, ty, vec![Expr::Var(x), Expr::Var(y)]),
        }
    }

    // ---- Show ----

    /// `Name`, `Name(a, b)` or `Name(x: a, y: b)`.
    fn show_enum(&mut self, f: FnId, ty: TypeId, x: Local) -> Expr {
        let (id, variants) = self.variants(ty);
        let mut arms = Vec::with_capacity(variants.len());
        for (k, tys) in variants.into_iter().enumerate() {
            let ctor = CtorRef {
                enum_id: id,
                variant: k,
            };
            let def = &self.out.enums[id].variants[k];
            let name = def.name.clone();
            let labels = match &def.fields {
                VariantFields::Named(names) => Some(names.clone()),
                _ => None,
            };
            let xs = self.binds("x", &tys);
            let mut parts = Parts::default();
            parts.text(&name);
            for (i, (&t, &v)) in tys.iter().zip(&xs).enumerate() {
                parts.text(if i == 0 { "(" } else { ", " });
                if let Some(labels) = &labels {
                    parts.text(&format!("{}: ", labels[i]));
                }
                let part = self.show_val(f, t, v);
                parts.push(part);
            }
            if !tys.is_empty() {
                parts.text(")");
            }
            arms.push(Arm {
                pat: ctor_pat(ctor, &xs),
                guard: None,
                body: Expr::Concat(parts.0),
            });
        }
        if arms.is_empty() {
            return Expr::Concat(vec![StrPart::Value(Expr::Var(x), ty)]);
        }
        Expr::Match {
            scrutinee: Box::new(Expr::Var(x)),
            arms,
        }
    }

    /// `x` (of type `ty`) shown inside a value: a String quoted.
    fn show_val(&mut self, f: FnId, ty: TypeId, x: Local) -> StrPart {
        match self.out.ty(ty).clone() {
            Type::String => StrPart::Value(
                Expr::Builtin {
                    op: Builtin::ShowString,
                    ty_args: Vec::new(),
                    args: vec![Expr::Var(x)],
                },
                TypeId::STRING,
            ),
            _ if self.structural(f, ty, &mut Vec::new()) => StrPart::Value(Expr::Var(x), ty),
            Type::Tuple(ts) => {
                let xs = self.binds("x", &ts);
                let mut parts = Parts::default();
                parts.text("(");
                for (i, (&t, &v)) in ts.iter().zip(&xs).enumerate() {
                    if i > 0 {
                        parts.text(", ");
                    }
                    let part = self.show_val(f, t, v);
                    parts.push(part);
                }
                parts.text(")");
                StrPart::Value(take_apart(x, &xs, Expr::Concat(parts.0)), TypeId::STRING)
            }
            _ => StrPart::Value(self.call_trait(f, ty, vec![Expr::Var(x)]), TypeId::STRING),
        }
    }

    // ---- Ord ----

    /// Constructors in declaration order, then the fields from the first.
    fn compare_enum(&mut self, cx: &Cmp, ty: TypeId, a: Local, b: Local) -> Expr {
        let (id, variants) = self.variants(ty);
        if variants.is_empty() {
            let name = self.out.enums[id].name.clone();
            return Expr::Builtin {
                op: Builtin::Panic,
                ty_args: Vec::new(),
                args: vec![Expr::Lit(Lit::String(format!(
                    "cannot compare values of {name}"
                )))],
            };
        }
        let many = variants.len() > 1;
        let mut arms = Vec::with_capacity(variants.len());
        for (k, tys) in variants.iter().enumerate() {
            let ctor = CtorRef {
                enum_id: id,
                variant: k,
            };
            let xs = self.binds("x", tys);
            let ys = self.binds("y", tys);
            let cmps = tys
                .iter()
                .zip(xs.iter().zip(&ys))
                .map(|(&t, (&x, &y))| self.cmp_val(cx, t, x, y))
                .collect();
            let lex = self.lex(cx, cmps);
            let mut inner = vec![Arm {
                pat: ctor_pat(ctor, &ys),
                guard: None,
                body: lex,
            }];
            if many {
                // Not reached: the tags were equal.
                inner.push(Arm {
                    pat: Pat::Wild,
                    guard: None,
                    body: cx.ordering(ORD_EQUAL),
                });
            }
            arms.push(Arm {
                pat: ctor_pat(ctor, &xs),
                guard: None,
                body: Expr::Match {
                    scrutinee: Box::new(Expr::Var(b)),
                    arms: inner,
                },
            });
        }
        let fields = Expr::Match {
            scrutinee: Box::new(Expr::Var(a)),
            arms,
        };
        if !many {
            return fields;
        }
        // The constructors' indices first.
        let tag = |m: &mut Self, v: Local| {
            let arms = variants
                .iter()
                .enumerate()
                .map(|(k, tys)| Arm {
                    pat: Pat::Ctor {
                        ctor: CtorRef {
                            enum_id: id,
                            variant: k,
                        },
                        fields: vec![Pat::Wild; tys.len()],
                    },
                    guard: None,
                    body: Expr::Lit(Lit::Int(k as i32)),
                })
                .collect();
            let t = m.out.local("tag", TypeId::INT);
            (
                t,
                Expr::Match {
                    scrutinee: Box::new(Expr::Var(v)),
                    arms,
                },
            )
        };
        let (ta, ta_value) = tag(self, a);
        let (tb, tb_value) = tag(self, b);
        let lt = |l, r| Expr::Binary {
            op: BinOp::Lt,
            ty: OpTy::Int,
            lhs: Box::new(Expr::Var(l)),
            rhs: Box::new(Expr::Var(r)),
        };
        let body = Expr::If {
            cond: Box::new(lt(ta, tb)),
            then: Box::new(cx.ordering(ORD_LESS)),
            else_: Box::new(Expr::If {
                cond: Box::new(lt(tb, ta)),
                then: Box::new(cx.ordering(ORD_GREATER)),
                else_: Box::new(fields),
            }),
        };
        Expr::Let {
            var: ta,
            value: Box::new(ta_value),
            body: Box::new(Expr::Let {
                var: tb,
                value: Box::new(tb_value),
                body: Box::new(body),
            }),
        }
    }

    /// The order of `x` and `y` (of type `ty`).
    fn cmp_val(&mut self, cx: &Cmp, ty: TypeId, x: Local, y: Local) -> Expr {
        let bin = |op, op_ty| Expr::Binary {
            op,
            ty: op_ty,
            lhs: Box::new(Expr::Var(x)),
            rhs: Box::new(Expr::Var(y)),
        };
        match self.out.ty(ty).clone() {
            Type::Unit | Type::Never => cx.ordering(ORD_EQUAL),
            // `False < True` (spec 15.1).
            Type::Bool => Expr::If {
                cond: Box::new(bin(BinOp::Eq, OpTy::Bool)),
                then: Box::new(cx.ordering(ORD_EQUAL)),
                else_: Box::new(Expr::If {
                    cond: Box::new(Expr::Var(x)),
                    then: Box::new(cx.ordering(ORD_GREATER)),
                    else_: Box::new(cx.ordering(ORD_LESS)),
                }),
            },
            t if prim_op(&t).is_some() => {
                let op_ty = prim_op(&t).expect("checked");
                Expr::If {
                    cond: Box::new(bin(BinOp::Lt, op_ty)),
                    then: Box::new(cx.ordering(ORD_LESS)),
                    else_: Box::new(Expr::If {
                        cond: Box::new(bin(BinOp::Eq, op_ty)),
                        then: Box::new(cx.ordering(ORD_EQUAL)),
                        else_: Box::new(cx.ordering(ORD_GREATER)),
                    }),
                }
            }
            Type::Tuple(ts) => {
                let xs = self.binds("x", &ts);
                let ys = self.binds("y", &ts);
                let cmps = ts
                    .iter()
                    .zip(xs.iter().zip(&ys))
                    .map(|(&t, (&x, &y))| self.cmp_val(cx, t, x, y))
                    .collect();
                let lex = self.lex(cx, cmps);
                take_apart(x, &xs, take_apart(y, &ys, lex))
            }
            _ => self.call_trait(cx.f, ty, vec![Expr::Var(x), Expr::Var(y)]),
        }
    }

    /// The first of `cmps` that is not `Equal`, evaluated in order.
    fn lex(&mut self, cx: &Cmp, cmps: Vec<Expr>) -> Expr {
        let mut acc: Option<Expr> = None;
        for c in cmps.into_iter().rev() {
            acc = Some(match acc {
                None => c,
                Some(rest) => {
                    let ty = self.out.intern(Type::Enum(cx.ordering, Vec::new()));
                    let o = self.out.local("o", ty);
                    Expr::Match {
                        scrutinee: Box::new(c),
                        arms: vec![
                            Arm {
                                pat: Pat::Ctor {
                                    ctor: CtorRef {
                                        enum_id: cx.ordering,
                                        variant: ORD_EQUAL,
                                    },
                                    fields: Vec::new(),
                                },
                                guard: None,
                                body: rest,
                            },
                            Arm {
                                pat: Pat::Bind(o),
                                guard: None,
                                body: Expr::Var(o),
                            },
                        ],
                    }
                }
            });
        }
        acc.unwrap_or_else(|| cx.ordering(ORD_EQUAL))
    }
}

/// The indices of Ordering's constructors (`enum Ordering { Less, Equal, Greater }`).
const ORD_LESS: usize = 0;
const ORD_EQUAL: usize = 1;
const ORD_GREATER: usize = 2;

/// What comparing needs: the trait function (`Ord.compare`) and the Ordering enum.
struct Cmp {
    f: FnId,
    ordering: EnumId,
}

impl Cmp {
    fn ordering(&self, variant: usize) -> Expr {
        Expr::Ctor {
            ctor: CtorRef {
                enum_id: self.ordering,
                variant,
            },
            ty_args: Vec::new(),
            args: Vec::new(),
        }
    }
}

/// The parts of a string, adjacent texts merged.
#[derive(Default)]
pub(super) struct Parts(pub(super) Vec<StrPart>);

impl Parts {
    pub(super) fn text(&mut self, s: &str) {
        if let Some(StrPart::Lit(last)) = self.0.last_mut() {
            last.push_str(s);
        } else {
            self.0.push(StrPart::Lit(s.to_owned()));
        }
    }

    pub(super) fn push(&mut self, part: StrPart) {
        match part {
            StrPart::Lit(s) => self.text(&s),
            v => self.0.push(v),
        }
    }
}

/// The operand type of `==` and `<` on a basic type that has one.
fn prim_op(ty: &Type) -> Option<OpTy> {
    match ty {
        Type::Int => Some(OpTy::Int),
        Type::Int64 => Some(OpTy::Int64),
        Type::Float => Some(OpTy::Float),
        Type::Bool => Some(OpTy::Bool),
        Type::String => Some(OpTy::String),
        _ => None,
    }
}

pub(super) fn ctor_pat(ctor: CtorRef, binds: &[Local]) -> Pat {
    Pat::Ctor {
        ctor,
        fields: binds.iter().map(|&l| Pat::Bind(l)).collect(),
    }
}

/// `match tuple { (binds..) -> body }`.
pub(super) fn take_apart(tuple: Local, binds: &[Local], body: Expr) -> Expr {
    Expr::Match {
        scrutinee: Box::new(Expr::Var(tuple)),
        arms: vec![Arm {
            pat: Pat::Tuple(binds.iter().map(|&l| Pat::Bind(l)).collect()),
            guard: None,
            body,
        }],
    }
}

/// `a && b && ...`, `True` for none.
fn and_all(es: Vec<Expr>) -> Expr {
    es.into_iter()
        .rev()
        .reduce(|rest, e| Expr::Binary {
            op: BinOp::And,
            ty: OpTy::Bool,
            lhs: Box::new(e),
            rhs: Box::new(rest),
        })
        .unwrap_or(Expr::Lit(Lit::Bool(true)))
}
