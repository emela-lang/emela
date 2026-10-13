//! Fibers (spec chapter 11). Builds IR by hand, emits JS and runs it on node. Checks the order of
//! lines printed with the host's `console.log` and the result of `main`. The emitted JS is also
//! snapshotted with insta.

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// Calls `main`, steps it with `$start` if it is a generator, and prints one result line. Like the
/// entry module (emela-driver), when the event loop empties without a resume, it unwinds with the
/// function `$start` returned.
///
/// - a value: `42` (a suspending main prints `async 42`); an error: `error: Oops`; a defect:
///   `defect: …`
/// - unwound while suspended: `interrupted`
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
        .join("codegen-js-fiber")
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

/// Parts for building IR by hand.
struct Kit {
    /// An external function calling `console.log(x)`.
    log: FnId,
    /// `effect Res { fn name() -> String }`．
    res: EffectId,
    /// `effect Clock { suspend fn sleep(ms: Int) -> () }`．
    clock: EffectId,
    /// A `JsClock` that resumes after `setTimeout`.
    js_clock: FnId,
    /// A `NeverClock` that drops resume without calling it.
    never_clock: FnId,
    /// `error Oops`．
    oops: CtorRef,
    /// `enum Exit[A, E] { Success(A), Failure(Cause[E]) }`．
    exit: EnumId,
    /// `enum CauseKind { Fail, Die, Interrupt }`．
    kind: EnumId,
}

