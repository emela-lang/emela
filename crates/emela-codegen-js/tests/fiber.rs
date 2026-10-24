//! Fibers (spec chapter 11). Builds IR by hand, emits JS and runs it on node. Checks the order of
//! lines printed with the host's `console.log` and the result of `main`. The emitted JS is also
//! snapshotted with insta.

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

/// A host function for the tests: resolves after `ms` milliseconds, and clears its timer when
/// `signal` aborts (spec 12.2). It prints `timer cleared` then.
const TIMER: &str = r#"export function sleep(ms, signal) {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => {
      clearTimeout(timer);
      process.stdout.write("timer cleared\n");
    }, { once: true });
  });
}
"#;

fn run(name: &str, m: Module) -> String {
    run_timed(name, m).0
}

/// Also returns how long node ran.
fn run_timed(name: &str, mut m: Module) -> (String, std::time::Duration) {
    tail::loopify(&mut m);
    assert_well_typed(&m);
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
    std::fs::write(dir.join("timer.mjs"), TIMER).unwrap();
    let clock = bundled_files()
        .iter()
        .find(|(path, _)| *path == "std/clock.mjs")
        .unwrap()
        .1;
    std::fs::write(dir.join("clock.mjs"), clock).unwrap();
    let started = std::time::Instant::now();
    let out = Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("cannot start node");
    let elapsed = started.elapsed();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "node failed:\n{stderr}\n{js}");
    insta::assert_snapshot!(name, js);
    (String::from_utf8(out.stdout).unwrap(), elapsed)
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
    /// `@external type Cause[E]`．
    cause: EnumId,
    /// `@external type Fiber[A]`．
    fiber: EnumId,
    /// `@external type Deferred[A]`．
    deferred: EnumId,
    /// `fn() -> String`, the type of `name`'s implementation.
    name_ty: TypeId,
    /// The types of `Fiber.scope` bodies, without and with `fails`. They return `any`: the tests
    /// build them without tracking the body's type.
    scope_body_ty: [TypeId; 2],
    /// The types of fork children and attempted functions (`fn() -> any fails`), not suspending
    /// and suspending.
    thunk_ty: [TypeId; 2],
}

