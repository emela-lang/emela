//! The same programs on the JS and the WASM target (spec chapter 16: every target gives the same
//! observable result). Each program uses only the pure language and `dbg`, which both targets
//! write to stderr (JS `console.error`, WASM `wasi:cli/stderr.write-via-stream`), so their output
//! can be compared without host functions.
//!
//! Needs node (`EMELA_NODE` or `node`) and wasmtime (`EMELA_WASMTIME` or `wasmtime`); without
//! wasmtime the tests are skipped with a message, like the C tests without a C compiler.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use emela_driver::{
    Analysis, CoreJs, CoreWasm, Input, Lower, MemoryFiles, Output, RunOptions, Target,
    node_program, run, wasmtime_program,
};

/// Programs that use only the pure language and `dbg`, so both targets must run them the same way.
///
/// `string_graphemes` is skipped unless Node's ICU is Unicode 17.x: the JS target segments with
/// the host's `Intl.Segmenter`, while WASM uses the vendored Unicode 17.0.0 tables (spec 18.1 #18).
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
        "string_graphemes",
        r#"
fn main() {
  dbg(String.length("\u{0915}\u{094d}\u{0937}"))
  dbg(String.chars("\u{1f44d}\u{1f3fd}e\u{301}"))
  dbg(String.split("\u{1f1ef}\u{1f1f5}\u{1f1fa}\u{1f1f8}", ""))
  dbg(String.length("\r\n"))
  dbg(String.split("a\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}b", ""))
  dbg(String.join(String.split("x\u{1f1ef}\u{1f1f5}y", ""), "|"))
  dbg(String.length("\u{1100}\u{1161}\u{11a8}"))
  dbg(String.split("\u{ac00}\u{ac01}", ""))
}
"#,
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
        "floats",
        r#"
fn parse(s: String) -> String {
  Float.to_string(Float.parse(s)) escape { ParseError(input:) -> "bad #{input}" }
}

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
  dbg(2.0 / 0.0)
  dbg(-2.0 / 0.0)
  dbg(0.0 / 0.0)
  dbg(5.5 % 2.0)
  dbg(-5.5 % 2.0)
  dbg(Float.floor(-1.5))
  dbg(Float.ceil(-1.5))
  dbg(Float.round(2.5))
  dbg(Float.round(-2.5))
  dbg(Float.truncate(-2.7))
  dbg(Float.floor(2147483647.9))
  dbg(Float.truncate(-2147483648.9))
  dbg(Float.from_int(-2147483647 - 1))
  dbg(1.0 < 2.0)
  dbg(2.0 <= 1.0)
  dbg(0.0 / 0.0 == 0.0 / 0.0)
  dbg(parse("1.25"))
  dbg(parse("1e3"))
  dbg(parse("-0.0"))
  dbg(parse("Infinity"))
  dbg(parse("-Infinity"))
  dbg(parse("NaN"))
  dbg(parse("2.2250738585072011e-308"))
  dbg(parse("1."))
  dbg(parse(" 1"))
  dbg(5.641232424577593e-278)
  dbg(5.641232424577592e-278)
  dbg(3.6893488147419103e+19)
  dbg(3.68934881474191e+19)
  dbg(4.3039187809582457e-283)
  dbg(4.303918780958246e-283)
  dbg(Float.parse(Float.to_string(5.641232424577593e-278)) == 5.641232424577593e-278)
  dbg(Float.parse(Float.to_string(5.641232424577592e-278)) == 5.641232424577592e-278)
  dbg(Float.parse(Float.to_string(3.6893488147419103e+19)) == 3.6893488147419103e+19)
  dbg(Float.parse(Float.to_string(3.68934881474191e+19)) == 3.68934881474191e+19)
  dbg(Float.parse(Float.to_string(4.3039187809582457e-283)) == 4.3039187809582457e-283)
  dbg(Float.parse(Float.to_string(4.303918780958246e-283)) == 4.303918780958246e-283)
}
"#,
    ),
    (
        "float_defect",
        "fn main() {\n  dbg(Float.floor(0.0 / 0.0))\n}\n",
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
const NOT_ON_WASM: &[(&str, &str, &str)] = &[(
    "fibers",
    "import Std.Fiber.{Fiber}\nfn main() -> () use { Async } {\n  Fiber.scope(fn() { () })\n}\n",
    "fibers are not supported on target `wasm`",
)];

/// The Unicode version of the Node used for the JS target (`process.versions.unicode`).
fn node_unicode() -> Option<&'static str> {
    static VERSION: OnceLock<Option<String>> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let output = Command::new(node_program())
                .arg("-e")
                .arg("process.stdout.write(process.versions.unicode)")
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })
        .as_deref()
}

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
        Target::C => unreachable!("parity compares js and wasm"),
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
        if *name == "string_graphemes" {
            match node_unicode() {
                Some(version) if version.starts_with("17.") => {}
                Some(version) => {
                    eprintln!(
                        "skipping {name}: node uses Unicode {version}, need 17.x \
                         (Node 24.18 / ICU 78.3)"
                    );
                    continue;
                }
                // Let the normal JS build report a missing node.
                None => {}
            }
        }
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

