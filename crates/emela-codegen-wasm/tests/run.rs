//! Programs run under `wasmtime` and their output is checked (skipped without `wasmtime`).

mod common;

use common::*;

fn stderr(name: &str, src: &str) -> Option<String> {
    run(name, src).map(|r| {
        assert_eq!(r.code, 0, "{name} exited with {}: {}", r.code, r.stderr);
        r.stderr
    })
}

#[test]
fn integers_and_control() {
    let Some(out) = stderr(
        "int",
        r#"
fn fib(n: Int) -> Int {
  if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() {
  dbg(fib(10))
  dbg(2 + 3 * 4)
  dbg(7 / 2)
  dbg(7 % 2)
  dbg(-7 / 2)
  dbg(-7 % 2)
  dbg(1 == 1)
  dbg(1 != 1)
  dbg(1 < 2)
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "55\n14\n3\n1\n-3\n-1\nTrue\nFalse\nTrue\n");
}

#[test]
fn enums_and_match() {
    let Some(out) = stderr(
        "enum",
        r#"
enum Color {
  Red
  Green
  Blue
}
enum Maybe {
  Missing
  Found(value: Int)
}

fn num(c: Color) -> Int {
  c match {
    Red -> 1
    Green -> 2
    Blue -> 3
  }
}
fn get(o: Maybe) -> Int {
  o match {
    Missing -> 0
    Found(value:) -> value
  }
}
fn main() {
  dbg(num(Blue))
  dbg(get(Found(value: 42)))
  dbg(get(Missing))
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "3\n42\n0\n");
}

#[test]
fn lists_recursion() {
    let Some(out) = stderr(
        "list",
        r#"
fn sum(xs: List[Int], acc: Int) -> Int {
  xs match {
    [] -> acc
    [x, ..rest] -> sum(rest, acc + x)
  }
}
fn main() {
  dbg(sum([1, 2, 3, 4], 0))
  dbg(sum([], 7))
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "10\n7\n");
}

#[test]
fn closures_capture() {
    let Some(out) = stderr(
        "closure",
        r#"
fn adder(a: Int) -> fn(Int) -> Int {
  fn(b: Int) { a + b }
}
fn main() {
  add2 = adder(2)
  dbg(add2(40))
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "42\n");
}

#[test]
fn strings() {
    let Some(out) = stderr(
        "string",
        r#"
fn main() {
  dbg("x=#{1 + 2}!")
  dbg(String.append("ab", "cd"))
  dbg("ab" == "ab")
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "\"x=3!\"\n\"abcd\"\nTrue\n");
}

#[test]
fn effects_and_handlers() {
    let Some(out) = stderr(
        "effect",
        r#"
effect Greet {
  fn hello(name: String) -> String
}

handler Hi: Greet {
  fn hello(name) { "hi #{name}" }
}

fn greet(name: String) -> String use Greet {
  (use Greet).hello(name)
}

fn main() {
  with Hi {
    dbg(greet("bob"))
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "\"hi bob\"\n");
}

#[test]
fn errors_and_escape() {
    let Some(out) = stderr(
        "error",
        r#"
error NotFound(id: Int)

fn risky(n: Int) -> Int fails NotFound {
  if n == 0 { fail NotFound(id: 0) }
  n
}

fn main() {
  a = risky(5) escape { NotFound(_) -> -1 }
  b = risky(0) escape { NotFound(_) -> -1 }
  dbg(a)
  dbg(b)
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "5\n-1\n");
}

#[test]
fn strings_extended() {
    let Some(out) = stderr(
        "strings2",
        "fn main() {\n  dbg(String.contains(\"hello world\", \"o w\"))\n  dbg(String.starts_with(\"hello\", \"he\"))\n  dbg(String.ends_with(\"hello\", \"lo\"))\n  dbg(String.split(\"a,b,c\", \",\"))\n  dbg(String.join([\"a\", \"b\", \"c\"], \"-\"))\n  dbg(String.trim(\"  hi  \"))\n  dbg(String.code_points(\"A\u{20AC}\"))\n  dbg(String.split(\"\", \",\"))\n  dbg(String.trim(\"\u{00A0}hi\u{2003}\"))\n  dbg(String.starts_with(\"h\u{00E9}llo\", \"h\u{00E9}\"))\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "True\nTrue\nTrue\n[\"a\", \"b\", \"c\"]\n\"a-b-c\"\n\"hi\"\n[65, 8364]\n[\"\"]\n\"hi\"\nTrue\n"
    );
}

#[test]
fn split_with_an_empty_separator_is_a_defect() {
    // The constant empty separator is rejected before the Heap IR; a separator that is empty
    // only at run time reaches the runtime, which raises the defect instead of splitting.
    let Some(run) = run(
        "split-empty",
        "fn main() {\n  sep = String.trim(\" \")\n  dbg(String.split(\"ab\", sep))\n}\n",
    ) else {
        return;
    };
    assert_eq!(run.code, 101);
    assert!(
        run.stderr
            .starts_with("defect: `String.split` with an empty separator"),
        "{}",
        run.stderr
    );
}

#[test]
fn state_holds_references() {
    // Regression: `State`'s `$Cell` builtins must take their own reference, like the C runtime
    // (spec 8.8). Without the `dup` the state's value is freed too early and a second `update`
    // reads a dangling cell (WASM would trap or hang, and even print garbage).
    let Some(out) = stderr(
        "state-list",
        r#"
import Std.State.{State}

fn bump(lines: List[String]) -> List[String] {
  List.append(lines, ["x"])
}

fn main() {
  (_, final) = State.run([], fn() {
    st = use State[List[String]]
    st.update(bump)
    st.update(bump)
  })
  dbg(final)
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "[\"x\", \"x\"]\n");
}

#[test]
fn state_through_a_handler() {
    // The same, reached through a handler operation that calls `use State` each time (the shape
    // of `TestConsole`, spec 15.2).
    let Some(out) = stderr(
        "state-handler",
        r#"
import Std.State.{State}

effect Counter {
  fn add() -> ()
}

handler CounterHandler: Counter {
  fn add() { (use State[List[String]]).update(fn(lines) { List.append(lines, ["a"]) }) }
}

fn main() {
  (_, final) = State.run([], fn() {
    with CounterHandler {
      c = use Counter
      c.add()
      c.add()
    }
  })
  dbg(final)
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "[\"a\", \"a\"]\n");
}

#[test]
fn state_of_a_string() {
    let Some(out) = stderr(
        "state-string",
        r#"
import Std.State.{State}

fn main() {
  (_, final) = State.run("a", fn() {
    st = use State[String]
    st.update(fn(s) { String.append(s, "b") })
    st.update(fn(s) { String.append(s, "c") })
  })
  dbg(final)
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "\"abc\"\n");
}

#[test]
fn options_from_builtins() {
    let Some(out) = stderr(
        "options",
        "fn main() {\n  dbg(Int.checked_add(1, 2))\n  dbg(Int.checked_add(2147483647, 1))\n  dbg(Int.checked_add(-2147483647, -2))\n  dbg(String.from_code_point(65))\n  dbg(String.from_code_point(8364))\n  dbg(String.from_code_point(55296))\n  dbg(Int.parse(\"42\"))\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "Some(3)\nNone\nNone\nSome(\"A\")\nSome(\"\u{20AC}\")\nNone\n42\n"
    );
}

#[test]
fn the_scratch_area_does_not_overlap_the_static_data() {
    // Regression: the runtime's scratch buffers used to overlap the static data (the digits of
    // `Int64.to_string` wrote over the list-cell descriptor `String.split` uses), so dropping the
    // split result read a corrupted descriptor and trapped. Every runtime buffer that writes to
    // the scratch area goes first, then the operations that read the descriptors back.
    let Some(out) = stderr(
        "scratch-overlap",
        "fn main() {\n  big = Int64.from_int(2147483647)\n  dbg(Int64.to_string(big * big))\n  dbg(Int.to_string(0 - 2147483647 - 1))\n  dbg(String.from_code_point(65))\n  dbg(String.split(\"a,b,c\", \",\"))\n  dbg(String.code_points(\"AB\"))\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "\"4611686014132420609\"\n\"-2147483648\"\nSome(\"A\")\n\
         [\"a\", \"b\", \"c\"]\n[65, 66]\n"
    );
}

#[test]
fn all_scratch_buffers_are_stepped_in_one_program() {
    // 1 つのプログラムでスクラッチの全バッファ（WASI の iovec / 書き込みバイト数，
    // 整数の桁，改行，Int64 の桁，JSON escape，1 コードポイントの UTF-8）を触りつつ，
    // 静的データ（記述子など）を読み戻す。重なりがあれば #257 のように trap する.
    let Some(out) = stderr(
        "scratch-all",
        "fn main() {\n  dbg(42)\n  dbg(Int.to_string(-12345))\n  big = Int64.from_int(2147483647)\n  dbg(Int64.to_string(big * big))\n  dbg(String.from_code_point(65))\n  dbg(String.split(\"a,b,c\", \",\"))\n  dbg(String.code_points(\"AB\"))\n  dbg(String.trim(\"  hi  \"))\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "42\n\"-12345\"\n\"4611686014132420609\"\nSome(\"A\")\n\
         [\"a\", \"b\", \"c\"]\n[65, 66]\n\"hi\"\n"
    );
}
