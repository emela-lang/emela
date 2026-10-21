//! The same programs on the JS and the WASM target (spec chapter 16: every target gives the same
//! observable result). Each program uses only the pure language and `dbg`, which both targets
//! write to stderr (JS `console.error`, WASM `fd_write(2)`), so their output can be compared
//! without host functions.
//!
//! Needs node (`EMELA_NODE` or `node`) and wasmtime (`EMELA_WASMTIME` or `wasmtime`); without
//! wasmtime the tests are skipped with a message, like the C tests without a C compiler.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreJs, CoreWasm, Input, Lower, MemoryFiles, Output, RunOptions, Target,
    node_program, run, wasmtime_program,
};

/// Programs that use only the pure language and `dbg`, so both targets must run them the same way.
const PROGRAMS: &[(&str, &str)] = &[
    (
        "xorshift",
        r#"
fn next(seed: Int) -> Int {
  s = if seed == 0 { 1831565813 } else { seed }
  a = Int.bit_xor(s, Int.shift_left(s, 13))
  b = Int.bit_xor(a, Int.shift_right_unsigned(a, 17))
  Int.bit_xor(b, Int.shift_left(b, 5))
}

fn main() {
  seeds = [0, 1, -1, 8, 20261013, 0 - 2147483647 - 1, 2147483647]
  dbg(List.map(seeds, next))
  dbg(List.map(seeds, fn(seed) { next(next(next(seed))) % 1000 }))
}
"#,
    ),
    (
        "integers",
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
    ),
    (
        "enums",
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
    ),
    (
        "lists",
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
    ),
    (
        "closures",
        r#"
fn adder(a: Int) -> fn(Int) -> Int {
  fn(b: Int) { a + b }
}
fn main() {
  add2 = adder(2)
  dbg(add2(40))
}
"#,
    ),
    (
        "strings",
        r#"
fn main() {
  dbg("x=#{1 + 2}!")
  dbg(String.append("ab", "cd"))
  dbg("ab" == "ab")
}
"#,
    ),
    (
        "strings_extended",
        "fn main() {\n  dbg(String.contains(\"hello world\", \"o w\"))\n  dbg(String.starts_with(\"hello\", \"he\"))\n  dbg(String.ends_with(\"hello\", \"lo\"))\n  dbg(String.split(\"a,b,c\", \",\"))\n  dbg(String.join([\"a\", \"b\", \"c\"], \"-\"))\n  dbg(String.trim(\"  hi  \"))\n  dbg(String.code_points(\"A\u{20AC}\"))\n  dbg(String.split(\"\", \",\"))\n  dbg(String.trim(\"\u{00A0}hi\u{2003}\"))\n  dbg(String.starts_with(\"h\u{00E9}llo\", \"h\u{00E9}\"))\n}\n",
    ),
    (
        "effects",
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
    ),
    (
        "errors",
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
    ),
    (
        "state",
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
    ),
    (
        "effect_labels",
        r##"
import Std.State.{State}

effect Log {
  fn log(msg: String) -> ()
}

effect Audit of Log
effect Debug of Log default
effect AuditA of Log
effect AuditB of Log

handler PrintLog: Log {
  fn log(msg) { _ = dbg("print: #{msg}") }
}

handler AuditLog: Log {
  fn log(msg) { _ = dbg("audit: #{msg}") }
}

handler Tagged(tag: String): Log {
  init {
    _ = dbg("open")
    Tagged(tag: "t")
  }
  release {
    _ = dbg("close #{self.tag}")
  }
  fn log(msg) { _ = dbg("#{self.tag}: #{msg}") }
}

fn a() { (use Log).log("from a") }
fn b() { (use Log).log("from b") }

fn c() {
  with Audit { a() }
  with Debug { b() }
}

fn d() {
  with Audit { a() }
}

fn f() {
  with Audit by AuditA { c() }
  with Audit by AuditB { d() }
}

fn direct() {
  log = use Audit
  _ = List.map(["x", "y"], log.log)
  (use Debug).log("direct debug")
}

effect Hits of State[Int]
effect Misses of State[Int]

fn count(hit: Bool) {
  if hit { (use Hits).update(fn(n) { n + 1 }) } else { (use Misses).update(fn(n) { n + 1 }) }
}

fn tally() -> (Int, Int) use { Hits, Misses } {
  count(True)
  count(False)
  count(True)
  ((use Hits).get(), (use Misses).get())
}

fn main() {
  with PrintLog, Audit by AuditLog { c() }
  with PrintLog, Audit by Log, Debug by Tagged { c() }
  with PrintLog, AuditA by AuditLog, AuditB by Log { f() }
  with PrintLog, Audit by AuditLog { direct() }
  with PrintLog, Audit by AuditLog {
    with Tagged { c() }
  }
  (_, misses) = State.run(0, fn() {
    with Misses by State[Int] {
      (counts, hits) = State.run(0, fn() { with Hits by State[Int] { tally() } })
      (h, m) = counts
      _ = dbg("tally #{h} #{m}, hits #{hits}")
    }
  })
  _ = dbg("misses #{misses}")
}
"##,
    ),
    (
        "options",
        "fn main() {\n  dbg(Int.checked_add(1, 2))\n  dbg(Int.checked_add(2147483647, 1))\n  dbg(Int.checked_add(-2147483647, -2))\n  dbg(String.from_code_point(65))\n  dbg(String.from_code_point(8364))\n  dbg(String.from_code_point(55296))\n  dbg(Int.parse(\"42\"))\n}\n",
    ),
    (
        "escapes",
        r#"
fn main() {
  dbg("a\"b\\c")
  dbg("line1\nline2")
}
"#,
    ),
    (
        // Raw control characters (Rust escapes put the real bytes in the source) and `#{`.
        "controls",
        "fn main() {\n  dbg(\"tab:\there\")\n  dbg(\"one:\u{1}del:\u{7f}\")\n  dbg(\"hash:\\#{x}\")\n}\n",
    ),
    (
        "nested_and_empty",
        r#"
fn main() {
  dbg(String.code_points(""))
  dbg([[1, 2], [3]])
  dbg(["a", "b"])
  dbg(String.join(["only"], "-"))
  dbg("")
}
"#,
    ),
    (
        "multi_byte_split",
        r#"
fn main() {
  dbg(String.split("a\u{20AC}b\u{20AC}c", "\u{20AC}"))
  dbg(String.split("\u{20AC}", "\u{20AC}"))
}
"#,
    ),
    (
        "inspect",
        r##"
enum Shape {
  Square(side: Int)
  Rect(Int, Int)
  Empty
}
  derive Show

type Name(value: String)

impl Name: Show {
  fn show(self) { "<#{self.value}>" }
}

type Button(label: Name, on_click: fn(Int) -> Int)

fn twice(x: Int) -> Int { x * 2 }

fn show_any[A: Show](x: A) -> String { inspect(x) }

fn dbg_any[A](x: A) -> A { dbg(x) }

fn main() {
  dbg(inspect(42))
  dbg(inspect("a\"b\\c\nd \#{e}"))
  dbg(inspect([Square(side: 1), Rect(1, 2), Empty]))
  dbg(inspect((1, "a", [Some("x"), None])))
  dbg(show_any([Some(Name(value: "n"))]))
  dbg(twice)
  dbg(Button(label: Name(value: "dbg"), on_click: twice))
  dbg_any([(1, fn(x: Int) { x + 1 })])
  dbg_any([Button(label: Name(value: "g"), on_click: twice)])
  dbg("top")
}
"##,
    ),
    (
        "traits",
        r##"
type Point(x: Int, y: Int)
type Box[A](value: A)

trait Size {
  fn size(self) -> Int

  fn double(self) -> Int { Size.size(self) * 2 }
}

impl Point: Size {
  fn size(self) { self.x + self.y }
}

impl Box[A: Size]: Size {
  fn size(self) { Size.size(self.value) + 1 }

  fn double(self) { 0 }
}

fn total[A: Size](xs: List[A]) -> Int { List.fold(xs, 0, fn(acc, x) { acc + Size.size(x) }) }

fn main() {
  p = Point(x: 1, y: 2)
  dbg(Size.size(p))
  dbg(Size.double(p))
  dbg(Size.size(Box(value: Box(value: p))))
  dbg(Size.double(Box(value: p)))
  dbg(total([p, Point(x: 3, y: 4)]))
}
"##,
    ),
    (
        "error_with_a_function_field",
        r#"
error Retry(after: fn() -> Int)

fn attempt(n: Int) -> Int fails Retry {
  if n > 1 { fail Retry(after: fn() { n }) }
  n
}

fn main() {
  dbg(attempt(1))
  dbg(attempt(2))
}
"#,
    ),
    (
        "derived",
        r##"
type Name(value: String)

impl Name: Show {
  fn show(self) { "<#{self.value}>" }
}

type User(id: Int, name: Name, nick: String) derive Show

enum Tree[A] {
  Leaf
  Node(Tree[A], A, Tree[A])
}
  derive Eq, Ord, Show

fn main() {
  dbg(Show.show(User(id: 1, name: Name(value: "ann"), nick: "a\"n")))
  t = Node(Leaf, 2, Node(Leaf, 3, Leaf))
  dbg(Show.show(t))
  dbg(Eq.eq(t, t))
  dbg(Ord.compare(t, Node(Leaf, 2, Leaf)))
  dbg(Ord.compare(Leaf, t))
}
"##,
    ),
    (
        "deep_recursion",
        "fn count(n: Int, acc: Int) -> Int {\n  if n == 0 { acc } else { count(n - 1, acc + 1) }\n}\nfn main() {\n  dbg(count(100000, 0))\n}\n",
    ),
];

