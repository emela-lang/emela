//! Snapshot tests for effect set inference (spec chapter 8, 9.2 to 9.5), handler checks, and the
//! restrictions on `@external` and capability values.
//!
//! `render_effects` prints, besides the `render_bodies` output (declared types and diagnostics),
//! the expressions of `main.emel` with a non-empty R, in source order, as `expr: R`. A fn named
//! `main` is checked as `main` (its R must be empty, spec 8.3).

mod common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{CheckOptions, Supplier, check_program_with};

use common::{describe, resolve_main};

fn render_effects(src: &str) -> String {
    render(src, false)
}

/// `render_effects` plus handler init E, `with` initialization orders, and layer expansions.
fn render_handlers(src: &str) -> String {
    render(src, true)
}

fn render(src: &str, handlers: bool) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let in_main =
        |def: emela_resolve::hir::DefId| program.defs[def].module == DefModule::Source(module);
    let main = program
        .defs
        .iter()
        .find(|(def, data)| in_main(*def) && data.kind == DefKind::Fn && data.name == "main")
        .map(|(def, _)| def);
    let tests = program
        .defs
        .iter()
        .filter(|(def, data)| in_main(*def) && data.name.starts_with("test_"))
        .map(|(def, _)| def)
        .collect();
    let types = check_program_with(&program, &CheckOptions { main, tests });
    let cons = &types.ty_table.cons;

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if !in_main(def) || data.kind != DefKind::Fn {
            continue;
        }
        match types.sigs.get(def) {
            Some(sig) => writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap(),
            None => writeln!(out, "{}: <no type>", data.name).unwrap(),
        }
    }
    for (def, row) in types.handler_uses.iter() {
        if in_main(def) {
            writeln!(
                out,
                "handler {}: use {}",
                program.defs[def].name,
                row.display_effects(cons, &[])
            )
            .unwrap();
        }
    }
    if handlers {
        for (def, row) in types.handler_fails.iter() {
            if in_main(def) && !row.is_empty() {
                writeln!(
                    out,
                    "handler {}: init fails {}",
                    program.defs[def].name,
                    row.display(cons)
                )
                .unwrap();
            }
        }
        for (def, layer) in types.layers.iter() {
            if in_main(def) {
                let names: Vec<&str> = layer
                    .handlers
                    .iter()
                    .map(|&h| program.defs[h].name.as_str())
                    .collect();
                writeln!(
                    out,
                    "layer {}: [{}] use {}",
                    program.defs[def].name,
                    names.join(", "),
                    layer.uses.display_effects(cons, &[])
                )
                .unwrap();
            }
        }
        let mut withs: Vec<_> = types
            .with_handlers
            .iter()
            .map(|(expr, hs)| (program.exprs[expr].span.range, hs))
            .collect();
        withs.sort_by_key(|(range, _)| range.start());
        for (range, hs) in withs {
            let text = &src[usize::from(range.start())..usize::from(range.end())];
            let head = text.split('{').next().unwrap_or(text).trim();
            let names: Vec<String> = hs
                .binds
                .iter()
                .map(|b| {
                    let name = |def: emela_resolve::hir::DefId| program.defs[def].name.to_string();
                    match &b.supplier {
                        Supplier::Handler(h) if program.label_base(b.effect.def).is_some() => {
                            format!("{} by {}", name(b.effect.def), name(*h))
                        }
                        Supplier::Handler(h) => name(*h),
                        Supplier::Effect(t) => format!("{} <- {}", name(b.effect.def), name(t.def)),
                    }
                })
                .collect();
            let defaults: Vec<String> = hs
                .defaults
                .iter()
                .map(|(t, i)| format!(" {} from {i}", program.defs[t.def].name))
                .collect();
            writeln!(
                out,
                "{head}: init [{}]{}",
                names.join(", "),
                defaults.concat()
            )
            .unwrap();
        }
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    let mut diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| d.span.is_some_and(|s| s.module == module))
        .collect();
    diags.sort_by_key(|d| d.span.map(|s| s.range.start()));
    for d in diags {
        writeln!(out, "{}", describe(src, cons, d)).unwrap();
    }

    let mut rows: Vec<_> = types
        .effects
        .iter()
        .filter(|(_, row)| !row.is_empty())
        .filter_map(|(expr, row)| {
            let span = program.exprs[expr].span;
            (span.module == module).then_some((span.range, row))
        })
        .collect();
    rows.sort_by_key(|(range, _)| (range.start(), std::cmp::Reverse(range.end())));
    out.push_str("--- R\n");
    for (range, row) in rows {
        let text = &src[usize::from(range.start())..usize::from(range.end())];
        let text = text.replace('\n', " ");
        writeln!(out, "{text}: {}", row.display_effects(cons, &[])).unwrap();
    }
    out
}

