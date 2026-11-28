//! Arguments of init in `with` items and layers.

use super::{render_effects, render_handlers};

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
/// is E4301, with the help to list the handler after the layer.
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

/// `[effect.handler]`: a handler whose init takes parameters needs arguments (E4104), and
/// anything else takes none (E4104, `with H()` included). The arguments are checked as those of
/// a call (E3101, E3103, E3104, E3001), also for a handler a later item overrides. The
/// parameters of init need types (E3006).
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
/// handler value, so it is not E4005; constructing the value outside init still is.
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

handler LazyKv(url: fn() -> String uses { Config }): Kv {
  init(url: fn() -> String uses { Config }) { LazyKv(url:) }
  fn get(key) { (self.url)() }
}

handler TakesString(url: String): Kv {
  init(url: String) { TakesString(url:) }
  fn get(key) { self.url }
}

fn closure() {
  c = use Config
  with LazyKv(url: || c.url()) { (use Kv).get("a") }
}

fn value() {
  c = use Config
  with TakesString(url: c) { (use Kv).get("a") }
}
"#
    ));
}

/// `[effect.handler]`, `[effect.layer]`: a handler listed in a layer without arguments needs them
/// when its init takes parameters (E4104), also in a layer no `with` names and in a layer where
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
/// evaluated, so a call in them that may suspend is not reported in an fn operation (E4303), and
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
