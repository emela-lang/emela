//! Showing and structural equality: inline for basic types, generated functions per type for
//! lists, tuples and enums (`show$List[Int]`, `eq$Point`). The input is monomorphic, so every type
//! is concrete and each function is for one type. The names append the type as the Core dump
//! writes it after a `$`, which never occurs in Emela names, so they cannot clash with the
//! program's functions.
//!
//! Show follows spec 10.6 and the JS runtime (`$showInt`, `$showString`, `$showList`,
//! `$showTuple`, the generated `$show$E`): Bool as `True` / `False`, Float by `Float.to_string`,
//! `()` as `()`, a String inside a value quoted and escaped (`RuntimeOp::ShowString`),
//! constructors as written in source (`Circle(radius: 1.0)`, `Rect(1.0, 2.0)`, `Empty`). Equality
//! follows `$eq`: lists and tuples element by element, enums by constructor and then fields,
//! Float by IEEE `==`.

use emela_core::{Builtin, CtorRef, Type, TypeId, VariantFields};

use super::{Derive, Lower, Piece, Shared};
use crate::enum_repr;
use crate::ir::{
    Callee, Case, CaseKey, Const, Function, Operand, PrimOp, PrimTy, RuntimeOp, Stmt, SwitchOn,
    Term, Value, VarId,
};
use crate::repr::Repr;

/// Builds a generated function declared by `Shared::derived`.
pub(super) fn build(sh: &mut Shared, kind: Derive, ty: TypeId, name: &str) -> Function {
    let mut l = Lower::new(sh, name);
    match kind {
        Derive::Show => {
            l.f.ret = TypeId::STRING;
            let x = l.param("x", ty);
            l.show_body(ty, x);
        }
        Derive::Eq => {
            l.f.ret = TypeId::BOOL;
            let a = l.param("a", ty);
            let b = l.param("b", ty);
            l.eq_body(ty, a, b);
        }
        Derive::ErrorName => {
            l.f.ret = TypeId::STRING;
            l.param("x", ty);
            let Type::Enum(id, _) = l.ty(ty) else {
                unreachable!("name functions are made for error enums")
            };
            // The constructor's name, unqualified like the one `show$E` writes. An error has one
            // constructor.
            let name = l.sh.core.enums[*id].variants[0].name.clone();
            l.term(Term::Return(Some(string(&name))));
        }
    }
    l.finish()
}

fn string(s: &str) -> Operand {
    Operand::Const(Const::String(s.to_owned()))
}

fn bool_(b: bool) -> Operand {
    Operand::Const(Const::Bool(b))
}

