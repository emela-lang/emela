//! Suspension and resumption (spec 8.4, 14.5, 16.5). Builds IR by hand, emits JS and runs it on
//! node. Suspending functions become generators stepped by the runtime's `$start`. The emitted JS
//! is also snapshotted with insta.

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// Checks that the hand-built IR is well typed (`emela_core::verify`).
fn assert_well_typed(m: &Module) {
    if let Err(errors) = verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("ill-typed IR:\n{}", lines.join("\n"));
    }
}

/// Calls `main`, steps it with `$start` if it is a generator, and prints one result line. Like the
/// entry module (emela-driver), it also prints whether it finished before `$start` returned
/// (`sync` / `async`).
///
/// - a value: `sync 42`; an error: `async error: Oops`; a defect: `sync defect: …`
/// - the event loop emptied without a resume, or the continuation was dropped without an
///   immediate resume: `interrupted`
/// - a defect inside a host callback: `defect: …`
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
    assert_well_typed(&m);
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
        .expect("cannot start node");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "node failed:\n{stderr}\n{js}");
    insta::assert_snapshot!(name, js);
    String::from_utf8(out.stdout).unwrap()
}

fn int_op(op: BinOp, a: Expr, b: Expr) -> Expr {
    bin(op, OpTy::Int, a, b)
}

/// A function type that neither fails nor takes an evidence table.
fn fn_ty(params: Vec<TypeId>, ret: TypeId, suspends: bool) -> Type {
    Type::Fn(FnType {
        params,
        ret,
        fails: false,
        suspends,
        ev: false,
        sets: None,
    })
}

/// The type of resume in Clock's `sleep`: `fn(Int) -> ()`.
fn resume_ty() -> Type {
    fn_ty(vec![TypeId::INT], TypeId::UNIT, false)
}

/// The type of an implementation of Clock's `sleep`, which takes resume:
/// `fn(Int, fn(Int) -> ()) -> ()`.
fn sleep_ty(m: &mut Module, suspends: bool) -> TypeId {
    let resume = m.intern(resume_ty());
    m.intern(fn_ty(vec![TypeId::INT, resume], TypeId::UNIT, suspends))
}

/// `effect Clock { suspend fn sleep(ms: Int) -> Int }`. `sleep` returns the resumed value.
fn clock(m: &mut Module) -> EffectId {
    m.add_effect(EffectDef::new(
        "Clock",
        vec![OpDef::suspend("sleep", vec![TypeId::INT], TypeId::INT)],
    ))
}

