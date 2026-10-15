//! IR parts for checking suspension (spec 14.5), shared by `run.rs` and `test_runner.rs`.

#![allow(dead_code)]

use emela_core::build::*;
use emela_core::*;

/// An external function calling the host function `path` (a global reference). With `suspends`,
/// it is an `@external suspend fn`.
pub fn external(
    m: &mut Module,
    name: &str,
    path: &str,
    params: Vec<TypeId>,
    ret: TypeId,
    suspends: bool,
) -> FnId {
    let f = m.declare(name, false);
    let ps = params.into_iter().map(|t| m.local("x", t)).collect();
    m.define_external(
        f,
        ps,
        ret,
        External {
            bindings: vec![ExternalBinding {
                target: "js".into(),
                host: HostRef::Global(path.into()),
            }],
        },
    );
    m.functions[f].suspends = suspends;
    f
}

/// An external function awaiting `Promise.resolve(x)`.
pub fn resolve(m: &mut Module) -> FnId {
    external(
        m,
        "resolve",
        "Promise.resolve",
        vec![TypeId::INT],
        TypeId::INT,
        true,
    )
}

/// `setTimeout(callback, ms)`．
pub fn set_timeout(m: &mut Module) -> FnId {
    let callback = m.intern(fn_type(vec![], TypeId::UNIT));
    external(
        m,
        "set_timeout",
        "setTimeout",
        vec![callback, TypeId::INT],
        TypeId::UNIT,
        false,
    )
}

/// A suspend fn operation `Wait.forever` that drops resume without calling it, and the handler
/// `Never` implementing it.
pub fn never(m: &mut Module) -> (FnId, EffectId) {
    let wait = m.add_effect(EffectDef {
        name: "Wait".into(),
        ops: vec![OpDef::suspend("forever")],
    });
    let f = m.declare("Never", false);
    let resume = m.local("resume", resume_type());
    let op_ty = op_type(m);
    m.define(
        f,
        vec![],
        Type::Handler(wait),
        Expr::Handler {
            name: "Never".into(),
            effect: wait,
            ops: vec![lambda(vec![resume], op_ty, unit())],
        },
    );
    (f, wait)
}

/// `h.forever()`．
pub fn forever(wait: EffectId, h: Local) -> Expr {
    call_op(var(h), wait, 0, vec![], false, true)
}

/// `export suspend fn main() -> ret { body }`．
pub fn main(m: &mut Module, ret: TypeId, body: Expr) {
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(main, vec![], ret, body);
}

/// An external function calling `console.log(x)`.
pub fn log(m: &mut Module) -> FnId {
    external(
        m,
        "log",
        "console.log",
        vec![TypeId::STRING],
        TypeId::UNIT,
        false,
    )
}

/// A binding of handler `name` that calls `log(message)` on release. It has no operations of its
/// own (it resumes `Wait`'s `forever` immediately).
pub fn released(m: &mut Module, log: FnId, h: Local, name: &str, message: &str) -> WithBind {
    let wait = m
        .effects
        .iter()
        .find(|(_, e)| e.name == "Wait")
        .map(|(id, _)| id)
        .expect("add Wait with never first");
    let resume = m.local("resume", resume_type());
    let op_ty = op_type(m);
    let handler = Expr::Handler {
        name: name.into(),
        effect: wait,
        ops: vec![lambda(
            vec![resume],
            op_ty,
            call_value(var(resume), vec![unit()]),
        )],
    };
    WithBind::managed(h, handler, name, call(log, vec![string(message)]))
}

/// The type of `forever`'s implementation, which takes resume: `fn(fn(()) -> ()) -> ()`.
fn op_type(m: &mut Module) -> TypeId {
    let resume = m.intern(resume_type());
    m.fn_type(vec![resume], TypeId::UNIT)
}

/// A non-failing, non-suspending function type without an evidence table.
pub fn fn_type(params: Vec<TypeId>, ret: TypeId) -> Type {
    Type::Fn(FnType {
        params,
        ret,
        fails: false,
        suspends: false,
        ev: false,
    })
}

/// The type of `forever`'s resume: `fn(()) -> ()`.
pub fn resume_type() -> Type {
    fn_type(vec![TypeId::UNIT], TypeId::UNIT)
}