/// `use X` puts X in R, even inside a branch, for the whole function's R (spec 8.2). Calls inherit
/// the callee's R.
#[test]
fn use_adds_to_r() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

fn log(msg: String) {
  (use Logger).info(msg)
}

fn maybe(c: Bool) -> Int {
  if c { (use Clock).now() } else { 0 }
}

fn both() {
  log("a")
  maybe(True)
}
"#
    ));
}

/// `with` removes the provided effects from R and adds the handlers' dependencies (spec 8.3, 9.4).
/// Handler dependencies may be met by handlers for other effects in the same `with` (spec 14.2).
#[test]
fn with_provides_effects() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

handler Quiet: Logger {
  fn info(msg) { () }
}

handler Stamped: Logger {
  fn info(msg) {
    t = (use Clock).now()
    ()
  }
}

handler Fixed: Clock {
  fn now() { 0 }
}

fn greet() {
  (use Logger).info("hi")
  (use Clock).now()
}

fn partly() {
  with Quiet { greet() }
}

fn needs_clock() {
  with Stamped { (use Logger).info("x") }
}

fn sibling() {
  with Stamped, Fixed { greet() }
}

fn main() {
  with Quiet, Fixed { greet() }
}
"#
    ));
}

/// Handlers in the same `with` depending on each other's effects form a cycle (E0411).
#[test]
fn handler_cycles() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Ping {
  fn ping() -> ()
}

effect Pong {
  fn pong() -> ()
}

handler HPing: Ping {
  fn ping() { (use Pong).pong() }
}

handler HPong: Pong {
  fn pong() { (use Ping).ping() }
}

fn f() {
  with HPing, HPong { (use Ping).ping() }
}
"#
    ));
}

/// Capability operation types come from the effect declaration, and an operation's fails adds to
/// the call's E (spec 8.1). Named arguments use the operation's parameter names. Unknown
/// operations are E0401.
#[test]
fn operations() {
    insta::assert_snapshot!(render_effects(
        r#"
error NotFound(id: Int)

effect Users {
  fn find(id: Int) -> String fails NotFound
}

fn by_label() {
  users = use Users
  users.find(id: 1)
}

fn wrong() {
  users = use Users
  a = users.find("x")
  users.fnd(1)
}
"#
    ));
}

/// A written `use` clause is exactly that set (spec 9.3); a larger body R is E0402. A function
/// type with `use {}` accepts only functions without effects (spec 9.2).
#[test]
fn declared_uses() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

fn declared() -> () use Logger {
  (use Logger).info("a")
}

fn too_many() -> () use Logger {
  (use Clock).now()
  ()
}

fn pure_only() -> () use {} {
  (use Logger).info("a")
}

fn run_pure(f: fn() -> () use {}) { f() }

fn callers() {
  run_pure(fn() { () })
  run_pure(fn() { (use Logger).info("x") })
  run_pure(declared)
}
"#
    ));
}

/// Omitting `use` on a function-typed parameter gives an invisible set variable that joins the
/// caller's R (spec 9.2). Lambdas enclose their body's R. Type parameters in `use R` (spec 9.5).
#[test]
fn hidden_and_named_sets() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }

fn update[A, R](x: A, f: fn(A) -> A use R) -> A use R { f(x) }

fn make() {
  fn() { (use Logger).info("later") }
}

fn callers() {
  a = apply(fn(n) { (use Logger).info("n")
  n }, 1)
  b = update(1, fn(n) { n + 1 })
  c = List.map([1], fn(n) { (use Logger).info("m") })
  f = make()
}

fn caller() {
  fn(g) { g() }
}
"#
    ));
}

/// Handler checks (spec 8.4): every operation implemented exactly once, parameter counts from the
/// effect, operation body E within the declared fails. A handler with fields needs init.
#[test]
fn handler_checks() {
    insta::assert_snapshot!(render_effects(
        r#"
error Boom

effect Logger {
  fn info(msg: String) -> ()
  fn warn(msg: String) -> () fails Boom
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

handler Half: Logger {
  fn info(msg) { () }
}

handler Twice: Logger {
  fn info(msg) { () }
  fn info(msg) { () }
  fn warn(a, b) { () }
}

handler Fails: Logger {
  fn info(msg) { fail Boom }
  fn warn(msg) { fail Boom }
}

handler Typed: Logger {
  fn info(msg) { msg + 1 }
  fn warn(msg) { self }
}

handler Stateful(n: Int): Logger {
  fn info(msg) { () }
  fn warn(msg) { () }
}

handler Sleepy: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
}
"#
    ));
}

