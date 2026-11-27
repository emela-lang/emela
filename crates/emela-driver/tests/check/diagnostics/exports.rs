//! Diagnostics of entries and exports: the names the launcher reserves, export types, targets,
//! HTTP services, `main` and the operations checked for the target.

use emela_driver::{Lower, Target, Typecheck, render};

use super::check_files;

/// An external function without a JS `@external`, called from main through a handler.
/// `wasm_unused` is never used.
const WASM_ONLY: &str = r#"effect Clock {
  fn now() -> Int
}

@external(wasm, "env", "now")
fn wasm_now() -> Int

@external(wasm, "env", "unused")
fn wasm_unused() -> Int

handler WasmClock: Clock {
  fn now() { wasm_now() }
}

handler UnusedClock: Clock {
  fn now() { wasm_unused() }
}

fn main() {
  with WasmClock { dbg((use Clock).now()) }
  ()
}
"#;

#[test]
fn reachable_external_without_target() {
    // Lowering for build, run and test requires JS annotations on external functions reachable
    // from main (spec 12.1).
    let files = [("home/main.emel", WASM_ONLY)];
    let analysis = check_files(&files, "home/main.emel", &mut Lower::default());
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
    // check and the LSP (up to type checking) choose no target, so they do not check.
    let analysis = check_files(&files, "home/main.emel", &mut Typecheck::default());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

/// `pub` fns of the entry with trait-bounded type parameters. Only `@export` fns are exported
/// (`[target.js.module]`), so they are not roots and build on every target.
const CONSTRAINED_PUB_FNS: &str = r#"type User(name: String)

impl User: Show {
  fn show(self) { "User(#{self.name})" }
}

pub fn label[A: Show](x: A) -> String { Show.show(x) }

pub fn pair[A, B: Show + Eq](a: A, b: B) -> B { b }

fn main() {
  dbg(label(User(name: "ann")))
  ()
}
"#;

#[test]
fn pub_fns_are_not_exported() {
    let files = [("home/main.emel", CONSTRAINED_PUB_FNS)];
    for frontend in [
        Lower::default(),
        Lower::default().with_target(Target::Native),
    ] {
        let mut frontend = frontend;
        let analysis = check_files(&files, "home/main.emel", &mut frontend);
        assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    }
}

fn export_diagnostics(files: &[(&str, &str)]) -> String {
    let analysis = check_files(files, "home/main.emel", &mut Typecheck::default());
    render(&analysis.diagnostics, &analysis.sources, false)
}

/// A JS host export may not take a name that `main.mjs` already exports (`[target.js.module]`):
/// `main` when the entry has it, and `$start` when the launcher or a host export may suspend.
/// They are duplicates of those exports (E5103).
#[test]
fn reserved_js_host_names_are_diagnosed() {
    insta::assert_snapshot!(export_diagnostics(&[(
        "home/main.emel",
        r#"@export(js, "main")
fn a() -> Int { 1 }

@export(js, "$start")
fn b() -> Int { 1 }

fn main() uses { Async } { () }
"#,
    )]));
}

/// Without `main` and without a suspending export, the launcher's names are free.
#[test]
fn launcher_names_are_free_without_main_or_suspension() {
    assert_eq!(
        export_diagnostics(&[(
            "home/main.emel",
            r#"@export(js, "main")
fn a() -> Int { 1 }

@export(js, "$start")
fn b() -> Int { 1 }
"#,
        )]),
        ""
    );
}

/// A build does not emit test functions, so a suspending test does not make the launcher export
/// `$start`.
#[test]
fn a_suspending_test_does_not_reserve_the_launcher_names() {
    assert_eq!(
        export_diagnostics(&[(
            "home/main.emel",
            r#"@export(js, "main")
fn a() -> Int { 1 }

@export(js, "$start")
fn b() -> Int { 1 }

@test
fn t() uses { Async } { () }
"#,
        )]),
        ""
    );
}

/// `$start` is emitted when a `js` host export may suspend; a suspending export of another target
/// does not make the launcher export it.
#[test]
fn only_suspending_js_exports_reserve_start() {
    assert_eq!(
        export_diagnostics(&[(
            "home/main.emel",
            r#"@export(wasm, "env", "late")
@export(c, "late")
fn late() uses { Async } { () }

@export(js, "$start")
fn start() -> Int { 1 }
"#,
        )]),
        ""
    );
}

/// A `js` host export that may suspend makes the launcher export `$start`, so that name is
/// reserved even without a suspending `main`.
#[test]
fn a_suspending_js_export_reserves_start() {
    let output = export_diagnostics(&[(
        "home/main.emel",
        r#"@export(js, "slow")
fn slow() uses { Async } { () }

@export(js, "$start")
fn start() -> Int { 1 }
"#,
    )]);
    assert_eq!(output.matches("error[E5103]").count(), 1, "{output}");
    insta::assert_snapshot!(output);
}

/// The three-argument form is a property of the default export object, not a named export, so
/// `$start` is free there.
#[test]
fn a_default_export_property_may_be_named_start() {
    assert_eq!(
        export_diagnostics(&[(
            "home/main.emel",
            r#"@export(js, "slow")
fn slow() uses { Async } { () }

@export(js, "default", "$start")
fn value() -> Int { 1 }
"#,
        )]),
        ""
    );
}

/// A `@test fn main` is not the entry's main, so `main` is not a launcher export of the build.
#[test]
fn a_test_named_main_does_not_reserve_the_name() {
    assert_eq!(
        export_diagnostics(&[(
            "home/main.emel",
            r#"@export(js, "main")
fn a() -> Int { 1 }

@test
fn main() {}
"#,
        )]),
        ""
    );
}

/// A reserved name and a duplicate are reported once each: the first export collides with the
/// launcher's `main`, and the second is a duplicate of the first (E5103).
#[test]
fn a_reserved_name_and_a_duplicate_are_two_diagnostics() {
    let output = export_diagnostics(&[(
        "home/main.emel",
        r#"@export(js, "main")
fn a() -> Int { 1 }

@export(js, "main")
fn b() -> Int { 1 }

fn main() {}
"#,
    )]);
    assert_eq!(output.matches("error[E5103]").count(), 2, "{output}");
    insta::assert_snapshot!(output);
}

/// An export made by `@entry` is reported at the `@entry`, not at the adapter's `@export`.
#[test]
fn a_reserved_name_from_entry_is_reported_once_at_the_attribute() {
    insta::assert_snapshot!(export_diagnostics(&[(
        "home/main.emel",
        r#"@export(js, "main")
fn serve(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }

@entry(serve)
fn app(n: Int) -> Int { n }

fn main() {}
"#,
    )]));
}

/// `[ext.export]`: the parameters and the return value are boundary types (E5101), E is empty and
/// R is empty except for Async (E4301).
#[test]
fn export_types_and_effects() {
    insta::assert_snapshot!(export_diagnostics(&[(
        "home/main.emel",
        r#"@external
type Conn

effect Log {
  fn info(msg: String) -> ()
}

error Oops

@export(js, "ok")
pub fn ok(a: Int, b: Int64, c: Float, d: Bool, e: String, f: (), g: Conn, h: fn(Int) -> String fails {} uses {}) -> () {}

@export(js, "callback")
pub fn callback(h: fn(Int) -> String) -> String { h(1) }

@export(js, "list")
pub fn list(xs: List[Int]) -> Int { 0 }

@export(js, "option")
pub fn option() -> Option[Int] { None }

@export(js, "generic")
pub fn generic[A](x: A) -> Int { 0 }

@export(js, "logs")
pub fn logs() -> () { (use Log).info("x") }

@export(js, "fails")
pub fn fails() -> Int fails Oops { fail Oops }

fn main() {}
"#,
    )]));
}

/// What each target cannot expose: an export that may suspend on `c` (E5202,
/// `[target.native.export]`), a callback on `c` (not supported yet), and on `wasm` any `@export(wasm, ...)`
/// a user writes (`[target.wasm.external]`), also through `@entry`. Each target reports only its
/// own exports, and check (no target) reports none.
#[test]
fn export_target_errors() {
    let files = [(
        "home/main.emel",
        r#"import Std.Clock.{Clock, SystemClock}

@export(c, "wait")
@export(js, "wait")
fn wait(ms: Int) -> () {
  with SystemClock { (use Clock).sleep(ms) }
}

@export(c, "apply")
fn apply(x: Int, f: fn(Int) -> Int fails {} uses {}) -> Int { f(x) }

@export(wasm, "env", "add")
@export(js, "add")
fn add(a: Int, b: Int) -> Int { a + b }

@export(wasm, "wasi:http/handler@0.3.0", "handle")
fn serve(app: fn(Int) -> Int fails {} uses {}, x: Int) -> Int { app(x) }

@entry(serve)
fn app(x: Int) -> Int { x }

fn main() {}
"#,
    )];
    let mut out = String::new();
    for target in [Target::Js, Target::Native, Target::Wasm] {
        let analysis = check_files(
            &files,
            "home/main.emel",
            &mut Lower::default().with_target(target),
        );
        out.push_str(&format!("== {}\n", target.name()));
        out.push_str(&render(&analysis.diagnostics, &analysis.sources, false));
    }
    let analysis = check_files(&files, "home/main.emel", &mut Typecheck::default());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    insta::assert_snapshot!(out);
}

/// `[lib.http-service]`: `@entry(Http.serve)` takes an fn from `Request` to `Response` whose E is
/// anything and whose R has only Async and Spawn, whose tasks may fail with anything and use only
/// Async (E5105 otherwise). The native target does not support
/// HTTP services (E5203); an entry adapter of the program's own named `serve` in a module `Http`
/// is not one. check (no target) reports nothing.
#[test]
fn http_service_targets() {
    let files = [
        (
            "home/main.emel",
            r#"import Std.Http
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}

error Teapot

@entry(Http.serve)
pub fn app(request: Request) -> Response fails Teapot {
  if request.body == "tea" { fail Teapot } else { Response.new(200, request.url) }
}
"#,
        ),
        (
            "other/main.emel",
            r#"import Std.Http
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}
import Std.Console.{Console}

@entry(Http.serve)
pub fn app(request: Request) -> Response {
  (use Console).print_line(request.url)
  Response.new(200, "")
}
"#,
        ),
        (
            "spawns/main.emel",
            r#"import Std.Http
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}
import Std.Task.{Spawn}

error Teapot

fn brew() -> () fails Teapot uses { Async } { fail Teapot }

@entry(Http.serve)
pub fn app(request: Request) -> Response uses { Spawn[Teapot, { Async }] } {
  spawn <- Spawn
  spawn.start(|| brew())
  Response.new(200, request.url)
}
"#,
        ),
        (
            "wide/main.emel",
            r#"import Std.Http
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}
import Std.Task.{Spawn}
import Std.Console.{Console}

