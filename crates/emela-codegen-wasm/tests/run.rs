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

handler Hi implements Greet {
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