/// An `@external` fn with `use` omitted only inside handler operation bodies; with `use {}`
/// anywhere (spec 12.2).
#[test]
fn external_functions() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Console {
  fn print_line(text: String) -> ()
}

@external(js, "console.log")
fn console_log(text: String) -> ()

@external(js, "Math.abs")
fn abs(x: Int) -> Int use {}

handler SystemConsole: Console {
  fn print_line(text) {
    f = fn() { console_log(text) }
    f()
  }
}

fn outside() {
  console_log("x")
  g = console_log
  abs(1)
}
"#
    ));
}

/// Capability values cannot be passed as arguments, stored in data structures, or returned
/// (spec 8.2).
#[test]
fn capability_restrictions() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

type Box(log: Int)

fn f() {
  log = use Logger
  xs = [log]
  p = (log, 1)
  s = Some(log)
  pipe = log |> Some
  id = fn(x) { x }
  id(log)
  log
}

fn g() {
  log = use Logger
  fn() { log }
}

fn h() {
  log = use Logger
  fn() { log.info("closure") }
}
"#
    ));
}

/// The R of `main` and test functions must be empty (spec 8.3, 13.3). A const's value cannot have
/// effects (spec 5.6).
#[test]
fn closed_definitions() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

const C: () = (use Logger).info("c")

fn main() {
  (use Logger).info("main")
}

fn test_inside() {
  with Quiet { (use Logger).info("ok") }
}

fn test_outside() -> () use Logger {
  (use Logger).info("t")
}
"#
    ));
}

/// `use { Quiet, R }` mixing an effect and a set type parameter (spec 8.3, 17.4), handled like the
/// error case `fails Timeout | E` (spec 7.4). The listed effect is removed by `with` and the set
/// type parameter remains. R gets the R of the passed function minus Quiet.
#[test]
fn mixed_effect_sets() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Quiet {
  fn hush() -> ()
}

effect Logger {
  fn info(msg: String) -> ()
}

handler Silent: Quiet {
  fn hush() { () }
}

fn run[R](f: fn() -> () use { Quiet, R }) -> () use R {
  with Silent { f() }
}

fn run_rev[R](f: fn() -> () use { R, Quiet }) -> () use R {
  with Silent { f() }
}

fn run_inferred[R](f: fn() -> () use { Quiet, R }) {
  with Silent { f() }
}

fn run_hidden(f: fn() -> ()) {
  with Silent { f() }
}

fn not_provided[R](f: fn() -> () use { Quiet, R }) -> () use R {
  f()
}

fn wrap[R](f: fn() -> () use R) -> fn() -> () use { Quiet, R } {
  fn() {
    (use Quiet).hush()
    f()
  }
}

fn forward(g: fn() -> ()) {
  run(g)
}

fn callers() {
  a = run(fn() {
    (use Quiet).hush()
    (use Logger).info("x")
  })
  b = run(fn() { (use Quiet).hush() })
  c = run(fn() { () })
  d = run_hidden(fn() { (use Quiet).hush() })
  e = run(wrap(fn() { (use Logger).info("y") }))
}
"#
    ));
}

/// suspend fn operations and implementations (spec 8.1, 8.4, 8.7). An implementation takes resume
/// as its last parameter (a function taking the operation's return type and returning `()`).
/// Using resume other than by calling it directly (storing it, capturing it in a closure, passing
/// it) puts Async in the handler's dependencies. Only Async may remain in the R of `main` and test
/// functions.
#[test]
fn suspend_operations() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Int
}

effect Ask {
  suspend fn ask() -> Int
}

@external(js, "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

fn later(f: fn(()) -> ()) { () }

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { 0 }
}

handler SystemClock: Clock {
  suspend fn sleep(ms, resume) {
    set_timeout(ms, fn() { resume(()) })
  }
  fn now() { 0 }
}

handler Stored: Clock {
  suspend fn sleep(ms, resume) {
    r = resume
    r(())
  }
  fn now() { 0 }
}

handler Passed: Clock {
  suspend fn sleep(ms, resume) { later(resume) }
  fn now() { 0 }
}

handler Piped: Ask {
  suspend fn ask(resume) { 42 |> resume }
}

handler WrongValue: Ask {
  suspend fn ask(resume) { resume("x") }
}

handler ReturnsValue: Ask {
  suspend fn ask(resume) { 42 }
}

handler NoResume: Ask {
  suspend fn ask() { () }
}

handler Mismatched: Clock {
  fn sleep(ms) { () }
  suspend fn now(resume) { resume(0) }
}

fn nap() {
  (use Clock).sleep(10)
  (use Clock).now()
}

fn instant() { with InstantClock { nap() } }

fn waits() { with SystemClock { nap() } }

fn pure_only() -> Int use {} { with SystemClock { nap() } }