/// Long decimal inputs whose kept prefix is exactly a rounding midpoint. They exercise the
/// sticky bit of `Float.parse`: a digit far past the prefix (`1e-1000` or `1e-400` away from the
/// midpoint) decides whether the result rounds up or down.
#[test]
fn float_binade_boundaries_agree() {
    if !have_wasmtime() {
        return;
    }
    // The midpoint between `1.0` and the next value (`1 + 2^-53`) plus `1e-1000`; rounds up.
    let upper_midpoint_of_one = "1.00000000000000011102230246251565404236316680908203125";
    let long = format!("{upper_midpoint_of_one}{}1", "0".repeat(946));
    // The midpoint between `2.0` and the value below it (`2 - 2^-53`), where the spacing halves,
    // plus `1e-1000` (rounds up to `2.0`) and minus `1e-1000` (rounds down).
    let lower_midpoint_of_two = "1.99999999999999988897769753748434595763683319091796875";
    let below_two_plus = format!("{lower_midpoint_of_two}{}1", "0".repeat(946));
    let below_two_minus = format!(
        "{}4{}",
        &lower_midpoint_of_two[..lower_midpoint_of_two.len() - 1],
        "9".repeat(947)
    );
    // The midpoint between `2^-940` and the value below it, where the spacing halves, plus
    // `1e-400` (rounds up to `2^-940`) and minus `1e-400` (rounds down).
    const MID940_PLUS: &str = r"0.0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001075979695239561483777072477772896074635094516691117125448020546603240539387726582021595911969458952933221273331410415223859788488456668170033636639775606559493555975440516565180825997389970088147382452531623758360409886551424296132080794234440609781551703426704888919440901549501484952854207838836112069648714903885585980785165990775218294313779769805151898192120053077589252739633927572002277949120432807835019821447557965909832461958333493648264294970458816923955734617685882095528156293361305439246899064220934909907703441303124831591674683726509538299025575645771392094967790955333135406408333968888240519732039940574591070524969407112047698885885932798007197708548519921034625212996616028249263763427734375";
    let mid940_minus = MID940_PLUS.replacen("1410415223859788", "1410413223859788", 1);
    let src = format!(
        r#"
fn main() {{
  dbg(Float.to_string(Float.parse("{long}")))
  dbg(Float.to_string(Float.parse("{below_two_plus}")))
  dbg(Float.to_string(Float.parse("{below_two_minus}")))
  dbg(Float.to_string(Float.parse("{MID940_PLUS}")))
  dbg(Float.to_string(Float.parse("{mid940_minus}")))
  dbg(Float.parse(Float.to_string(5.641232424577593e-278)) == 5.641232424577593e-278)
  dbg(Float.parse(Float.to_string(3.6893488147419103e+19)) == 3.6893488147419103e+19)
}}
"#
    );
    let js = match run_on("float-boundaries", &src, Target::Js) {
        Ok(out) => out,
        Err(analysis) => panic!("JS did not build:\n{}", messages(&analysis).join("\n")),
    };
    let wasm = match run_on("float-boundaries", &src, Target::Wasm) {
        Ok(out) => out,
        Err(analysis) => panic!("WASM did not build:\n{}", messages(&analysis).join("\n")),
    };
    assert_eq!(js, wasm);
}

/// Inputs whose exponent has more than six digits and is offset by as many digits in the
/// significand. The exponent must not stop accumulating before it outgrows any digit count.
#[test]
fn float_long_exponents_agree() {
    if !have_wasmtime() {
        return;
    }
    let zeros = "0".repeat(1_000_000);
    let src = format!(
        r#"
fn main() {{
  dbg(Float.to_string(Float.parse("1{zeros}e-1000000")))
  dbg(Float.to_string(Float.parse("0.{zeros}1e1000001")))
  dbg(Float.to_string(Float.parse("1e99999999999999999999")))
  dbg(Float.to_string(Float.parse("1e-99999999999999999999")))
}}
"#
    );
    let js = match run_on("float-long-exponents", &src, Target::Js) {
        Ok(out) => out,
        Err(analysis) => panic!("JS did not build:\n{}", messages(&analysis).join("\n")),
    };
    let wasm = match run_on("float-long-exponents", &src, Target::Wasm) {
        Ok(out) => out,
        Err(analysis) => panic!("WASM did not build:\n{}", messages(&analysis).join("\n")),
    };
    assert_eq!(js, wasm);
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