/// The function of `handler Name: Clock { suspend fn sleep(ms, resume) { body(ms, resume) } }`.
fn clock_handler(
    m: &mut Module,
    clock: EffectId,
    name: &str,
    body: impl FnOnce(&mut Module, Local, Local) -> Expr,
) -> FnId {
    let f = m.declare(name, false);
    let ms = m.local("ms", TypeId::INT);
    let resume = m.local("resume", resume_ty());
    let body = body(m, ms, resume);
    let op_ty = sleep_ty(m, false);
    m.define(
        f,
        vec![],
        Type::Handler(clock),
        Expr::Handler {
            name: name.into(),
            effect: clock,
            ops: vec![lambda(vec![ms, resume], op_ty, body)],
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

/// An external function calling the host's `setTimeout(callback, ms)`.
fn set_timeout(m: &mut Module) -> FnId {
    let callback = m.intern(fn_ty(vec![], TypeId::UNIT, false));
    external(
        m,
        "set_timeout",
        "setTimeout",
        vec![callback, TypeId::INT],
        TypeId::UNIT,
        false,
    )
}

/// An external function calling the host function `path` (a global reference). With `suspends`,
/// it is an `@external suspend fn`.
fn external(
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

/// `export suspend fn main() -> Int { with H { body(h) } }`．
fn suspend_main(
    m: &mut Module,
    clock: EffectId,
    handler: FnId,
    body: impl FnOnce(&mut Module, Local) -> Expr,
) {
    let h = m.local("h", Type::Handler(clock));
    let body = body(m, h);
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(
        main,
        vec![],
        TypeId::INT,
        Expr::With {
            binds: vec![WithBind::new(h, call(handler, vec![]))],
            body: Box::new(body),
        },
    );
}

#[test]
fn immediate_resume_continues_without_suspending() {
    // A handler that resumes immediately does not suspend (spec 14.5, item 3). Suspending calls
    // inside a 100,000-iteration self tail call loop do not grow the stack. The loop yields for
    // fairness now and then, through the host's event loop too (spec 11.1), so it finishes after
    // `$start` returns.
    let mut m = Module::new();
    let clock = clock(&mut m);
    let instant = clock_handler(&mut m, clock, "InstantClock", |_, ms, resume| {
        resume_with(resume, var(ms))
    });

    // suspend fn count(h, n, acc) = if n == 0 { acc } else { count(h, n - 1, acc + h.sleep(1)) }
    let count = m.declare("count", false);
    m.functions[count].suspends = true;
    let (h, n, acc) = (
        m.local("h", Type::Handler(clock)),
        m.local("n", TypeId::INT),
        m.local("acc", TypeId::INT),
    );
    m.define(
        count,
        vec![h, n, acc],
        TypeId::INT,
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
    suspend_main(&mut m, clock, instant, |_, h| {
        call_suspends(count, vec![var(h), int(100_000), int(0)])
    });
    assert_eq!(run("immediate_resume", m), "async 100000\n");
}

#[test]
fn later_resume_continues_from_the_callback() {
    // Resuming later from a callback resumes the suspended caller (spec 14.5, item 2). The same
    // holds for a handler taken from the evidence table, and across a suspending function call.
    let mut m = Module::new();
    let clock = clock(&mut m);
    let set_timeout = set_timeout(&mut m);
    let js_clock = clock_handler(&mut m, clock, "JsClock", |m, ms, resume| {
        let callback = m.fn_type(vec![], TypeId::UNIT);
        call(
            set_timeout,
            vec![
                lambda(vec![], callback, resume_with(resume, var(ms))),
                var(ms),
            ],
        )
    });

    // suspend fn wait(ev) = (use Clock).sleep(5) + (use Clock).sleep(7)
    let wait = m.declare("wait", false);
    m.functions[wait].suspends = true;
    let ev = m.local("ev", TypeId::EVIDENCE);
    let use_clock = || Expr::Use {
        ctx: Box::new(var(ev)),
        effect: clock,
    };
    m.define(
        wait,
        vec![ev],
        TypeId::INT,
        int_op(
            BinOp::Add,
            call_op(use_clock(), clock, 0, vec![int(5)], false, true),
            call_op(use_clock(), clock, 0, vec![int(7)], false, true),
        ),
    );
    suspend_main(&mut m, clock, js_clock, |m, h| {
        let ctx = m.local("ev", TypeId::EVIDENCE);
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
    // A second resume is a defect whose message names the handler and operation (spec 14.5).
    let mut m = Module::new();
    let clock = clock(&mut m);
    let twice = clock_handler(&mut m, clock, "TwiceClock", |m, ms, resume| {
        let first = m.local("_", TypeId::UNIT);
        let_(
            first,
            resume_with(resume, var(ms)),
            resume_with(resume, var(ms)),
        )
    });
    suspend_main(&mut m, clock, twice, |_, h| sleep(h, clock, 1));
    assert_eq!(
        run("resume_twice", m),
        "sync defect: resume called twice in operation `sleep` of handler `TwiceClock`\n"
    );
}

#[test]
fn resuming_twice_later_is_a_defect() {
    // A second resume inside a callback is also a defect. The first resume runs the caller up to
    // the next sleep, and the second throws a defect inside the host callback.
    let mut m = Module::new();
    let clock = clock(&mut m);
    let set_timeout = set_timeout(&mut m);
    let late = clock_handler(&mut m, clock, "LateTwiceClock", |m, ms, resume| {
        let first = m.local("_", TypeId::UNIT);
        let callback = m.fn_type(vec![], TypeId::UNIT);
        call(
            set_timeout,
            vec![
                lambda(
                    vec![],
                    callback,
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
    suspend_main(&mut m, clock, late, |_, h| {
        int_op(BinOp::Add, sleep(h, clock, 1), sleep(h, clock, 2))
    });
    assert_eq!(
        run("resume_twice_later", m),
        "defect: resume called twice in operation `sleep` of handler `LateTwiceClock`\n"
    );
}

#[test]
fn dropped_resume_interrupts_main() {
    // Dropping resume without calling it leaves main suspended (spec 14.5).
    let mut m = Module::new();
    let clock = clock(&mut m);
    let never = clock_handler(&mut m, clock, "NeverClock", |_, _, _| unit());
    suspend_main(&mut m, clock, never, |_, h| sleep(h, clock, 1));
    assert_eq!(run("dropped_resume", m), "interrupted\n");
}

#[test]
fn failing_operation_fails_at_the_call() {
    // An operation implementation that fails without resuming returns the failure to the
    // operation's caller (the call's `fails` is true).
    let mut m = Module::new();
    let oops = m.add_enum(EnumDef {
        name: "Oops".into(),
        params: 0,
        variants: vec![VariantDef::unit("Oops")],
        error: true,
        external: false,
    });
    let oops = CtorRef {
        enum_id: oops,
        variant: 0,
    };
    let clock = clock(&mut m);
    let failing = clock_handler(&mut m, clock, "FailingClock", |_, _, _| {
        fail(ctor(oops, vec![]))
    });
    suspend_main(&mut m, clock, failing, |_, h| {
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
    // `@external suspend fn` awaits the Promise the host returns (spec 12.2).
    let mut m = Module::new();
    let resolve = external(
        &mut m,
        "resolve",
        "Promise.resolve",
        vec![TypeId::INT],
        TypeId::INT,
        true,
    );
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(
        main,
        vec![],
        TypeId::INT,
        int_op(BinOp::Add, call_suspends(resolve, vec![int(41)]), int(1)),
    );
    assert_eq!(run("external_promise", m), "async 42\n");
}

#[test]
fn rejected_promises_are_defects() {
    // A rejected Promise is a defect (spec 12.5). A reason that is not an Error shows as
    // `uncaught exception: …`.
    let mut m = Module::new();
    let reject = external(
        &mut m,
        "reject",
        "Promise.reject",
        vec![TypeId::STRING],
        TypeId::UNIT,
        true,
    );
    let main = m.declare("main", true);
    m.functions[main].suspends = true;
    m.define(
        main,
        vec![],
        TypeId::UNIT,
        call_suspends(reject, vec![string("boom")]),
    );
    assert_eq!(
        run("rejected_promise", m),
        "async defect: uncaught exception: boom\n"
    );
}

#[test]
fn suspending_function_values() {
    // A function value called by a suspending call may or may not suspend. A suspending operation
    // implementation (a generator) may await a Promise before resuming.
    let mut m = Module::new();
    let clock = clock(&mut m);
    let resolve = external(
        &mut m,
        "resolve",
        "Promise.resolve",
        vec![TypeId::INT],
        TypeId::INT,
        true,
    );
    // suspend fn sleep(ms, resume) { resume(resolve(ms) * 10) }
    let awaiting = m.declare("AwaitingClock", false);
    let (ms, resume) = (m.local("ms", TypeId::INT), m.local("resume", resume_ty()));
    let op_ty = sleep_ty(&mut m, true);
    m.define(
        awaiting,
        vec![],
        Type::Handler(clock),
        Expr::Handler {
            name: "AwaitingClock".into(),
            effect: clock,
            ops: vec![suspend_lambda(
                vec![ms, resume],
                op_ty,
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
    let f = m.local("f", fn_ty(vec![], TypeId::INT, true));
    m.define(
        apply,
        vec![f],
        TypeId::INT,
        call_value_suspends(var(f), vec![]),
    );

    suspend_main(&mut m, clock, awaiting, |m, h| {
        let suspending = m.intern(fn_ty(vec![], TypeId::INT, true));
        let immediate = m.intern(fn_ty(vec![], TypeId::INT, false));
        int_op(
            BinOp::Add,
            call_suspends(
                apply,
                vec![suspend_lambda(vec![], suspending, sleep(h, clock, 4))],
            ),
            call_suspends(apply, vec![lambda(vec![], immediate, int(2))]),
        )
    });
    assert_eq!(run("function_values", m), "async 42\n");
}

#[test]
fn functions_without_the_mark_are_unchanged() {
    // Unmarked functions are not generators, and `$start` is not exported.
    let mut m = Module::new();
    let main = m.declare("main", true);
    m.define(main, vec![], TypeId::INT, int(1));
    assert_well_typed(&m);
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
    // Suspending calls can appear in a non-suspending function (R is Immediate) inside a `with`
    // of an immediately resuming handler (spec 8.7). They run to completion on the spot; without
    // a resume, the result is interrupted.
    let mut m = Module::new();
    let clock_effect = clock(&mut m);
    let instant = clock_handler(&mut m, clock_effect, "InstantClock", |_, ms, resume| {
        resume_with(resume, var(ms))
    });
    // suspend fn twice(h) = h.sleep(20) + h.sleep(1)
    let twice = m.declare("twice", false);
    m.functions[twice].suspends = true;
    let h = m.local("h", Type::Handler(clock_effect));
    m.define(
        twice,
        vec![h],
        TypeId::INT,
        int_op(
            BinOp::Add,
            sleep(h, clock_effect, 20),
            sleep(h, clock_effect, 1),
        ),
    );
    let h = m.local("h", Type::Handler(clock_effect));
    let main = m.declare("main", true);
    m.define(
        main,
        vec![],
        TypeId::INT,
        Expr::With {
            binds: vec![WithBind::new(h, call(instant, vec![]))],
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
    let h = m.local("h", Type::Handler(clock_effect));
    let main = m.declare("main", true);
    m.define(
        main,
        vec![],
        TypeId::INT,
        Expr::With {
            binds: vec![WithBind::new(h, call(never, vec![]))],
            body: Box::new(sleep(h, clock_effect, 1)),
        },
    );
    assert_eq!(run("sync_dropped", m), "interrupted\n");
}