@entry(Http.serve)
pub fn app(request: Request) -> Response uses { Spawn[{}, { Async, Console }] } {
  spawn <- Spawn
  spawn.start(|| {
    console <- Console
    console.print_line(request.url)
  })
  Response.new(200, "")
}
"#,
        ),
        (
            "own/main.emel",
            r#"import Http

@entry(Http.serve)
fn app(x: Int) -> Int { x }

fn main() {}
"#,
        ),
        (
            "own/http.emel",
            r#"@export(c, "serve")
pub fn serve(app: fn(Int) -> Int fails {} uses {}, x: Int) -> Int { app(x) }
"#,
        ),
    ];
    let mut out = String::new();
    for entry in [
        "home/main.emel",
        "other/main.emel",
        "spawns/main.emel",
        "wide/main.emel",
        "own/main.emel",
    ] {
        for target in [Target::Js, Target::Native] {
            let analysis = check_files(&files, entry, &mut Lower::default().with_target(target));
            out.push_str(&format!("== {entry} {}\n", target.name()));
            out.push_str(&render(&analysis.diagnostics, &analysis.sources, false));
        }
    }
    let analysis = check_files(&files, "home/main.emel", &mut Typecheck::default());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    insta::assert_snapshot!(out);
}

/// `[ext.export]` and `[ext.entry]`: an `@export` fn that is not an entry adapter only in the
/// entry module (E5102), `@entry` only in the entry module (E5102) and naming an entry adapter
/// (E5104), and the same name exported twice for the same target (E5103).
#[test]
fn export_placement() {
    insta::assert_snapshot!(export_diagnostics(&[
        (
            "home/main.emel",
            r#"import Util

@export(js, "add")
@export(js, "default", "add")
pub fn add(a: Int, b: Int) -> Int { a + b }

@export(js, "add")
@export(c, "add")
pub fn add_again(a: Int, b: Int) -> Int { a + b }

@entry(Util.serve)
fn app(x: Int) -> Int { x }

@entry(Util.serve)
fn app_again(x: Int) -> Int { x }

@entry(Util.helper)
fn not_adapter(x: Int) -> Int { x }

@entry
fn no_name(x: Int) -> Int { x }

fn main() {}
"#,
        ),
        (
            "home/util.emel",
            r#"@export(js, "default", "fetch")
pub fn serve(app: fn(Int) -> Int fails {} uses {}, x: Int) -> Int { app(x) }

@export(js, "helper")
pub fn helper(x: Int) -> Int { x }

@entry(serve)
pub fn misplaced(x: Int) -> Int { x }
"#,
        ),
    ]));
}

