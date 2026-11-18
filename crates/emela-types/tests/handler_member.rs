//! Handler members (`[effect.decl]`, `[effect.handler]`, `[effect.with]`, `[effect.use]`,
//! `[type.rules]`, `[type.effect-bound]`).
//!
//! The output shows each fn's type and its local variables' types, then the diagnostics.

mod common;

use common::render_bodies;

const FILES: &str = r#"
effect ReadFile {
  fn read(max: Int) -> Int
}

error Missing

effect Files {
  handler Open(path: String): ReadFile fails Missing
}

effect Console {
  fn print_line(s: String) -> ()
}

handler MemFiles: Files {
  handler Open(path: String, n: Int): ReadFile {
    init(path: String) { Self(path:, n: 3) }
    fn read(max) { self.n + max }
  }
}
"#;

fn run(src: &str) -> String {
    render_bodies(&format!("{FILES}{src}"))
}

/// `x = Files.Open(p)` requires Files, fails as the member declares, and binds x to `use
/// ReadFile` (`[type.rules]`); the operations of x add x, which the `with` removes. Opening
/// through a capability requires the capability (`[effect.use]`).
#[test]
fn opening_handler_members() {
    insta::assert_snapshot!(run(r#"
fn count(f: use ReadFile) -> Int { f.read(1) }

fn by_effect(path: String) {
  with f = Files.Open(path), g = Files.Open("b") {
    count(f) + g.read(2)
  }
}

fn by_capability(fs: use Files, path: String) {
  with f = fs.Open(path) {
    count(f)
  }
}

fn in_lambda(path: String) {
  with f = Files.Open(path) {
    read = || f.read(1)
    read()
  }
}

fn provided_here() {
  with MemFiles, f = Files.Open("a") {
    count(f)
  } escape {
    Missing -> 0
  }
}
"#));
}

/// A bound item that is not a handler member provides its effect by name as well
/// (`[effect.with]`).
#[test]
fn binding_handlers() {
    insta::assert_snapshot!(run(r#"
handler Out: Console {
  fn print_line(s) { () }
}

handler Err: Console {
  fn print_line(s) { () }
}

fn both() {
  with out = Out, err = Err {
    out.print_line("a")
    err.print_line("b")
    (use Console).print_line("c")
  }
}
"#));
}

/// An inner handler's requirements are the outer handler's; inside an init, a handler member is
/// opened by calling it, and one of the handler's own effect is the outer provider's. The
/// operations of a capability in a field of `self` add nothing to R (`[effect.use]`).
#[test]
fn wrapping_providers() {
    insta::assert_snapshot!(run(r#"
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) { Self(inner: Files.Open(path)) }
    fn read(max) {
      (use Console).print_line("read")
      self.inner.read(max)
    }
  }
}

handler Opener(f: use ReadFile): Console {
  init {
    fs = use Files
    Self(f: fs.Open("a"))
  }
  fn print_line(s) { () }
}

fn traced() -> Int fails Missing uses { Files, Console } {
  with TracedFiles {
    with f = Files.Open("a") { f.read(1) }
  }
}

fn opener() -> () fails Missing uses { Files } {
  with Opener { (use Console).print_line("x") }
}
"#));
}

/// E0447, E0448, E0449, E0404 and E0405 for handler members.
#[test]
fn handler_member_errors() {
    insta::assert_snapshot!(run(r#"
handler Out: Console {
  fn print_line(s) { () }
}

layer App {
  Out
}

layer Bad {
  Files.Open
}

handler NoOpen: Files {
}

handler Twice: Files {
  handler Open(path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read(max) { max }
  }
  handler Open(path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read(max) { max }
  }
}

handler WrongParams: Files {
  handler Open(path: Int): ReadFile {
    init(path: Int) { Self(path:) }
    fn read(max) { max }
  }
}

handler NoInit: Files {
  handler Open: ReadFile {
    fn read(max) { max }
  }
}

handler WrongEffect: Files {
  handler Open(): Console {
    init(path: String) { Self() }
    fn print_line(s) { () }
  }
}

fn f() -> () fails Missing uses { Files } {
  x = Files.Open("a")
  with Files.Open("b") { () }
  with y = App { () }
  ()
}
"#));
}

/// A handler member without `suspend` checks the init and the release of its inner handlers as
/// the body of an fn operation (E0417), and an effect with a `suspend handler` member is not
/// Immediate (E0416, `[type.effect-bound]`).
#[test]
fn suspending_handler_members() {
    insta::assert_snapshot!(run(r#"
effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

effect Slow {
  suspend handler Open(path: String): ReadFile
}

effect Quick {
  handler Open(path: String): ReadFile
}

handler BadQuick: Quick {
  handler Open(path: String): ReadFile {
    init(path: String) {
      (use Clock).sleep(1)
      Self(path:)
    }
    release { (use Clock).sleep(1) }
    fn read(max) { max }
  }
}

handler GoodSlow: Slow {
  handler Open(path: String): ReadFile {
    init(path: String) {
      (use Clock).sleep(1)
      Self(path:)
    }
    release { (use Clock).sleep(1) }
    fn read(max) { max }
  }
}

fn on_click[R: Immediate](f: fn() -> () uses { R }) -> () { () }

fn g() -> () uses { Quick, Slow } {
  on_click(|| with f = Quick.Open("a") { () })
  on_click(|| with f = Slow.Open("a") { () })
}
"#));
}

/// An operation of a capability in a field of `self` is not required by name, but it counts as
/// its effect for suspension: an fn operation that calls a suspend fn through a field is E0417.
/// In release, a suspend fn called that way makes Async a requirement, so that the `with` shows
/// that leaving it may suspend (`[effect.use]`).
#[test]
fn capability_fields_and_suspension() {
    insta::assert_snapshot!(run(r#"
effect Slow {
  suspend fn wait() -> Int
}

effect Quick {
  fn get() -> Int
}

handler Through(inner: use Slow): Quick {
  init(inner: use Slow) { Self(inner:) }
  fn get() { self.inner.wait() }
}

handler Closing(inner: use Slow): Console {
  init(inner: use Slow) { Self(inner:) }
  release { _ = self.inner.wait() }
  fn print_line(s) { () }
}

fn closing(s: use Slow) -> () {
  with Closing(inner: s) { (use Console).print_line("x") }
}
"#));
}