fn main() {
  with SystemClock { nap() }
}

fn test_waits() {
  with SystemClock { nap() }
}
"#
    ));
}

/// Async is a built-in effect (spec 8.7). Handlers for it cannot be written (E0413), nor can
/// `use Async` (E0414). It may appear in use clauses.
#[test]
fn async_effect() {
    insta::assert_snapshot!(render_effects(
        r#"
handler MyAsync: Async {
}

fn grab() { use Async }

fn declared() -> Int use Async { 1 }

fn main() {
  with MyAsync { declared() }
}
"#
    ));
}

/// `@external suspend fn` requires Async and can only be used inside handlers (spec 12.2). fn
/// operation bodies cannot wait on it (E0417).
#[test]
fn external_suspend_functions() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Http {
  suspend fn get(url: String) -> String
}

@external(js, "./ffi.mjs", "fetchText")
suspend fn fetch_text(url: String) -> String

handler JsHttp: Http {
  suspend fn get(url, resume) { resume(fetch_text(url)) }
}

effect Blocking {
  fn get(url: String) -> String
}

handler AwaitingInFn: Blocking {
  fn get(url) { fetch_text(url) }
}

fn outside() { fetch_text("x") }

fn main() {
  with JsHttp { (use Http).get("x") }
}
"#
    ));
}

/// The body of an fn operation implementation must either have an Immediate R or contain no call
/// that may suspend (E0417, spec 8.1), the latter judged per call. Operation calls are decided by
/// the operation's declaration, so the fn operation `now` of Clock (which has a suspend fn) may be
/// called (Clock lands in the body's R and becomes a handler dependency). Calls of functions and
/// function values are decided by their type's R, so going through a function that only calls fn
/// operations, a closure capturing the capability, or an operation taken out as a value stays
/// E0417. Calls of suspend fn operations are E0417 too. But if the body's R is Immediate, the
/// calls are not looked at (sleep inside a `with` of PureClock, which resumes immediately). Mixing
/// the outer Clock's now with sleep inside the inner `with` satisfies neither and is E0417.
#[test]
fn fn_op_bodies_are_checked_per_call() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Int
}

effect Logger {
  fn info(msg: String) -> ()
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { (use Clock).now() }
}

handler CapturedClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    (use Logger).info("now")
    clock.now() + (clock.now() |> fn(x) { x })
  }
}

handler LazyClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    later = fn() { clock.sleep(1) }
    1
  }
}

fn stamp() -> Int use { Clock } { (use Clock).now() }

handler ViaFn: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { stamp() }
}

handler ViaClosure: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    f = fn() { clock.now() }
    f()
  }
}

handler ViaValue: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    g = clock.now
    g()
  }
}

handler Sleeping: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    (use Clock).sleep(1)
    0
  }
}

handler PureClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { 0 }
}

handler LocalSleep: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    with PureClock { (use Clock).sleep(1) }
    0
  }
}

handler MixedSleep: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    t = (use Clock).now()
    with PureClock { (use Clock).sleep(1) }
    t
  }
}
"#
    ));
}

/// `R: Immediate` (spec 9.5). An effect is Immediate when it has no suspend fn and is not Async; a
/// set type parameter only when it has the bound. Callbacks passed to external functions must be
/// Immediate too.
#[test]
fn immediate_constraint() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

@external(js, "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

@external(js, "./ffi.mjs", "fetchText")
suspend fn fetch_text(url: String) -> String

@external(js, "onTick")
fn on_tick(f: fn() -> () use { Clock }) -> ()

fn update[A, R: Immediate](x: A, f: fn(A) -> A use R) -> A use R { f(x) }

fn logs() -> Int {
  update(1, fn(x) {
    (use Logger).info("x")
    x
  })
}

fn sleeps() -> Int {
  update(1, fn(x) {
    (use Clock).sleep(1)
    x
  })
}

fn hidden(f: fn(Int) -> Int) -> Int { update(1, f) }

fn named[R: Immediate](f: fn(Int) -> Int use R) -> Int use R { update(1, f) }

fn unnamed[R](f: fn(Int) -> Int use R) -> Int use R { update(1, f) }

fn inferred() { fn(g) { update(1, g) } }

fn not_effect[A: Immediate](x: A) -> A { x }

fn trait_bound[R: Show](f: fn() -> () use R) -> () use R { f() }

handler Ticking: Logger {
  fn info(msg) {
    set_timeout(1, fn() { (use Logger).info(msg) })
    set_timeout(1, fn() { (use Clock).sleep(1) })
    set_timeout(1, fn() {
      fetch_text(msg)
      ()
    })
  }
}
"#
    ));
}