/// `[ext.entry]`: the fn with `@entry` MUST be accepted as the first argument of the adapter
/// (E5105). A generic fn is instantiated. An adapter that lets the E or R of the fn through has
/// them in its own E and R (E4301), and so does the fn made by `@entry` (E5105).
#[test]
fn entry_fits_the_adapter() {
    insta::assert_snapshot!(export_diagnostics(&[(
        "home/main.emel",
        r#"effect Log {
  fn info(msg: String) -> ()
}

error Oops

@export(js, "default", "one")
fn one(app: fn(String) -> String fails {} uses {}, s: String) -> String { app(s) }

@export(js, "default", "two")
fn two(app: fn(String) -> String fails {} uses {}, s: String) -> String { app(s) }

@export(js, "default", "three")
fn three(app: fn(String) -> String fails {} uses {}, s: String) -> String { app(s) }

@export(js, "default", "open")
fn open(app: fn(String) -> String, s: String) -> String { app(s) }

@export(js, "default", "four")
fn four(app: fn(String) -> String fails {} uses {}, s: String) -> String { app(s) }

@entry(one)
fn fits(s: String) -> String { s }

@entry(two)
fn generic[A](x: A) -> A { x }

@entry(three)
fn wrong_type(x: Int) -> Int { x }

@entry(four)
fn wrong_effects(s: String) -> String {
  (use Log).info(s)
  s
}

@entry(open)
fn through(s: String) -> String fails Oops { fail Oops }

fn main() {}
"#,
    )]));
}

