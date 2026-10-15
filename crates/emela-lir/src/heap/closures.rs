//! Closure conversion: nested closures become lifted functions and closure records.
//!
//! `dst = closure(params) [captures] { body }` in a function `f` becomes
//! `dst = make_closure f$closureN[captures]`, and a new function
//! `f$closureN(env, params) env[capture types]` whose body starts by reading each capture from the
//! environment (`env_get env#i`) and goes on with a copy of `body`. Closures nested in `body` are
//! lifted from the new function in turn. A top-level function used as a value (`fn_ref g`)
//! becomes `make_closure g$fn[]`, where `g$fn(env, params)` calls `g`, so that every function
//! value has the calling convention of a closure record.

use emela_core::{Type, TypeId};
use rustc_hash::FxHashMap;

use super::copy::{Copier, compact};
use crate::ir::{
    Callee, FnId, Function, Module, OnFail, Operand, Seq, SeqId, Stage, Stmt, StmtId, Term, Value,
    VarData, VarId,
};
use crate::repr::{Repr, repr_of};

/// Lifts every closure of the module and replaces function references by closure records. The
/// module must be at [`Stage::Flow`]; it is at [`Stage::ClosuresConverted`] afterwards.
pub fn convert_closures(m: &mut Module) {
    assert_eq!(
        m.stage,
        Stage::Flow,
        "closure conversion runs on the Flow IR"
    );
    let mut wrappers: FxHashMap<FnId, FnId> = FxHashMap::default();
    let mut todo: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    todo.reverse();
    while let Some(id) = todo.pop() {
        if m.functions[id].body.is_none() {
            continue;
        }
        let mut counter = 0;
        let sites = sites(&m.functions[id]);
        for s in sites {
            let Stmt::Let { dst, value, .. } = &m.functions[id].stmts[s] else {
                unreachable!("a site is a let");
            };
            let dst = *dst;
            let new = match value {
                Value::Closure(_) => {
                    let name =
                        unique_name(m, &format!("{}$closure{counter}", m.functions[id].name));
                    counter += 1;
                    let (lifted, captures) = lift(m, id, s, &name);
                    let lifted = m.functions.alloc(lifted);
                    // Lifted functions may hold closures in turn.
                    todo.push(lifted);
                    Value::MakeClosure {
                        function: lifted,
                        captures,
                    }
                }
                Value::FnRef(target) => {
                    let target = *target;
                    let wrapper = match wrappers.get(&target) {
                        Some(&w) => w,
                        None => {
                            let ty = dst.map_or(TypeId::ANY, |d| m.functions[id].vars[d].ty);
                            let w = wrapper(m, target, ty);
                            let w = m.functions.alloc(w);
                            wrappers.insert(target, w);
                            w
                        }
                    };
                    Value::MakeClosure {
                        function: wrapper,
                        captures: Vec::new(),
                    }
                }
                _ => unreachable!("a site is a closure or a function reference"),
            };
            m.functions[id].stmts[s] = Stmt::Let {
                dst,
                value: new,
                on_defect: None,
            };
        }
        m.functions[id] = compact(&m.functions[id]);
    }
    m.stage = Stage::ClosuresConverted;
}

