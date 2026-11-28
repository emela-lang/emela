//! `@external` functions and the host effects.

use super::render_effects;

/// An `@external` fn with `uses` omitted adds the External of its module, which the operation
/// bodies of its handlers remove; with `uses {}` it adds nothing (`[ext.call]`,
/// `[effect.external]`). Calling one outside a handler is allowed, and puts External in R.
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
fn abs(x: Int) -> Int uses {}

handler SystemConsole: Console {
  fn print_line(text) {
    f = || console_log(text)
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

/// `@external suspend fn` requires Async and the External of its module (`[ext.call]`), which
/// only the handlers of the module remove (`[effect.external]`). fn operation bodies cannot wait
/// on it (E4303).
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

/// Host effects (`[effect.host]`): no handler can implement `ProcessHost` or `PageHost` (E4003)
/// and neither can be taken with `use` (E4402), but both can be written in a uses clause. Calling
/// an external function declared with `uses { ProcessHost }` adds `ProcessHost` to R
/// and the External of its module (`[ext.call]`); a handler of the module removes External,
/// not the host effect (`[effect.external]`). A handler that calls one requires the host
/// effect, so it stays in the R of a with that lists the handler. `main` and a test function are
/// given Async and ProcessHost, not PageHost (E4301).
#[test]
fn host_effects() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Files {
  fn read(path: String) -> String
}

@external(js, "./fs.mjs", "readText")
fn read_host(path: String) -> String uses { ProcessHost }

@external(js, "./dom.mjs", "title")
fn title_host() -> String uses { PageHost }

handler SystemFiles: Files {
  fn read(path) { read_host(path) }
}

handler PageFiles: Files {
  fn read(path) { title_host() }
}

handler Process: ProcessHost {
}

handler Page: PageHost {
}

fn grab() { use ProcessHost }

fn declared() -> Int uses { ProcessHost } { 1 }

fn outside() { read_host("x") }

fn load() -> String { with SystemFiles { (use Files).read("a") } }

fn in_page() -> String { with PageFiles { (use Files).read("a") } }

fn provided() -> Int { with Process { declared() } }

fn main() {
  load()
  declared()
  in_page()
}

fn test_load() {
  load()
  in_page()
}
"#
    ));
}

/// The uses clause of an external function contains only ProcessHost and PageHost, or nothing
/// (`[ext.call]`). Any other effect, Async and a set type parameter included, is E5002, at the
/// effect.
#[test]
fn external_uses_only_host_effects() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Marker {
  fn mark() -> ()
}

@external(js, "./x.mjs", "a")
fn with_marker() -> () uses { Marker }

@external(js, "./x.mjs", "b")
fn with_async() -> () uses { Async }

@external(js, "./x.mjs", "c")
fn with_set[R](f: fn() -> () uses { R }) -> () uses { R }

@external(js, "./x.mjs", "d")
fn with_host() -> () uses { ProcessHost, PageHost }

@external(js, "./x.mjs", "e")
fn pure() -> () uses {}
"#
    ));
}

/// `[effect.external]`, `[ext.call]`: what each external function adds to R, and the External in
/// the functions it returns. `uses` omitted adds External; host effects add External and the host
/// effects; `uses {}` adds nothing, except on a `suspend fn`. A returned function has External
/// and the host effects in its R, and so do the functions it returns in turn; the parameters of
/// those function types are callbacks and are left as written. `fails` and `uses` omitted on a
/// returned function type are empty (`[ext.boundary]`, issue #815). A private fn that calls an
/// external function has External in R; the init, the release and the operation bodies of a
/// handler of the module remove it, inner handlers included, and a closure written in a body
/// keeps it.
#[test]
fn external_effect() {
    insta::assert_snapshot!(render_effects(
        r##"
effect Log {
  fn info(msg: String) -> ()
}

effect Get {
  fn get() -> fn(String) -> () uses { External }
}

@external(js, "./h.mjs", "write")
fn host_write(s: String) -> ()

@external(js, "./h.mjs", "read")
fn host_read(path: String) -> String uses { ProcessHost }

@external(js, "./h.mjs", "fold")
fn fold_case(s: String) -> String uses {}

@external(js, "./h.mjs", "wait")
suspend fn wait() -> () uses {}

@external(js, "./h.mjs", "back")
fn take_back() -> fn(String) -> ()

@external(js, "./h.mjs", "nested")
fn nested() -> fn(fn(Int) -> Int) -> fn(String) -> () uses { ProcessHost }

@external(js, "./h.mjs", "pure")
fn pure_back() -> fn(Int) -> Int fails {} uses {} uses {}

fn tagged(tag: String, msg: String) { host_write("[#{tag}] #{msg}") }

fn as_value() { host_write }

handler FileLog: Log {
  init {
    tagged("init", "x")
    FileLog
  }
  fn info(msg) {
    f = || tagged("info", msg)
    f()
  }
  release { host_write("release") }
}

effect Files {
  fn name() -> String
  handler Open(path: String): Read
}

effect Read {
  fn read() -> String
}

handler HostFiles: Files {
  fn name() { "host" }
  handler Open(path: String): Read {
    init(path: String) {
      host_write("open #{path}")
      Self(path:)
    }
    fn read() { fold_case(self.path) }
    release { host_write("close") }
  }
}

handler Getter: Get {
  fn get() { |s| host_write(s) }
}
"##
    ));
}