/// Declaration type parameters used in fails and use (`type Handler[E, R]` in spec 9.2). The kind
/// of a type parameter is decided by where it is used. A type argument position takes one tag of
/// the right kind or a set type parameter. Constructor calls infer E and R from the field
/// functions, and calling a field function puts its E and R on the caller.
#[test]
fn declaration_sets() {
    insta::assert_snapshot!(render_effects(
        r#"
type Request(path: String)
type Response(body: String)

error NotFound
error Timeout

effect Db {
  fn query(q: String) -> String
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

type Handler[E, R](run: fn(Request) -> Response fails E use R)

enum Step[E] {
  Done,
  Next(fn() -> Int fails E),
}

type Server[E](inner: Handler[E, Db])

type Ticker[R: Immediate](tick: fn() -> () use R)

handler MemDb: Db {
  fn query(q) { q }
}

fn db_handler() {
  Handler(run: fn(req) {
    body = (use Db).query(req.path)
    Response(body:)
  })
}

fn failing() -> Handler[NotFound, Db] {
  Handler(run: fn(req) { fail NotFound })
}

fn serve[E, R](h: Handler[E, R], req: Request) -> Response fails E use R {
  h.run(req)
}

fn served() -> Response {
  with MemDb {
    serve(failing(), Request(path: "/")) escape {
      NotFound -> Response(body: "404")
    }
  }
}

fn inferred(req: Request) {
  serve(db_handler(), req)
}

fn too_much() -> Handler[NotFound, Db] {
  Handler(run: fn(req) {
    (use Clock).sleep(1)
    fail Timeout
  })
}

fn bad_args(a: Handler[Int, Db], b: Handler[NotFound, NotFound], c: List[Db]) { () }

fn steps(s: Step[Timeout]) -> Int fails Timeout {
  s match {
    Done -> 0
    Next(f) -> f()
  }
}

fn server(s: Server[NotFound], req: Request) -> Response fails NotFound use Db {
  s.inner.run(req)
}

fn tickers() {
  a = Ticker(tick: fn() {
    (use Db).query("x")
    ()
  })
  b = Ticker(tick: fn() { (use Clock).sleep(1) })
}
"#
    ));
}

/// Sets written in type argument positions (spec 9.2). Error sets are joined with `|`; effect sets
/// are wrapped in braces. An empty `{}` fits either position. A set in a non-set type argument
/// position is E0324.
#[test]
fn declaration_set_args() {
    insta::assert_snapshot!(render_effects(
        r#"
type Request(path: String)
type Response(body: String)

error NotFound
error Timeout

effect Db {
  fn query(q: String) -> String
}

effect Log {
  fn info(msg: String) -> ()
}

type Handler[E, R](run: fn(Request) -> Response fails E use R)

fn both() -> Handler[NotFound | Timeout, { Db, Log }] {
  Handler(run: fn(req) {
    (use Log).info(req.path)
    if req.path == "/" { fail Timeout }
    body = (use Db).query(req.path)
    if body == "" { fail NotFound }
    Response(body:)
  })
}

fn pure() -> Handler[{}, {}] {
  Handler(run: fn(req) { Response(body: req.path) })
}

fn too_much() -> Handler[{}, {}] {
  Handler(run: fn(req) { fail NotFound })
}

fn wrap[E, R](h: Handler[E, R]) -> Handler[E | Timeout, { R, Log }] {
  Handler(run: fn(req) {
    (use Log).info("wrap")
    h.run(req)
  })
}

fn misplaced(a: Handler[{ Db }, Db], b: Handler[NotFound, NotFound | Timeout], c: List[{}], d: Handler[NotFound | Db, {}]) { () }
"#
    ));
}

/// Stateful handlers (spec 8.4): field types and `self`; init's value is the handler value; `use`
/// inside init and release are dependencies; init's E goes into the E of `with` (spec 9.4).
/// release may call suspend operations. The initialization order of `with` follows dependencies
/// (spec 14.2).
#[test]
fn stateful_handlers() {
    insta::assert_snapshot!(render_handlers(
        r#"
error DbDown

type Connection(url: String)

effect Config {
  fn db_url() -> String
}

effect Users {
  fn find(id: Int) -> String
}

effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

fn connect(url: String) -> Connection fails DbDown {
  if url == "" { fail DbDown } else { Connection(url:) }
}

handler PostgresUsers(conn: Connection): Users {
  init {
    config = use Config
    PostgresUsers(conn: connect(config.db_url()))
  }

  release {
    (use Logger).info(self.conn.url)
    (use Clock).sleep(10)
  }

  fn find(id) { self.conn.url }
}

handler EnvConfig: Config {
  fn db_url() { "postgres://" }
}

handler Quiet: Logger {
  init { Quiet }
  release { () }
  fn info(msg) { () }
}

handler Unit(): Logger {
  init { Unit() }
  fn info(msg) { () }
}

fn lookup() {
  with EnvConfig, PostgresUsers { (use Users).find(1) }
}

fn reversed() {
  with PostgresUsers, EnvConfig { (use Users).find(1) }
}

fn logged() {
  with Quiet, EnvConfig, PostgresUsers { (use Users).find(1) }
}
"#
    ));
}

/// Stateful handler errors: building the handler value outside init (E0418), fields without init
/// (E0419), a release that can fail (E0420), the type of init's value, and unknown fields.
#[test]
fn stateful_handler_errors() {
    insta::assert_snapshot!(render_handlers(
        r#"
error Boom

effect Logger {
  fn info(msg: String) -> ()
}

fn risky() -> () fails Boom { fail Boom }

handler Counter(n: Int): Logger {
  init { Counter(n: 0) }
  release { risky() }
  fn info(msg) {
    next = Counter(n: self.n + 1)
    ()
  }
}

handler Careful(n: Int): Logger {
  init { Careful(n: 1) }
  release { risky() escape { Boom -> () } }
  fn info(msg) { () }
}

handler NoInit(n: Int): Logger {
  fn info(msg) { () }
}

handler WrongInit(n: Int): Logger {
  init { 1 }
  fn info(msg) { self.m }
}

handler Plain: Logger {
  fn info(msg) {
    me = self
    ()
  }
}

fn make() { Counter(n: 1) }

fn lambda() { fn() { Counter(n: 1) } }
"#
    ));
}

/// Layers (spec 8.5): contents are handlers or other layers. Initialization order follows
/// dependencies, and unmet dependencies remain in the R of `with`. Two handlers providing the same
/// effect (E0421), mutual inclusion (E0422) and dependency cycles (E0411) are reported at the
/// layer declaration. In `with AppLive, FixedClock` the later one wins.
#[test]
fn layers() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Config {
  fn db_url() -> String
}

effect Users {
  fn find(id: Int) -> String
}

effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

effect Ping {
  fn ping() -> ()
}

effect Pong {
  fn pong() -> ()
}

handler EnvConfig: Config {
  fn db_url() { "x" }
}

handler ConsoleLogger: Logger {
  fn info(msg) { () }
}

handler SilentLogger: Logger {
  fn info(msg) { () }
}

handler FixedClock: Clock {
  fn now() { 0 }
}

handler RealClock: Clock {
  fn now() {
    (use Logger).info("now")
    1
  }
}

handler DbUsers(url: String): Users {
  init { DbUsers(url: (use Config).db_url()) }
  fn find(id) {
    (use Logger).info(self.url)
    self.url
  }
}

handler HPing: Ping {
  fn ping() { (use Pong).pong() }
}

handler HPong: Pong {
  fn pong() { (use Ping).ping() }
}

layer Base {
  ConsoleLogger
  RealClock
}

layer AppLive {
  DbUsers
  Base
  EnvConfig
}

layer Partial {
  DbUsers
  ConsoleLogger
}

layer Dup {
  ConsoleLogger
  SilentLogger
}

layer DupNested {
  Dup
  Base
}

layer Twice { ConsoleLogger, ConsoleLogger }

layer LoopA { LoopB }

layer LoopB { LoopA }

layer PingPong {
  HPing
  HPong
}

fn main() {
  with AppLive { (use Users).find(1) }
}

fn partial() {
  with Partial { (use Users).find(1) }
}

fn test_override() {
  with AppLive, FixedClock { (use Clock).now() }
}

fn later_wins() {
  with ConsoleLogger, SilentLogger { (use Logger).info("x") }
}

fn ping_pong() {
  with PingPong { (use Ping).ping() }
}

fn loops() {
  with LoopA { () }
}
"#
    ));
}