impl Kit {
    fn new(m: &mut Module) -> Kit {
        let log = external(m, "log", "console.log", vec![TypeId::STRING], TypeId::UNIT);
        let res = m.add_effect(EffectDef::new(
            "Res",
            vec![OpDef::new("name", vec![], TypeId::STRING)],
        ));
        let clock = m.add_effect(EffectDef::new(
            "Clock",
            vec![OpDef::suspend("sleep", vec![TypeId::INT], TypeId::UNIT)],
        ));
        let callback = m.intern(fn_ty(vec![], TypeId::UNIT, false));
        let set_timeout = external(
            m,
            "set_timeout",
            "setTimeout",
            vec![callback, TypeId::INT],
            TypeId::UNIT,
        );
        let js_clock = m.declare("JsClock", false);
        let (ms, resume) = sleep_params(m);
        let op_ty = sleep_ty(m, false);
        let body = call(
            set_timeout,
            vec![
                lambda(vec![], callback, call_value(var(resume), vec![unit()])),
                var(ms),
            ],
        );
        m.define(
            js_clock,
            vec![],
            Type::Handler(clock),
            Expr::Handler {
                name: "JsClock".into(),
                effect: clock,
                ops: vec![lambda(vec![ms, resume], op_ty, body)],
            },
        );
        let never_clock = m.declare("NeverClock", false);
        let (ms, resume) = sleep_params(m);
        m.define(
            never_clock,
            vec![],
            Type::Handler(clock),
            Expr::Handler {
                name: "NeverClock".into(),
                effect: clock,
                ops: vec![lambda(vec![ms, resume], op_ty, unit())],
            },
        );
        let oops = m.add_enum(EnumDef {
            name: "Oops".into(),
            params: 0,
            variants: vec![VariantDef::unit("Oops")],
            error: true,
            external: false,
        });
        // A Cause is opaque (an `@external type`, an enum without constructors); its field type
        // is never shown here.
        let cause = m.add_enum(EnumDef {
            name: "Cause".into(),
            params: 1,
            variants: vec![],
            error: false,
            external: true,
        });
        let (a, e) = (m.type_param(0), m.type_param(1));
        let cause_e = m.enum_type(cause, vec![e]);
        let exit = m.add_enum(EnumDef {
            name: "Exit".into(),
            params: 2,
            variants: vec![
                VariantDef::positional("Success", vec![a]),
                VariantDef::positional("Failure", vec![cause_e]),
            ],
            error: false,
            external: false,
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
            external: false,
        });
        let opaque = |m: &mut Module, name: &str| {
            m.add_enum(EnumDef {
                name: name.into(),
                params: 1,
                variants: vec![],
                error: false,
                external: true,
            })
        };
        let fiber = opaque(m, "Fiber");
        let deferred = opaque(m, "Deferred");
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
            cause,
            fiber,
            deferred,
            name_ty: m.fn_type(vec![], TypeId::STRING),
            scope_body_ty: [false, true].map(|fails| {
                m.func(FnType {
                    params: vec![TypeId::FIBER_SCOPE],
                    ret: TypeId::ANY,
                    fails,
                    suspends: true,
                    ev: false,
                    sets: None,
                })
            }),
            thunk_ty: [false, true].map(|suspends| {
                m.func(FnType {
                    params: vec![],
                    ret: TypeId::ANY,
                    fails: true,
                    suspends,
                    ev: false,
                    sets: None,
                })
            }),
        }
    }

    /// A Clock handler value.
    fn clock_ty(&self) -> Type {
        Type::Handler(self.clock)
    }

    /// A Res handler value.
    fn res_ty(&self) -> Type {
        Type::Handler(self.res)
    }

    /// `Fiber[a]`．
    fn fiber_ty(&self, a: TypeId) -> Type {
        Type::Enum(self.fiber, vec![a])
    }

    /// `Deferred[a]`．
    fn deferred_ty(&self, a: TypeId) -> Type {
        Type::Enum(self.deferred, vec![a])
    }

    /// `Exit[a, E]`. The error set argument has no run-time representation (`()`).
    fn exit_ty(&self, a: TypeId) -> Type {
        Type::Enum(self.exit, vec![a, TypeId::UNIT])
    }

    /// `Cause[E]`．
    fn cause_ty(&self) -> Type {
        Type::Enum(self.cause, vec![TypeId::UNIT])
    }

    /// A Clock handler local.
    fn clock_local(&self, m: &mut Module, hint: &str) -> Local {
        m.local(hint, self.clock_ty())
    }

    /// A local for the run-time scope a `Fiber.scope` body receives.
    fn scope_local(&self, m: &mut Module, hint: &str) -> Local {
        m.local(hint, TypeId::FIBER_SCOPE)
    }

    /// `log(text)`．
    fn log(&self, text: &str) -> Expr {
        call(self.log, vec![string(text)])
    }

    /// `log(text); then`．
    fn log_then(&self, m: &mut Module, text: &str, then: Expr) -> Expr {
        self.then(m, self.log(text), then)
    }

    /// `first; then`, where `first` is `()`.
    fn then(&self, m: &mut Module, first: Expr, then: Expr) -> Expr {
        self.then_ty(m, first, Type::Unit, then)
    }

    /// `first; then`, where `first` is of type `ty`.
    fn then_ty(&self, m: &mut Module, first: Expr, ty: Type, then: Expr) -> Expr {
        let x = m.local("_", ty);
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

    /// The value of `handler Name: Res { fn name() { "name" } }`.
    fn handler(&self, name: &str) -> Expr {
        Expr::Handler {
            name: name.into(),
            effect: self.res,
            ops: vec![lambda(vec![], self.name_ty, string(name))],
        }
    }

    /// `with` a handler that prints `init <name>` on init and `release <name>` on release.
    fn managed(&self, m: &mut Module, name: &str, body: Expr) -> Expr {
        let h = m.local("h", self.res_ty());
        let init = self.log_then(m, &format!("init {name}"), self.handler(name));
        let bind = WithBind::managed(h, init, name, self.log(&format!("release {name}")));
        with(vec![bind], body)
    }

    /// `with` a handler whose release prints `release <name>` and then calls `panic(message)`.
    fn breaking(&self, m: &mut Module, name: &str, message: &str, body: Expr) -> Expr {
        let h = m.local("h", self.res_ty());
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
        let ty = self.scope_body_ty[usize::from(fails)];
        fiber(
            FiberOp::Scope { fails },
            vec![suspend_lambda(vec![s], ty, body)],
        )
    }

    /// `s.fork(fn() { body })`．
    fn fork(&self, s: Local, body: Expr) -> Expr {
        let ty = self.thunk_ty[1];
        fiber(
            FiberOp::Fork,
            vec![var(s), suspend_lambda(vec![], ty, body)],
        )
    }

    /// `Fiber.attempt(fn() { body })`．
    fn attempt(&self, suspends: bool, body: Expr) -> Expr {
        let ty = self.thunk_ty[usize::from(suspends)];
        let f = if suspends {
            suspend_lambda(vec![], ty, body)
        } else {
            lambda(vec![], ty, body)
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
        // Some tests describe the Exit of a `()` fiber that never succeeds, so the value is `any`.
        let (v, c) = (m.local("v", TypeId::ANY), m.local("c", self.cause_ty()));
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
                    Expr::Concat(vec![lit_part("Success "), value_part(var(v), TypeId::INT)]),
                ),
                arm(
                    p_ctor(exit_ctor(EXIT_FAILURE), vec![Pat::Bind(c)]),
                    Expr::Concat(vec![
                        value_part(kind_name, TypeId::STRING),
                        lit_part(" "),
                        value_part(fiber(FiberOp::CauseToString, vec![var(c)]), TypeId::STRING),
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
fn external(m: &mut Module, name: &str, path: &str, params: Vec<TypeId>, ret: TypeId) -> FnId {
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
    f
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

/// The `ms` and `resume` parameters of Clock's `sleep`.
fn sleep_params(m: &mut Module) -> (Local, Local) {
    let resume = fn_ty(vec![TypeId::UNIT], TypeId::UNIT, false);
    (m.local("ms", TypeId::INT), m.local("resume", resume))
}

/// The type of an implementation of Clock's `sleep`, which takes resume:
/// `fn(Int, fn(()) -> ()) -> ()`.
fn sleep_ty(m: &mut Module, suspends: bool) -> TypeId {
    let resume = m.intern(fn_ty(vec![TypeId::UNIT], TypeId::UNIT, false));
    m.intern(fn_ty(vec![TypeId::INT, resume], TypeId::UNIT, suspends))
}

fn with(binds: Vec<WithBind>, body: Expr) -> Expr {
    Expr::With {
        binds,
        body: Box::new(body),
    }
}

fn main_fn(m: &mut Module, suspends: bool, ret: TypeId, body: Expr) {
    let main = m.declare("main", true);
    m.functions[main].suspends = suspends;
    m.define(main, vec![], ret, body);
}

/// Statements in order: `a; b; …; last`, where all but `last` are `()`.
fn seq(k: &Kit, m: &mut Module, es: Vec<Expr>) -> Expr {
    seq_ty(k, m, es.into_iter().map(|e| (e, Type::Unit)).collect())
}

/// Like `seq`, with the type of each statement's discarded value (the last type is unused).
fn seq_ty(k: &Kit, m: &mut Module, mut es: Vec<(Expr, Type)>) -> Expr {
    let mut body = es.pop().unwrap().0;
    while let Some((e, ty)) = es.pop() {
        body = k.then_ty(m, e, ty, body);
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
    let (c, n, s) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
    );
    let (a, b) = (
        m.local("a", k.fiber_ty(TypeId::INT)),
        m.local("b", k.fiber_ty(TypeId::INT)),
    );
    let (x, y) = (m.local("x", TypeId::INT), m.local("y", TypeId::INT));
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
    main_fn(
        &mut m,
        true,
        TypeId::INT,
        k.clocks(c, n, k.scope(s, false, body)),
    );
    assert_eq!(run("join", m), "b\na\njoined\nasync 3\n");
}

#[test]
fn the_body_value_interrupts_the_remaining_children() {
    // When the body returns, the children still running are interrupted, and the scope returns
    // after their releases (11.2, 11.4).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
    );
    let stuck = k.managed(&mut m, "A", k.sleep(n, 1));
    let quick = k.log("quick");
    let body = seq_ty(
        &k,
        &mut m,
        vec![
            (k.fork(s, stuck), k.fiber_ty(TypeId::UNIT)),
            (k.fork(s, quick), k.fiber_ty(TypeId::UNIT)),
            (k.sleep(c, 10), Type::Unit),
            (k.log("body done"), Type::Unit),
            (int(7), Type::Int),
        ],
    );
    let r = m.local("r", TypeId::INT);
    let main = let_(
        r,
        k.scope(s, false, body),
        k.log_then(&mut m, "after", var(r)),
    );
    main_fn(&mut m, true, TypeId::INT, k.clocks(c, n, main));
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
    let (c, n, s) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
    );
    let failing = seq(&k, &mut m, vec![k.sleep(c, 10), k.oops()]);
    let stuck = k.managed(&mut m, "A", k.sleep(n, 1));
    let inner = seq_ty(
        &k,
        &mut m,
        vec![
            (k.fork(s, failing), k.fiber_ty(TypeId::NEVER)),
            (k.fork(s, stuck), k.fiber_ty(TypeId::UNIT)),
            (k.sleep(c, 50), Type::Unit),
            (int(1), Type::Int),
        ],
    );
    let body = k.managed(&mut m, "B", inner);
    main_fn(
        &mut m,
        true,
        TypeId::INT,
        k.clocks(c, n, k.scope(s, true, body)),
    );
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
        let (c, n, s) = (
            k.clock_local(&mut m, "c"),
            k.clock_local(&mut m, "n"),
            k.scope_local(&mut m, "s"),
        );
        let failure = if first {
            k.oops()
        } else {
            builtin(Builtin::Panic, vec![string("first")])
        };
        let failing = seq(&k, &mut m, vec![k.sleep(c, 10), failure]);
        let breaking = k.breaking(&mut m, "B", "second", k.sleep(n, 1));
        let body = seq_ty(
            &k,
            &mut m,
            vec![
                (k.fork(s, failing), k.fiber_ty(TypeId::NEVER)),
                (k.fork(s, breaking), k.fiber_ty(TypeId::UNIT)),
                (k.sleep(c, 50), Type::Unit),
                (int(1), Type::Int),
            ],
        );
        main_fn(
            &mut m,
            true,
            TypeId::INT,
            k.clocks(c, n, k.scope(s, true, body)),
        );
        assert_eq!(run(name, m), expected);
    }
}

#[test]
fn interrupt_waits_for_the_unwinding_and_join_is_a_defect() {
    // `Fiber.interrupt` returns after the child's release (11.4). await sees the Interrupt, and
    // join of an interrupted child is a defect (11.5).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, h) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        m.local("h", k.fiber_ty(TypeId::UNIT)),
    );
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
    main_fn(
        &mut m,
        true,
        TypeId::UNIT,
        k.clocks(c, n, k.scope(s, false, body)),
    );
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
    let (e, cause) = (
        m.local("e", k.exit_ty(TypeId::NEVER)),
        m.local("cause", k.cause_ty()),
    );
    let raised = escape(
        fiber(FiberOp::CauseRaise { fails: true }, vec![var(cause)]),
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
    main_fn(&mut m, false, TypeId::INT, body);
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
    let (c, n, s) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
    );
    let attempted = k.attempt(true, k.sleep(n, 1));
    let child = seq_ty(
        &k,
        &mut m,
        vec![
            (attempted, k.exit_ty(TypeId::UNIT)),
            (k.log("attempt returned"), Type::Unit),
            (unit(), Type::Unit),
        ],
    );
    let body = seq_ty(
        &k,
        &mut m,
        vec![
            (k.fork(s, child), k.fiber_ty(TypeId::UNIT)),
            (k.sleep(c, 1), Type::Unit),
            (k.log("body done"), Type::Unit),
            (int(0), Type::Int),
        ],
    );
    main_fn(
        &mut m,
        true,
        TypeId::INT,
        k.clocks(c, n, k.scope(s, false, body)),
    );
    assert_eq!(run("attempt_interrupt", m), "body done\nasync 0\n");
}

#[test]
fn deferred_takes_the_first_value() {
    // The first complete wins and wakes the waiting body; later ones return False (11.7).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, d) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        m.local("d", k.deferred_ty(TypeId::STRING)),
    );
    let (r, v, again) = (
        m.local("r", TypeId::BOOL),
        m.local("v", TypeId::STRING),
        m.local("again", TypeId::BOOL),
    );
    let complete = |d: Local, text: &str| fiber(FiberOp::Complete, vec![var(d), string(text)]);
    let report = |label: &str, r: Local| {
        Expr::Concat(vec![lit_part(label), value_part(var(r), TypeId::BOOL)])
    };
    let r2 = m.local("r", TypeId::BOOL);
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
        seq_ty(
            &k,
            &mut m,
            vec![
                (k.fork(s, slow), k.fiber_ty(TypeId::UNIT)),
                (k.fork(s, fast), k.fiber_ty(TypeId::UNIT)),
                (call(k.log, vec![empty]), Type::Unit),
                (after, Type::String),
            ],
        ),
    );
    main_fn(
        &mut m,
        true,
        TypeId::STRING,
        k.clocks(c, n, k.scope(s, false, body)),
    );
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
    let (s1, s2, s3) = (
        k.scope_local(&mut m, "s"),
        k.scope_local(&mut m, "s"),
        k.scope_local(&mut m, "s"),
    );
    let (x, h, d) = (
        k.scope_local(&mut m, "x"),
        m.local("h", k.fiber_ty(TypeId::INT)),
        m.local("d", k.deferred_ty(TypeId::INT)),
    );
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
    let (s4, d2) = (
        k.scope_local(&mut m, "s"),
        m.local("d", k.deferred_ty(TypeId::INT)),
    );
    // The poll result is an `Option[Int]`, but adding Option to this module (`option_enum`)
    // would change the output, so the discarded value is typed `()` here.
    let polled = m.local("_", TypeId::UNIT);
    let poll_after = let_(
        d2,
        k.scope(s4, false, fiber(FiberOp::Deferred, vec![var(s4)])),
        let_(polled, fiber(FiberOp::Poll, vec![var(d2)]), int(1)),
    );
    let lines = vec![
        k.log_exit(&mut m, k.attempt(true, fork_after)),
        k.log_exit(&mut m, k.attempt(true, join_after)),
        k.log_exit(&mut m, k.attempt(true, complete_after)),
        k.log_exit(&mut m, k.attempt(true, poll_after)),
        int(0),
    ];
    let body = seq(&k, &mut m, lines);
    main_fn(&mut m, true, TypeId::INT, body);
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
    let (n, s, h) = (
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        m.local("h", k.fiber_ty(TypeId::UNIT)),
    );
    let inner = m.declare("inner", false);
    let n2 = k.clock_local(&mut m, "n");
    let inner_body = with(
        vec![WithBind::new(n2, call(k.never_clock, vec![]))],
        k.sleep(n2, 1),
    );
    m.define(inner, vec![], TypeId::UNIT, inner_body);
    let child = fiber(
        FiberOp::Fork,
        vec![var(s), lambda(vec![], k.thunk_ty[0], call(inner, vec![]))],
    );
    let awaited = k.log_exit(&mut m, k.await_(var(h)));
    let body = let_(h, child, seq(&k, &mut m, vec![awaited, int(0)]));
    let main = with(
        vec![WithBind::new(n, call(k.never_clock, vec![]))],
        k.scope(s, false, body),
    );
    main_fn(&mut m, true, TypeId::INT, main);
    assert_eq!(run("sync_child", m), "Interrupt interrupted\nsync 0\n");
}

