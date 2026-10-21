//! The debug display of `dbg` for a type that does not satisfy Show (spec 10.6): the value as
//! constructed in source, like derived Show, with the parts whose types satisfy Show shown
//! through Show. Functions are `<fn>`, and opaque types show their contents.
//!
//! A part whose display reaches no written Show impl is shown by the backends' display of the
//! structure, which already gives this. Other enums and lists get a function per type
//! (`Plain: debug`), generated here once monomorphization knows the type.

use super::Mono;
use super::derive::{Parts, ctor_pat, take_apart};
use crate::Builtin;
use crate::dump::type_name;
use crate::ir::*;

impl Mono<'_> {
    /// `x` (of type `ty`) in the debug display, inside a value: a String quoted.
    pub(super) fn debug_val(&mut self, ty: TypeId, x: Local) -> StrPart {
        if !self.written_show(ty) {
            if ty == TypeId::STRING {
                return StrPart::Value(
                    Expr::Builtin {
                        op: Builtin::ShowString,
                        ty_args: Vec::new(),
                        args: vec![Expr::Var(x)],
                    },
                    TypeId::STRING,
                );
            }
            return StrPart::Value(Expr::Var(x), ty);
        }
        if self.shows(ty)
            && let Some(show) = self.m.traits.show
        {
            return StrPart::Value(
                self.trait_call(show, ty, vec![Expr::Var(x)]),
                TypeId::STRING,
            );
        }
        match self.out.ty(ty).clone() {
            Type::Tuple(ts) => {
                let xs = self.binds("x", &ts);
                let mut parts = Parts::default();
                parts.text("(");
                for (i, (&t, &v)) in ts.iter().zip(&xs).enumerate() {
                    if i > 0 {
                        parts.text(", ");
                    }
                    let part = self.debug_val(t, v);
                    parts.push(part);
                }
                parts.text(")");
                StrPart::Value(take_apart(x, &xs, Expr::Concat(parts.0)), TypeId::STRING)
            }
            Type::Enum(..) => {
                let f = self.debug_fn(ty);
                StrPart::Value(call(f, vec![Expr::Var(x)]), TypeId::STRING)
            }
            Type::List(_) => {
                let f = self.debug_fn(ty);
                let args = vec![
                    Expr::Var(x),
                    Expr::Lit(Lit::String("[".to_owned())),
                    Expr::Lit(Lit::String(String::new())),
                ];
                StrPart::Value(call(f, args), TypeId::STRING)
            }
            _ => StrPart::Value(Expr::Var(x), ty),
        }
    }

    /// The function showing a value of the enum or list type `ty` for `dbg`, created on first
    /// use. An enum's takes the value; a list's takes the rest of the list, the text so far and
    /// the separator, and calls itself in tail position (a loop).
    fn debug_fn(&mut self, ty: TypeId) -> FnId {
        if let Some(&f) = self.debug_fns.get(&ty) {
            return f;
        }
        let list = matches!(self.out.ty(ty), Type::List(_));
        let mut params = vec![self.out.local("x", ty)];
        if list {
            params.push(self.out.local("acc", TypeId::STRING));
            params.push(self.out.local("sep", TypeId::STRING));
        }
        let ty_name = type_name(&self.out, &self.out.types, ty);
        let name = self.fresh_name(format!("{ty_name}: debug"));
        let f = self.out.functions.alloc(Function {
            name,
            exported: false,
            params: params.clone(),
            ret: TypeId::STRING,
            type_params: 0,
            set_params: Vec::new(),
            fails: false,
            body: Expr::Lit(Lit::Unit),
            external: None,
            suspends: false,
            constant: false,
            sets: None,
        });
        if let Some(show) = self.m.traits.show {
            self.origins.insert(f, show);
        }
        self.debug_fns.insert(ty, f);
        let body = if list {
            self.debug_list(f, ty, &params)
        } else {
            self.debug_enum(ty, params[0])
        };
        self.out.functions[f].body = body;
        if list {
            // The list function calls itself in tail position on the rest of the list. Lowering
            // loopified everything before monomorphization, so this one is loopified here, to
            // keep long lists off the stack.
            crate::tail::loopify_function(&mut self.out, f);
        }
        f
    }

    /// `Name`, `Name(a, b)` or `Name(x: a, y: b)`, each part in the debug display.
    fn debug_enum(&mut self, ty: TypeId, x: Local) -> Expr {
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
                let part = self.debug_val(t, v);
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

    /// `match xs { [] -> "#{acc}]", [x, ..rest] -> f(rest, "#{acc}#{sep}#{x}", ", ") }`.
    fn debug_list(&mut self, f: FnId, ty: TypeId, params: &[Local]) -> Expr {
        let Type::List(elem) = *self.out.ty(ty) else {
            unreachable!("a list's debug function");
        };
        let (xs, acc, sep) = (params[0], params[1], params[2]);
        let x = self.out.local("x", elem);
        let rest = self.out.local("rest", ty);
        let shown = self.debug_val(elem, x);
        let empty = Expr::Concat(vec![
            StrPart::Value(Expr::Var(acc), TypeId::STRING),
            StrPart::Lit("]".to_owned()),
        ]);
        let next = Expr::Concat(vec![
            StrPart::Value(Expr::Var(acc), TypeId::STRING),
            StrPart::Value(Expr::Var(sep), TypeId::STRING),
            shown,
        ]);
        let more = call(
            f,
            vec![
                Expr::Var(rest),
                next,
                Expr::Lit(Lit::String(", ".to_owned())),
            ],
        );
        Expr::Match {
            scrutinee: Box::new(Expr::Var(xs)),
            arms: vec![
                Arm {
                    pat: Pat::List {
                        elems: Vec::new(),
                        rest: None,
                    },
                    guard: None,
                    body: empty,
                },
                Arm {
                    pat: Pat::List {
                        elems: vec![Pat::Bind(x)],
                        rest: Some(Box::new(Pat::Bind(rest))),
                    },
                    guard: None,
                    body: more,
                },
            ],
        }
    }
}

/// A pure call of the function `f`.
fn call(f: FnId, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Direct {
            f,
            ty_args: Vec::new(),
        },
        args,
        fails: false,
        suspends: false,
    }
}