impl Lower<'_> {
    /// The string showing `op` (of type `ty`; `None` without representation).
    pub(super) fn show(&mut self, ty: TypeId, op: Option<Operand>) -> Operand {
        let Some(x) = op else {
            // `()`, and `Never`, which has no values (the code is never reached).
            return string(if ty == TypeId::UNIT { "()" } else { "" });
        };
        let builtin = |l: &mut Self, op| {
            Operand::Var(l.assign(
                "s",
                TypeId::STRING,
                Value::Builtin {
                    op,
                    args: vec![x.clone()],
                },
            ))
        };
        match self.ty(ty).clone() {
            Type::Int => builtin(self, Builtin::IntToString),
            Type::Int64 => builtin(self, Builtin::Int64ToString),
            Type::Float => builtin(self, Builtin::FloatToString),
            Type::Bool => {
                if let Operand::Const(Const::Bool(b)) = x {
                    return string(if b { "True" } else { "False" });
                }
                let dst = self.var("s", TypeId::STRING);
                self.open();
                self.term(Term::Yield(Some(string("True"))));
                let then = self.close();
                self.open();
                self.term(Term::Yield(Some(string("False"))));
                let else_ = self.close();
                self.push(Stmt::If {
                    dst: Some(dst),
                    cond: x,
                    then,
                    else_,
                });
                Operand::Var(dst)
            }
            Type::String => Operand::Var(self.assign(
                "s",
                TypeId::STRING,
                Value::Runtime {
                    op: RuntimeOp::ShowString,
                    args: vec![x],
                },
            )),
            Type::List(_) | Type::Tuple(_) | Type::Enum(..) => {
                let f = self.sh.derived(Derive::Show, ty);
                Operand::Var(self.assign(
                    "s",
                    TypeId::STRING,
                    Value::Call {
                        callee: Callee::Direct(f),
                        args: vec![x],
                        suspends: false,
                        on_fail: None,
                    },
                ))
            }
            // Debug display (`inspect`, spec 10.6). Interpolation of a function is rejected in
            // lowering.
            Type::Fn(_) => string("<fn>"),
            _ => {
                let name = crate::core_ty(&self.sh.core, ty);
                self.error(format!("a value of type {name} cannot be shown"));
                string("")
            }
        }
    }

    /// Whether `a` and `b` (of type `ty`) are structurally equal (`i32`).
    pub(super) fn eq(&mut self, ty: TypeId, a: Option<Operand>, b: Option<Operand>) -> Operand {
        let (Some(a), Some(b)) = (a, b) else {
            // `()` (and `Never`): always equal.
            return bool_(true);
        };
        let prim = |l: &mut Self, ty| {
            Operand::Var(l.assign(
                "eq",
                TypeId::BOOL,
                Value::Prim {
                    op: PrimOp::Eq,
                    ty,
                    args: vec![a.clone(), b.clone()],
                },
            ))
        };
        match self.ty(ty).clone() {
            Type::Int | Type::Bool => prim(self, PrimTy::I32),
            Type::Int64 => prim(self, PrimTy::I64),
            Type::Float => prim(self, PrimTy::F64),
            Type::String => prim(self, PrimTy::String),
            // An enum of constructors without fields is its tag.
            Type::Enum(id, _) if enum_repr(&self.sh.core, id) == Repr::I32 => {
                prim(self, PrimTy::I32)
            }
            Type::List(_) | Type::Tuple(_) | Type::Enum(..) => {
                let f = self.sh.derived(Derive::Eq, ty);
                Operand::Var(self.assign(
                    "eq",
                    TypeId::BOOL,
                    Value::Call {
                        callee: Callee::Direct(f),
                        args: vec![a, b],
                        suspends: false,
                        on_fail: None,
                    },
                ))
            }
            _ => {
                let name = crate::core_ty(&self.sh.core, ty);
                self.error(format!("values of type {name} cannot be compared"));
                bool_(false)
            }
        }
    }

    /// Returns false unless `cond`.
    fn or_return_false(&mut self, cond: Operand) {
        if cond == bool_(true) {
            return;
        }
        let then = self.empty();
        self.open();
        self.term(Term::Return(Some(bool_(false))));
        let else_ = self.close();
        self.push(Stmt::If {
            dst: None,
            cond,
            then,
            else_,
        });
    }

    fn show_body(&mut self, ty: TypeId, x: VarId) {
        match self.ty(ty).clone() {
            Type::List(elem) => self.show_list(ty, elem, x),
            Type::Tuple(ts) => {
                let mut pieces = vec![Piece::Text("(".into())];
                for (i, t) in ts.into_iter().enumerate() {
                    if i > 0 {
                        pieces.push(Piece::Text(", ".into()));
                    }
                    let e = self.get("e", t, Value::TupleGet { base: x, index: i });
                    let s = self.show(t, e.map(Operand::Var));
                    pieces.push(Piece::Op(s));
                }
                pieces.push(Piece::Text(")".into()));
                let s = self.concat(pieces);
                self.term(Term::Return(Some(s)));
            }
            Type::Enum(id, args) => {
                let def = &self.sh.core.enums[id];
                let (count, external, name) = (def.variants.len(), def.external, def.name.clone());
                let body = |l: &mut Self, k| {
                    let ctor = CtorRef {
                        enum_id: id,
                        variant: k,
                    };
                    l.show_variant(ctor, &args, x);
                };
                match count {
                    0 => self.no_values(external, &format!("cannot show a value of {name}")),
                    1 => body(self, 0),
                    _ => self.tag_switch(x, count, body),
                }
            }
            _ => unreachable!("show functions are made for lists, tuples and enums"),
        }
    }

    /// The body of a function over a value of an enum without constructors: an external type
    /// (host handle) has values, so `message` is a defect; otherwise it is never called.
    fn no_values(&mut self, external: bool, message: &str) {
        if external {
            self.term(Term::Defect(string(message)));
        } else {
            self.term(Term::Unreachable);
        }
    }

    /// `switch tag(x)` with a case per constructor, each built by `case` (which must end with a
    /// terminator).
    fn tag_switch(&mut self, x: VarId, count: usize, mut case: impl FnMut(&mut Self, usize)) {
        let mut cases = Vec::with_capacity(count);
        for k in 0..count {
            self.open();
            case(self, k);
            cases.push(Case {
                key: CaseKey::Variant(k),
                body: self.close(),
            });
        }
        self.push(Stmt::Switch {
            dst: None,
            on: SwitchOn::Tag(x),
            cases,
            default: None,
        });
        self.term(Term::Unreachable);
    }

    /// Returns the string showing `x`, a value of `ctor`.
    fn show_variant(&mut self, ctor: CtorRef, args: &[TypeId], x: VarId) {
        let variant = self.sh.core.variant(ctor).clone();
        let mut pieces = vec![Piece::Text(variant.name.clone())];
        for (i, &field) in variant.tys.iter().enumerate() {
            pieces.push(Piece::Text(if i == 0 { "(" } else { ", " }.into()));
            if let VariantFields::Named(names) = &variant.fields {
                pieces.push(Piece::Text(format!("{}: ", names[i])));
            }
            let t = self.subst(field, args);
            let f = self.get(
                "f",
                t,
                Value::Field {
                    base: x,
                    ctor,
                    index: i,
                },
            );
            let s = self.show(t, f.map(Operand::Var));
            pieces.push(Piece::Op(s));
        }
        if !variant.tys.is_empty() {
            pieces.push(Piece::Text(")".into()));
        }
        let s = self.concat(pieces);
        self.term(Term::Return(Some(s)));
    }

    /// `[a, b]`: a loop over the list, joining the shown elements with `, `.
    fn show_list(&mut self, ty: TypeId, elem: TypeId, x: VarId) {
        if elem == TypeId::NEVER {
            // Only the empty list has this type.
            self.term(Term::Return(Some(string("[]"))));
            return;
        }
        let p = self.var("p", ty);
        let acc = self.var("acc", TypeId::STRING);
        let sep = self.var("sep", TypeId::STRING);
        let joined = self.var("joined", TypeId::STRING);
        self.open();
        let empty = self.assign("empty", TypeId::BOOL, Value::ListIsEmpty(p));
        let then = self.empty();
        self.open();
        let h = self.get("h", elem, Value::ListHead(p));
        let t = self.assign("t", ty, Value::ListTail(p));
        let s = self.show(elem, h.map(Operand::Var));
        let next = self.concat(vec![
            Piece::Op(Operand::Var(acc)),
            Piece::Op(Operand::Var(sep)),
            Piece::Op(s),
        ]);
        self.term(Term::Recur(vec![Operand::Var(t), next, string(", ")]));
        let else_ = self.close();
        self.push(Stmt::If {
            dst: None,
            cond: Operand::Var(empty),
            then,
            else_,
        });
        self.term(Term::Yield(Some(Operand::Var(acc))));
        let body = self.close();
        self.push(Stmt::Loop {
            dst: Some(joined),
            vars: vec![p, acc, sep],
            inits: vec![Operand::Var(x), string(""), string("")],
            body,
        });
        let s = self.concat(vec![
            Piece::Text("[".into()),
            Piece::Op(Operand::Var(joined)),
            Piece::Text("]".into()),
        ]);
        self.term(Term::Return(Some(s)));
    }

    fn eq_body(&mut self, ty: TypeId, a: VarId, b: VarId) {
        match self.ty(ty).clone() {
            Type::List(elem) => self.eq_list(ty, elem, a, b),
            Type::Tuple(ts) => {
                for (i, t) in ts.into_iter().enumerate() {
                    let x = self.get("x", t, Value::TupleGet { base: a, index: i });
                    let y = self.get("y", t, Value::TupleGet { base: b, index: i });
                    let c = self.eq(t, x.map(Operand::Var), y.map(Operand::Var));
                    self.or_return_false(c);
                }
                self.term(Term::Return(Some(bool_(true))));
            }
            Type::Enum(id, args) => {
                let def = &self.sh.core.enums[id];
                let (count, external, name) = (def.variants.len(), def.external, def.name.clone());
                let fields = |l: &mut Self, k| {
                    let ctor = CtorRef {
                        enum_id: id,
                        variant: k,
                    };
                    let tys = l.sh.core.variant(ctor).tys.clone();
                    for (i, field) in tys.into_iter().enumerate() {
                        let t = l.subst(field, &args);
                        let field = |base| Value::Field {
                            base,
                            ctor,
                            index: i,
                        };
                        let x = l.get("x", t, field(a));
                        let y = l.get("y", t, field(b));
                        let c = l.eq(t, x.map(Operand::Var), y.map(Operand::Var));
                        l.or_return_false(c);
                    }
                    l.term(Term::Return(Some(bool_(true))));
                };
                match count {
                    0 => self.no_values(external, &format!("cannot compare values of {name}")),
                    1 => fields(self, 0),
                    _ => {
                        let ta = self.assign("ta", TypeId::INT, Value::Tag(a));
                        let tb = self.assign("tb", TypeId::INT, Value::Tag(b));
                        let same = self.assign(
                            "same",
                            TypeId::BOOL,
                            Value::Prim {
                                op: PrimOp::Eq,
                                ty: PrimTy::I32,
                                args: vec![Operand::Var(ta), Operand::Var(tb)],
                            },
                        );
                        self.or_return_false(Operand::Var(same));
                        self.tag_switch(a, count, fields);
                    }
                }
            }
            _ => unreachable!("equality functions are made for lists, tuples and enums"),
        }
    }

    /// Both lists element by element; equal when both end together.
    fn eq_list(&mut self, ty: TypeId, elem: TypeId, a: VarId, b: VarId) {
        let p = self.var("p", ty);
        let q = self.var("q", ty);
        let r = self.var("r", TypeId::BOOL);
        self.open();
        let pe = self.assign("pe", TypeId::BOOL, Value::ListIsEmpty(p));
        let qe = self.assign("qe", TypeId::BOOL, Value::ListIsEmpty(q));
        let then = self.empty();
        self.open();
        self.open();
        self.term(Term::Return(Some(bool_(false))));
        let stop = self.close();
        let go = self.empty();
        self.push(Stmt::If {
            dst: None,
            cond: Operand::Var(qe),
            then: stop,
            else_: go,
        });
        let x = self.get("x", elem, Value::ListHead(p));
        let y = self.get("y", elem, Value::ListHead(q));
        let c = self.eq(elem, x.map(Operand::Var), y.map(Operand::Var));
        self.or_return_false(c);
        let pt = self.assign("pt", ty, Value::ListTail(p));
        let qt = self.assign("qt", ty, Value::ListTail(q));
        self.term(Term::Recur(vec![Operand::Var(pt), Operand::Var(qt)]));
        let else_ = self.close();
        self.push(Stmt::If {
            dst: None,
            cond: Operand::Var(pe),
            then,
            else_,
        });
        // The first list ended: equal if the second did too.
        self.term(Term::Yield(Some(Operand::Var(qe))));
        let body = self.close();
        self.push(Stmt::Loop {
            dst: Some(r),
            vars: vec![p, q],
            inits: vec![Operand::Var(a), Operand::Var(b)],
            body,
        });
        self.term(Term::Return(Some(Operand::Var(r))));
    }
}