#[test]
fn a_running_release_is_not_interrupted() {
    // The scope ends while a child runs a suspending release; the interruption waits for it
    // (11.4).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (c, n, s, a) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        m.local("a", k.res_ty()),
    );
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
    let body = seq_ty(
        &k,
        &mut m,
        vec![
            (k.fork(s, child), k.fiber_ty(TypeId::UNIT)),
            (k.sleep(c, 10), Type::Unit),
            (int(0), Type::Int),
        ],
    );
    let r = m.local("r", TypeId::INT);
    let main = let_(
        r,
        k.scope(s, false, body),
        k.log_then(&mut m, "after scope", var(r)),
    );
    main_fn(&mut m, true, TypeId::INT, k.clocks(c, n, main));
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
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        k.scope_local(&mut m, "s2"),
        m.local("o", k.fiber_ty(TypeId::UNIT)),
    );
    let outer_child_body = seq(
        &k,
        &mut m,
        vec![k.sleep(c, 30), k.log("outer child done"), unit()],
    );
    let outer_child = k.managed(&mut m, "O", outer_child_body);
    let failing = seq(&k, &mut m, vec![k.sleep(c, 5), k.oops()]);
    let inner_body = k.managed(&mut m, "I", k.sleep(c, 100));
    let inner = seq_ty(
        &k,
        &mut m,
        vec![
            (k.fork(s2, failing), k.fiber_ty(TypeId::NEVER)),
            (inner_body, Type::Unit),
        ],
    );
    let attempted = k.attempt(true, k.scope(s2, true, inner));
    let logged = k.log_exit(&mut m, attempted);
    let rest = seq(
        &k,
        &mut m,
        vec![logged, fiber(FiberOp::Join, vec![var(o)]), string("end")],
    );
    let body = let_(o, k.fork(s, outer_child), rest);
    main_fn(
        &mut m,
        true,
        TypeId::STRING,
        k.clocks(c, n, k.scope(s, false, body)),
    );
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
    let (c, n, s, h) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        m.local("h", k.fiber_ty(TypeId::UNIT)),
    );
    let stuck = k.managed(&mut m, "A", k.sleep(n, 1));
    let body = let_(h, k.fork(s, stuck), fiber(FiberOp::Join, vec![var(h)]));
    let main = k.managed(&mut m, "M", k.scope(s, false, body));
    main_fn(&mut m, true, TypeId::UNIT, k.clocks(c, n, main));
    assert_eq!(
        run("dropped_main", m),
        "init M\ninit A\nrelease A\nrelease M\nasync interrupted\n"
    );
}

