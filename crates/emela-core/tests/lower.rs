//! Tests for lowering (`emela_core::lower`).

mod support;

use emela_core::{EnumDef, Expr, Module, Roots, Type, TypeId, VariantFields, dump};
use support::{lower_files, try_lower, try_lower_with};

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
  add_k = fn(x) { x + k }
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
    dbg(apply(fn() { clock.now() + (use Clock).now() }))
    dbg(List.map([1], fn(x) { x + (use Clock).now() }))
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
import Std.State.{State}\n\nfn modify[S](f: fn(S) -> S fails {} use {}) -> S use State[S] {
  (use State[S]).update(f)
  (use State[S]).get()
}

fn main() {
  dbg(State.run(\"\", fn() {
    State.run(1, fn() {
      (use State[String]).set(\"x\")
      modify(fn(n) { n + 1 })
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
import Std.Fiber.{Fiber, Scope, Exit}
import Std.Deferred.{Deferred}

effect Log { fn info(msg: String) -> () }

fn main() -> Int use { Log, Async } {
  Fiber.scope(fn() {
    s = use Scope
    a = s.fork(fn() { (use Log).info(\"a\") })
    b = Fiber.fork(fn() { 2 })
    d = s.deferred()
    Deferred.complete(d, 1)
    Fiber.join(a)
    e = Fiber.attempt(fn() { (use Log).info(\"e\") })
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

/// A test core module `HostTest` with an external function pointing to a bundled JS file.
const HOST_TEST: &str = "\
@external(js, \"./host/../host_test.mjs\", \"now\")
@external(wasm, \"env\", \"now\")
pub fn now() -> Int use {}

@external(js, \"./missing.mjs\", \"gone\")
pub fn gone() -> Int use {}
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
  suspend fn sleep(ms, resume) { after(ms, fn() { resume(ms) }) }
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

fn naps(xs: List[Int]) -> List[Int] use Clock {
  clock = use Clock
  List.map(xs, fn(ms) { clock.sleep(ms) + clock.now() })
}

fn main() {
  with SystemClock { dbg(naps([1, 2])) }
  with InstantClock { dbg((use Clock).sleep(1)) }
  with FetchingClock { dbg((use Clock).sleep(1)) }
  dbg(List.map([1], fn(x) { x }))
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
  suspend fn sleep(ms, resume) { after(ms, fn() { resume(ms) }) }
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
        module("[u.show()].map(fn(s) { s.append(\"!\") }).join(\",\")"),
        module("List.join(List.map([Show.show(u)], fn(s) { String.append(s, \"!\") }), \",\")")
    );
}
