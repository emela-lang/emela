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
fn split_with_an_empty_separator_splits_by_graphemes() {
    // A separator that is empty only at run time reaches the runtime, which splits per grapheme
    // cluster like `String.chars` (16.2).
    let Some(out) = stderr(
        "split-empty",
        "fn main() {\n  sep = String.trim(\" \")\n  dbg(String.split(\"a\u{1F44D}\u{1F3FD}\", sep))\n  dbg(String.length(\"a\u{1F44D}\u{1F3FD}\"))\n}\n",
    ) else {
        return;
    };
    assert_eq!(out, "[\"a\", \"\u{1F44D}\u{1F3FD}\"]\n2\n");
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

#[test]
fn floats_show_like_js() {
    // The shortest round-trip digits, `.0` on a finite integral value, `-0.0` as `0.0`,
    // exponents, the infinities and NaN (spec 10.6). The values and the expected text are the
    // same as the JS target's.
    let Some(out) = stderr(
        "floats",
        r#"
fn main() {
  dbg(1.5)
  dbg(1.0)
  dbg(-0.0)
  dbg(0.1)
  dbg(0.1 + 0.2)
  dbg(1.0 / 3.0)
  dbg(1.0e21)
  dbg(1.0e20)
  dbg(1.5e-7)
  dbg(0.000001)
  dbg(123456789.125)
  dbg(1.7976931348623157e308)
  dbg(5.0e-324)
  dbg(1.0e-7)
  dbg(123.0e-20)
  dbg(5.641232424577593e-278)
  dbg(5.641232424577592e-278)
  dbg(3.6893488147419103e+19)
  dbg(3.68934881474191e+19)
  dbg(4.3039187809582457e-283)
  dbg(4.303918780958246e-283)
  dbg(2.0 / 0.0)
  dbg(-2.0 / 0.0)
  dbg(0.0 / 0.0)
  dbg(5.5 % 2.0)
  dbg(-5.5 % 2.0)
  dbg("v=#{1.5} #{2.0} #{-0.0}")
  dbg(inspect(1.5))
  assert Float.floor(2.5) == 2
  assert 1.0 < 2.0
  assert !(0.0 / 0.0 == 0.0 / 0.0)
}
"#,
    ) else {
        return;
    };
    assert_eq!(
        out,
        "1.5\n1.0\n0.0\n0.1\n0.30000000000000004\n0.3333333333333333\n1e+21\n\
         100000000000000000000.0\n1.5e-7\n0.000001\n123456789.125\n\
         1.7976931348623157e+308\n5e-324\n1e-7\n1.23e-18\n\
         5.641232424577593e-278\n5.641232424577592e-278\n\
         36893488147419103000.0\n36893488147419100000.0\n\
         4.3039187809582457e-283\n4.303918780958246e-283\n\
         Infinity\n-Infinity\nNaN\n\
         1.5\n-1.5\n\"v=1.5 2.0 0.0\"\n\"1.5\"\n"
    );
}

#[test]
fn float_rounding_to_int() {
    // Ties away from zero, -0.0 to 0, and the ends of the Int range (spec 16.1).
    let Some(out) = stderr(
        "float-round",
        r#"
fn main() {
  dbg(Float.floor(2.5))
  dbg(Float.ceil(2.5))
  dbg(Float.round(2.5))
  dbg(Float.round(-2.5))
  dbg(Float.round(0.5))
  dbg(Float.round(-0.5))
  dbg(Float.round(0.49999999999999994))
  dbg(Float.truncate(-2.7))
  dbg(Float.floor(-1.5))
  dbg(Float.ceil(-1.5))
  dbg(Float.floor(-0.0))
  dbg(Float.ceil(-0.0))
  dbg(Float.round(-0.0))
  dbg(Float.truncate(-0.0))
  dbg(Float.floor(2147483647.9))
  dbg(Float.truncate(-2147483648.9))
  dbg(Float.round(2147483646.6))
  dbg(Float.ceil(-2147483647.6))
}
"#,
    ) else {
        return;
    };
    assert_eq!(
        out,
        "2\n3\n3\n-3\n1\n-1\n0\n-2\n-2\n-1\n0\n0\n0\n0\n2147483647\n-2147483648\n2147483647\n\
         -2147483647\n"
    );
}

#[test]
fn float_parse_matches_js() {
    // `Float.parse` (spec 15.3): the forms `to_string` writes, an optional sign, `Infinity`
    // and `NaN`; anything else is `None`.
    let Some(out) = stderr(
        "float-parse",
        r#"
fn f(s: String) -> String {
  Float.to_string(Float.parse(s)) escape { ParseError(input:) -> "bad #{input}" }
}

fn main() {
  dbg(f("1.5"))
  dbg(f("-0.25"))
  dbg(f("3"))
  dbg(f("1e+21"))
  dbg(f("1.0e20"))
  dbg(f("Infinity"))
  dbg(f("-Infinity"))
  dbg(f("NaN"))
  dbg(f("+3"))
  dbg(f("-0"))
  dbg(f("-0.0"))
  dbg(f("0.1"))
  dbg(f("2.2250738585072011e-308"))
  dbg(f("5e-324"))
  dbg(f("1e400"))
  dbg(f("1e-400"))
  dbg(f(""))
  dbg(f("1."))
  dbg(f(".5"))
  dbg(f("1e"))
  dbg(f("inf"))
  dbg(f(" 1"))
  dbg(f("0x10"))
  dbg(f("+NaN"))
  dbg(Float.parse("1.5"))
}
"#,
    ) else {
        return;
    };
    assert_eq!(
        out,
        "\"1.5\"\n\"-0.25\"\n\"3.0\"\n\"1e+21\"\n\"100000000000000000000.0\"\n\"Infinity\"\n\
         \"-Infinity\"\n\"NaN\"\n\"3.0\"\n\"0.0\"\n\"0.0\"\n\"0.1\"\n\
         \"2.225073858507201e-308\"\n\"5e-324\"\n\"Infinity\"\n\"0.0\"\n\"bad \"\n\"bad 1.\"\n\
         \"bad .5\"\n\"bad 1e\"\n\"bad inf\"\n\"bad  1\"\n\"bad 0x10\"\n\"bad +NaN\"\n\
         1.5\n"
    );
}

#[test]
fn float_parse_keeps_the_dropped_digits() {
    // `1 + 2^-53` is a midpoint; with `1e-1000` appended the input is above it, even though the
    // first 800 significant digits are the midpoint itself (the sticky bit decides the tie).
    let midpoint = "1.00000000000000011102230246251565404236316680908203125";
    let input = format!("{midpoint}{}1", "0".repeat(946));
    let src = format!("fn main() {{\n  dbg(Float.to_string(Float.parse(\"{input}\")))\n}}\n");
    let Some(out) = stderr("float-long-midpoint", &src) else {
        return;
    };
    assert_eq!(out, "\"1.0000000000000002\"\n");
}

#[test]
fn float_to_int_out_of_range_is_a_defect() {
    let cases = [
        ("Float.floor(0.0 / 0.0)", "NaN"),
        ("Float.ceil(1.0 / 0.0)", "Infinity"),
        ("Float.truncate(-1.0 / 0.0)", "-Infinity"),
        ("Float.truncate(2147483648.0)", "2147483648.0"),
        ("Float.round(2147483647.5)", "2147483647.5"),
        ("Float.floor(-2147483648.5)", "-2147483648.5"),
        ("Float.ceil(2147483647.1)", "2147483647.1"),
    ];
    for (i, (call, shown)) in cases.into_iter().enumerate() {
        let src = format!("fn main() {{\n  dbg({call})\n}}\n");
        let Some(run) = run(&format!("float-defect-{i}"), &src) else {
            return;
        };
        assert_eq!(run.code, 101, "{call}");
        assert_eq!(
            run.stderr,
            format!("defect: cannot convert {shown} to Int\n"),
            "{call}"
        );
    }
}

#[test]
fn a_failed_assert_shows_floats() {
    // `assert` shows both sides through Show (spec 10.6), so a Float is shown like `dbg` does.
    let Some(run) = run("float-assert", "fn main() {\n  assert 1.5 == 2.5\n}\n") else {
        return;
    };
    assert_eq!(run.code, 101);
    assert_eq!(
        run.stderr,
        "defect: assertion failed: 1.5 == 2.5\n  left: 1.5\n right: 2.5\n"
    );
}
