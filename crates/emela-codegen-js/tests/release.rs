//! Handlers with init and release (spec 8.4, 8.6, 14.2, 14.4, 14.5). Builds IR by hand, emits JS
//! and runs it on node. Checks that release runs however the `with` body ends, by the order of
//! lines printed with the host's `console.log`. The emitted JS is also snapshotted with insta.

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
/// - a value: `42` (a suspending main prints `sync 42` / `async 42`); an error: `error: Oops`;
///   a defect: `defect: …`
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
    assert_well_typed(&m);
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
    /// `error Oops`．
    oops: CtorRef,
    /// `fn() -> String`, the type of `name`'s implementation.
    name_ty: TypeId,
}

impl Kit {
    fn new(m: &mut Module) -> Kit {
        let log = external(
            m,
            "log",
            "console.log",
            vec![TypeId::STRING],
            TypeId::UNIT,
            false,
        );
        let res = m.add_effect(EffectDef::new(
            "Res",
            vec![OpDef::new("name", vec![], TypeId::STRING)],
        ));
        let oops = m.add_enum(EnumDef {
            name: "Oops".into(),
            params: 0,
            variants: vec![VariantDef::unit("Oops")],
            error: true,
            external: false,
        });
        Kit {
            log,
            res,
            name_ty: m.fn_type(vec![], TypeId::STRING),
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
        let x = m.local("_", TypeId::UNIT);
        let_(x, self.log(text), then)
    }

    /// The value of `handler Name: Res { fn name() { "name" } }`.
    fn handler(&self, name: &str) -> Expr {
        Expr::Handler {
            name: name.into(),
            effect: self.res,
            ops: vec![lambda(vec![], self.name_ty, string(name))],
        }
    }

    /// A handler binding that prints `init <name>` on init and `release <name>` on release.
    fn managed(&self, m: &mut Module, h: Local, name: &str) -> WithBind {
        let init = self.log_then(m, &format!("init {name}"), self.handler(name));
        WithBind::managed(h, init, name, self.log(&format!("release {name}")))
    }

    /// A handler binding that prints `release <name>` on release and then calls
    /// `panic(message)`.
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

/// A function type that neither fails nor takes an evidence table.
fn fn_ty(params: Vec<TypeId>, ret: TypeId) -> Type {
    Type::Fn(FnType {
        params,
        ret,
        fails: false,
        suspends: false,
        ev: false,
        sets: None,
    })
}

/// The `ms` and `resume` parameters of Clock's `sleep`.
fn sleep_params(m: &mut Module) -> (Local, Local) {
    let resume = fn_ty(vec![TypeId::UNIT], TypeId::UNIT);
    (m.local("ms", TypeId::INT), m.local("resume", resume))
}

/// The type of `sleep`'s implementation, which takes resume: `fn(Int, fn(()) -> ()) -> ()`.
fn sleep_ty(m: &mut Module) -> TypeId {
    let resume = m.intern(fn_ty(vec![TypeId::UNIT], TypeId::UNIT));
    m.fn_type(vec![TypeId::INT, resume], TypeId::UNIT)
}

/// `effect Clock { suspend fn sleep(ms: Int) -> () }`．
fn clock(m: &mut Module) -> EffectId {
    m.add_effect(EffectDef::new(
        "Clock",
        vec![OpDef::suspend("sleep", vec![TypeId::INT], TypeId::UNIT)],
    ))
}

/// A `JsClock` that resumes after `setTimeout`.
fn js_clock(m: &mut Module, clock: EffectId) -> FnId {
    let callback = m.intern(fn_ty(vec![], TypeId::UNIT));
    let set_timeout = external(
        m,
        "set_timeout",
        "setTimeout",
        vec![callback, TypeId::INT],
        TypeId::UNIT,
        false,
    );
    let f = m.declare("JsClock", false);
    let (ms, resume) = sleep_params(m);
    let op_ty = sleep_ty(m);
    let body = call(
        set_timeout,
        vec![
            lambda(vec![], callback, call_value(var(resume), vec![unit()])),
            var(ms),
        ],
    );
    m.define(
        f,
        vec![],
        Type::Handler(clock),
        Expr::Handler {
            name: "JsClock".into(),
            effect: clock,
            ops: vec![lambda(vec![ms, resume], op_ty, body)],
        },
    );
    f
}

/// A `NeverClock` that drops resume without calling it.
fn never_clock(m: &mut Module, clock: EffectId) -> FnId {
    let f = m.declare("NeverClock", false);
    let (ms, resume) = sleep_params(m);
    let op_ty = sleep_ty(m);
    m.define(
        f,
        vec![],
        Type::Handler(clock),
        Expr::Handler {
            name: "NeverClock".into(),
            effect: clock,
            ops: vec![lambda(vec![ms, resume], op_ty, unit())],
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
    // init runs in binding order and release in reverse (spec 14.2). The `with` evaluates to the
    // body's value.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
    );
    let binds = vec![k.managed(&mut m, a, "A"), k.managed(&mut m, b, "B")];
    let body = k.log_then(&mut m, "body", int(42));
    main_fn(&mut m, false, TypeId::INT, with(binds, body));
    assert_eq!(
        run("value", m),
        "init A\ninit B\nbody\nrelease B\nrelease A\n42\n"
    );
}

#[test]
fn releases_run_when_the_body_fails() {
    // A body failure propagates outward after release runs. A `with` inside an escape leaves by
    // `break`.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
    );
    let inner = with(
        vec![k.managed(&mut m, a, "A")],
        k.log_then(&mut m, "body", k.oops()),
    );
    let caught = escape(inner, vec![arm(p_ctor(k.oops, vec![]), int(7))]);
    let x = m.local("x", TypeId::INT);
    let outer = with(
        vec![k.managed(&mut m, b, "B")],
        let_(x, caught, k.log_then(&mut m, "caught", k.oops())),
    );
    main_fn(&mut m, false, TypeId::NEVER, outer);
    assert_eq!(
        run("fail", m),
        "init B\ninit A\nbody\nrelease A\ncaught\nrelease B\nerror: Oops\n"
    );
}

#[test]
fn releases_run_when_the_body_defects() {
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
    );
    let binds = vec![k.managed(&mut m, a, "A"), k.managed(&mut m, b, "B")];
    main_fn(
        &mut m,
        false,
        TypeId::NEVER,
        with(binds, builtin(Builtin::Panic, vec![string("boom")])),
    );
    assert_eq!(
        run("defect", m),
        "init A\ninit B\nrelease B\nrelease A\ndefect: boom\n"
    );
}

#[test]
fn failing_init_releases_only_the_initialized_handlers() {
    // If B's init fails, the body and C are not evaluated, and only the initialized A is released.
    // B's own release does not run (spec 14.2).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let init_b = m.declare("init_b", false);
    let failing = k.log_then(&mut m, "init B", k.oops());
    m.define(init_b, vec![], Type::Handler(k.res), failing);
    let (a, b, c) = (
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
        m.local("c", Type::Handler(k.res)),
    );
    let binds = vec![
        k.managed(&mut m, a, "A"),
        WithBind::managed(b, call_fails(init_b, vec![]), "B", k.log("release B")),
        k.managed(&mut m, c, "C"),
    ];
    let body = k.log_then(&mut m, "body", int(1));
    main_fn(&mut m, false, TypeId::INT, with(binds, body));
    assert_eq!(
        run("init_fails", m),
        "init A\ninit B\nrelease A\nerror: Oops\n"
    );
}

#[test]
fn a_release_defect_replaces_a_value_or_an_error() {
    // If the body ended with a value, the release defect is the `with`'s result. The remaining
    // releases still run (spec 14.4).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
    );
    let binds = vec![
        k.managed(&mut m, a, "A"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(&mut m, false, TypeId::INT, with(binds, int(1)));
    assert_eq!(
        run("release_defect_value", m),
        "init A\nrelease B\nrelease A\ndefect: B broke\n"
    );

    // Same when it ended with an error.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let a = m.local("a", Type::Handler(k.res));
    let binds = vec![k.breaking(&mut m, a, "A", "A broke")];
    main_fn(&mut m, false, TypeId::NEVER, with(binds, k.oops()));
    assert_eq!(
        run("release_defect_error", m),
        "release A\ndefect: A broke\n"
    );
}

#[test]
fn the_first_defect_wins_and_later_ones_are_noted() {
    // If the body is a defect, it stays the result; the release defect becomes a note in its
    // message (spec 14.4).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
    );
    let binds = vec![
        k.breaking(&mut m, a, "A", "A broke"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(
        &mut m,
        false,
        TypeId::NEVER,
        with(binds, builtin(Builtin::Panic, vec![string("boom")])),
    );
    assert_eq!(
        run("first_defect", m),
        "release B\nrelease A\ndefect: boom\n\
         note: the release of handler B also failed: B broke\n\
         note: the release of handler A also failed: A broke\n"
    );

    // If the body is a value, the first release defect is the result and later ones are notes.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, b) = (
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
    );
    let binds = vec![
        k.breaking(&mut m, a, "A", "A broke"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(&mut m, false, TypeId::INT, with(binds, int(1)));
    assert_eq!(
        run("first_release_defect", m),
        "release B\nrelease A\ndefect: B broke\n\
         note: the release of handler A also failed: A broke\n"
    );
}

#[test]
fn released_handlers_are_defects_when_used() {
    // Using a capability taken outside the `with` after its release is a defect (spec 8.6).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let (a, f) = (
        m.local("a", Type::Handler(k.res)),
        m.local("f", fn_ty(vec![], TypeId::STRING)),
    );
    let leak = with(
        vec![k.managed(&mut m, a, "Pool")],
        lambda(
            vec![],
            k.name_ty,
            call_op(var(a), k.res, 0, vec![], false, false),
        ),
    );
    // Inside, it works.
    let i = m.local("i", Type::Handler(k.res));
    let inside = with(
        vec![k.managed(&mut m, i, "Inside")],
        call_op(var(i), k.res, 0, vec![], false, false),
    );
    let x = m.local("x", TypeId::UNIT);
    main_fn(
        &mut m,
        false,
        TypeId::STRING,
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
    // Even if the body suspends and is resumed later, release runs after the body ends.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let js_clock = js_clock(&mut m, clock);
    let (c, a) = (
        m.local("c", Type::Handler(clock)),
        m.local("a", Type::Handler(k.res)),
    );
    let binds = vec![
        WithBind::new(c, call(js_clock, vec![])),
        k.managed(&mut m, a, "A"),
    ];
    let after = k.log_then(&mut m, "resumed", int(42));
    let x = m.local("_", TypeId::UNIT);
    let body = let_(x, sleep(c, clock, 1), after);
    main_fn(&mut m, true, TypeId::INT, with(binds, body));
    assert_eq!(
        run("later_resume", m),
        "init A\nresumed\nrelease A\nasync 42\n"
    );
}

#[test]
fn dropped_continuations_release_from_the_inside() {
    // A main dropped without a resume runs the releases of open `with`s from the inside out, then
    // ends as interrupted (spec 14.5). The inner `with` is inside the generator behind `yield*`.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let never = never_clock(&mut m, clock);
    let inner = m.declare("inner", false);
    m.functions[inner].suspends = true;
    let (c, b) = (
        m.local("c", Type::Handler(clock)),
        m.local("b", Type::Handler(k.res)),
    );
    let inner_body = with(
        vec![
            WithBind::new(c, call(never, vec![])),
            k.managed(&mut m, b, "B"),
        ],
        sleep(c, clock, 1),
    );
    m.define(inner, vec![], TypeId::UNIT, inner_body);
    let a = m.local("a", Type::Handler(k.res));
    let body = with(
        vec![k.managed(&mut m, a, "A")],
        call_suspends(inner, vec![]),
    );
    main_fn(&mut m, true, TypeId::UNIT, body);
    assert_eq!(
        run("dropped", m),
        "init A\ninit B\nrelease B\nrelease A\nasync interrupted\n"
    );
}

#[test]
fn immediate_contexts_release_when_dropped() {
    // A continuation not resumed inside a non-suspending main (R is Immediate) is also unwound on
    // the spot, running releases. The `with` (B) inside the generator driven by `$sync` leaves via
    // `gen.return()`, and the `with` (A) in main leaves via the `$Interrupted` exception
    // (spec 14.5).
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let never = never_clock(&mut m, clock);
    let inner = m.declare("inner", false);
    m.functions[inner].suspends = true;
    let (c, b) = (
        m.local("c", Type::Handler(clock)),
        m.local("b", Type::Handler(k.res)),
    );
    let inner_body = with(
        vec![
            WithBind::new(c, call(never, vec![])),
            k.managed(&mut m, b, "B"),
        ],
        sleep(c, clock, 1),
    );
    m.define(inner, vec![], TypeId::UNIT, inner_body);
    let a = m.local("a", Type::Handler(k.res));
    let body = with(
        vec![k.managed(&mut m, a, "A")],
        call_suspends(inner, vec![]),
    );
    main_fn(&mut m, false, TypeId::UNIT, body);
    assert_eq!(
        run("sync_dropped", m),
        "init A\ninit B\nrelease B\nrelease A\ninterrupted\n"
    );
}

#[test]
fn releases_may_suspend() {
    // A release may call suspend operations (spec 8.4). Resumed later, it continues. The same
    // holds when unwinding while suspended (interrupted comes after release finishes).
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
        let (c, n, a) = (
            m.local("c", Type::Handler(clock)),
            m.local("n", Type::Handler(clock)),
            m.local("a", Type::Handler(k.res)),
        );
        let x = m.local("_", TypeId::UNIT);
        let release = k.log_then(
            &mut m,
            "release A",
            let_(x, sleep(c, clock, 1), k.log("slept")),
        );
        let init = k.log_then(&mut m, "init A", k.handler("A"));
        let (body, body_ty) = if dropped {
            (sleep(n, clock, 1), TypeId::UNIT)
        } else {
            (int(1), TypeId::INT)
        };
        let binds = vec![
            WithBind::new(c, call(js_clock, vec![])),
            WithBind::new(n, call(never, vec![])),
            WithBind::managed(a, init, "A", release),
        ];
        main_fn(&mut m, true, body_ty, with(binds, body));
        assert_eq!(run(name, m), expected);
    }
}

#[test]
fn a_release_defect_while_interrupted_is_the_result() {
    // A release defect while unwinding a suspension becomes that run's defect. The remaining
    // releases still run.
    let mut m = Module::new();
    let k = Kit::new(&mut m);
    let clock = clock(&mut m);
    let never = never_clock(&mut m, clock);
    let (c, a, b) = (
        m.local("c", Type::Handler(clock)),
        m.local("a", Type::Handler(k.res)),
        m.local("b", Type::Handler(k.res)),
    );
    let binds = vec![
        WithBind::new(c, call(never, vec![])),
        k.managed(&mut m, a, "A"),
        k.breaking(&mut m, b, "B", "B broke"),
    ];
    main_fn(&mut m, true, TypeId::UNIT, with(binds, sleep(c, clock, 1)));
    assert_eq!(
        run("interrupted_release_defect", m),
        "init A\nrelease B\nrelease A\nasync defect: B broke\n"
    );
}
