//! `@export` and `@entry` (spec 12.7, 12.8): where they are written, boundary types, the effects
//! of an exported fn, and the names they take (E0438, `[target.js.module]`).

mod common;

use common::resolve_main;
use emela_resolve::ModuleId;
use emela_resolve::hir::{DefKind, DefModule, Program, Span};
use emela_types::{
    CheckOptions, add_entry_exports, check_program_with, host_exports, is_entry_adapter,
    is_http_serve, is_http_service,
};

/// Checks `src` as the entry module (`entry`), or as a module that is not the entry, with its
/// `main` as the entry's main. Each diagnostic is `` `text` Kind { .. } ``, those of
/// [`add_entry_exports`] first; a diagnostic outside `src` shows `<core>`.
fn check(src: &str, entry: bool) -> Vec<String> {
    let (mut program, module, _) = resolve_main(src);
    let entry = entry.then_some(module);
    let mut diags = add_entry_exports(&mut program, entry);
    let options = CheckOptions {
        entry,
        main: entry.and_then(|m| def(&program, m, "main")),
        ..CheckOptions::default()
    };
    diags.extend(check_program_with(&program, &options).diags);
    diags
        .iter()
        .map(|d| format!("`{}` {:?}", text(src, module, d.span), d.kind))
        .collect()
}

fn diagnostics(src: &str) -> Vec<String> {
    check(src, true)
}

/// The fn named `name` of `module`.
fn def(program: &Program, module: ModuleId, name: &str) -> Option<emela_resolve::hir::DefId> {
    program
        .defs
        .iter()
        .find(|(_, d)| {
            d.name == name && d.kind == DefKind::Fn && d.module == DefModule::Source(module)
        })
        .map(|(id, _)| id)
}

fn text(src: &str, module: ModuleId, span: Option<Span>) -> &str {
    match span {
        Some(s) if s.module == module => &src[s.range],
        Some(_) => "<core>",
        None => "<no span>",
    }
}

/// Every parameter and the return value of an `@export` fn are boundary types (E0436).
#[test]
fn export_parameters_and_return_are_boundary_types() {
    let found = diagnostics(
        r#"
@export(js, "f")
pub fn f(a: Int, b: List[Int]) -> List[Int] { b }
"#,
    );
    assert_eq!(
        found,
        [
            r#"`@export(js, "f")` NotBoundary { function: "f", ty: "List[Int]", ret: false }"#,
            r#"`@export(js, "f")` NotBoundary { function: "f", ty: "List[Int]", ret: true }"#,
        ]
    );
}

/// The boundary types of `[ext.boundary]`: the basic types, Bytes, `Array[A]` of a boundary type
/// that is not a function type, `@external type`s, and callbacks made of these.
#[test]
fn boundary_types() {
    let found = diagnostics(
        r#"
import Std.Bytes.{Bytes}
import Std.Array.{Array}

@external
type Handle

@export(js, "basic")
pub fn basic(a: Int, b: Int64, c: Float, d: Bool, e: String, f: ()) -> () { () }

@export(js, "built_in")
pub fn built_in(a: Bytes, b: Array[Int], c: Array[Array[Bytes]]) -> Handle {
  built_in(a, b, c)
}

@export(js, "callback")
pub fn callback(a: Handle, g: fn(Handle) -> Handle fails {} uses {}) -> Handle { g(a) }
"#,
    );
    assert_eq!(found, Vec::<String>::new());
}

/// A user type, an enum, and an Array of functions or of a non-boundary type are not boundary
/// types.
#[test]
fn types_that_are_not_boundary_types() {
    let found = diagnostics(
        r#"
import Std.Array.{Array}

type User(id: Int)
type Box[A](value: A)

@export(js, "f")
pub fn f(
  a: User,
  box: Box[Int],
  callback: fn(Int) -> List[Int] fails {} uses {},
  b: Option[Int],
  c: Array[fn() -> Int fails {} uses {}],
  d: Array[User],
  e: (Int, Int),
) -> () { () }
"#,
    );
    let tys: Vec<&str> = found
        .iter()
        .map(|d| d.split("ty: ").nth(1).unwrap().split(',').next().unwrap())
        .collect();
    assert_eq!(
        tys,
        [
            r#""User""#,
            r#""Box[Int]""#,
            r#""fn(Int) -> List[Int]""#,
            r#""Option[Int]""#,
            r#""Array[fn() -> Int]""#,
            r#""Array[User]""#,
            r#""(Int"#,
        ]
    );
}

