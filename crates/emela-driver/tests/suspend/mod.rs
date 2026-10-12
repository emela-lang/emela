//! 中断（14.5）の確かめに使う IR の部品．`run.rs` と `test_runner.rs` で使う．

#![allow(dead_code)]

use emela_core::build::*;
use emela_core::*;

/// ホストの関数 `path`（グローバルの参照）を呼ぶ外部関数．`suspends` なら `@external suspend fn`．
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
                args: vec![path.into()],
            }],
            params,
            ret,
        },
    );
    m.functions[f].suspends = suspends;
    f
}

/// `Promise.resolve(x)` を待つ外部関数．
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

/// resume を呼ばずに捨てる suspend fn の操作 `Wait.forever` と，それを実装するハンドラ `Never`．
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