/// Labels (issue #255). `with L` provides the base from `L`; `L by P` provides `L`; a provider
/// of the base also provides the labels declared `default` that the body uses. A label's R holds
/// the label until something provides it.
#[test]
fn labels() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Log {
  fn log(msg: String) -> ()
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

effect Audit of Log
effect Debug of Log default
effect Other of Log
effect Ticks of Clock

handler PrintLog: Log {
  fn log(msg) { () }
}

handler Tagged: Log {
  fn log(msg) { (use Other).log(msg) }
}

fn a() { (use Log).log("a") }

fn c() {
  with Audit { a() }
  with Debug { a() }
}

fn direct() { (use Audit).log("x") }

fn run_hidden(f: fn() -> ()) { with PrintLog { f() } }

fn main() {
  with PrintLog, Audit by Log { c() }
  with PrintLog, Audit by PrintLog, Debug by Tagged, Other by Log { c() }
  with Audit by PrintLog { direct() }
  with PrintLog, Audit by Log { with Other by Audit { run_hidden(fn() { (use Other).log("y") }) } }
}

fn ticks[R: Immediate](f: fn() -> () use R) { f() }

fn sleeps() { ticks(fn() { (use Ticks).sleep(1) }) }
"#
    ));
}

/// Misused labels (issue #255): bases that cannot be used (E0429), effects that are not labels
/// (E0430), providers of another effect (E0431), two providers for a label (E0432), and handlers
/// of labels (E0433).
#[test]
fn label_errors() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Log {
  fn log(msg: String) -> ()
}

