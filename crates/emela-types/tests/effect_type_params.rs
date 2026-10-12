//! エフェクトの型引数（8.1）と State（8.8）．型引数の違うエフェクトを R の中で区別すること，
//! with と `State.run` が型引数まで同じエフェクトを消すこと，型引数の省略と数の誤りを確かめる．
//!
//! State は同梱の core のモジュール（`emela-core/lib/state.emel`）．出力は宣言の型，局所変数の型，
//! handler の依存，`main.emel` の診断，R が空でない式．

mod common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{CheckOptions, check_program_with};

use common::{describe, resolve_main};

fn render(src: &str) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let in_main =
        |def: emela_resolve::hir::DefId| program.defs[def].module == DefModule::Source(module);
    let tests = program
        .defs
        .iter()
        .filter(|(def, data)| in_main(*def) && data.name.starts_with("test_"))
        .map(|(def, _)| def)
        .collect();
    let types = check_program_with(&program, &CheckOptions { main: None, tests });
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
        for (local, ty) in types.locals.iter() {
            if types.local_owner.get(local) == Some(&def) {
                let name = &program.locals[local].name;
                writeln!(out, "  {name}: {}", ty.display(cons)).unwrap();
            }
        }
    }
    for (def, row) in types.handler_uses.iter() {
        if in_main(def) {
            let name = &program.defs[def].name;
            writeln!(
                out,
                "handler {name}: use {}",
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

/// 同梱の State のソースは，core と一緒に誤りなく型検査を通る．`run` の型は仕様 8.8 のとおり．
#[test]
fn state_source_typechecks() {
    let (program, _, resolve_diagnostics) = resolve_main("fn unused() { () }\n");
    assert_eq!(resolve_diagnostics, []);
    let types = check_program_with(&program, &CheckOptions::default());
    assert_eq!(types.diags, []);
    let cons = &types.ty_table.cons;
    let state = program
        .defs
        .iter()
        .find(|(_, d)| d.kind == DefKind::Effect && d.name == "State")
        .map(|(def, d)| (def, d.module.clone()))
        .expect("State のエフェクトがある");
    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        let in_state = data.module == state.1 && data.kind == DefKind::Fn;
        if !(in_state || data.parent == Some(state.0)) {
            continue;
        }
        if let Some(sig) = types.sigs.get(def) {
            writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap();
        }
    }
    insta::assert_snapshot!(out);
}

/// 仕様 8.8 の例．TestConsole は State[List[String]] に依存するハンドラで，`State.run` がそれを
/// 供給する．`State.run` の `S` は初期値の `[]` だけでは決まらず，body の R から決まる．
#[test]
fn spec_test_console() {
    insta::assert_snapshot!(render(
        r#"
fn greet(id: Int) -> () use { Console } {
  (use Console).print_line("Hello, alice!")
}

handler TestConsole implements Console {
  fn print_line(text) {
    st = use State[List[String]]
    st.set([text, ..st.get()])
  }
  fn print_error_line(text) { () }
}

fn test_greets() {
  (_, lines) = State.run([], fn() { with TestConsole { greet(1) } })
  assert lines == ["Hello, alice!"]
}

fn captured() {
  State.run([], fn() { with TestConsole { greet(1) } })
}
"#
    ));
}

/// 時刻を進める TestClock．State[Instant] に依存し，sleep はその場で resume する．
#[test]
fn test_clock() {
    insta::assert_snapshot!(render(
        r#"
handler TestClock implements Clock {
  suspend fn sleep(ms, resume) {
    (use State[Instant]).update(fn(t) { Instant.add(t, Duration.milliseconds(Int.to_int64(ms))) })
    resume(())
  }
  fn now() { (use State[Instant]).get() }
}

fn elapsed() -> Duration use { Clock } {
  clock = use Clock
  start = clock.now()
  clock.sleep(1500)
  clock.now() |> Instant.since(start)
}

fn test_elapsed() {
  start = Instant.from_unix_milliseconds(Int.to_int64(0))
  (d, last) = State.run(start, fn() { with TestClock { elapsed() } })
  assert Duration.to_milliseconds(d) == Int.to_int64(1500)
}
"#
    ));
}

/// 型引数の違うエフェクトは別のエフェクト．with は型引数まで同じものだけを消す．
#[test]
fn distinct_by_type_args() {
    insta::assert_snapshot!(render(
        r#"
handler Zero implements State[Int] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

handler Empty implements State[String] {
  fn get() { "" }
  fn set(value) { () }
  fn update(f) { () }
}

fn both() -> () use { State[Int], State[String] } {
  n = (use State[Int]).get()
  s = (use State[String]).get()
  ()
}

fn inferred() {
  (use State[Int]).update(fn(n) { n + 1 })
  (use State[String]).get()
}

fn only_int() { with Zero { both() } }

fn both_provided() { with Zero, Empty { both() } }

fn counted() {
  State.run(0, fn() {
    (use State[Int]).update(fn(n) { n + 1 })
    (use State[String]).get()
  })
}

fn nested() {
  State.run(0, fn() {
    State.run("", fn() { inferred() })
  })
}

fn test_leftover() { with Zero { both() } }
"#
    ));
}

/// 型引数のある fn は，型引数を `use State[S]` に書ける．呼ぶ側で型引数が決まる．
#[test]
fn generic_state() {
    insta::assert_snapshot!(render(
        r#"
fn modify[S](f: fn(S) -> S fails {} use {}) -> S use State[S] {
  st = use State[S]
  st.update(f)
  st.get()
}

fn bump() { modify(fn(n) { n + 1 }) }

fn run_bump() { State.run(41, fn() { bump() }) }

fn shout() { State.run("a", fn() { modify(fn(s) { String.concat(s, "!") }) }) }

fn peek() { modify(fn(x) { x }) }

fn peek_int() { State.run(1, fn() { peek() }) }
"#
    ));
}

/// 型引数の省略（E0423），数の誤り（E0304），集合として使った型引数（E0424），型の合わない操作．
#[test]
fn errors() {
    insta::assert_snapshot!(render(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Raise[E] {
  fn guard(f: fn() -> () fails E) -> ()
}

handler Generic implements State {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

handler TooMany implements State[Int, String] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

fn omitted() { (use State).get() }

fn clause() -> () use State { () }

fn logger() { (use Logger[Int]).info("a") }

fn wrong_value() { (use State[Int]).set("a") }

fn undeclared() -> () use State[Int] {
  (use State[String]).set("a")
}

fn as_type(x: State[Int]) { () }
"#
    ));
}

/// layer と with に型引数の違う State のハンドラを並べる．型引数まで同じものだけが重複（E0421）で，
/// 後ろが優先される．ハンドラの操作の実装は implements の型で検査する．
#[test]
fn handlers_and_layers() {
    insta::assert_snapshot!(render(
        r#"
handler Zero implements State[Int] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

handler One implements State[Int] {
  fn get() { 1 }
  fn set(value) { () }
  fn update(f) { () }
}

handler Empty implements State[String] {
  fn get() { "" }
  fn set(value) { () }
  fn update(f) { () }
}

handler Wrong implements State[Int] {
  fn get() { "x" }
  fn set(value) { value + 1 }
  fn update(f) { () }
}

handler Reads implements Console {
  fn print_line(text) {
    n = (use State[Int]).get()
    ()
  }
  fn print_error_line(text) { () }
}

layer Both {
  Zero
  Empty
}

layer Twice {
  Zero
  One
}

fn later_wins() { with Zero, One { (use State[Int]).get() } }

fn layered() { with Both { (use State[String]).get() } }

fn same_with() { with Zero, Reads { (use Console).print_line("x") } }

fn outside() { with Reads { (use Console).print_line("x") } }
"#
    ));
}