/// A name not used by another function of the module.
fn unique_name(m: &Module, base: &str) -> String {
    let taken = |n: &str| m.functions.iter().any(|(_, f)| f.name == n);
    if !taken(base) {
        return base.to_owned();
    }
    (1..)
        .map(|i| format!("{base}${i}"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// The lets of closures and function references in the function's body, not inside closures.
fn sites(f: &Function) -> Vec<StmtId> {
    fn walk(f: &Function, seq: SeqId, out: &mut Vec<StmtId>) {
        for &s in &f.seqs[seq].stmts {
            match &f.stmts[s] {
                Stmt::Let {
                    value: Value::Closure(_) | Value::FnRef(_),
                    ..
                } => out.push(s),
                Stmt::Let { .. } | Stmt::Dup(_) | Stmt::Drop(_) => {}
                Stmt::If { then, else_, .. } => {
                    walk(f, *then, out);
                    walk(f, *else_, out);
                }
                Stmt::Switch { cases, default, .. } => {
                    for c in cases {
                        walk(f, c.body, out);
                    }
                    if let Some(d) = default {
                        walk(f, *d, out);
                    }
                }
                Stmt::Loop { body, .. } | Stmt::Block { body, .. } => walk(f, *body, out),
                Stmt::Protect { body, cleanup, .. } => {
                    walk(f, *body, out);
                    walk(f, *cleanup, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    if let Some(body) = f.body {
        walk(f, body, &mut out);
    }
    out
}

/// The function lifted from the closure at statement `s` of function `id`, and the captures to
/// put in its record.
fn lift(m: &Module, id: FnId, s: StmtId, name: &str) -> (Function, Vec<VarId>) {
    let src = &m.functions[id];
    let Stmt::Let {
        dst: Some(dst),
        value: Value::Closure(c),
        ..
    } = &src.stmts[s]
    else {
        unreachable!("a closure has a destination");
    };
    let ty = src.vars[*dst].ty;
    let Type::Fn(ft) = m.core.ty(ty) else {
        unreachable!("a closure has a function type");
    };
    let mut f = Function::new(name);
    f.ret = ft.ret;
    f.fails = ft.fails;
    f.suspends = c.suspends;
    f.env = Some(c.captures.iter().map(|&v| src.vars[v].ty).collect());
    let env = f.vars.alloc(VarData {
        hint: "env".into(),
        repr: Repr::Ref,
        ty,
    });
    let mut copier = Copier::new(src);
    let reads: Vec<(VarId, usize)> = c
        .captures
        .iter()
        .enumerate()
        .map(|(i, &v)| (copier.fresh(&mut f, v), i))
        .collect();
    f.params.push(env);
    for &p in &c.params {
        let p = copier.fresh(&mut f, p);
        f.params.push(p);
    }
    let body = copier.seq(&mut f, c.body);
    let mut stmts: Vec<StmtId> = reads
        .into_iter()
        .map(|(v, index)| {
            f.stmts.alloc(Stmt::Let {
                dst: Some(v),
                value: Value::EnvGet { env, index },
                on_defect: None,
            })
        })
        .collect();
    stmts.append(&mut f.seqs[body].stmts);
    f.seqs[body].stmts = stmts;
    f.body = Some(body);
    (f, c.captures.clone())
}

/// `name$fn(env, params)`, calling the top-level function `target` with the parameters: the
/// closure record of `target` used as a value of the function type `ty`.
fn wrapper(m: &Module, target: FnId, ty: TypeId) -> Function {
    let t = &m.functions[target];
    let base = format!("{}$fn", t.name);
    let mut f = Function::new(&unique_name(m, &base));
    f.ret = t.ret;
    f.fails = t.fails;
    f.suspends = t.suspends;
    f.env = Some(Vec::new());
    let env = f.vars.alloc(VarData {
        hint: "env".into(),
        repr: Repr::Ref,
        ty,
    });
    f.params.push(env);
    let args: Vec<Operand> = t
        .params
        .iter()
        .map(|&p| {
            let v = f.vars.alloc(t.vars[p].clone());
            f.params.push(v);
            Operand::Var(v)
        })
        .collect();
    let ret = repr_of(&m.core, t.ret);
    let r = (ret != Repr::Void).then(|| {
        f.vars.alloc(VarData {
            hint: "r".into(),
            repr: ret,
            ty: t.ret,
        })
    });
    let call = f.stmts.alloc(Stmt::Let {
        dst: r,
        value: Value::Call {
            callee: Callee::Direct(target),
            args,
            suspends: t.suspends,
            on_fail: t.fails.then_some(OnFail::Propagate),
        },
        on_defect: None,
    });
    let term = if m.core.ty(t.ret) == &Type::Never {
        Term::Unreachable
    } else {
        Term::Return(r.map(Operand::Var))
    };
    f.body = Some(f.seqs.alloc(Seq {
        stmts: vec![call],
        term,
    }));
    f
}
