//! 中断と再開（仕様 8.4，14.5，16.5）の出力．IR を手で組み立てて JS を出し，node で実行する．
//! 中断しうる関数はジェネレータになり，ランタイムの `$start` が進める．出力した JS は insta の
//! スナップショットでも見る．

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// `main` を呼び，ジェネレータなら `$start` で進めて結果を1行出す．起動用モジュール（emela-driver）
/// と同じ形で，`$start` が戻る前に終わったかどうか（`sync` / `async`）も出す．
///
/// - 値: `sync 42`，エラー: `async error: Oops`，defect: `sync defect: …`
/// - resume されないままイベントループが空になったか，その場で resume されずに捨てられた: `interrupted`
/// - ホストのコールバックの中で起きた defect: `defect: …`
const HARNESS: &str = r#"import * as m from "./main.mjs";
let over = false;
let sync = true;
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
  over = true;
  out("interrupted");
});
try {
  const v = m.main();
  if (Object.prototype.toString.call(v) === "[object Generator]") {
    m.$start(v, (ok, r) => {
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
        .join("codegen-js-suspend")
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

fn int_op(op: BinOp, a: Expr, b: Expr) -> Expr {
    bin(op, OpTy::Int, a, b)
}

/// `effect Clock { suspend fn sleep(ms: Int) -> Int }`．`sleep` は resume された値を返す．
fn clock(m: &mut Module) -> EffectId {
    m.add_effect(EffectDef {
        name: "Clock".into(),
        ops: vec![OpDef::suspend("sleep")],
    })
}

/// `handler name implements Clock { suspend fn sleep(ms, resume) { body(ms, resume) } }` の関数．
fn clock_handler(
    m: &mut Module,
    clock: EffectId,
    name: &str,
    body: impl FnOnce(&mut Module, Local, Local) -> Expr,
) -> FnId {
    let f = m.declare(name, false);
    let ms = m.local("ms");
    let resume = m.local("resume");
    let body = body(m, ms, resume);
    m.define(
        f,
        vec![],
        Expr::Handler {
            name: name.into(),
            effect: clock,
            ops: vec![lambda(vec![ms, resume], body)],
        },
    );
    f
}

/// `resume(v)`．
fn resume_with(resume: Local, v: Expr) -> Expr {
    call_value(var(resume), vec![v])
}

/// `h.sleep(ms)`．
fn sleep(h: Local, clock: EffectId, ms: i32) -> Expr {
    call_op(var(h), clock, 0, vec![int(ms)], false, true)
}

/// ホストの `setTimeout(callback, ms)` を呼ぶ外部関数．
fn set_timeout(m: &mut Module) -> FnId {
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

/// `export suspend fn main() { with H { body(h) } }`．
fn suspend_main(m: &mut Module, handler: FnId, body: impl FnOnce(&mut Module, Local) -> Expr) {
    let h = m.local("h");
    let body = body(m, h);
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(
        main,
        vec![],
        Expr::With {
            binds: vec![(h, call(handler, vec![]))],
            body: Box::new(body),
        },
    );
}

#[test]
fn immediate_resume_continues_without_suspending() {
    // その場で resume するハンドラは中断しない（14.5 の3）．`$start` が戻る前に終わる．
    // 10万回の自己末尾呼び出しのループの中で中断しうる呼び出しをしても，スタックは伸びない．
    let mut m = Module::new();
    let clock = clock(&mut m);
    let instant = clock_handler(&mut m, clock, "InstantClock", |_, ms, resume| {
        resume_with(resume, var(ms))
    });

    // suspend fn count(h, n, acc) = if n == 0 { acc } else { count(h, n - 1, acc + h.sleep(1)) }
    let count = m.declare("count", false);
    m.functions[count].suspends = true;
    let (h, n, acc) = (m.local("h"), m.local("n"), m.local("acc"));
    m.define(
        count,
        vec![h, n, acc],
        if_(
            bin(BinOp::Eq, OpTy::Int, var(n), int(0)),
            var(acc),
            call_suspends(
                count,
                vec![
                    var(h),
                    int_op(BinOp::Sub, var(n), int(1)),
                    int_op(BinOp::Add, var(acc), sleep(h, clock, 1)),
                ],
            ),
        ),
    );
    suspend_main(&mut m, instant, |_, h| {
        call_suspends(count, vec![var(h), int(100_000), int(0)])
    });
    assert_eq!(run("immediate_resume", m), "sync 100000\n");
}

#[test]
fn later_resume_continues_from_the_callback() {
    // コールバックの中で後から resume すると，中断していた呼び出し元が再開する（14.5 の2）．
    // 証拠の表から引いたハンドラでも，中断しうる関数の呼び出しを挟んでも同じ．
    let mut m = Module::new();
    let clock = clock(&mut m);
    let set_timeout = set_timeout(&mut m);
    let js_clock = clock_handler(&mut m, clock, "JsClock", |_, ms, resume| {
        call(
            set_timeout,
            vec![lambda(vec![], resume_with(resume, var(ms))), var(ms)],
        )
    });

    // suspend fn wait(ev) = (use Clock).sleep(5) + (use Clock).sleep(7)
    let wait = m.declare("wait", false);
    m.functions[wait].suspends = true;
    let ev = m.local("ev");
    let use_clock = || Expr::Use {
        ctx: Box::new(var(ev)),
        effect: clock,
    };
    m.define(
        wait,
        vec![ev],
        int_op(
            BinOp::Add,
            call_op(use_clock(), clock, 0, vec![int(5)], false, true),
            call_op(use_clock(), clock, 0, vec![int(7)], false, true),
        ),
    );
    suspend_main(&mut m, js_clock, |m, h| {
        let ctx = m.local("ev");
        let_(
            ctx,
            Expr::Provide {
                base: None,
                bindings: vec![(clock, var(h))],
            },
            int_op(BinOp::Mul, call_suspends(wait, vec![var(ctx)]), int(2)),
        )
    });
    assert_eq!(run("later_resume", m), "async 24\n");
}

#[test]
fn resuming_twice_in_place_is_a_defect() {
    // 2回目の resume は defect．文面にハンドラと操作の名前を出す（14.5）．
    let mut m = Module::new();
    let clock = clock(&mut m);
    let twice = clock_handler(&mut m, clock, "TwiceClock", |m, ms, resume| {
        let first = m.local("_");
        let_(
            first,
            resume_with(resume, var(ms)),
            resume_with(resume, var(ms)),
        )
    });
    suspend_main(&mut m, twice, |_, h| sleep(h, clock, 1));
    assert_eq!(
        run("resume_twice", m),
        "sync defect: resume called twice in operation `sleep` of handler `TwiceClock`\n"
    );
}

#[test]
fn resuming_twice_later_is_a_defect() {
    // コールバックの中の2回目の resume も defect．1回目の resume で呼び出し元は次の sleep まで進み，
    // 2回目はホストのコールバックの中で defect を投げる．
    let mut m = Module::new();
    let clock = clock(&mut m);
    let set_timeout = set_timeout(&mut m);
    let late = clock_handler(&mut m, clock, "LateTwiceClock", |m, ms, resume| {
        let first = m.local("_");
        call(
            set_timeout,
            vec![
                lambda(
                    vec![],
                    let_(
                        first,
                        resume_with(resume, var(ms)),
                        resume_with(resume, var(ms)),
                    ),
                ),
                int(0),
            ],
        )
    });
    suspend_main(&mut m, late, |_, h| {
        int_op(BinOp::Add, sleep(h, clock, 1), sleep(h, clock, 2))
    });
    assert_eq!(
        run("resume_twice_later", m),
        "defect: resume called twice in operation `sleep` of handler `LateTwiceClock`\n"
    );
}

#[test]
fn dropped_resume_interrupts_main() {
    // resume を呼ばずに捨てると，main は中断されたまま終わる（14.5）．
    let mut m = Module::new();
    let clock = clock(&mut m);
    let never = clock_handler(&mut m, clock, "NeverClock", |_, _, _| unit());
    suspend_main(&mut m, never, |_, h| sleep(h, clock, 1));
    assert_eq!(run("dropped_resume", m), "interrupted\n");
}

#[test]
fn failing_operation_fails_at_the_call() {
    // resume せずに失敗した操作の実装は，操作の呼び出し元へ失敗を返す（呼び出しの fails が真）．
    let mut m = Module::new();
    let oops = m.add_enum(EnumDef {
        name: "Oops".into(),
        params: 0,
        variants: vec![VariantDef::unit("Oops")],
        error: true,
    });
    let oops = CtorRef {
        enum_id: oops,
        variant: 0,
    };
    let clock = clock(&mut m);
    let failing = clock_handler(&mut m, clock, "FailingClock", |_, _, _| {
        fail(ctor(oops, vec![]))
    });
    suspend_main(&mut m, failing, |_, h| {
        int_op(
            BinOp::Add,
            escape(
                call_op(var(h), clock, 0, vec![int(1)], true, true),
                vec![arm(p_ctor(oops, vec![]), int(40))],
            ),
            call_op(var(h), clock, 0, vec![int(1)], true, true),
        )
    });
    assert_eq!(run("failing_operation", m), "sync error: Oops\n");
}

#[test]
fn external_promises_are_awaited() {
    // `@external suspend fn` はホストの返した Promise を待つ（12.2）．
    let mut m = Module::new();
    let resolve = external(
        &mut m,
        "resolve",
        "Promise.resolve",
        vec![Boundary::Plain],
        Boundary::Plain,
        true,
    );
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(
        main,
        vec![],
        int_op(BinOp::Add, call_suspends(resolve, vec![int(41)]), int(1)),
    );
    assert_eq!(run("external_promise", m), "async 42\n");
}

#[test]
fn rejected_promises_are_defects() {
    // 拒否された Promise は defect（12.5）．Error でない理由は `uncaught exception: …`．
    let mut m = Module::new();
    let reject = external(
        &mut m,
        "reject",
        "Promise.reject",
        vec![Boundary::Plain],
        Boundary::Unit,
        true,
    );
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(main, vec![], call_suspends(reject, vec![string("boom")]));
    assert_eq!(
        run("rejected_promise", m),
        "async defect: uncaught exception: boom\n"
    );
}

#[test]
fn suspending_function_values() {
    // 中断しうる呼び出しで呼ぶ関数の値は，中断しうるものでもしないものでもよい．
    // 中断しうる操作の実装（ジェネレータ）は，Promise を待ってから resume してよい．
    let mut m = Module::new();
    let clock = clock(&mut m);
    let resolve = external(
        &mut m,
        "resolve",
        "Promise.resolve",
        vec![Boundary::Plain],
        Boundary::Plain,
        true,
    );
    // suspend fn sleep(ms, resume) { resume(resolve(ms) * 10) }
    let awaiting = m.declare("AwaitingClock", false);
    let (ms, resume) = (m.local("ms"), m.local("resume"));
    m.define(
        awaiting,
        vec![],
        Expr::Handler {
            name: "AwaitingClock".into(),
            effect: clock,
            ops: vec![suspend_lambda(
                vec![ms, resume],
                resume_with(
                    resume,
                    int_op(BinOp::Mul, call_suspends(resolve, vec![var(ms)]), int(10)),
                ),
            )],
        },
    );

    // suspend fn apply(f) = f()
    let apply = m.declare("apply", false);
    m.functions[apply].suspends = true;
    let f = m.local("f");
    m.define(apply, vec![f], call_value_suspends(var(f), vec![]));

    suspend_main(&mut m, awaiting, |_, h| {
        int_op(
            BinOp::Add,
            call_suspends(apply, vec![suspend_lambda(vec![], sleep(h, clock, 4))]),
            call_suspends(apply, vec![lambda(vec![], int(2))]),
        )
    });
    assert_eq!(run("function_values", m), "async 42\n");
}

#[test]
fn functions_without_the_mark_are_unchanged() {
    // 印のない関数はジェネレータにせず，`$start` も公開しない．
    let mut m = Module::new();
    let main = m.declare("main", true);
    m.define(main, vec![], int(1));
    let js = emit_module(
        &m,
        &Options {
            runtime: RuntimeMode::Import(None),
        },
    );
    assert!(js.contains("function main() {"), "{js}");
    assert!(js.contains("export { main };"), "{js}");
    assert!(!js.contains("$start"), "{js}");
}

#[test]
fn immediate_contexts_run_suspending_calls_in_place() {
    // 中断しない関数（R が Immediate）の中でも，その場で resume するハンドラの with の中なら
    // 中断しうる呼び出しがある（8.7）．その場で最後まで進める．resume されなければ interrupted．
    let mut m = Module::new();
    let clock_effect = clock(&mut m);
    let instant = clock_handler(&mut m, clock_effect, "InstantClock", |_, ms, resume| {
        resume_with(resume, var(ms))
    });
    // suspend fn twice(h) = h.sleep(20) + h.sleep(1)
    let twice = m.declare("twice", false);
    m.functions[twice].suspends = true;
    let h = m.local("h");
    m.define(
        twice,
        vec![h],
        int_op(
            BinOp::Add,
            sleep(h, clock_effect, 20),
            sleep(h, clock_effect, 1),
        ),
    );
    let h = m.local("h");
    let main = m.declare("main", true);
    m.define(
        main,
        vec![],
        Expr::With {
            binds: vec![(h, call(instant, vec![]))],
            body: Box::new(int_op(
                BinOp::Add,
                call_suspends(twice, vec![var(h)]),
                sleep(h, clock_effect, 21),
            )),
        },
    );
    assert_eq!(run("sync_in_place", m), "42\n");

    let mut m = Module::new();
    let clock_effect = clock(&mut m);
    let never = clock_handler(&mut m, clock_effect, "NeverClock", |_, _, _| unit());
    let h = m.local("h");
    let main = m.declare("main", true);
    m.define(
        main,
        vec![],
        Expr::With {
            binds: vec![(h, call(never, vec![]))],
            body: Box::new(sleep(h, clock_effect, 1)),
        },
    );
    assert_eq!(run("sync_dropped", m), "interrupted\n");
}
