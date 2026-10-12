//! init と release のあるハンドラ（仕様 8.4，8.6，14.2，14.4，14.5）の出力．IR を手で組み立てて JS を出し，
//! node で実行する．with の本体がどの形で終わっても release が走ることを，ホストの `console.log` に出した
//! 順で確かめる．出力した JS は insta のスナップショットでも見る．

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// `main` を呼び，ジェネレータなら `$start` で進めて結果を1行出す．起動用モジュール（emela-driver）と
/// 同じく，resume されないままイベントループが空になったら `$start` の返した関数で巻き戻す．
///
/// - 値: `42`（中断しうる main は `sync 42` / `async 42`），エラー: `error: Oops`，defect: `defect: …`
/// - 中断されたまま巻き戻った: `interrupted`
const HARNESS: &str = r#"import * as m from "./main.mjs";
let over = false;
let sync = true;
let cancel;
const out = (s) => process.stdout.write(s + "\n");
const describe = (e) =>
  e?.name === "Interrupted"
    ? "interrupted"
    : e?.name === "Defect"
      ? "defect: " + e.message
      : "throw: " + String(e);
const show = (v) => {
  const f = v?.[Symbol.for("emela.fail")];
  return typeof f === "function" ? "error: " + f.call(v) : String(v);
};
const crash = (e) => {
  if (over) return;
  over = true;
  out(describe(e));
};
process.on("uncaughtException", crash);
process.on("beforeExit", () => {
  if (over) return;
  if (cancel !== undefined) {
    cancel();
    return;
  }
  over = true;
  out("interrupted");
});
try {
  const v = m.main();
  if (Object.prototype.toString.call(v) === "[object Generator]") {
    cancel = m.$start(v, (ok, r) => {
      if (over) return;
      over = true;
      out((sync ? "sync " : "async ") + (ok ? show(r) : describe(r)));
    });
  } else {
    over = true;
    out(show(v));
  }
} catch (e) {
  crash(e);
}
sync = false;
"#;

fn run(name: &str, mut m: Module) -> String {
    tail::loopify(&mut m);
    let js = emit_module(
        &m,
        &Options {
            runtime: RuntimeMode::Import(None),
        },
    );
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-js-release")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.mjs"), &js).unwrap();
    std::fs::write(dir.join(RUNTIME_FILE), RUNTIME).unwrap();
    std::fs::write(dir.join("harness.mjs"), HARNESS).unwrap();
    let out = Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("node を起動できない");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "node が失敗した:\n{stderr}\n{js}");
    insta::assert_snapshot!(name, js);
    String::from_utf8(out.stdout).unwrap()
}

/// 手で組む IR の部品．
struct Kit {
    /// `console.log(x)` を呼ぶ外部関数．
    log: FnId,
    /// `effect Res { fn name() -> String }`．
    res: EffectId,
    /// `error Oops`．
    oops: CtorRef,
}

impl Kit {
    fn new(m: &mut Module) -> Kit {
        let log = external(
            m,
            "log",
            "console.log",
            vec![Boundary::Plain],
            Boundary::Unit,
            false,
        );
        let res = m.add_effect(EffectDef {
            name: "Res".into(),
            ops: vec![OpDef::new("name")],
        });
        let oops = m.add_enum(EnumDef {
            name: "Oops".into(),
            params: 0,
            variants: vec![VariantDef::unit("Oops")],
            error: true,
        });
        Kit {
            log,
            res,
            oops: CtorRef {
                enum_id: oops,
                variant: 0,
            },
        }
    }

    /// `log(text)`．
    fn log(&self, text: &str) -> Expr {
        call(self.log, vec![string(text)])
    }

    /// `log(text); then`．
    fn log_then(&self, m: &mut Module, text: &str, then: Expr) -> Expr {
        let x = m.local("_");
        let_(x, self.log(text), then)
    }

    /// `handler name implements Res { fn name() { "name" } }` の値．
    fn handler(&self, name: &str) -> Expr {
        Expr::Handler {
            name: name.into(),
            effect: self.res,
            ops: vec![lambda(vec![], string(name))],
        }
    }

    /// init で `init <name>` を，release で `release <name>` を出すハンドラの束縛．
    fn managed(&self, m: &mut Module, h: Local, name: &str) -> WithBind {
        let init = self.log_then(m, &format!("init {name}"), self.handler(name));
        WithBind::managed(h, init, name, self.log(&format!("release {name}")))
    }

