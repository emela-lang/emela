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
    params: Vec<Boundary>,
    ret: Boundary,
    suspends: bool,
) -> FnId {
    let f = m.declare(name, false);
    let ps = (0..params.len()).map(|_| m.local("x")).collect();
    m.define_external(
        f,
        ps,
        External {
            bindings: vec![ExternalBinding {
                target: "js".into(),
                host: HostRef::Global(path.into()),
            }],
            params,
            ret,
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
        vec![Boundary::Plain],
        Boundary::Plain,
        true,
    )
}

/// `setTimeout(callback, ms)`．
pub fn set_timeout(m: &mut Module) -> FnId {
    external(
        m,
        "set_timeout",
        "setTimeout",
        vec![
            Boundary::Fn {
                params: vec![],
                ret: Box::new(Boundary::Unit),
            },
            Boundary::Plain,
        ],
        Boundary::Unit,
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
    let resume = m.local("resume");
    m.define(
        f,
        vec![],
        Expr::Handler {
            name: "Never".into(),
            effect: wait,
            ops: vec![lambda(vec![resume], unit())],
        },
    );
    (f, wait)
}

/// `h.forever()`．
pub fn forever(wait: EffectId, h: Local) -> Expr {
    call_op(var(h), wait, 0, vec![], false, true)
}

/// `export suspend fn main() { body }`．
pub fn main(m: &mut Module, body: Expr) {
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(main, vec![], body);
}

/// An external function calling `console.log(x)`.
pub fn log(m: &mut Module) -> FnId {
    external(
        m,
        "log",
        "console.log",
        vec![Boundary::Plain],
        Boundary::Unit,
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
    let resume = m.local("resume");
    let handler = Expr::Handler {
        name: name.into(),
        effect: wait,
        ops: vec![lambda(vec![resume], call_value(var(resume), vec![unit()]))],
    };
    WithBind::managed(h, handler, name, call(log, vec![string(message)]))
}