/// Programs that build on JS but stop on WASM, with a part of the diagnostic message that stops
/// them (the WASM target's "not supported yet", which has no diagnostic code any more).
const NOT_ON_WASM: &[(&str, &str, &str)] = &[
    ("float", "fn main() { dbg(1.5) }\n", "`Float` operation"),
    (
        "string_length",
        "fn count(s: String) -> Int { String.length(s) }\nfn main() { dbg(count(\"abc\")) }\n",
        "`String.length` needs the Unicode grapheme cluster tables",
    ),
    (
        "string_chars",
        "fn points(s: String) -> List[String] { String.chars(s) }\nfn main() { dbg(points(\"abc\")) }\n",
        "`String.chars` needs the Unicode grapheme cluster tables",
    ),
    (
        "fibers",
        "import Std.Fiber.{Fiber}\nfn main() -> () use { Async } {\n  Fiber.scope(fn() { () })\n}\n",
        "fibers are not supported on target `wasm`",
    ),
];

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "emela-driver-wasm-parity-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Whether wasmtime starts; prints why the tests are skipped otherwise.
fn have_wasmtime() -> bool {
    let wasmtime = wasmtime_program();
    let ok = Command::new(&wasmtime)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start `{}`", wasmtime.display());
    }
    ok
}

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs `src` on `target`: its exit code, stdout and stderr, or the diagnostics that stopped it.
fn run_on(name: &str, src: &str, target: Target) -> Result<Outcome, Box<Analysis>> {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", src)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{name}-{target}")),
        node: match target {
            Target::Wasm => wasmtime_program(),
            _ => node_program(),
        },
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, output) = match target {
        Target::Js => run(&fs, &input, &mut frontend, &mut CoreJs, &options, |_| {}),
        Target::Wasm => run(&fs, &input, &mut frontend, &mut CoreWasm, &options, |_| {}),
        Target::C | Target::CEmbedded => unreachable!("parity compares js and wasm"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    match output {
        Some(out) if !analysis.has_errors() => Ok(Outcome {
            code: out.code,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
        _ => Err(Box::new(analysis)),
    }
}

fn messages(analysis: &Analysis) -> Vec<String> {
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect()
}

#[test]
fn programs_agree_on_js_and_wasm() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for (name, src) in PROGRAMS {
        let js = match run_on(name, src, Target::Js) {
            Ok(out) => out,
            Err(analysis) => {
                failures.push(format!(
                    "{name}: JS did not build:\n{}",
                    messages(&analysis).join("\n")
                ));
                continue;
            }
        };
        let wasm = match run_on(name, src, Target::Wasm) {
            Ok(out) => out,
            Err(analysis) => {
                failures.push(format!(
                    "{name}: WASM did not build:\n{}",
                    messages(&analysis).join("\n")
                ));
                continue;
            }
        };
        if js != wasm {
            failures.push(format!(
                "{name}: JS and WASM differ\n  JS:   {js:?}\n  WASM: {wasm:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn unsupported_programs_stop_on_wasm() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for (name, src, message) in NOT_ON_WASM {
        if let Err(analysis) = run_on(name, src, Target::Js) {
            failures.push(format!(
                "{name}: JS did not build:\n{}",
                messages(&analysis).join("\n")
            ));
        }
        match run_on(name, src, Target::Wasm) {
            Ok(out) => failures.push(format!("{name}: WASM ran but should not have: {out:?}")),
            Err(analysis) => {
                let messages = messages(&analysis);
                if !messages.iter().any(|m| m.contains(message)) {
                    failures.push(format!(
                        "{name}: the WASM diagnostic does not mention {message:?}:\n{}",
                        messages.join("\n")
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