    /// release で `release <name>` を出してから `panic(message)` するハンドラの束縛．
    fn breaking(&self, m: &mut Module, h: Local, name: &str, message: &str) -> WithBind {
        let release = self.log_then(
            m,
            &format!("release {name}"),
            builtin(Builtin::Panic, vec![string(message)]),
        );
        WithBind::managed(h, self.handler(name), name, release)
    }

    fn oops(&self) -> Expr {
        fail(ctor(self.oops, vec![]))
    }
}

/// ホストの関数 `path`（グローバルの参照）を呼ぶ外部関数．`suspends` なら `@external suspend fn`．
fn external(
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

fn with(binds: Vec<WithBind>, body: Expr) -> Expr {
    Expr::With {
        binds,
        body: Box::new(body),
    }
}

fn main_fn(m: &mut Module, suspends: bool, body: Expr) {
    let main = m.declare("main", true);
    m.functions[main].suspends = suspends;
    m.define(main, vec![], body);
}

/// `effect Clock { suspend fn sleep(ms: Int) -> () }`．
fn clock(m: &mut Module) -> EffectId {
    m.add_effect(EffectDef {
        name: "Clock".into(),
        ops: vec![OpDef::suspend("sleep")],
    })
}

/// `setTimeout` の後で resume する `JsClock`．
fn js_clock(m: &mut Module, clock: EffectId) -> FnId {
    let set_timeout = external(
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
    );
    let f = m.declare("JsClock", false);
    let (ms, resume) = (m.local("ms"), m.local("resume"));
    let body = call(
        set_timeout,
        vec![
            lambda(vec![], call_value(var(resume), vec![unit()])),
            var(ms),
        ],
    );
    m.define(
        f,
        vec![],
        Expr::Handler {
            name: "JsClock".into(),
            effect: clock,
            ops: vec![lambda(vec![ms, resume], body)],
        },
    );
    f
}

/// resume を呼ばずに捨てる `NeverClock`．
fn never_clock(m: &mut Module, clock: EffectId) -> FnId {
    let f = m.declare("NeverClock", false);
    let (ms, resume) = (m.local("ms"), m.local("resume"));
    m.define(
        f,
        vec![],
        Expr::Handler {
            name: "NeverClock".into(),
            effect: clock,
            ops: vec![lambda(vec![ms, resume], unit())],
        },
    );
    f
}

/// `h.sleep(ms)`．
fn sleep(h: Local, clock: EffectId, ms: i32) -> Expr {
    call_op(var(h), clock, 0, vec![int(ms)], false, true)
}

#[test]
fn releases_run_in_reverse_order_after_a_value() {
    // init は束縛の順，release は逆の順（14.2）．with の値は本体の値．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (m.local("a"), m.local("b"));
    let binds = vec![k.managed(&mut m, a, "A"), k.managed(&mut m, b, "B")];
    let body = k.log_then(&mut m, "body", int(42));
    main_fn(&mut m, false, with(binds, body));
    assert_eq!(
        run("value", m),
        "init A\ninit B\nbody\nrelease B\nrelease A\n42\n"
    );
}

#[test]
fn releases_run_when_the_body_fails() {
    // 本体の失敗は，release を走らせてから外側へ伝わる．escape の中の with は break で抜ける．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (m.local("a"), m.local("b"));
    let inner = with(
        vec![k.managed(&mut m, a, "A")],
        k.log_then(&mut m, "body", k.oops()),
    );
    let caught = escape(inner, vec![arm(p_ctor(k.oops, vec![]), int(7))]);
    let x = m.local("x");
    let outer = with(
        vec![k.managed(&mut m, b, "B")],
        let_(x, caught, k.log_then(&mut m, "caught", k.oops())),
    );
    main_fn(&mut m, false, outer);
    assert_eq!(
        run("fail", m),
        "init B\ninit A\nbody\nrelease A\ncaught\nrelease B\nerror: Oops\n"
    );
}

#[test]
fn releases_run_when_the_body_defects() {
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (m.local("a"), m.local("b"));
    let binds = vec![k.managed(&mut m, a, "A"), k.managed(&mut m, b, "B")];
    main_fn(
        &mut m,
        false,
        with(binds, builtin(Builtin::Panic, vec![string("boom")])),
    );
    assert_eq!(
        run("defect", m),
        "init A\ninit B\nrelease B\nrelease A\ndefect: boom\n"
    );
}

#[test]
fn failing_init_releases_only_the_initialized_handlers() {
    // B の init が失敗したら，本体と C は評価せず，初期化の済んだ A だけを release する．
    // B 自身の release は走らない（14.2）．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let init_b = m.declare("init_b", false);
    let failing = k.log_then(&mut m, "init B", k.oops());
    m.define(init_b, vec![], failing);
    let (a, b, c) = (m.local("a"), m.local("b"), m.local("c"));
    let binds = vec![
        k.managed(&mut m, a, "A"),
        WithBind::managed(b, call_fails(init_b, vec![]), "B", k.log("release B")),
        k.managed(&mut m, c, "C"),
    ];
    let body = k.log_then(&mut m, "body", int(1));
    main_fn(&mut m, false, with(binds, body));
    assert_eq!(
        run("init_fails", m),
        "init A\ninit B\nrelease A\nerror: Oops\n"
    );
}

#[test]
fn a_release_defect_replaces_a_value_or_an_error() {
    // 本体が値で終わっていたら，release の defect が with の結果．残りの release も走る（14.4）．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (m.local("a"), m.local("b"));
    let binds = vec![
        k.managed(&mut m, a, "A"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(&mut m, false, with(binds, int(1)));
    assert_eq!(
        run("release_defect_value", m),
        "init A\nrelease B\nrelease A\ndefect: B broke\n"
    );

    // エラーで終わっていても同じ．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let a = m.local("a");
    let binds = vec![k.breaking(&mut m, a, "A", "A broke")];
    main_fn(&mut m, false, with(binds, k.oops()));
    assert_eq!(
        run("release_defect_error", m),
        "release A\ndefect: A broke\n"
    );
}

#[test]
fn the_first_defect_wins_and_later_ones_are_noted() {
    // 本体が defect なら，それが結果のまま．release の defect は文面の注記になる（14.4）．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (m.local("a"), m.local("b"));
    let binds = vec![
        k.breaking(&mut m, a, "A", "A broke"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(
        &mut m,
        false,
        with(binds, builtin(Builtin::Panic, vec![string("boom")])),
    );
    assert_eq!(
        run("first_defect", m),
        "release B\nrelease A\ndefect: boom\n\
         note: the release of handler B also failed: B broke\n\
         note: the release of handler A also failed: A broke\n"
    );

    // 本体が値なら，最初の release の defect が結果で，後の release の defect が注記．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (m.local("a"), m.local("b"));
    let binds = vec![
        k.breaking(&mut m, a, "A", "A broke"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(&mut m, false, with(binds, int(1)));
    assert_eq!(
        run("first_release_defect", m),
        "release B\nrelease A\ndefect: B broke\n\
         note: the release of handler A also failed: A broke\n"
    );
}

#[test]
fn released_handlers_are_defects_when_used() {
    // with の外へ持ち出した能力を release の後に使うと defect（8.6）．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, f) = (m.local("a"), m.local("f"));
    let leak = with(
        vec![k.managed(&mut m, a, "Pool")],
        lambda(vec![], call_op(var(a), k.res, 0, vec![], false, false)),
    );
    // 中では使える．
    let i = m.local("i");
    let inside = with(
        vec![k.managed(&mut m, i, "Inside")],
        call_op(var(i), k.res, 0, vec![], false, false),
    );
    let x = m.local("x");
    main_fn(
        &mut m,
        false,
        let_(
            x,
            call(k.log, vec![inside]),
            let_(f, leak, call_value(var(f), vec![])),
        ),
    );
    assert_eq!(
        run("used_after_release", m),
        "init Inside\nrelease Inside\nInside\ninit Pool\nrelease Pool\n\
         defect: handler Pool was used after release\n"
    );
}

#[test]
fn releases_run_after_a_later_resume() {
    // 本体が中断して後から resume されても，release は本体が終わった後に走る．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let js_clock = js_clock(&mut m, clock);
    let (c, a) = (m.local("c"), m.local("a"));
    let binds = vec![
        WithBind::new(c, call(js_clock, vec![])),
        k.managed(&mut m, a, "A"),
    ];
    let after = k.log_then(&mut m, "resumed", int(42));
    let x = m.local("_");
    let body = let_(x, sleep(c, clock, 1), after);
    main_fn(&mut m, true, with(binds, body));
    assert_eq!(
        run("later_resume", m),
        "init A\nresumed\nrelease A\nasync 42\n"
    );
}

#[test]
fn dropped_continuations_release_from_the_inside() {
    // resume されないまま捨てられた main は，開いている with の release を内側から走らせてから
    // interrupted で終わる（14.5）．内側の with は `yield*` の先のジェネレータの中にある．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let never = never_clock(&mut m, clock);
    let inner = m.declare("inner", false);
    m.functions[inner].suspends = true;
    let (c, b) = (m.local("c"), m.local("b"));
    let inner_body = with(
        vec![
            WithBind::new(c, call(never, vec![])),
            k.managed(&mut m, b, "B"),
        ],
        sleep(c, clock, 1),
    );
    m.define(inner, vec![], inner_body);
    let a = m.local("a");
    let body = with(
        vec![k.managed(&mut m, a, "A")],
        call_suspends(inner, vec![]),
    );
    main_fn(&mut m, true, body);
    assert_eq!(
        run("dropped", m),
        "init A\ninit B\nrelease B\nrelease A\nasync interrupted\n"
    );
}

#[test]
fn immediate_contexts_release_when_dropped() {
    // 中断しない main（R が Immediate）の中で resume されなかった継続も，その場で巻き戻して release を
    // 走らせる．`$sync` で進めたジェネレータの中の with（B）は `gen.return()` で，main の中の with（A）は
    // `$Interrupted` の例外で抜ける（14.5）．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let never = never_clock(&mut m, clock);
    let inner = m.declare("inner", false);
    m.functions[inner].suspends = true;
    let (c, b) = (m.local("c"), m.local("b"));
    let inner_body = with(
        vec![
            WithBind::new(c, call(never, vec![])),
            k.managed(&mut m, b, "B"),
        ],
        sleep(c, clock, 1),
    );
    m.define(inner, vec![], inner_body);
    let a = m.local("a");
    let body = with(
        vec![k.managed(&mut m, a, "A")],
        call_suspends(inner, vec![]),
    );
    main_fn(&mut m, false, body);
    assert_eq!(
        run("sync_dropped", m),
        "init A\ninit B\nrelease B\nrelease A\ninterrupted\n"
    );
}

#[test]
fn releases_may_suspend() {
    // release の中で suspend な操作を呼んでよい（8.4）．後から resume されたら続きを走らせる．
    // 中断されたまま巻き戻るときも同じ（release が終わってから interrupted）．
    for (name, dropped, expected) in [
        (
            "release_suspends",
            false,
            "init A\nrelease A\nslept\nasync 1\n",
        ),
        (
            "release_suspends_dropped",
            true,
            "init A\nrelease A\nslept\nasync interrupted\n",
        ),
    ] {
        let mut m = Module::new();
        let k = Kit::new(&mut m);
        let clock = clock(&mut m);
        let js_clock = js_clock(&mut m, clock);
        let never = never_clock(&mut m, clock);
        let (c, n, a) = (m.local("c"), m.local("n"), m.local("a"));
        let x = m.local("_");
        let release = k.log_then(
            &mut m,
            "release A",
            let_(x, sleep(c, clock, 1), k.log("slept")),
        );
        let init = k.log_then(&mut m, "init A", k.handler("A"));
        let body = if dropped { sleep(n, clock, 1) } else { int(1) };
        let binds = vec![
            WithBind::new(c, call(js_clock, vec![])),
            WithBind::new(n, call(never, vec![])),
            WithBind::managed(a, init, "A", release),
        ];
        main_fn(&mut m, true, with(binds, body));
        assert_eq!(run(name, m), expected);
    }
}

#[test]
fn a_release_defect_while_interrupted_is_the_result() {
    // 中断されたまま巻き戻る途中の release の defect は，その起動の defect になる．残りの release も走る．
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let never = never_clock(&mut m, clock);
    let (c, a, b) = (m.local("c"), m.local("a"), m.local("b"));
    let binds = vec![
        WithBind::new(c, call(never, vec![])),
        k.managed(&mut m, a, "A"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(&mut m, true, with(binds, sleep(c, clock, 1)));
    assert_eq!(
        run("interrupted_release_defect", m),
        "init A\nrelease B\nrelease A\nasync defect: B broke\n"
    );
}