effect Other {
  fn other() -> ()
}

effect Audit of Log
effect Nested of Audit
effect Bad of Async

handler PrintLog: Log {
  fn log(msg) { () }
}

handler OtherH: Other {
  fn other() { () }
}

handler AuditH: Audit {
  fn log(msg) { () }
}

fn a() { (use Log).log("a") }

fn uses_audit() { (use Audit).log("x") }

fn main() {
  with Log { a() }
  with Log by PrintLog { a() }
  with PrintLog, Audit by OtherH { uses_audit() }
  with PrintLog, Audit by PrintLog, Audit by Log { uses_audit() }
}

fn undefault() {
  with PrintLog { uses_audit() }
}

fn unused(f: fn() -> ()) {
  with PrintLog, Audit by PrintLog { a() }
  with PrintLog, Audit by PrintLog { f() }
}
"#
    ));
}

/// Labels of an effect with type arguments (issue #255): the label's operations are typed with
/// the base's arguments, and the right of `by` may carry arguments too.
#[test]
fn labels_of_generic_effects() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Box[T] {
  fn get() -> T
}

effect Hits of Box[Int]
effect Names of Box[String] default
effect Bare of Box

handler IntBox: Box[Int] {
  fn get() { 1 }
}

handler StrBox: Box[String] {
  fn get() { "s" }
}

fn hits() -> Int { (use Hits).get() + 1 }

fn name() -> String { (use Names).get() }

fn main() {
  with Hits by IntBox { hits() }
  with IntBox, Hits by Box[Int] { hits() }
  with StrBox { name() }
  with IntBox { name() }
  with Hits by StrBox { hits() }
  with IntBox, Hits by Box[String] { hits() }
  with IntBox, Hits by Box { hits() }
}
"#
    ));
}

/// `[effect.with]`: the arguments of init are evaluated outside the `with`, so the items of the
/// same `with` do not provide their R, and the R of the arguments of a handler that is not
/// overridden is added to the R of the `with` (`[type.rules]`). The arguments of a handler a
/// later item overrides are not evaluated: their R is not added. `L by H(...)` is the same
/// (`[effect.label]`).
#[test]
fn init_arguments_are_evaluated_outside_the_with() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Config {
  fn url() -> String
}

effect Kv {
  fn get(key: String) -> String
}

effect AuditKv of Kv

handler EnvConfig: Config {
  fn url() { "kv://" }
}

handler RemoteKv(url: String): Kv {
  init(url: String) { RemoteKv(url:) }
  fn get(key) { self.url }
}

handler FakeKv: Kv {
  fn get(key) { "" }
}

fn own_config() {
  with EnvConfig, RemoteKv(url: (use Config).url()) { (use Kv).get("a") }
}

fn overridden() {
  with RemoteKv(url: (use Config).url()), FakeKv { (use Kv).get("a") }
}

fn label() {
  with AuditKv by RemoteKv(url: (use Config).url()) { (use AuditKv).get("a") }
}

fn main() {
  with EnvConfig {
    with EnvConfig, RemoteKv((use Config).url()) { (use Kv).get("a") }
  }
}
"#
    ));
}

/// `[effect.layer]`: the arguments of a layer's items are pure. Function calls are allowed; `use`
/// is E0402, with the help to list the handler after the layer.
#[test]
fn layer_arguments_are_pure() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Config {
  fn url() -> String
}

effect Kv {
  fn get(key: String) -> String
}

handler RemoteKv(url: String): Kv {
  init(url: String) { RemoteKv(url:) }
  fn get(key) { self.url }
}

const HOST: String = "kv"

fn url_of(host: String) -> String { "kv://#{host}" }

layer Pure {
  RemoteKv(url: url_of(HOST))
}

layer Impure {
  RemoteKv(url: (use Config).url())
}

fn main() {
  with Pure { (use Kv).get("a") }
}
"#
    ));
}