impl Kit {
    fn new(m: &mut Module) -> Kit {
        let log = external(
            m,
            "log",
            "console.log",
            vec![Boundary::Plain],
            Boundary::Unit,
        );
        let res = m.add_effect(EffectDef {
            name: "Res".into(),
            ops: vec![OpDef::new("name")],
        });
        let clock = m.add_effect(EffectDef {
            name: "Clock".into(),
            ops: vec![OpDef::suspend("sleep")],
        });
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
        );
        let js_clock = m.declare("JsClock", false);
        let (ms, resume) = (m.local("ms"), m.local("resume"));
        let body = call(
            set_timeout,
            vec![
                lambda(vec![], call_value(var(resume), vec![unit()])),
                var(ms),
            ],
        );
        m.define(
            js_clock,
            vec![],
            Expr::Handler {
                name: "JsClock".into(),
                effect: clock,
                ops: vec![lambda(vec![ms, resume], body)],
            },
        );
        let never_clock = m.declare("NeverClock", false);
        let (ms, resume) = (m.local("ms"), m.local("resume"));
        m.define(
            never_clock,
            vec![],
            Expr::Handler {
                name: "NeverClock".into(),
                effect: clock,
                ops: vec![lambda(vec![ms, resume], unit())],
            },
        );
        let oops = m.add_enum(EnumDef {
            name: "Oops".into(),
            params: 0,
            variants: vec![VariantDef::unit("Oops")],
            error: true,
        });
        // A Cause is opaque; its field type is never shown here.
        let exit = m.add_enum(EnumDef {
            name: "Exit".into(),
            params: 2,
            variants: vec![
                VariantDef::positional("Success", vec![Type::Param(0)]),
                VariantDef::positional("Failure", vec![Type::Fn]),
            ],
            error: false,
        });
        let kind = m.add_enum(EnumDef {
            name: "CauseKind".into(),
            params: 0,
            variants: vec![
                VariantDef::unit("Fail"),
                VariantDef::unit("Die"),
                VariantDef::unit("Interrupt"),
            ],
            error: false,
        });
        Kit {
            log,
            res,
            clock,
            js_clock,
            never_clock,
            oops: CtorRef {
                enum_id: oops,
                variant: 0,
            },
            exit,
            kind,
        }
    }

    /// `log(text)`．
    fn log(&self, text: &str) -> Expr {
        call(self.log, vec![string(text)])
    }

    /// `log(text); then`．
    fn log_then(&self, m: &mut Module, text: &str, then: Expr) -> Expr {
        self.then(m, self.log(text), then)
    }

    /// `first; then`．
    fn then(&self, m: &mut Module, first: Expr, then: Expr) -> Expr {
        let x = m.local("_");
        let_(x, first, then)
    }

    /// `h.sleep(ms)` with the clock in `c`.
    fn sleep(&self, c: Local, ms: i32) -> Expr {
        call_op(var(c), self.clock, 0, vec![int(ms)], false, true)
    }

    /// `with c = JsClock, n = NeverClock { body }`．
    fn clocks(&self, c: Local, n: Local, body: Expr) -> Expr {
        with(
            vec![
                WithBind::new(c, call(self.js_clock, vec![])),
                WithBind::new(n, call(self.never_clock, vec![])),
            ],
            body,
        )
    }

    /// The value of `handler name implements Res { fn name() { "name" } }`.
    fn handler(&self, name: &str) -> Expr {
        Expr::Handler {
            name: name.into(),
            effect: self.res,
            ops: vec![lambda(vec![], string(name))],
        }
    }

    /// `with` a handler that prints `init <name>` on init and `release <name>` on release.
    fn managed(&self, m: &mut Module, name: &str, body: Expr) -> Expr {
        let h = m.local("h");
        let init = self.log_then(m, &format!("init {name}"), self.handler(name));
        let bind = WithBind::managed(h, init, name, self.log(&format!("release {name}")));
        with(vec![bind], body)
    }

    /// `with` a handler whose release prints `release <name>` and then calls `panic(message)`.
    fn breaking(&self, m: &mut Module, name: &str, message: &str, body: Expr) -> Expr {
        let h = m.local("h");
        let release = self.log_then(
            m,
            &format!("release {name}"),
            builtin(Builtin::Panic, vec![string(message)]),
        );
        with(
            vec![WithBind::managed(h, self.handler(name), name, release)],
            body,
        )
    }

    fn oops(&self) -> Expr {
        fail(ctor(self.oops, vec![]))
    }

    /// `Fiber.scope(fn(s) { body })`．
    fn scope(&self, s: Local, fails: bool, body: Expr) -> Expr {
        fiber(
            FiberOp::Scope { fails },
            vec![suspend_lambda(vec![s], body)],
        )
    }

    /// `s.fork(fn() { body })`．
    fn fork(&self, s: Local, body: Expr) -> Expr {
        fiber(FiberOp::Fork, vec![var(s), suspend_lambda(vec![], body)])
    }

    /// `Fiber.attempt(fn() { body })`．
    fn attempt(&self, suspends: bool, body: Expr) -> Expr {
        let f = if suspends {
            suspend_lambda(vec![], body)
        } else {
            lambda(vec![], body)
        };
        fiber(
            FiberOp::Attempt {
                exit: self.exit,
                suspends,
            },
            vec![f],
        )
    }

    /// `Fiber.await(h)`．
    fn await_(&self, h: Expr) -> Expr {
        fiber(FiberOp::Await { exit: self.exit }, vec![h])
    }

    /// Describes an Exit: `Success <value>` (an Int) or `<kind> <Cause.to_string>`.
    fn describe(&self, m: &mut Module, exit: Expr) -> Expr {
        let (v, c) = (m.local("v"), m.local("c"));
        let kind = |k: usize| CtorRef {
            enum_id: self.kind,
            variant: k,
        };
        let kind_name = match_(
            fiber(FiberOp::CauseKind { kind: self.kind }, vec![var(c)]),
            vec![
                arm(p_ctor(kind(CAUSE_FAIL), vec![]), string("Fail")),
                arm(p_ctor(kind(CAUSE_DIE), vec![]), string("Die")),
                arm(p_ctor(kind(CAUSE_INTERRUPT), vec![]), string("Interrupt")),
            ],
        );
        let exit_ctor = |k: usize| CtorRef {
            enum_id: self.exit,
            variant: k,
        };
        match_(
            exit,
            vec![
                arm(
                    p_ctor(exit_ctor(EXIT_SUCCESS), vec![Pat::Bind(v)]),
                    Expr::Concat(vec![lit_part("Success "), value_part(var(v), Type::Int)]),
                ),
                arm(
                    p_ctor(exit_ctor(EXIT_FAILURE), vec![Pat::Bind(c)]),
                    Expr::Concat(vec![
                        value_part(kind_name, Type::String),
                        lit_part(" "),
                        value_part(fiber(FiberOp::CauseToString, vec![var(c)]), Type::String),
                    ]),
                ),
            ],
        )
    }

    /// `log(describe(exit))`．
    fn log_exit(&self, m: &mut Module, exit: Expr) -> Expr {
        let d = self.describe(m, exit);
        call(self.log, vec![d])
    }
}

