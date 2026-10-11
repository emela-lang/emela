//! 効果の集合 R の推論（8章，9.2〜9.5），handler の検査，`@external` と能力の値の制約を
//! スナップショットで確かめる．
//!
//! `render_effects` は `render_bodies` の出力（宣言の型と診断）に加え，`main.emel` の式のうち
//! R が空でないものを，位置の順に `式: R` の形で出す．`main` という名前の fn は main として
//! 検査する（R が空でなければならない，8.3）．

mod common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{CheckOptions, check_program_with};

use common::{describe, resolve_main};

fn render_effects(src: &str) -> String {
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

/// `use X` は X を R に載せる．分岐の中でも関数全体の R に載る（8.2）．呼び出しは呼ばれる側の R を引き継ぐ．
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

/// with は供給したエフェクトを R から消し，ハンドラの依存を足す（8.3，9.4）．
/// ハンドラの依存は，同じ with の別のエフェクトのハンドラが満たしてよい（14.2）．
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

handler Quiet implements Logger {
  fn info(msg) { () }
}

handler Stamped implements Logger {
  fn info(msg) {
    t = (use Clock).now()
    ()
  }
}

handler Fixed implements Clock {
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

/// 同じ with のハンドラが互いの効果に依存すると循環になる（E0411）．
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

handler HPing implements Ping {
  fn ping() { (use Pong).pong() }
}

handler HPong implements Pong {
  fn pong() { (use Ping).ping() }
}

fn f() {
  with HPing, HPong { (use Ping).ping() }
}
"#
    ));
}

/// 能力の値の操作の型はエフェクトの宣言から決まる．操作の fails は呼び出しの E に足す（8.1）．
/// 名前付き引数は操作の引数の名前．ない操作は E0401．
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

/// `use` 節を書いたら，ちょうどその集合（9.3）．本体の R が多ければ E0402．
/// 関数型の `use {}` は効果を持たない関数だけを受け取る（9.2）．
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

/// 引数の関数型で `use` を省略すると見えない集合変数で，呼んだ側の R に加わる（9.2）．
/// 無名関数は本体の R を閉じ込める．`use R` の型引数（9.5）．
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

/// handler の検査（8.4）: 全部の操作を1回ずつ実装する，引数の数はエフェクトから，
/// 操作の本体の E は宣言の fails を超えない．状態を持つ handler と suspend はまだ扱わない．
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

handler Half implements Logger {
  fn info(msg) { () }
}

handler Twice implements Logger {
  fn info(msg) { () }
  fn info(msg) { () }
  fn warn(a, b) { () }
}

handler Fails implements Logger {
  fn info(msg) { fail Boom }
  fn warn(msg) { fail Boom }
}

handler Typed implements Logger {
  fn info(msg) { msg + 1 }
  fn warn(msg) { self }
}

handler Stateful(n: Int) implements Logger {
  fn info(msg) { () }
  fn warn(msg) { () }
}

handler Sleepy implements Clock {
  suspend fn sleep(ms, resume) { resume(()) }
}
"#
    ));
}

/// `use` を省略した `@external` の fn はハンドラの操作の本体の中だけ．`use {}` ならどこでも（12.2）．
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

handler JsConsole implements Console {
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

/// 能力の値は引数に渡せず，データ構造に入れられず，返せない（8.2）．
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

/// main とテスト関数の R は空でなければならない（8.3，13.3）．const の値は効果を持てない（5.6）．
#[test]
fn closed_definitions() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet implements Logger {
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