#[test]
fn fiber_operations_are_dumped() {
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (s, h) = (
        k.scope_local(&mut m, "s"),
        m.local("h", k.fiber_ty(TypeId::INT)),
    );
    let awaited = m.local("_", k.exit_ty(TypeId::INT));
    let body = let_(
        h,
        k.fork(s, int(1)),
        let_(
            awaited,
            k.await_(var(h)),
            fiber(FiberOp::Join, vec![var(h)]),
        ),
    );
    main_fn(&mut m, true, TypeId::INT, k.scope(s, true, body));
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
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        k.scope_local(&mut m, "s2"),
        m.local("r", k.res_ty()),
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
        seq_ty(
            &k,
            &mut m,
            vec![
                (k.fork(s2, inner_child), k.fiber_ty(TypeId::NEVER)),
                (k.sleep(c, 100), Type::Unit),
                (unit(), Type::Unit),
            ],
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
    let body = seq_ty(
        &k,
        &mut m,
        vec![
            (k.fork(s, child), k.fiber_ty(TypeId::UNIT)),
            (k.sleep(c, 10), Type::Unit),
            (int(0), Type::Int),
        ],
    );
    main_fn(
        &mut m,
        true,
        TypeId::INT,
        k.clocks(c, n, k.scope(s, false, body)),
    );
    assert_eq!(
        run("unwound_again", m),
        "release R\nreleased R\ncaught\nafter escape\nasync 0\n"
    );
}

/// An external function `name` exported by the module `module` (next to main.mjs). With
/// `suspends`, it is an `@external suspend fn`.
fn module_external(
    m: &mut Module,
    name: &str,
    module: &str,
    export: &str,
    params: Vec<TypeId>,
    suspends: bool,
) -> FnId {
    let f = m.declare(name, false);
    let ps = params.into_iter().map(|t| m.local("x", t)).collect();
    m.define_external(
        f,
        ps,
        TypeId::UNIT,
        External {
            bindings: vec![ExternalBinding {
                target: "js".into(),
                host: HostRef::Import {
                    module: HostModule::File(module.into()),
                    name: export.into(),
                },
            }],
        },
    );
    m.functions[f].suspends = suspends;
    f
}

/// A handler of `Clock` whose `sleep(ms)` calls the suspending external function `wait(ms)` and
/// then resumes, the form of SystemClock and TestClock (spec 12.2).
fn waiting_clock(m: &mut Module, k: &Kit, name: &str, wait: FnId) -> FnId {
    let f = m.declare(name, false);
    let (ms, resume) = sleep_params(m);
    let op_ty = sleep_ty(m, true);
    let body = k.then(
        m,
        call_suspends(wait, vec![var(ms)]),
        call_value(var(resume), vec![unit()]),
    );
    m.define(
        f,
        vec![],
        k.clock_ty(),
        Expr::Handler {
            name: name.into(),
            effect: k.clock,
            ops: vec![suspend_lambda(vec![ms, resume], op_ty, body)],
        },
    );
    f
}

#[test]
fn a_busy_fiber_yields_to_the_others() {
    // A fiber running a long suspending loop that never really suspends (its clock resumes at
    // once) yields for fairness, so the heartbeat fiber and its host timer keep going (11.1).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let instant = m.declare("InstantClock", false);
    let (ms, resume) = sleep_params(&mut m);
    let op_ty = sleep_ty(&mut m, false);
    m.define(
        instant,
        vec![],
        k.clock_ty(),
        Expr::Handler {
            name: "InstantClock".into(),
            effect: k.clock,
            ops: vec![lambda(
                vec![ms, resume],
                op_ty,
                call_value(var(resume), vec![unit()]),
            )],
        },
    );
    // suspend fn spin(h, n) = if n == 0 { () } else { h.sleep(0); spin(h, n - 1) }
    let spin = m.declare("spin", false);
    m.functions[spin].suspends = true;
    let (h, n) = (k.clock_local(&mut m, "h"), m.local("n", TypeId::INT));
    let again = k.then(
        &mut m,
        k.sleep(h, 0),
        call_suspends(
            spin,
            vec![var(h), bin(BinOp::Sub, OpTy::Int, var(n), int(1))],
        ),
    );
    m.define(
        spin,
        vec![h, n],
        TypeId::UNIT,
        if_(bin(BinOp::Eq, OpTy::Int, var(n), int(0)), unit(), again),
    );
    // suspend fn beat(c, n) = if n == 0 { () } else { c.sleep(1); log("beat"); beat(c, n - 1) }
    let beat = m.declare("beat", false);
    m.functions[beat].suspends = true;
    let (c2, n2) = (k.clock_local(&mut m, "c"), m.local("n", TypeId::INT));
    let next = seq(
        &k,
        &mut m,
        vec![
            k.sleep(c2, 1),
            k.log("beat"),
            call_suspends(
                beat,
                vec![var(c2), bin(BinOp::Sub, OpTy::Int, var(n2), int(1))],
            ),
        ],
    );
    m.define(
        beat,
        vec![c2, n2],
        TypeId::UNIT,
        if_(bin(BinOp::Eq, OpTy::Int, var(n2), int(0)), unit(), next),
    );
    let (c, n, s, i, b) = (
        k.clock_local(&mut m, "c"),
        k.clock_local(&mut m, "n"),
        k.scope_local(&mut m, "s"),
        k.clock_local(&mut m, "i"),
        m.local("b", k.fiber_ty(TypeId::UNIT)),
    );
    let busy = with(
        vec![WithBind::new(i, call(instant, vec![]))],
        seq(
            &k,
            &mut m,
            vec![
                call_suspends(spin, vec![var(i), int(2_000_000)]),
                k.log("busy done"),
                unit(),
            ],
        ),
    );
    let body = let_(
        b,
        k.fork(s, busy),
        seq_ty(
            &k,
            &mut m,
            vec![
                (
                    k.fork(s, call_suspends(beat, vec![var(c), int(3)])),
                    k.fiber_ty(TypeId::UNIT),
                ),
                (fiber(FiberOp::Join, vec![var(b)]), Type::Unit),
                (int(0), Type::Int),
            ],
        ),
    );
    main_fn(
        &mut m,
        true,
        TypeId::INT,
        k.clocks(c, n, k.scope(s, false, body)),
    );
    assert_eq!(run("fairness", m), "beat\nbeat\nbeat\nbusy done\nasync 0\n");
}

#[test]
fn interrupting_a_sleeper_clears_its_timer() {
    // A child sleeps 10 s in a handler that waits for a host timer. When the scope ends, the
    // child is interrupted, the AbortSignal passed to the host function aborts, and node exits at
    // once (12.2).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let wait = module_external(
        &mut m,
        "wait",
        "./timer.mjs",
        "sleep",
        vec![TypeId::INT],
        true,
    );
    let host_clock = waiting_clock(&mut m, &k, "HostClock", wait);
    let (c, s) = (k.clock_local(&mut m, "c"), k.scope_local(&mut m, "s"));
    let sleeper = k.managed(&mut m, "A", k.sleep(c, 10_000));
    let body = seq_ty(
        &k,
        &mut m,
        vec![
            (k.fork(s, sleeper), k.fiber_ty(TypeId::UNIT)),
            (k.sleep(c, 1), Type::Unit),
            (int(0), Type::Int),
        ],
    );
    let main = with(
        vec![WithBind::new(c, call(host_clock, vec![]))],
        k.scope(s, false, body),
    );
    main_fn(&mut m, true, TypeId::INT, main);
    let (out, elapsed) = run_timed("abort", m);
    assert_eq!(out, "init A\ntimer cleared\nrelease A\nasync 0\n");
    assert!(elapsed.as_secs() < 5, "node ran for {elapsed:?}");
}

