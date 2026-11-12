//! Tests for lowering (`emela_core::lower`).

mod support;

use emela_core::{EnumDef, Expr, Module, Roots, Type, TypeId, VariantFields, dump};
use support::{Options, lower_files, lower_with, try_lower, try_lower_with};

/// Renders an enum definition as `Name[2] { Ctor(first: A, second: B), Unit }`, with type
/// parameters as `A`, `B`, ….
fn render_enum(m: &Module, def: &EnumDef) -> String {
    let variants: Vec<String> = def
        .variants
        .iter()
        .map(|v| {
            let tys = v.tys.iter().map(|t| render_ty(m, t));
            match &v.fields {
                VariantFields::Unit => v.name.clone(),
                VariantFields::Positional(_) => {
                    format!("{}({})", v.name, tys.collect::<Vec<_>>().join(", "))
                }
                VariantFields::Named(names) => {
                    let fields: Vec<String> = names
                        .iter()
                        .zip(tys)
                        .map(|(n, t)| format!("{n}: {t}"))
                        .collect();
                    format!("{}({})", v.name, fields.join(", "))
                }
            }
        })
        .collect();
    format!("{}[{}] {{ {} }}", def.name, def.params, variants.join(", "))
}

fn render_ty(m: &Module, ty: &TypeId) -> String {
    let list = |ts: &[TypeId]| {
        ts.iter()
            .map(|t| render_ty(m, t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match m.ty(*ty) {
        Type::Param(i) => char::from(b'A' + *i as u8).to_string(),
        Type::Tuple(ts) => format!("({})", list(ts)),
        Type::List(t) => format!("List[{}]", render_ty(m, t)),
        Type::Enum(id, ts) if ts.is_empty() => m.enums[*id].name.clone(),
        Type::Enum(id, ts) => format!("{}[{}]", m.enums[*id].name, list(ts)),
        other => format!("{other:?}"),
    }
}

fn enum_named<'m>(m: &'m Module, name: &str) -> &'m EnumDef {
    m.enums
        .iter()
        .map(|(_, e)| e)
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no enum `{name}`"))
}

/// The part of the dump whose first line contains `key`.
fn fn_part(m: &Module, key: &str) -> String {
    let out = dump(m);
    out.split("\n\n")
        .find(|part| part.lines().next().unwrap_or_default().contains(key))
        .unwrap_or_else(|| panic!("no {key}"))
        .to_owned()
}

fn fn_names(m: &Module) -> Vec<&str> {
    m.functions.iter().map(|(_, f)| f.name.as_str()).collect()
}

fn enum_names(m: &Module) -> Vec<&str> {
    m.enums.iter().map(|(_, e)| e.name.as_str()).collect()
}

#[test]
fn names_are_qualified_except_in_the_entry() {
    let m = lower_files(&[
        (
            "main.emel",
            "import Shapes\n\nfn main() { helper() }\n\nfn helper() -> List[Int] { Shapes.count() }\n\nfn unused() -> Int { 2 }\n",
        ),
        (
            "shapes.emel",
            "pub enum Shape { Circle(Float), Square(Float) }\n\npub fn count() -> List[Int] { List.range(0, 2) }\n",
        ),
    ]);
    // Only fns reachable from `main` are lowered: not the unused `unused`, nor other core
    // functions. The private `List.range_onto` also gets its module name.
    let mut fns = fn_names(&m);
    fns.sort_unstable();
    assert_eq!(
        fns,
        [
            "List.range",
            "List.range_onto",
            "Shapes.count",
            "helper",
            "main"
        ]
    );

    let enums = enum_names(&m);
    assert!(enums.contains(&"Shapes.Shape"), "{enums:?}");
    // Only one Option, and Bool is not an IR enum.
    assert_eq!(
        enums.iter().filter(|&&e| e == "Option").count(),
        1,
        "{enums:?}"
    );
    assert!(!enums.iter().any(|e| e.ends_with("Bool")), "{enums:?}");
}

/// The roots of a normal build are `main` and the `@export` fns (`[ext.export]`); a `pub` fn is not
/// one (`[target.js.module]`). An exported fn carries its host names.
#[test]
fn entry_exports_main_and_export_fns() {
    let m = lower_files(&[(
        "main.emel",
        "fn main() { 0 }\n\n@export(js, \"api\")\n@export(js, \"default\", \"api\")\npub fn api() -> Int { 1 }\n\npub fn other() -> Int { 2 }\n",
    )]);
    let exported: Vec<&str> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(_, f)| f.name.as_str())
        .collect();
    assert_eq!(exported, ["main", "api"]);
    let (_, api) = m.functions.iter().find(|(_, f)| f.name == "api").unwrap();
    let names: Vec<(Option<&str>, &str)> = api
        .host_exports
        .iter()
        .map(|x| (x.interface.as_deref(), x.name.as_str()))
        .collect();
    assert_eq!(names, [(None, "api"), (Some("default"), "api")]);
}

#[test]
fn self_tail_calls_come_out_as_loops() {
    // Lowering loopifies once for every backend (spec 16.3).
    let m = lower_files(&[(
        "main.emel",
        "fn main() -> Int { count(10, 0) }\n\nfn count(n: Int, acc: Int) -> Int {\n  if n == 0 { acc } else { count(n - 1, acc + 1) }\n}\n",
    )]);
    let (_, count) = m.functions.iter().find(|(_, f)| f.name == "count").unwrap();
    assert!(matches!(count.body, Expr::Loop { .. }), "{}", dump(&m));
}

#[test]
fn ctors_carry_field_names_and_types() {
    let m = lower_files(&[(
        "main.emel",
        "\
type Pair[A, B](first: A, second: B)

enum Shape {
  Circle(Float),
  Rect(width: Float, height: Float),
  Empty,
}

type Wrapper(items: List[Option[Shape]], pair: Pair[Int, (Bool, String)])

error NotFound(id: Int)

fn main() { 0 }
",
    )]);
    let rendered: Vec<String> = ["Pair", "Shape", "Wrapper", "NotFound", "Option"]
        .iter()
        .map(|name| render_enum(&m, enum_named(&m, name)))
        .collect();
    assert_eq!(
        rendered,
        [
            "Pair[2] { Pair(first: A, second: B) }",
            "Shape[0] { Circle(Float), Rect(width: Float, height: Float), Empty }",
            "Wrapper[0] { Wrapper(items: List[Option[Shape]], pair: Pair[Int, (Bool, String)]) }",
            "NotFound[0] { NotFound(id: Int) }",
            "Option[1] { Some(A), None }",
        ]
    );
}