/// A handler whose `sleep` has no JS `@external`. Only operations called somewhere reachable are
/// checked (spec 12.1), so using only `now` builds, and calling `sleep` reports `wasm_sleep`.
const PARTIAL_HANDLER: &str = r#"effect Clock {
  fn now() -> Int
  fn sleep(ms: Int) -> ()
}

@external(js, "Date.now")
fn js_now() -> Int

@external(wasm, "env", "sleep")
fn wasm_sleep(ms: Int) -> ()

handler HostClock: Clock {
  fn now() { js_now() }
  fn sleep(ms) { wasm_sleep(ms) }
}

fn main() {
  with HostClock { dbg(CALL) }
  ()
}
"#;

#[test]
fn only_called_operations_are_checked_for_the_target() {
    let render_main = |call: &str| {
        let main = PARTIAL_HANDLER.replace("CALL", call);
        let files = [("home/main.emel", main.as_str())];
        let analysis = check_files(&files, "home/main.emel", &mut Lower::default());
        render(&analysis.diagnostics, &analysis.sources, false)
    };
    assert_eq!(render_main("(use Clock).now()"), "");
    let called = render_main("(use Clock).sleep(1)");
    assert!(
        called
            .contains("external function `wasm_sleep` has no `@external(js, ...)` for target `js`"),
        "{called}"
    );
    // An operation used as a value counts as used.
    let as_value = render_main("List.map([1], (use Clock).sleep)");
    assert!(as_value.contains("`wasm_sleep`"), "{as_value}");
}