#[test]
fn stalled_sleepers_wake_in_time_order() {
    // The form of TestClock (8.8): each sleeper waits until the run stalls, and the earliest time
    // wakes first; equal times wake in the order of the calls. An interrupted sleeper is taken
    // back. Here `sleep(ms)` waits for the time `ms` itself.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let stalled = module_external(
        &mut m,
        "when_stalled",
        "./clock.mjs",
        "whenStalled",
        vec![TypeId::INT],
        true,
    );
    let virtual_clock = waiting_clock(&mut m, &k, "VirtualClock", stalled);
    let (c, s, d) = (
        k.clock_local(&mut m, "c"),
        k.scope_local(&mut m, "s"),
        m.local("d", k.fiber_ty(TypeId::UNIT)),
    );
    let sleeper = |k: &Kit, m: &mut Module, at: i32, text: &str| {
        let body = seq(k, m, vec![k.sleep(c, at), k.log(text), unit()]);
        k.fork(s, body)
    };
    let a = sleeper(&k, &mut m, 30, "A at 30");
    let b = sleeper(&k, &mut m, 10, "B at 10");
    let cc = sleeper(&k, &mut m, 10, "C at 10");
    let dropped = sleeper(&k, &mut m, 5, "D at 5");
    let sleeper_ty = k.fiber_ty(TypeId::UNIT);
    let body = seq_ty(
        &k,
        &mut m,
        vec![
            (a, sleeper_ty.clone()),
            (b, sleeper_ty.clone()),
            (cc, sleeper_ty),
            (k.sleep(c, 1), Type::Unit),
            (fiber(FiberOp::Interrupt, vec![var(d)]), Type::Unit),
            (k.sleep(c, 40), Type::Unit),
            (k.log("body at 40"), Type::Unit),
            (int(0), Type::Int),
        ],
    );
    let body = let_(d, dropped, body);
    let main = with(
        vec![WithBind::new(c, call(virtual_clock, vec![]))],
        k.scope(s, false, body),
    );
    main_fn(&mut m, true, TypeId::INT, main);
    assert_eq!(
        run("stalled", m),
        "B at 10\nC at 10\nA at 30\nbody at 40\nasync 0\n"
    );
}