/// An external function calling the host function `path` (a global reference).
fn external(m: &mut Module, name: &str, path: &str, params: Vec<Boundary>, ret: Boundary) -> FnId {
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

/// Statements in order: `a; b; …; last`.
fn seq(k: &Kit, m: &mut Module, mut es: Vec<Expr>) -> Expr {
    let mut body = es.pop().unwrap();
    while let Some(e) = es.pop() {
        body = k.then(m, e, body);
    }
    body
}

fn add(a: Expr, b: Expr) -> Expr {
    bin(BinOp::Add, OpTy::Int, a, b)
}

#[test]
fn children_run_concurrently_and_are_joined() {
    // The scope waits only where the fibers wait: b's shorter sleep ends first (11.1, 11.2).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s) = (m.local("c"), m.local("n"), m.local("s"));
    let (a, b) = (m.local("a"), m.local("b"));
    let (x, y) = (m.local("x"), m.local("y"));
    let child_a = seq(&k, &mut m, vec![k.sleep(c, 20), k.log("a"), int(1)]);
    let child_b = seq(&k, &mut m, vec![k.sleep(c, 10), k.log("b"), int(2)]);
    let joined = k.log_then(&mut m, "joined", add(var(x), var(y)));
    let body = let_(
        a,
        k.fork(s, child_a),
        let_(
            b,
            k.fork(s, child_b),
            let_(
                x,
                fiber(FiberOp::Join, vec![var(a)]),
                let_(y, fiber(FiberOp::Join, vec![var(b)]), joined),
            ),
        ),
    );
    main_fn(&mut m, true, k.clocks(c, n, k.scope(s, false, body)));
    assert_eq!(run("join", m), "b\na\njoined\nasync 3\n");
}

#[test]
fn the_body_value_interrupts_the_remaining_children() {
    // When the body returns, the children still running are interrupted, and the scope returns
    // after their releases (11.2, 11.4).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s) = (m.local("c"), m.local("n"), m.local("s"));
    let stuck = k.managed(&mut m, "A", k.sleep(n, 1));
    let quick = k.log("quick");
    let body = seq(
        &k,
        &mut m,
        vec![
            k.fork(s, stuck),
            k.fork(s, quick),
            k.sleep(c, 10),
            k.log("body done"),
            int(7),
        ],
    );
    let r = m.local("r");
    let main = let_(
        r,
        k.scope(s, false, body),
        k.log_then(&mut m, "after", var(r)),
    );
    main_fn(&mut m, true, k.clocks(c, n, main));
    assert_eq!(
        run("body_value", m),
        "init A\nquick\nbody done\nrelease A\nafter\nasync 7\n"
    );
}

#[test]
fn a_failing_child_interrupts_its_siblings_and_the_body() {
    // A child's error interrupts the siblings and the body, and becomes the scope's error (11.2).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s) = (m.local("c"), m.local("n"), m.local("s"));
    let failing = seq(&k, &mut m, vec![k.sleep(c, 10), k.oops()]);
    let stuck = k.managed(&mut m, "A", k.sleep(n, 1));
    let inner = seq(
        &k,
        &mut m,
        vec![k.fork(s, failing), k.fork(s, stuck), k.sleep(c, 50), int(1)],
    );
    let body = k.managed(&mut m, "B", inner);
    main_fn(&mut m, true, k.clocks(c, n, k.scope(s, true, body)));
    assert_eq!(
        run("child_fails", m),
        "init B\ninit A\nrelease A\nrelease B\nasync error: Oops\n"
    );
}

#[test]
fn later_failures_are_added_as_notes() {
    // The first failure is the result; a sibling failing while it unwinds is a note (11.2).
    for (name, first, expected) in [
        (
            "notes_defect",
            false,
            "release B\nasync defect: first\nnote: another fiber also failed: defect: second\n",
        ),
        (
            "notes_error",
            true,
            "release B\nasync error: Oops\nnote: another fiber also failed: defect: second\n",
        ),
    ] {
        let mut m = Module::new();
        let k = Kit::new(&mut m);
        let (c, n, s) = (m.local("c"), m.local("n"), m.local("s"));
        let failure = if first {
            k.oops()
        } else {
            builtin(Builtin::Panic, vec![string("first")])
        };
        let failing = seq(&k, &mut m, vec![k.sleep(c, 10), failure]);
        let breaking = k.breaking(&mut m, "B", "second", k.sleep(n, 1));
        let body = seq(
            &k,
            &mut m,
            vec![
                k.fork(s, failing),
                k.fork(s, breaking),
                k.sleep(c, 50),
                int(1),
            ],
        );
        main_fn(&mut m, true, k.clocks(c, n, k.scope(s, true, body)));
        assert_eq!(run(name, m), expected);
    }
}

#[test]
fn interrupt_waits_for_the_unwinding_and_join_is_a_defect() {
    // `Fiber.interrupt` returns after the child's release (11.4). await sees the Interrupt, and
    // join of an interrupted child is a defect (11.5).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, h) = (m.local("c"), m.local("n"), m.local("s"), m.local("h"));
    let stuck = k.managed(&mut m, "A", k.sleep(n, 1));
    let awaited = k.log_exit(&mut m, k.await_(var(h)));
    let rest = seq(
        &k,
        &mut m,
        vec![
            k.sleep(c, 1),
            fiber(FiberOp::Interrupt, vec![var(h)]),
            k.log("interrupt returned"),
            awaited,
            fiber(FiberOp::Interrupt, vec![var(h)]),
            fiber(FiberOp::Join, vec![var(h)]),
        ],
    );
    let body = let_(h, k.fork(s, stuck), rest);
    main_fn(&mut m, true, k.clocks(c, n, k.scope(s, false, body)));
    assert_eq!(
        run("interrupt", m),
        "init A\nrelease A\ninterrupt returned\nInterrupt interrupted\n\
         async defect: fiber was interrupted\n"
    );
}

#[test]
fn attempt_turns_failures_into_values() {
    // attempt is the boundary that turns Fail and Die into values; raise throws them again
    // (11.5).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let fails = k.attempt(false, k.oops());
    let dies = k.attempt(false, builtin(Builtin::Panic, vec![string("boom")]));
    let ok = k.attempt(false, int(5));
    let (e, cause) = (m.local("e"), m.local("cause"));
    let raised = escape(
        fiber(FiberOp::CauseRaise, vec![var(cause)]),
        vec![arm(p_ctor(k.oops, vec![]), k.log("caught Oops"))],
    );
    let failure = CtorRef {
        enum_id: k.exit,
        variant: EXIT_FAILURE,
    };
    let reraise = let_(
        e,
        k.attempt(false, k.oops()),
        match_(
            var(e),
            vec![
                arm(p_ctor(failure, vec![Pat::Bind(cause)]), raised),
                arm(Pat::Wild, unit()),
            ],
        ),
    );
    let lines = vec![
        k.log_exit(&mut m, fails),
        k.log_exit(&mut m, dies),
        k.log_exit(&mut m, ok),
        reraise,
        int(0),
    ];
    let body = seq(&k, &mut m, lines);
    main_fn(&mut m, false, body);
    assert_eq!(
        run("attempt", m),
        "Fail error: Oops\nDie defect: boom\nSuccess 5\ncaught Oops\n0\n"
    );
}

#[test]
fn attempt_does_not_catch_its_own_interruption() {
    // A child inside attempt is still interrupted when the scope ends (11.5).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s) = (m.local("c"), m.local("n"), m.local("s"));
    let attempted = k.attempt(true, k.sleep(n, 1));
    let child = seq(
        &k,
        &mut m,
        vec![attempted, k.log("attempt returned"), unit()],
    );
    let body = seq(
        &k,
        &mut m,
        vec![k.fork(s, child), k.sleep(c, 1), k.log("body done"), int(0)],
    );
    main_fn(&mut m, true, k.clocks(c, n, k.scope(s, false, body)));
    assert_eq!(run("attempt_interrupt", m), "body done\nasync 0\n");
}

#[test]
fn deferred_takes_the_first_value() {
    // The first complete wins and wakes the waiting body; later ones return False (11.7).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, d) = (m.local("c"), m.local("n"), m.local("s"), m.local("d"));
    let (r, v, again) = (m.local("r"), m.local("v"), m.local("again"));
    let complete = |d: Local, text: &str| fiber(FiberOp::Complete, vec![var(d), string(text)]);
    let report =
        |label: &str, r: Local| Expr::Concat(vec![lit_part(label), value_part(var(r), Type::Bool)]);
    let r2 = m.local("r");
    let slow = let_(
        r2,
        complete(d, "slow"),
        call(k.log, vec![report("slow ", r2)]),
    );
    let slow = k.then(&mut m, k.sleep(c, 30), slow);
    let fast = let_(
        r,
        complete(d, "fast"),
        call(k.log, vec![report("fast ", r)]),
    );
    let fast = k.then(&mut m, k.sleep(c, 10), fast);
    let option = m.option_enum();
    let empty = match_(
        fiber(FiberOp::Poll, vec![var(d)]),
        vec![
            arm(
                p_ctor(
                    CtorRef {
                        enum_id: option,
                        variant: OPTION_NONE,
                    },
                    vec![],
                ),
                string("empty"),
            ),
            arm(Pat::Wild, string("full")),
        ],
    );
    let after = let_(
        v,
        fiber(FiberOp::DeferredAwait, vec![var(d)]),
        let_(
            again,
            complete(d, "again"),
            seq(
                &k,
                &mut m,
                vec![
                    call(k.log, vec![var(v)]),
                    call(k.log, vec![report("again ", again)]),
                    var(v),
                ],
            ),
        ),
    );
    let body = let_(
        d,
        fiber(FiberOp::Deferred, vec![var(s)]),
        seq(
            &k,
            &mut m,
            vec![
                k.fork(s, slow),
                k.fork(s, fast),
                call(k.log, vec![empty]),
                after,
            ],
        ),
    );
    main_fn(&mut m, true, k.clocks(c, n, k.scope(s, false, body)));
    assert_eq!(
        run("deferred", m),
        "empty\nfast True\nfast\nagain False\nasync fast\n"
    );
}

#[test]
fn using_a_scope_after_it_ended_is_a_defect() {
    // What escapes a scope at run time cannot be used after it ended (11.8).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (s1, s2, s3) = (m.local("s"), m.local("s"), m.local("s"));
    let (x, h, d) = (m.local("x"), m.local("h"), m.local("d"));
    let fork_after = let_(x, k.scope(s1, false, var(s1)), k.fork(x, int(1)));
    let join_after = let_(
        h,
        k.scope(s2, false, k.fork(s2, int(1))),
        fiber(FiberOp::Join, vec![var(h)]),
    );
    let complete_after = let_(
        d,
        k.scope(s3, false, fiber(FiberOp::Deferred, vec![var(s3)])),
        fiber(FiberOp::Complete, vec![var(d), int(1)]),
    );
    let (s4, d2) = (m.local("s"), m.local("d"));
    let poll_after = let_(
        d2,
        k.scope(s4, false, fiber(FiberOp::Deferred, vec![var(s4)])),
        let_(m.local("_"), fiber(FiberOp::Poll, vec![var(d2)]), int(1)),
    );
    let lines = vec![
        k.log_exit(&mut m, k.attempt(true, fork_after)),
        k.log_exit(&mut m, k.attempt(true, join_after)),
        k.log_exit(&mut m, k.attempt(true, complete_after)),
        k.log_exit(&mut m, k.attempt(true, poll_after)),
        int(0),
    ];
    let body = seq(&k, &mut m, lines);
    main_fn(&mut m, true, body);
    assert_eq!(
        run("after_scope", m),
        "Die defect: fiber scope was used after it ended\n\
         Die defect: fiber was joined after its scope ended\n\
         Die defect: deferred was completed after its scope ended\n\
         Die defect: deferred was polled after its scope ended\n\
         sync 0\n"
    );
}

#[test]
fn a_dropped_continuation_in_a_child_interrupts_only_that_child() {
    // A suspend operation dropped inside a non-suspending function (`$sync`) unwinds that fiber
    // as interrupted; the scope goes on (11.5, 14.5).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (n, s, h) = (m.local("n"), m.local("s"), m.local("h"));
    let inner = m.declare("inner", false);
    let n2 = m.local("n");
    let inner_body = with(
        vec![WithBind::new(n2, call(k.never_clock, vec![]))],
        k.sleep(n2, 1),
    );
    m.define(inner, vec![], inner_body);
    let child = fiber(
        FiberOp::Fork,
        vec![var(s), lambda(vec![], call(inner, vec![]))],
    );
    let awaited = k.log_exit(&mut m, k.await_(var(h)));
    let body = let_(h, child, seq(&k, &mut m, vec![awaited, int(0)]));
    let main = with(
        vec![WithBind::new(n, call(k.never_clock, vec![]))],
        k.scope(s, false, body),
    );
    main_fn(&mut m, true, main);
    assert_eq!(run("sync_child", m), "Interrupt interrupted\nsync 0\n");
}

#[test]
fn a_running_release_is_not_interrupted() {
    // The scope ends while a child runs a suspending release; the interruption waits for it
    // (11.4).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, a) = (m.local("c"), m.local("n"), m.local("s"), m.local("a"));
    let release = seq(
        &k,
        &mut m,
        vec![k.log("release A"), k.sleep(c, 20), k.log("released A")],
    );
    let child_body = seq(&k, &mut m, vec![k.sleep(c, 5), k.log("child done"), unit()]);
    let child = with(
        vec![WithBind::managed(a, k.handler("A"), "A", release)],
        child_body,
    );
    let body = seq(&k, &mut m, vec![k.fork(s, child), k.sleep(c, 10), int(0)]);
    let r = m.local("r");
    let main = let_(
        r,
        k.scope(s, false, body),
        k.log_then(&mut m, "after scope", var(r)),
    );
    main_fn(&mut m, true, k.clocks(c, n, main));
    assert_eq!(
        run("release_hold", m),
        "child done\nrelease A\nreleased A\nafter scope\nasync 0\n"
    );
}

#[test]
fn an_inner_scope_failure_only_unwinds_its_body() {
    // A child failure interrupts the body of its own scope, not the outer one (11.2).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, s2, o) = (
        m.local("c"),
        m.local("n"),
        m.local("s"),
        m.local("s2"),
        m.local("o"),
    );
    let outer_child_body = seq(
        &k,
        &mut m,
        vec![k.sleep(c, 30), k.log("outer child done"), unit()],
    );
    let outer_child = k.managed(&mut m, "O", outer_child_body);
    let failing = seq(&k, &mut m, vec![k.sleep(c, 5), k.oops()]);
    let inner_body = k.managed(&mut m, "I", k.sleep(c, 100));
    let inner = seq(&k, &mut m, vec![k.fork(s2, failing), inner_body]);
    let attempted = k.attempt(true, k.scope(s2, true, inner));
    let logged = k.log_exit(&mut m, attempted);
    let rest = seq(
        &k,
        &mut m,
        vec![logged, fiber(FiberOp::Join, vec![var(o)]), string("end")],
    );
    let body = let_(o, k.fork(s, outer_child), rest);
    main_fn(&mut m, true, k.clocks(c, n, k.scope(s, false, body)));
    assert_eq!(
        run("nested", m),
        "init I\ninit O\nrelease I\nFail error: Oops\nouter child done\nrelease O\nasync end\n"
    );
}