/// Handlers of one effect with different type arguments (spec 8.1). The check runs on the
/// monomorphized IR, where `Store[Int]` and `Store[String]` are different effects, so calling
/// `put` of `Store[String]` follows only `StringStore`'s implementation.
const STORES: &str = r#"effect Store[T] {
  fn put(x: T) -> ()
}

@external(js, "console.log")
fn js_log(s: String) -> ()

@external(wasm, "env", "log_int")
fn wasm_log_int(n: Int) -> ()

handler IntStore: Store[Int] {
  fn put(x) { wasm_log_int(x) }
}

handler StringStore: Store[String] {
  fn put(x) { js_log(x) }
}

fn main() {
  with IntStore, StringStore { CALL }
}
"#;

#[test]
fn operations_are_checked_per_effect_instance() {
    let render_main = |call: &str| {
        let main = STORES.replace("CALL", call);
        let files = [("home/main.emel", main.as_str())];
        let analysis = check_files(&files, "home/main.emel", &mut Lower::default());
        render(&analysis.diagnostics, &analysis.sources, false)
    };
    assert_eq!(render_main("(use Store[String]).put(\"a\")"), "");
    let int = render_main("(use Store[Int]).put(1)");
    assert!(
        int.contains(
            "external function `wasm_log_int` has no `@external(js, ...)` for target `js`"
        ),
        "{int}"
    );
}

/// `[ext.boundary]`: `Array[A]` is a boundary type when `A` is a boundary type other than a
/// function type, nested arrays and Bytes included. An array of functions, or of a type that is not
/// a boundary type, is not one (E5101 for an exported fn).
#[test]
fn array_boundary_types() {
    insta::assert_snapshot!(export_diagnostics(&[(
        "home/main.emel",
        r#"import Std.Array.{Array}
import Std.Bytes.{Bytes}

@external
type Conn

@export(js, "ok")
pub fn ok(a: Array[Int], b: Array[Array[String]], c: Array[Bytes], d: Array[Conn], e: Array[()]) -> Array[Float] { Array.empty() }

@export(js, "callbacks")
pub fn callbacks(fs: Array[fn(Int) -> Int fails {} uses {}]) -> Int { 0 }

@export(js, "lists")
pub fn lists() -> Array[List[Int]] { Array.empty() }

@export(js, "deep")
pub fn deep(a: Array[Array[Option[Int]]]) -> Int { 0 }

fn main() {}
"#,
    )]));
}

/// The launcher exports the entry's `main` even when that fn has a `js` `@export` of its own
/// (`[target.js.module]`), so `@export(js, "main")` on `main` duplicates it (E5103).
#[test]
fn main_exported_as_main_duplicates_the_launcher_export() {
    let output = export_diagnostics(&[(
        "home/main.emel",
        r#"@export(js, "main")
fn main() -> Int { 1 }
"#,
    )]);
    assert_eq!(output.matches("error[E5103]").count(), 1, "{output}");
}

/// A bare entry adapter is not exposed by itself (`[ext.entry]`), so its `@export(js, "main")`
/// does not take the launcher's `main` until an `@entry` uses it.
#[test]
fn an_unused_entry_adapter_does_not_take_main() {
    assert_eq!(
        export_diagnostics(&[(
            "home/main.emel",
            r#"@export(js, "main")
fn serve(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }

fn main() {}
"#,
        )]),
        ""
    );
}