#[test]
fn dbg_of_arithmetic() {
    let m = lower_files(&[(
        "main.emel",
        "\
fn main() {
  x = 1 + 2 * 3
  big: Int64 = 0x7FFF_FFFF_FF
  _ = dbg(x)
  dbg(-big)
  greet(name: \"emela\", times: 2)
}

fn greet(times: Int, name: String) -> Bool {
  if times > 0 && String.length(name) != 0 { True } else { False }
}
",
    )]);
    insta::assert_snapshot!(dump(&m));
}

#[test]
fn unsupported_expressions_are_all_reported() {
    let errors = try_lower(
        &[(
            "main.emel",
            "\
import Std.Console.{Console, SystemConsole}

const LIMIT: Int = 3

fn main() {
  assert LIMIT == 3
  with SystemConsole {
    x = use Console
    dbg(\"x = #{x}\")
  }
}
",
        )],
        Roots::Exported,
    )
    .unwrap_err();
    assert_eq!(errors, ["`x` showing a capability value"]);
}

/// With every fn as a root, checks that all core functions lower. Unreachable fns are not lowered
/// in a normal build, so errors in unused core functions are only caught here.
#[test]
fn every_core_function_lowers() {
    let m = match try_lower(&[("main.emel", "fn main() { 0 }\n")], Roots::All) {
        Ok(m) => m,
        Err(errors) => panic!("{errors:#?}"),
    };
    let names = fn_names(&m);
    assert!(names.contains(&"List.map"));
    // Handlers and external functions are lowered too (the bundled SystemConsole).
    assert!(names.contains(&"Std.Console.SystemConsole"), "{names:?}");
    assert!(names.contains(&"Std.Console.console_log"));
}

#[test]
fn match_and_binding_patterns() {
    let m = lower_files(&[(
        "main.emel",
        "\
enum Shape {
  Circle(Float)
  Rect(width: Float, height: Float)
  Empty
}

type Pair[A, B](first: A, second: B)

fn main() {
  dbg(area(Rect(height: 2.0, width: 3.0)))
  dbg(describe([1, 2, 3]))
  (a, b) = (1, \"x\")
  Pair(second:, ..) = Pair(first: a, second: b)
  dbg(second)
}

fn area(s: Shape) -> Float {
  s match {
    Circle(r) -> r * r * 3.0
    Rect(width: w, ..) if w > 10.0 -> 0.0
    Rect(width: w, height: h) -> w * h
    Empty -> 0.0
  }
}

fn describe(xs: List[Int]) -> String {
  xs match {
    [] -> \"empty\"
    [1, ..] -> \"one first\"
    [_, ..rest] -> if rest == [] { \"single\" } else { \"many\" }
  }
}
",
    )]);
    insta::assert_snapshot!(dump(&m));
}

/// A pattern's `-` and number are one literal value (spec 6.5); Int64 and Float keep their sign.
#[test]
fn negative_number_patterns() {
    let m = lower_files(&[(
        "main.emel",
        "\
fn main() {
  dbg(int(-1))
  dbg(long(-5))
  dbg(float(-0.0))
}

fn int(n: Int) -> Int {
  n match {
    -2147483648 -> 0
    -0x10 -> 1
    _ -> 2
  }
}

fn long(n: Int64) -> Int {
  n match {
    -9223372036854775808 -> 0
    _ -> 1
  }
}

fn float(x: Float) -> Int {
  x match {
    -0.0 -> 0
    -1.5 -> 1
    _ -> 2
  }
}
",
    )]);
    insta::assert_snapshot!(dump(&m));
}

#[test]
fn lambdas_and_wrapped_functions() {
    let m = lower_files(&[(
        "main.emel",
        "\
fn main() {
  k = 10
  add_k = |x| x + k
  dbg(add_k(1))
  dbg(List.map([1, 2], Some))
  dbg(List.map([\"a\", \"bc\"], String.length))
  dbg(apply(f: add_k, x: 2))
}

fn apply(x: Int, f: fn(Int) -> Int) -> Int {
  f(x)
}
",
    )]);
    let out = dump(&m);
    // Core functions are long, so only look at the entry module's functions.
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| f.contains("fn main(") || f.contains("fn apply("))
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

#[test]
fn fields_updates_and_interpolation() {
    let m = lower_files(&[(
        "main.emel",
        "\
type User(id: Int, name: String, tags: List[String])

type Pair[A, B](first: A, second: B)
  derive Show

fn main() {
  u = User(id: 1, name: \"a\", tags: [])
  renamed = User(tags: [\"x\"], name: String.append(u.name, \"!\"), ..u)
  p = Pair(first: 1, second: True)
  q = Pair(first: \"s\", ..p)
  \"#{renamed.name} has #{renamed.tags} and #{q}, id #{u.id}\"
}
",
    )]);
    let out = dump(&m);
    let main = out.split("\n\n").find(|f| f.contains("fn main(")).unwrap();
    insta::assert_snapshot!(main);
}

#[test]
fn asserts_and_consts() {
    let m = lower_files(&[(
        "main.emel",
        "\
const LIMIT: Int = 3
const ORIGIN: (Int, Int) = (0, 0)
const EMPTY = []

fn main() {
  assert LIMIT == 3
  assert classify(2) != \"big\"
  assert ORIGIN == (0, 0)
  assert List.length(EMPTY) == 0
  assert same(1, 1)
  assert is_small(LIMIT)
}

fn classify(n: Int) -> String {
  n match {
    LIMIT -> \"limit\"
    0 -> \"zero\"
    m if m > 100 -> \"big\"
    _ -> \"other\"
  }
}

fn same[A: Eq](a: A, b: A) -> Bool {
  assert a == b
  True
}

fn is_small(n: Int) -> Bool {
  n < LIMIT + 1
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            ["main(", "classify(", "same(", "LIMIT(", "ORIGIN(", "EMPTY("]
                .iter()
                .any(|name| f.contains(&format!("fn {name}")))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// fail and escape (spec 7.2, 7.3). Fallible calls get `?`, and errors become `error enum`.
#[test]
fn fail_and_escape() {
    let m = lower_files(&[(
        "main.emel",
        "\
error NotFound(id: Int)
error Unauthorized

fn find(id: Int) -> Int fails NotFound | Unauthorized {
  if id > 0 { id } else if id == 0 { fail NotFound(id:) } else { fail Unauthorized }
}

fn main() {
  a = find(1) escape {
    NotFound(id:) if id > 5 -> id
    Unauthorized -> fail NotFound(id: 0)
  }
  b = Some(1) |> Option.or_fail(Unauthorized)
  c = pure(a) + b
  dbg(c)
}

fn pure(n: Int) -> Int {
  n + 1
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            [
                "fn main(",
                "fn find(",
                "fn pure(",
                "fn Option.or_fail(",
                "error enum",
            ]
            .iter()
            .any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// An escape arm `_` lowers to an arm with the pattern `_`, after the arms above it
/// (`[error.escape]`). The core verifier accepts it.
#[test]
fn escape_wildcard_arm() {
    let m = lower_files(&[(
        "main.emel",
        "\
error NotFound(id: Int)
error Unauthorized

fn find(id: Int) -> Int fails NotFound | Unauthorized {
  if id > 0 { id } else if id == 0 { fail NotFound(id:) } else { fail Unauthorized }
}

fn main() {
  a = find(1) escape {
    NotFound(id: 3) -> 3
    _ -> 0
  }
  dbg(a)
}
",
    )]);
    let out = dump(&m);
    let main = out.split("\n\n").find(|f| f.contains("fn main(")).unwrap();
    insta::assert_snapshot!(main);
}

/// use, with, handlers, and operations on capability values (spec 8.2 to 8.4). Functions and
/// lambdas with a non-empty R take the evidence table as their last argument, and `with` extends
/// the table. The Clock in the same `with` satisfies Stamp's dependency.
#[test]
fn effects_and_handlers() {
    let m = lower_files(&[(
        "main.emel",
        "\
error Late

effect Clock {
  fn now() -> Int
}

effect Stamp {
  fn stamp(s: String) -> String fails Late
}

handler Fixed: Clock {
  fn now() { 42 }
}

handler ClockStamp: Stamp {
  fn stamp(s) {
    t = (use Clock).now()
    if t > 100 { fail Late }
    \"#{s}@#{t}\"
  }
}

fn stamped(s: String) {
  (use Stamp).stamp(s)
}

fn apply(f: fn() -> Int) -> Int {
  f()
}

fn main() {
  with Fixed, ClockStamp {
    clock = use Clock
    dbg(stamped(\"a\") escape { Late -> \"late\" })
    dbg(apply(|| clock.now() + (use Clock).now()))
    dbg(List.map([1], |x| x + (use Clock).now()))
    f = clock.now
    dbg(f())
  }
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            [
                "fn main(",
                "fn stamped(",
                "fn apply(",
                "fn Fixed(",
                "fn ClockStamp(",
                "effect ",
            ]
            .iter()
            .any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// Stateful handlers and layers (spec 8.4, 8.5, 14.2). A stateful handler becomes three functions:
/// init (`$init`), one that builds the handler value (taking the state as its first argument), and
/// release (`$release`). `with` expands the layer and orders it by dependency (Env, Pool), binding
/// init values without release and handler values with release. AppLive's Pool is overridden by
/// the later Spare, so it is not initialized.
#[test]
fn stateful_handlers_and_layers() {
    let m = lower_files(&[(
        "main.emel",
        "\
error Down

effect Config {
  fn url() -> String
}

effect Users {
  fn find(id: Int) -> String
}

handler Env: Config {
  fn url() { \"db\" }
}

handler Pool(url: String, n: Int): Users {
  init {
    u = (use Config).url()
    if u == \"\" { fail Down }
    Pool(n: 1, ..Pool(url: u, n: 0))
  }
  release { dbg(self.url) }
  fn find(id) { self.url }
}

handler Spare: Users {
  init { Spare }
  fn find(id) { \"spare\" }
}

layer AppLive {
  Pool
  Env
}

fn main() {
  dbg(with AppLive { (use Users).find(1) } escape { Down -> \"down\" })
  with AppLive, Spare { dbg((use Users).find(2)) }
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            ["fn main(", "Pool", "Spare", "fn Env("]
                .iter()
                .any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// The arguments of handler init (`[effect.handler]`, `[dyn.with]`). init takes its parameters
/// before the table. A `with` binds the arguments first, in the order the items are written and
/// without release, then runs the inits in the order of the dependencies. The arguments of a
/// handler a later item overrides are not bound. An argument written in a layer becomes a const
/// (`AppLive$arg0`), called where the layer is expanded.
#[test]
fn handler_init_arguments() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Config {
  fn url() -> String
}

effect Users {
  fn find(id: Int) -> String
}

handler Env(url: String): Config {
  init(url: String) { Env(url:) }
  fn url() { self.url }
}

handler Pool(url: String): Users {
  init(n: Int) { Pool(url: (use Config).url()) }
  fn find(id) { self.url }
}

handler Spare: Users {
  fn find(id) { \"spare\" }
}

fn port() -> Int { 5432 }

layer AppLive {
  Env(url: \"db\")
}

fn main() {
  with Pool(n: port()), Env(url: \"a\") { dbg((use Users).find(1)) }
  with AppLive, Pool(n: 2) { dbg((use Users).find(2)) }
  with Pool(n: port()), Spare { dbg((use Users).find(3)) }
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            ["fn main(", "Pool$init", "Env$init", "AppLive"]
                .iter()
                .any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// State.run (spec 8.8) and effects with type arguments (spec 8.1). State.run creates a cell and a
/// handler value that reads and writes it, adds it to the table under the key `State[S]`, and calls
/// body. A generic fn body looks it up by its own type parameter's key (`State[$modify.0]`), and
/// the caller adds the handler for the instantiated key (`State[Int]`) under that key.
#[test]
fn state_run_and_effect_type_args() {
    let m = lower_files(&[(
        "main.emel",
        "\
import Std.State.{State}\n\nfn modify[S](f: fn(S) -> S fails {} uses {}) -> S uses { State[S] } {
  (use State[S]).update(f)
  (use State[S]).get()
}

fn main() {
  dbg(State.run(\"\", || {
    State.run(1, || {
      (use State[String]).set(\"x\")
      modify(|n| n + 1)
    })
  }))
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            f.starts_with("effect ") || ["fn main(", "fn modify("].iter().any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// Concurrency (spec chapter 11). `Fiber.scope` builds the scope capability's handler (fork and
/// deferred) and calls the body, and fork wraps the child function. A child runs with the table at
/// the `Fiber.scope` call plus the forking capability value as Scope (spec 11.2). attempt wraps a
/// function with an R into one that calls it with the current table.
#[test]
fn fiber_scope_fork_and_attempt() {
    let m = lower_files(&[(
        "main.emel",
        "\
import Std.Fiber.{Fiber, Scope}
import Std.Exit.{Exit}
import Std.Deferred.{Deferred}

effect Log { fn info(msg: String) -> () }

fn main() -> Int uses { Log, Async } {
  Fiber.scope(|| {
    s = use Scope
    a = s.fork(|| (use Log).info(\"a\"))
    b = Fiber.fork(|| 2)
    d = s.deferred()
    Deferred.complete(d, 1)
    Fiber.join(a)
    e = Exit.attempt(|| (use Log).info(\"e\"))
    Fiber.join(b) + Deferred.await(d)
  })
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            f.starts_with("effect ")
                || ["fn main(", "fn Std.Fiber.fork("]
                    .iter()
                    .any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// The other concurrency functions of Std become operations of the IR (spec chapter 11).
#[test]
fn fiber_await_interrupt_poll_and_causes() {
    let m = lower_files(&[(
        "main.emel",
        "\
import Std.Fiber.{Fiber, Scope}
import Std.Exit.{Exit, Success, Failure}
import Std.Deferred.{Deferred}
import Std.Cause.{Cause, CauseKind}

error Boom

fn main() -> Int fails Boom uses { Async } {
  Fiber.scope(|| {
    a = Fiber.fork(|| 1)
    d = (use Scope).deferred()
    Fiber.interrupt(a)
    p = Deferred.poll(d)
    Fiber.await(a) match {
      Success(x) -> x
      Failure(c) -> {
        k = Cause.kind(c)
        s = Cause.to_string(c)
        Cause.raise(c)
      }
    }
  })
}
",
    )]);
    insta::assert_snapshot!(fn_part(&m, "fn main("));
}

/// The fns of the entry module (unqualified names, no `:` of impls), with ` exported` on roots.
fn entry_fns(m: &Module) -> Vec<String> {
    let mut out: Vec<String> = m
        .functions
        .iter()
        .filter(|(_, f)| !f.name.contains(['.', ':', '$', '#']))
        .map(|(_, f)| {
            let exported = if f.exported { " exported" } else { "" };
            format!("{}{exported}", f.name)
        })
        .collect();
    out.sort();
    out
}

/// `Roots::Public` starts from `main` and the entry's `pub` fns, except test functions and fns
/// with a trait-bounded type parameter.
#[test]
fn public_roots_are_pub_fns_of_the_entry() {
    let files = [
        (
            "main.emel",
            "\
import Lib

fn main() { 0 }

pub fn api() -> Int { helper() + Lib.lib_api() }

fn helper() -> Int { 1 }

pub fn shown[A: Show](x: A) -> String { Show.show(x) }

pub fn check() -> () { () }

pub fn same[A](x: A) -> A { x }

fn unused() -> Int { 2 }
",
        ),
        (
            "lib.emel",
            "pub fn lib_api() -> Int { 2 }\n\npub fn lib_other() -> Int { 3 }\n",
        ),
    ];
    let options = Options {
        roots: Roots::Public,
        tests: &["check"],
        ..Options::default()
    };
    let m = lower_with(&files, &options).unwrap().module;
    assert_eq!(
        entry_fns(&m),
        ["api exported", "helper", "main exported", "same exported"]
    );
    assert!(fn_names(&m).contains(&"Lib.lib_api"));
    assert!(!fn_names(&m).contains(&"Lib.lib_other"));
}

/// `Roots::Tests` starts from the test functions, in the order given, and returns their IR
/// functions after monomorphization. A test function named `main` is not the entry's main.
#[test]
fn test_roots_are_the_test_functions() {
    let files = [(
        "main.emel",
        "\
fn main() { 0 }

fn check_b() -> () { assert helper() == 1 }

fn check_a() -> () { () }

fn helper() -> Int { 1 }

fn other() -> Int { 2 }
",
    )];
    for target in [None, Some("js")] {
        let options = Options {
            roots: Roots::Tests,
            target,
            tests: &["check_b", "check_a"],
            ..Options::default()
        };
        let lowered = lower_with(&files, &options).unwrap();
        let names: Vec<&str> = lowered
            .tests
            .iter()
            .map(|&f| lowered.module.functions[f].name.as_str())
            .collect();
        assert_eq!(names, ["check_b", "check_a"], "{target:?}");
        assert_eq!(
            entry_fns(&lowered.module),
            ["check_a", "check_b", "helper"],
            "{target:?}"
        );
    }
    let options = Options {
        roots: Roots::Tests,
        tests: &["main"],
        ..Options::default()
    };
    let lowered = lower_with(&files, &options).unwrap();
    assert_eq!(entry_fns(&lowered.module), ["main"]);
}

/// `Roots::All` lowers every fn, const and handler, used or not.
#[test]
fn all_roots_include_unused_consts_and_handlers() {
    let m = try_lower(
        &[(
            "main.emel",
            "\
effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

const LIMIT: Int = 3

fn main() { 0 }
",
        )],
        Roots::All,
    )
    .unwrap();
    let names = fn_names(&m);
    assert!(names.contains(&"LIMIT"), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("Quiet")), "{names:?}");
}

/// External functions reachable from the entries without an `@external` for the target are
/// reported, in declaration order (spec 12.1). A handler's operation is followed only once it is
/// called.
#[test]
fn missing_externals_for_the_target() {
    let files = [(
        "main.emel",
        "\
effect Get {
  fn get() -> Int
}

effect Put {
  fn put() -> Int
}

@external(js, \"a\")
fn host_a() -> Int uses {}

@external(js, \"b\")
fn host_b() -> Int uses {}

@external(js, \"c\")
fn host_c() -> Int uses {}

@external(js, \"d\")
@external(c, \"./d.c\", \"d\")
fn host_d() -> Int uses {}

@external(js, \"e\")
fn host_e() -> Int uses {}

handler Gets: Get {
  fn get() { host_b() }
}

handler Puts: Put {
  fn put() { host_c() }
}

fn main() {
  f = host_e
  with Gets, Puts {
    host_a() + (use Get).get() + host_d() + f()
  }
}
",
    )];
    let options = |target| Options {
        target: Some(target),
        ..Options::default()
    };
    assert_eq!(
        lower_with(&files, &options("c")).unwrap_err(),
        [
            "`host_a` missing c external host_a",
            "`host_b` missing c external host_b",
            "`host_e` missing c external host_e",
        ]
    );
    assert!(lower_with(&files, &options("js")).is_ok());
}

/// The host names of each exported fn of the entry, for the target only (`[ext.export]`).
fn host_exports(m: &Module) -> Vec<String> {
    let mut out: Vec<String> = m
        .functions
        .iter()
        .flat_map(|(_, f)| {
            f.host_exports.iter().map(|e| {
                let interface = e
                    .interface
                    .as_deref()
                    .map_or(String::new(), |i| format!("{i} "));
                format!("{}: {} {interface}{}", f.name, e.target, e.name)
            })
        })
        .collect();
    out.sort();
    out
}

/// A build for a target exposes the exports for that target. On `c` a fn that may suspend cannot
/// be exported (E0441), and on `wasm` no export written by a user is (`[target.wasm.external]`).
#[test]
fn exports_for_the_target() {
    let files = [(
        "main.emel",
        "\
import Std.Fiber.{Fiber}

@export(js, \"api\")
@export(c, \"api\")
@export(wasm, \"api\")
pub fn api() -> Int { 1 }

@export(js, \"default\", \"late\")
@export(c, \"late\")
pub fn late() -> Int uses { Async } { Fiber.scope(|| 1) }

@export(js, \"only_js\")
pub fn only_js() -> Int { 2 }

fn main() { 0 }
",
    )];
    let lower = |target| {
        let options = Options {
            target: Some(target),
            ..Options::default()
        };
        lower_with(&files, &options).map(|l| host_exports(&l.module))
    };
    assert_eq!(
        lower("js").unwrap(),
        [
            "api: js api",
            "late: js default late",
            "only_js: js only_js"
        ]
    );
    assert_eq!(
        lower("c").unwrap_err(),
        // At the fn's first `@export`.
        ["`@export(js, \"default\", \"late\")` suspending export late"]
    );
    assert_eq!(
        lower("wasm").unwrap_err(),
        ["`@export(js, \"api\")` wasm export api"]
    );
}

/// `@entry(Http.serve)` makes an HTTP service: the runtime exposes it on `js` and `wasm`, and `c`
/// has none (`[lib.http-service]`).
#[test]
fn http_service_entry() {
    let files = [(
        "main.emel",
        "\
import Std.Http.{Http}
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}

@entry(Http.serve)
pub fn app(req: Request) -> Response uses { Async } { Http.response(200, \"ok\") }
",
    )];
    let lower = |target| {
        let options = Options {
            target: Some(target),
            ..Options::default()
        };
        lower_with(&files, &options).map(|l| (l.module.http_service, host_exports(&l.module)))
    };
    assert_eq!(
        lower("js").unwrap(),
        (true, vec!["app$entry0: js default fetch".to_owned()])
    );
    assert_eq!(
        lower("wasm").unwrap(),
        (
            true,
            vec!["app$entry0: wasm wasi:http/handler@0.3.0 handle".to_owned()]
        )
    );
    assert_eq!(
        lower("c").unwrap_err(),
        ["`@entry(Http.serve)` HTTP service on c"]
    );
}

/// External functions (spec chapter 12). A module that names a file is rewritten to a path next to
/// the source.
#[test]
fn external_functions() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Host {
  fn get() -> String
}

@external(js, \"./ffi.mjs\", \"get\")
@external(wasm, \"env\", \"get\")
fn host_get() -> String

handler JsHost: Host {
  fn get() { host_get() }
}

fn main() {
  with JsHost { dbg((use Host).get()) }
}
",
    )]);
    let out = dump(&m);
    let ext: Vec<&str> = out
        .split("\n\n")
        .filter(|f| f.contains("external fn host_get"))
        .collect();
    assert_eq!(ext.len(), 1, "{out}");
    // Test sources have no path, so the file specifier is left as is.
    assert_eq!(
        ext[0].trim(),
        "external fn host_get() -> String = js[\"./ffi.mjs\", \"get\"] wasm[\"env\", \"get\"]"
    );
}

/// An `@external` module naming a file is rewritten to the file next to the declaring source,
/// with `.` and `..` collapsed textually (spec 12.1).
#[test]
fn external_files_next_to_the_source() {
    let files = [(
        "main.emel",
        "\
@external(js, \"./ffi/../host/./a.mjs\", \"a\")
fn host_a() -> Int uses {}

@external(js, \"../../b.mjs\", \"b\")
fn host_b() -> Int uses {}

fn main() { host_a() + host_b() }
",
    )];
    let options = Options {
        entry_path: Some("src/main.emel"),
        ..Options::default()
    };
    let m = lower_with(&files, &options).unwrap().module;
    let out = dump(&m);
    let externals: Vec<&str> = out
        .split("\n\n")
        .filter(|f| f.starts_with("external fn"))
        .map(str::trim)
        .collect();
    assert_eq!(
        externals,
        [
            "external fn host_a() -> Int = js[file \"src/host/a.mjs\", \"a\"]",
            // Above the source's directory, `..` stays.
            "external fn host_b() -> Int = js[file \"../b.mjs\", \"b\"]",
        ]
    );
}

/// A test core module `HostTest` with an external function pointing to a bundled JS file.
const HOST_TEST: &str = "\
@external(js, \"./host/../host_test.mjs\", \"now\")
@external(wasm, \"env\", \"now\")
pub fn now() -> Int uses {}

@external(js, \"./missing.mjs\", \"gone\")
pub fn gone() -> Int uses {}
";

/// Bundled JS files. `host_test.mjs` imports `helper.mjs` relatively.
const HOST_TEST_JS: &[(&str, &str)] = &[
    (
        "core/host_test.mjs",
        "import { base } from \"./helper.mjs\";\nexport const now = () => base;\n",
    ),
    ("core/helper.mjs", "export const base = 42;\n"),
];

/// Lowers with `HostTest` added to the bundled core.
fn lower_with_host_test(main: &str) -> Result<Module, Vec<String>> {
    let mut core = emela_core::core_sources().to_vec();
    core.push(("HostTest", HOST_TEST));
    try_lower_with(&[("main.emel", main)], Roots::Exported, &core, HOST_TEST_JS)
}

/// A bundled core external function's file specifier points to the bundled file, by its path
/// relative to `lib/`. Once used, all bundled files go into `bundled_files` (for relative imports).
#[test]
fn core_external_points_to_bundled_file() {
    let m = lower_with_host_test("fn main() { dbg(HostTest.now()) }\n")
        .unwrap_or_else(|e| panic!("{e:?}"));
    let out = dump(&m);
    let ext: Vec<&str> = out
        .split("\n\n")
        .filter(|f| f.contains("external fn HostTest.now"))
        .collect();
    assert_eq!(ext.len(), 1, "{out}");
    assert_eq!(
        ext[0].trim(),
        "external fn HostTest.now() -> Int = js[bundled \"core/host_test.mjs\", \"now\"] wasm[\"env\", \"now\"]"
    );
    let files: Vec<(&str, &str)> = m
        .bundled_files
        .iter()
        .map(|f| (f.path.as_str(), f.contents.as_str()))
        .collect();
    assert_eq!(files, HOST_TEST_JS);
}

/// If no external function pointing to a bundled file is used, no bundled file is written.
#[test]
fn unused_bundled_files_are_not_output() {
    let m = lower_with_host_test("fn main() { dbg(1) }\n").unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(m.bundled_files, []);
}

/// A specifier not among the bundled files is an error in the bundled sources.
#[test]
fn missing_core_js_file_is_an_error() {
    let errors = lower_with_host_test("fn main() { dbg(HostTest.gone()) }\n").unwrap_err();
    assert_eq!(errors, ["HostTest: `gone` missing core file ./missing.mjs"]);
}

/// suspend (spec 8.1, 8.4, 12.2, 16.5). Suspending functions, lambdas and calls are marked. A
/// function with a polymorphic R (`List.map`) gets a suspending copy (`List.map$suspend…`) where a
/// suspending function is passed. Async left in main's R needs no evidence table (the runtime
/// provides it).
#[test]
fn suspend_marks_and_copies() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
  fn now() -> Int
}

@external(js, \"setTimeout\")
fn after(ms: Int, f: fn() -> ()) -> ()

@external(js, \"./ffi.mjs\", \"fetchText\")
suspend fn fetch_text(url: String) -> String

handler SystemClock: Clock {
  suspend fn sleep(ms, resume) { after(ms, || resume(ms)) }
  fn now() { 0 }
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(0) }
  fn now() { 0 }
}

handler FetchingClock: Clock {
  suspend fn sleep(ms, resume) { resume(String.length(fetch_text(\"x\"))) }
  fn now() { 0 }
}

fn naps(xs: List[Int]) -> List[Int] uses { Clock } {
  clock = use Clock
  List.map(xs, |ms| clock.sleep(ms) + clock.now())
}

fn main() {
  with SystemClock { dbg(naps([1, 2])) }
  with InstantClock { dbg((use Clock).sleep(1)) }
  with FetchingClock { dbg((use Clock).sleep(1)) }
  dbg(List.map([1], |x| x))
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            [
                "fn main(",
                "fn naps(",
                "fn SystemClock(",
                "fn InstantClock(",
                "fn FetchingClock(",
                "fn fetch_text(",
                "fn List.map",
                "effect ",
            ]
            .iter()
            .any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// An fn-operation implementation may suspend depending on the calls in its body (E0417, spec 8.1).
/// Even with a Clock that has a suspend fn in the body's R, it is not a generator if it only calls
/// the fn operation `now`, and the `now` call is not marked as suspending. If the body's R is
/// Immediate, calling sleep inside a `with` of PureClock, which resumes immediately, does not make
/// it a generator either (the call itself is marked as suspending).
#[test]
fn fn_op_calling_fn_ops_is_not_a_generator() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
  fn now() -> Int
}

@external(js, \"setTimeout\")
fn after(ms: Int, f: fn() -> ()) -> ()

handler SystemClock: Clock {
  suspend fn sleep(ms, resume) { after(ms, || resume(ms)) }
  fn now() { 42 }
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(0) }
  fn now() { (use Clock).now() + 1 }
}

handler CapturedClock: Clock {
  suspend fn sleep(ms, resume) { resume(0) }
  fn now() {
    clock = use Clock
    clock.now()
  }
}

handler PureClock: Clock {
  suspend fn sleep(ms, resume) { resume(0) }
  fn now() { 0 }
}

handler LocalSleep: Clock {
  suspend fn sleep(ms, resume) { resume(0) }
  fn now() {
    with PureClock { (use Clock).sleep(1) }
  }
}

fn main() {
  with SystemClock {
    with InstantClock { dbg((use Clock).now()) }
    with CapturedClock { dbg((use Clock).now()) }
    with LocalSleep { dbg((use Clock).now()) }
  }
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            ["fn InstantClock(", "fn CapturedClock(", "fn LocalSleep("]
                .iter()
                .any(|k| f.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// Suspending copies are made per set type parameter instantiated with a suspending set (spec
/// 9.5): for one that appears only as a type argument (`Task[R]`), through a recursive call, and
/// through a call to a function of the same recursive group.
#[test]
fn suspending_copies_follow_set_parameters() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Wait {
  suspend fn wait() -> Int
}

handler Waits: Wait {
  suspend fn wait(resume) { resume(1) }
}

type Task[R](run: fn() -> Int uses R)

fn wrap[R](t: Task[R]) -> Task[R] { Task(run: || (t.run)()) }

fn again[R](n: Int, f: fn() -> Int uses R) -> Int uses R {
  if n == 0 { f() } else { again(n - 1, f) + 1 }
}

fn ping[R](n: Int, f: fn() -> Int uses R) -> Int uses R {
  if n == 0 { f() } else { pong(n - 1, f) + 1 }
}

fn pong[R](n: Int, f: fn() -> Int uses R) -> Int uses R { ping(n, f) + 1 }

fn main() -> Int {
  with Waits {
    t = wrap(Task(run: || (use Wait).wait()))
    (t.run)() + again(2, || (use Wait).wait()) + ping(2, || (use Wait).wait()) + again(1, || 1)
  }
}
",
    )]);
    let out = dump(&m);
    let names: Vec<&str> = out
        .split("\n\n")
        .filter_map(|part| part.lines().next())
        .filter(|l| {
            ["wrap", "again", "ping", "pong"]
                .iter()
                .any(|k| l.contains(k))
        })
        .collect();
    insta::assert_snapshot!(names.join("\n"), @r"
    fn wrap$suspend1[A](t%22: Task[{A}]) -> Task[{A}] {
    suspend fn again$suspend1[A, B](n%19: Int, f%20: fn() -> Int fails suspends ev (E {B}, R {A}), ev%21: evidence) -> Int fails {
    suspend fn ping$suspend1[A, B](n%13: Int, f%14: fn() -> Int fails suspends ev (E {B}, R {A}), ev%15: evidence) -> Int fails {
    fn again[A, B](n%10: Int, f%11: fn() -> Int fails ev (E {B}, R {A}), ev%12: evidence) -> Int fails {
    suspend fn pong$suspend1[A, B](n%16: Int, f%17: fn() -> Int fails suspends ev (E {B}, R {A}), ev%18: evidence) -> Int fails {
    ");
    // The lambda calls `t.run`, which may suspend in this copy.
    insta::assert_snapshot!(fn_part(&m, "fn wrap$"), @r"
    fn wrap$suspend1[A](t%22: Task[{A}]) -> Task[{A}] {
      Task.Task[{A}](run: suspend fn(ev%23: evidence) -> Int {
        let f%24: fn() -> Int suspends ev (E {}, R {A}) = t%22.run#0
        (f%24)(ev%23)!
      })
    }
    ");
}

/// Functions used as values (spec 6.4): a fn with a body or `@external` is referenced; an
/// intrinsic, a
/// constructor with fields, and a generic fn whose R holds an effect with type arguments are
/// wrapped in a lambda. `self` of a stateless handler is `()`.
#[test]
fn function_values() {
    let m = lower_files(&[(
        "main.emel",
        "\
import Std.State.{State}

effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) {
    me = self
    ()
  }
}

type Point(x: Int)

fn twice(n: Int) -> Int { n * 2 }

@external(js, \"host\")
fn host(n: Int) -> Int uses {}

fn bump[S](f: fn(S) -> S) -> () uses { State[S] } {
  s = use State[S]
  s.set(f(s.get()))
}

fn main() {
  a = twice
  b = String.length
  c = Point
  d = bump
  e = host
  with Quiet { (use Log).info(\"x\") }
}
",
    )]);
    insta::assert_snapshot!(fn_part(&m, "fn main("));
    insta::assert_snapshot!(fn_part(&m, "fn Quiet("));
}

/// A callee that is an expression is evaluated once, before the arguments (spec 14.1): a function
/// value made by a call and a capability value chosen by `if` are bound first; `(use Log)` only looks up the
/// table, so it is not.
#[test]
fn callees_are_evaluated_first() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

fn pick(n: Int) -> fn(Int) -> Int { |x| x + n }

fn main() {
  a = pick(1)(2)
  with Quiet {
    (if a > 0 { use Log } else { use Log }).info(\"a\")
    (use Log).info(\"b\")
  }
}
",
    )]);
    insta::assert_snapshot!(fn_part(&m, "fn main("));
}

/// A fn whose R holds an effect with type arguments is wrapped when used as a value only if those
/// arguments contain its type parameters, however deep (`State[List[S]]`); a generic const in a
/// pattern takes its type arguments from the matched type.
#[test]
fn generic_tags_and_const_patterns() {
    let m = lower_files(&[(
        "main.emel",
        "\
import Std.State.{State}

const NOTHING = None

fn count() -> Int uses { State[Int] } { (use State[Int]).get() }

fn push[S](x: S) -> () uses { State[List[S]] } {
  s = use State[List[S]]
  s.set([x, ..s.get()])
}

fn main() -> Int {
  a = count
  b = push
  Some(\"x\") match {
    NOTHING -> 0
    _ -> 1
  }
}
",
    )]);
    insta::assert_snapshot!(fn_part(&m, "fn main("));
}

/// The E and R of an instantiation become set summaries in the type arguments (spec 16.5): an
/// effect needs the evidence table, Async suspends without it, and an error fails. Inside a group
/// of inferred functions, a call (to another or to itself) passes on the caller's own set type
/// parameters, and a local holding a parameter's function value keeps them in its type.
#[test]
fn set_summaries_of_inferred_groups() {
    let m = lower_files(&[(
        "main.emel",
        "\
import Std.Fiber.{Fiber}

error Oops

effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

fn ping(n: Int, f: fn() -> ()) {
  if n == 0 { f() } else { pong(n - 1, f) }
}

fn pong(n: Int, f: fn() -> ()) {
  g = f
  ping(n, g)
}

fn boom() -> () fails Oops { fail Oops }

fn again(n: Int, f: fn() -> ()) {
  if n == 0 { f() } else {
    again(n - 1, f)
    f()
  }
}

fn main() -> Int fails Oops uses { Async } {
  with Quiet {
    ping(2, || (use Log).info(\"x\"))
    ping(1, || fail Oops)
    ping(1, || Fiber.scope(|| ()))
    again(1, || (use Log).info(\"y\"))
    ping(1, boom)
    0
  }
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|p| {
            ["fn main(", "fn ping", "fn pong", "fn again"]
                .iter()
                .any(|k| p.contains(k))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

/// `fail` lowers `Show.show` only for an error with an impl of Show, which shows it if it reaches
/// the entry (spec 14.3), and for errors not known here (a type parameter).
#[test]
fn fail_lowers_show_only_for_shown_errors() {
    let shows = |src: &str| {
        let m = lower_files(&[("main.emel", src)]);
        m.traits.show.is_some()
    };
    assert!(!shows(
        "error Bare\n\nfn main() -> Int fails Bare { fail Bare }\n"
    ));
    assert!(shows(
        "error Shown derive Show\n\nfn main() -> Int fails Shown { fail Shown }\n"
    ));
    assert!(shows(
        "error Named\n\nimpl Named: Show {\n  fn show(self) { \"n\" }\n}\n\n\
         fn main() -> Int fails Named { fail Named }\n"
    ));
}

/// A const generalized over the R of the function it holds (spec 5.6): inside its body, the set
/// variable is the const's set type parameter.
#[test]
fn consts_generalized_over_sets() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

const APPLY = (|callback| callback(), ())

fn main() -> Int {
  (apply, _) = APPLY
  with Quiet {
    apply(|| 1) + apply(|| {
      (use Log).info(\"x\")
      2
    })
  }
}
",
    )]);
    insta::assert_snapshot!(fn_part(&m, "fn APPLY"));
}

/// Calls of other callees: a const holding a function is called as a value; a Bool comparison
/// uses Bool's `==`; a generic const in a list pattern takes the element type; `State.run` finds
/// State in its body's R also when Async is there too.
#[test]
fn const_callees_bool_comparisons_and_state_run() {
    let m = lower_files(&[(
        "main.emel",
        "\
import Std.Fiber.{Fiber}
import Std.State.{State}

const DOUBLE: fn(Int) -> Int = |x| x * 2

const EMPTY = []

fn main() -> Int uses { Async } {
  a = DOUBLE(2)
  b = (a == 4) == True
  c = [1] match {
    EMPTY -> 0
    _ -> 1
  }
  (result, _) = State.run(0, || {
    Fiber.scope(|| ())
    (use State[Int]).get()
  })
  result
}
",
    )]);
    insta::assert_snapshot!(fn_part(&m, "fn main("));
}

/// A trait function called with a suspending set gets a suspending copy, and so does the impl
/// function it dispatches to, whose set type parameter comes after the impl's own (`A` of
/// `Box[A]`) instead of `Self` (spec 9.5, 10.3).
#[test]
fn suspending_copies_of_impl_functions() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Wait {
  suspend fn wait() -> Int
}

handler Waits: Wait {
  suspend fn wait(resume) { resume(1) }
}

trait Runner {
  fn run[R](self, f: fn() -> Int uses R) -> Int uses R
}

type Box[A](value: A)

impl Box[A]: Runner {
  fn run(self, f) { f() }
}

fn main() -> Int {
  with Waits { Runner.run(Box(value: 1), || (use Wait).wait()) }
}
",
    )]);
    let out = dump(&m);
    let heads: Vec<&str> = out
        .split("\n\n")
        .filter_map(|p| p.lines().next())
        .filter(|l| l.contains("Runner.run"))
        .collect();
    insta::assert_snapshot!(heads.join("\n"), @r"
    suspend fn Runner.run$suspend2[A, B, C](self%4: A, f%5: fn() -> Int fails suspends ev (E {C}, R {B}), ev%6: evidence) -> Int fails {
    suspend fn Box: Runner.run$suspend2[A, B, C](self%7: Box[A], f%8: fn() -> Int fails suspends ev (E {C}, R {B}), ev%9: evidence) -> Int fails {
    ");
}

/// A label is provided by the provider of its base (spec 8.3): `by Log` in the same `with` takes
/// the handler initialized earlier there, a `default` label falls to it too, and `by Log` in an
/// inner `with` takes the one the outer table holds.
#[test]
fn labels_take_the_provider_of_their_base() {
    let m = lower_files(&[(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

effect Ping {
  fn ping() -> ()
}

effect Audit of Log

effect Debug of Log default

handler Quiet: Log {
  fn info(msg) { () }
}

handler Pong: Ping {
  fn ping() { () }
}

fn main() {
  with Pong, Quiet, Audit by Log {
    (use Audit).info(\"a\")
    (use Debug).info(\"b\")
    with Audit by Log { (use Audit).info(\"c\") }
  }
}
",
    )]);
    insta::assert_snapshot!(fn_part(&m, "fn main("));
}

/// A trait function with a default implementation, called for a type whose impl does not define
/// it, uses the default (spec 10.3); a build for a target does not report a missing impl.
#[test]
fn default_implementations_are_not_missing_impls() {
    let files = [(
        "main.emel",
        "\
trait Size {
  fn size(self) -> Int

  fn double(self) -> Int { Size.size(self) * 2 }
}

type Point(x: Int)

impl Point: Size {
  fn size(self) { self.x }
}

fn main() -> Int { Size.double(Point(x: 2)) }
",
    )];
    let options = Options {
        target: Some("js"),
        ..Options::default()
    };
    let m = lower_with(&files, &options).unwrap().module;
    assert_eq!(
        fn_names(&m),
        ["main", "Size.double[Point]", "Point: Size.size"]
    );
}

/// A method call is lowered as the call of the function it finds (`[type.rules]`): `x.f(a)` as
/// `M.f(x, a)` and `x.g()` of a trait as `Trait.g(x)`, with the receiver evaluated first.
#[test]
fn method_calls_lower_as_calls() {
    let module = |body: &str| {
        let main = format!(
            "type User(name: String)\n  derive Show\n\nfn rename(u: User, name: String) -> User {{ User(name:, ..u) }}\n\nfn main() {{\n  u = User(name: \"a\")\n  {body}\n}}\n"
        );
        let m = lower_files(&[("main.emel", &main)]);
        emela_core::dump(&m)
    };
    assert_eq!(
        module("u.rename(\"b\").show()"),
        module("Show.show(rename(u, \"b\"))")
    );
    assert_eq!(
        module("[u.show()].map(|s| s.append(\"!\")).join(\",\")"),
        module("List.join(List.map([Show.show(u)], |s| String.append(s, \"!\")), \",\")")
    );
}