/// The R of an `@export` fn is empty except for Async (E0402), with the export-specific context.
#[test]
fn export_effects_are_checked() {
    let found = diagnostics(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

@export(js, "f")
pub fn f() { (use Logger).info("x") }
"#,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("UnhandledEffects") && found[0].contains(r#"Export { name: "f" }"#),
        "{found:?}"
    );
}

/// The E of an `@export` fn is empty, even when a `fails` clause declares it (E0402).
#[test]
fn export_errors_are_checked() {
    let found = diagnostics(
        r#"
error Timeout

@export(js, "f")
pub fn f() -> Int fails Timeout { fail Timeout }
"#,
    );
    assert_eq!(
        found,
        [r#"`@export(js, "f")` ExportFails { function: "f", errors: "Timeout" }"#]
    );
}

/// `@entry` requires the fn to fit the adapter's first parameter, including its E and R (E0440).
#[test]
fn entry_checks_the_effects_of_the_function() {
    let adapter = r#"
error Timeout

@export(js, "default", "serve")
pub fn serve(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }
"#;
    let ok = diagnostics(&format!(
        "{adapter}\n@entry(serve)\npub fn app(n: Int) -> Int {{ n }}\n"
    ));
    assert_eq!(ok, Vec::<String>::new());

    // The type errors of the fn made by `@entry` become one E0440; other diagnostics stay.
    let found = diagnostics(&format!(
        "{adapter}\n@entry(serve)\npub fn app(n: Int) -> Int fails Timeout {{ fail Timeout }}\n\
         @export(js, \"g\")\npub fn g() -> Int fails Timeout {{ fail Timeout }}\n"
    ));
    assert_eq!(
        found,
        [
            r#"`@export(js, "g")` ExportFails { function: "g", errors: "Timeout" }"#,
            r#"`@entry(serve)` EntryMismatch { entry: "app", adapter: "serve" }"#,
        ]
    );
}

/// A type parameter is not a boundary type, and is shown by its name, not `'0`.
#[test]
fn export_boundary_types_show_type_parameter_names() {
    let found = diagnostics(
        r#"
@export(js, "f")
pub fn f[A](x: A) -> A { x }
"#,
    );
    assert_eq!(
        found,
        [
            r#"`@export(js, "f")` NotBoundary { function: "f", ty: "A", ret: false }"#,
            r#"`@export(js, "f")` NotBoundary { function: "f", ty: "A", ret: true }"#,
        ]
    );
}

/// An `@export` fn whose first parameter is a function type is an entry adapter: its first
/// parameter need not be a boundary type, but the others and the return value are (E0436).
#[test]
fn entry_adapter_skips_the_first_parameter() {
    let found = diagnostics(
        r#"
@export(js, "default", "serve")
pub fn serve(app: fn(List[Int]) -> Int fails {} uses {}, n: List[Int]) -> Int { app(n) }
"#,
    );
    assert_eq!(
        found,
        [
            r#"`@export(js, "default", "serve")` NotBoundary { function: "serve", ty: "List[Int]", ret: false }"#
        ]
    );
}

/// Only an `@export` fn whose first parameter has a function type is an entry adapter.
#[test]
fn what_is_an_entry_adapter() {
    let (program, main, _) = resolve_main(
        r#"
@export(js, "a")
pub fn adapter(app: fn() -> Int fails {} uses {}) -> Int { app() }
@export(js, "b")
pub fn plain(n: Int) -> Int { n }
pub fn not_exported(app: fn() -> Int fails {} uses {}) -> Int { app() }
"#,
    );
    let is = |name| is_entry_adapter(&program, def(&program, main, name).unwrap());
    assert!(is("adapter"));
    assert!(!is("plain") && !is("not_exported"));
}

/// Outside the entry module, `@entry` and an `@export` that is not an entry adapter are E0437;
/// an entry adapter may be anywhere.
#[test]
fn export_and_entry_outside_the_entry_module() {
    let found = check(
        r#"
@export(js, "default", "serve")
pub fn serve(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }

@export(js, "f")
pub fn f() -> Int { 1 }

@entry(serve)
pub fn app(n: Int) -> Int { n }
"#,
        false,
    );
    assert_eq!(
        found,
        [
            r#"`@export(js, "f")` ExportOutsideEntry { attr: "export" }"#,
            r#"`@entry(serve)` ExportOutsideEntry { attr: "entry" }"#,
        ]
    );
}

/// The argument of `@entry` names an entry adapter (E0439), shown qualified when it is in
/// another module.
#[test]
fn entry_names_an_entry_adapter() {
    let found = diagnostics(
        r#"
import Std.Http

pub fn helper(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }

@entry(helper)
pub fn a(n: Int) -> Int { n }

@entry(Http.get)
pub fn b(n: Int) -> Int { n }
"#,
    );
    assert_eq!(
        found,
        [
            r#"`@entry(helper)` NotEntryAdapter { name: Some("helper") }"#,
            r#"`@entry(Http.get)` NotEntryAdapter { name: Some("Http.get") }"#,
        ]
    );
}

/// `@entry(Http.serve)` makes an HTTP service (`[lib.http-service]`); another fn named `serve`
/// does not, and an adapter in another module is named qualified in E0440.
#[test]
fn http_services() {
    let src = r#"
import Std.Http.{Request, Response}

@export(js, "default", "serve")
pub fn serve(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }

@entry(Http.serve)
pub fn app(req: Request) -> Response { Http.response(200, "") }
"#;
    let (mut program, main, _) = resolve_main(src);
    assert_eq!(add_entry_exports(&mut program, Some(main)), []);
    assert!(is_http_service(&program));
    let own_serve = def(&program, main, "serve").unwrap();
    assert!(!is_http_serve(&program, own_serve));

    let (mut program, main, _) = resolve_main(&format!(
        "{}\n@entry(serve)\npub fn app(n: Int) -> Int {{ n }}\n",
        src.split("@entry(Http.serve)").next().unwrap()
    ));
    add_entry_exports(&mut program, Some(main));
    assert!(!is_http_service(&program));

    let found = diagnostics(
        r#"
import Std.Http

@entry(Http.serve)
pub fn app(n: Int) -> Int { n }
"#,
    );
    assert_eq!(
        found,
        [r#"`@entry(Http.serve)` EntryMismatch { entry: "app", adapter: "Http.serve" }"#]
    );
}

/// The fns the program exposes: the entry module's `@export` fns that are not entry adapters, then
/// one per `@entry`. An adapter whose remaining parameters name its own type parameters makes no
/// fn (E0436 is reported on the adapter).
#[test]
fn host_exports_of_the_entry_module() {
    let (mut program, main, _) = resolve_main(
        r#"
@export(js, "default", "serve")
pub fn serve(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }

@export(js, "default", "serve2")
pub fn serve2(app: fn(Int) -> Int fails {} uses {}) -> Int { app(2) }

@export(js, "default", "generic")
pub fn generic[A](app: fn(A) -> Int fails {} uses {}, a: A) -> Int { app(a) }

@export(js, "default", "in_tuple")
pub fn in_tuple[A](app: fn(A) -> Int fails {} uses {}, a: (Int, A)) -> Int { 1 }

@export(js, "default", "in_return")
pub fn in_return[A](app: fn(A) -> Int fails {} uses {}, f: fn(Int) -> A fails {} uses {}) -> Int { 1 }

@export(js, "f")
pub fn f() -> Int { 1 }

@entry(serve)
@entry(generic)
@entry(in_tuple)
@entry(in_return)
@entry(serve2)
pub fn app(n: Int) -> Int { n }

@entry(serve)
pub fn other(n: Int) -> Int { n }
"#,
    );
    add_entry_exports(&mut program, Some(main));
    let names: Vec<String> = host_exports(&program, Some(main))
        .iter()
        .map(|e| program.defs[e.def].name.to_string())
        .collect();
    // Numbered per fn.
    assert_eq!(names, ["f", "app$entry0", "app$entry1", "other$entry0"]);
    // Without an entry module, only the fns made by `@entry`.
    assert_eq!(host_exports(&program, None).len(), 3);
}

/// Two exports with the same name for the same target are E0438, at the second; another target or
/// another interface is another name.
#[test]
fn duplicate_exports() {
    let found = diagnostics(
        r#"
@export(js, "f")
@export(c, "f")
pub fn a() -> Int { 1 }

@export(js, "f")
@export(js, "default", "f")
pub fn b() -> Int { 1 }
"#,
    );
    assert_eq!(
        found,
        [r#"`@export(js, "f")` DuplicateExport { name: "f", target: "js" }"#]
    );
}

/// `main.mjs` exports `main` when the entry has a `main` (`[target.js.module]`), so a two-argument
/// `@export(js, "main")` is E0438; the three-argument form and other targets are not.
#[test]
fn the_launcher_takes_main() {
    let exports = r#"
@export(js, "main")
pub fn a() -> Int { 1 }

@export(js, "default", "main")
@export(c, "main")
pub fn b() -> Int { 1 }
"#;
    let with_main = diagnostics(&format!("{exports}\nfn main() {{ () }}\n"));
    assert_eq!(
        with_main,
        [r#"`@export(js, "main")` DuplicateExport { name: "main", target: "js" }"#]
    );
    assert_eq!(diagnostics(exports), Vec::<String>::new());
    // Without an entry module there is no `main.mjs`.
    assert_eq!(
        check(&format!("{exports}\nfn main() {{ () }}\n"), false)
            .iter()
            .filter(|d| d.contains("DuplicateExport"))
            .count(),
        0
    );
}

/// `main.mjs` exports `$start` when the entry's `main` or a `js` export may suspend, and only
/// then; it is reported once, at the first such export.
#[test]
fn the_launcher_takes_start_when_something_may_suspend() {
    let start = r#"
@export(js, "$start")
pub fn a() -> Int { 1 }

@export(js, "$start")
pub fn b() -> Int { 1 }
"#;
    let duplicate = r#"`@export(js, "$start")` DuplicateExport { name: "$start", target: "js" }"#;
    // Nothing suspends: only the duplicate of `a`'s export.
    assert_eq!(diagnostics(start), [duplicate]);

    let suspending_export =
        format!("{start}\n@export(js, \"tick\")\npub fn tick() -> () uses {{ Async }} {{ () }}\n");
    assert_eq!(diagnostics(&suspending_export), [duplicate, duplicate]);

    let suspending_main = format!("{start}\nfn main() uses {{ Async }} {{ () }}\n");
    assert_eq!(diagnostics(&suspending_main), [duplicate, duplicate]);

    // A suspending export for another target does not need `$start`.
    let c_only =
        format!("{start}\n@export(c, \"tick\")\npub fn tick() -> () uses {{ Async }} {{ () }}\n");
    assert_eq!(diagnostics(&c_only), [duplicate]);
}

/// An `@entry` whose adapter exports `main` twice is one export at the `@entry` for each: the
/// second is a duplicate of the first, and the first of the launcher's; each is reported once.
#[test]
fn an_entry_that_takes_main_twice() {
    let found = diagnostics(
        r#"
@export(js, "main")
@export(js, "main")
pub fn serve(app: fn(Int) -> Int fails {} uses {}) -> Int { app(1) }

@entry(serve)
pub fn app(n: Int) -> Int { n }

fn main() { () }
"#,
    );
    assert_eq!(
        found,
        [r#"`@entry(serve)` DuplicateExport { name: "main", target: "js" }"#]
    );
}