#[test]
fn a_dropped_main_unwinds_its_children() {
    // When nothing can resume any fiber, the run unwinds as interrupted, children included
    // (14.5).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, h) = (m.local("c"), m.local("n"), m.local("s"), m.local("h"));
    let stuck = k.managed(&mut m, "A", k.sleep(n, 1));
    let body = let_(h, k.fork(s, stuck), fiber(FiberOp::Join, vec![var(h)]));
    let main = k.managed(&mut m, "M", k.scope(s, false, body));
    main_fn(&mut m, true, k.clocks(c, n, main));
    assert_eq!(
        run("dropped_main", m),
        "init M\ninit A\nrelease A\nrelease M\nasync interrupted\n"
    );
}

#[test]
fn fiber_operations_are_dumped() {
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (s, h) = (m.local("s"), m.local("h"));
    let body = let_(
        h,
        k.fork(s, int(1)),
        let_(
            m.local("_"),
            k.await_(var(h)),
            fiber(FiberOp::Join, vec![var(h)]),
        ),
    );
    main_fn(&mut m, true, k.scope(s, true, body));
    let text = dump(&m);
    let main = &text[text.find("export suspend fn main").unwrap()..];
    insta::assert_snapshot!("dump", main);
}

#[test]
fn a_fiber_that_catches_a_failure_while_unwinding_is_unwound_again() {
    // The child C is interrupted while it waits in an inner scope. The inner child fails after
    // its release, so the inner scope returns that error and C's escape catches it. C is still
    // interrupted: it unwinds again at its next suspension point (11.4).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, s2, r) = (
        m.local("c"),
        m.local("n"),
        m.local("s"),
        m.local("s2"),
        m.local("r"),
    );
    let release = seq(
        &k,
        &mut m,
        vec![k.log("release R"), k.sleep(c, 20), k.log("released R")],
    );
    let held = with(
        vec![WithBind::managed(r, k.handler("R"), "R", release)],
        k.sleep(c, 5),
    );
    let inner_child = seq(&k, &mut m, vec![held, k.oops()]);
    let inner = k.scope(
        s2,
        true,
        seq(
            &k,
            &mut m,
            vec![k.fork(s2, inner_child), k.sleep(c, 100), unit()],
        ),
    );
    let caught = escape(inner, vec![arm(p_ctor(k.oops, vec![]), k.log("caught"))]);
    let child = seq(
        &k,
        &mut m,
        vec![
            caught,
            k.log("after escape"),
            k.sleep(c, 1),
            k.log("not reached"),
            unit(),
        ],
    );
    let body = seq(&k, &mut m, vec![k.fork(s, child), k.sleep(c, 10), int(0)]);
    main_fn(&mut m, true, k.clocks(c, n, k.scope(s, false, body)));
    assert_eq!(
        run("unwound_again", m),
        "release R\nreleased R\ncaught\nafter escape\nasync 0\n"
    );
}