/// W3001: a `main` whose return type is none of `()`, `ExitCode` and Never (`[dyn.error]`,
/// `[effect.with]`). Points at the annotation of the return type, or at the name `main` when the
/// type is inferred.
#[test]
fn main_return_value_discarded() {
    let mut out = String::new();
    for src in [
        "fn main() -> Int {\n  2\n}\n",
        "fn main() {\n  \"done\"\n}\n",
        "fn main() -> List[Int] {\n  [1]\n}\n",
    ] {
        let mut frontend = Typecheck::default();
        let analysis = check_files(&[("home/main.emel", src)], "home/main.emel", &mut frontend);
        out.push_str(&render(&analysis.diagnostics, &analysis.sources, false));
    }
    insta::assert_snapshot!(out);
}

/// A `main` that returns `()`, an `ExitCode` or Never gets no W3001 (`[dyn.error]`), and neither
/// does a fn named `main` that is not the entry's.
#[test]
fn main_return_types_without_warning() {
    for (path, files) in [
        (
            "home/main.emel",
            vec![("home/main.emel", "fn main() {\n  ()\n}\n")],
        ),
        (
            "home/main.emel",
            vec![(
                "home/main.emel",
                "import Std.Process.{ExitCode}\n\nfn main() -> ExitCode {\n  Process.exit_code(2)\n}\n",
            )],
        ),
        (
            "home/main.emel",
            vec![(
                "home/main.emel",
                "import Std.Process.{EXIT_FAILURE}\n\nfn main() {\n  EXIT_FAILURE\n}\n",
            )],
        ),
        (
            "home/main.emel",
            vec![(
                "home/main.emel",
                "fn main() -> Never {\n  panic(\"stop\")\n}\n",
            )],
        ),
        (
            "home/main.emel",
            vec![(
                "home/main.emel",
                "error Stop\n\nfn main() {\n  fail Stop\n}\n",
            )],
        ),
        (
            "app",
            vec![
                ("app/Pome.toml", ""),
                (
                    "app/src/main.emel",
                    "import Tool\n\nfn main() {\n  n = Tool.main()\n  dbg(n)\n  ()\n}\n",
                ),
                ("app/src/tool.emel", "pub fn main() -> Int {\n  2\n}\n"),
            ],
        ),
    ] {
        let mut frontend = Typecheck::default();
        let analysis = check_files(&files, path, &mut frontend);
        let rendered = render(&analysis.diagnostics, &analysis.sources, false);
        assert!(analysis.diagnostics.is_empty(), "{files:?}\n{rendered}");
    }
}

/// `Http.serve` provides ProcessHost (`[lib.http-service]`), so its fn can write
/// `with Process { ... }`, while an `@export` fn is given only Async (`[ext.export]`) and a
/// `with Process` in it is E4301 for ProcessHost.
#[test]
fn process_in_http_service_and_export() {
    let files = [
        (
            "service/main.emel",
            r#"import Std.Env.{Env}
import Std.Http
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}
import Std.Process.{Process}

@entry(Http.serve)
pub fn app(request: Request) -> Response {
  with Process {
    name = (use Env).get("NAME") match {
      Some(n) -> n
      None    -> "world"
    }
    Response.new(200, "hello #{name}")
  }
}
"#,
        ),
        (
            "export/main.emel",
            r#"import Std.Env.{Env}
import Std.Process.{Process}

@export(js, "name")
pub fn name() -> String {
  with Process { (use Env).get("NAME").or("") }
}

fn main() {}
"#,
        ),
    ];
    let mut out = String::new();
    for entry in ["service/main.emel", "export/main.emel"] {
        let analysis = check_files(&files, entry, &mut Typecheck::default());
        out.push_str(&format!("== {entry}\n"));
        out.push_str(&render(&analysis.diagnostics, &analysis.sources, false));
    }
    insta::assert_snapshot!(out);
}