/// `[effect.handler]`: a handler whose init takes parameters needs arguments (E0434), and
/// anything else takes none (E0435, `with H()` included). The arguments are checked as those of
/// a call (E0305, E0306, E0307, E0301), also for a handler a later item overrides. The
/// parameters of init need types (E0316).
#[test]
fn init_argument_errors() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Kv {
  fn get(key: String) -> String
}

effect AuditKv of Kv

handler RemoteKv(url: String): Kv {
  init(url: String, retries: Int) { RemoteKv(url:) }
  fn get(key) { self.url }
}

handler FakeKv: Kv {
  fn get(key) { "" }
}

handler Untyped(url: String): Kv {
  init(url) { Untyped(url:) }
  fn get(key) { self.url }
}

layer AppLive {
  FakeKv
}

layer Missing {
  RemoteKv
}

fn f() {
  with RemoteKv { (use Kv).get("a") }
  with AuditKv by RemoteKv { (use AuditKv).get("a") }
  with FakeKv() { (use Kv).get("a") }
  with FakeKv(1) { (use Kv).get("a") }
  with AppLive(1) { (use Kv).get("a") }
  with AuditKv(1) by FakeKv { (use AuditKv).get("a") }
  with RemoteKv("a") { (use Kv).get("a") }
  with RemoteKv("a", 1, 2) { (use Kv).get("a") }
  with RemoteKv(url: "a", tries: 1) { (use Kv).get("a") }
  with RemoteKv("a", url: "b") { (use Kv).get("a") }
  with RemoteKv(url: 1, retries: 1), FakeKv { (use Kv).get("a") }
}
"#
    ));
}

/// `[effect.handler]`: `H(...)` at a `with` is a call of init, not a construction of the
/// handler value, so it is not E0418; constructing the value outside init still is.
#[test]
fn init_arguments_do_not_construct() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Kv {
  fn get(key: String) -> String
}

handler RemoteKv(url: String): Kv {
  init(url: String) { RemoteKv(url:) }
  fn get(key) { self.url }
}

fn ok() {
  with RemoteKv(url: "a") { (use Kv).get("a") }
}

fn bad() {
  RemoteKv(url: "a")
}
"#
    ));
}

/// `[effect.handler]`, `[effect.use]`: a capability value cannot be passed as an argument of init
/// (no parameter type holds it), and a closure that captures one can.
#[test]
fn capabilities_in_init_arguments() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Config {
  fn url() -> String
}

effect Kv {
  fn get(key: String) -> String
}

handler LazyKv(url: fn() -> String use { Config }): Kv {
  init(url: fn() -> String use { Config }) { LazyKv(url:) }
  fn get(key) { (self.url)() }
}

handler TakesString(url: String): Kv {
  init(url: String) { TakesString(url:) }
  fn get(key) { self.url }
}

fn closure() {
  c = use Config
  with LazyKv(url: fn() { c.url() }) { (use Kv).get("a") }
}

fn value() {
  c = use Config
  with TakesString(url: c) { (use Kv).get("a") }
}
"#
    ));
}

/// `[effect.handler]`, `[effect.layer]`: a handler listed in a layer without arguments needs them
/// when its init takes parameters (E0434), also in a layer no `with` names and in a layer where
/// no other item has arguments.
#[test]
fn layer_items_need_init_arguments() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Kv {
  fn get(key: String) -> String
}

effect Log {
  fn info(msg: String) -> ()
}

handler RemoteKv(url: String): Kv {
  init(url: String) { RemoteKv(url:) }
  fn get(key) { self.url }
}

handler QuietLog: Log {
  fn info(msg) { () }
}

layer Unused {
  RemoteKv
}

layer Missing {
  QuietLog
  RemoteKv
}

fn main() {
  with Missing { (use Kv).get("a") }
}
"#
    ));
}

/// `[effect.with]`, spec 8.1, 8.7: the arguments of a handler a later item overrides are not
/// evaluated, so a call in them that may suspend is not reported in an fn operation (E0417), and
/// `resume` written in them does not make the operation resume later.
#[test]
fn overridden_init_arguments_do_not_run() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Clock {
  suspend fn wait() -> Int
  fn now() -> Int
}

effect Kv {
  fn get(key: String) -> String
}

handler Slow(ms: Int): Kv {
  init(ms: Int) { Slow(ms:) }
  fn get(key) { "" }
}

handler Kept(k: fn(Int) -> ()): Kv {
  init(k: fn(Int) -> ()) { Kept(k:) }
  fn get(key) { "" }
}

handler Fast: Kv {
  fn get(key) { "" }
}

handler Ticking: Clock {
  suspend fn wait(resume) {
    with Kept(k: resume), Fast { resume(1) }
  }
  fn now() {
    (use Clock).wait()
    with Slow(ms: (use Clock).wait()), Fast { (use Kv).get("a") }
    1
  }
}
"#
    ));
}
