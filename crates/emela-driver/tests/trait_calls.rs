//! Trait function calls through `emela run` (spec 10.3): monomorphization picks the impl function
//! for the type, and a default implementation is copied per type. Runs on JS, and on C when cc
//! starts; both must print the same.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreC, CoreJs, Input, Lower, MemoryFiles, Output, RunOptions, RunOutput, Target,
    cc_command, run,
};

fn c_backend() -> Option<CoreC> {
    let (cc, flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping C: cannot start the C compiler `{}`", cc.display());
        return None;
    }
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    Some(CoreC::with_cc(cc, &flags))
}

fn run_on(name: &str, main: &str, target: Target) -> (Analysis, Option<RunOutput>) {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let out_dir = std::env::temp_dir().join(format!(
        "emela-driver-traits-{}-{name}-{target}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out_dir);
    let options = RunOptions {
        out_dir: out_dir.clone(),
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let result = match target {
        Target::C => {
            let mut c = c_backend().expect("checked by the caller");
            run(&fs, &input, &mut frontend, &mut c, &options, |_, _| {})
        }
        _ => run(&fs, &input, &mut frontend, &mut CoreJs, &options, |_, _| {}),
    };
    let _ = std::fs::remove_dir_all(&out_dir);
    result
}

/// The stderr of main (where `dbg` prints) on JS, checked to be the same on C.
fn stderr_of(name: &str, main: &str) -> String {
    let mut outputs = Vec::new();
    let targets: &[Target] = if c_backend().is_some() {
        &[Target::Js, Target::C]
    } else {
        &[Target::Js]
    };
    for &target in targets {
        let (analysis, output) = run_on(name, main, target);
        assert!(
            analysis.diagnostics.is_empty(),
            "{target}: {:?}",
            analysis.diagnostics
        );
        let output = output.expect("the program ran");
        assert_eq!(output.code, 0, "{target}");
        outputs.push(String::from_utf8(output.stderr).unwrap());
    }
    if let [js, c] = outputs.as_slice() {
        assert_eq!(js, c, "JS and C differ");
    }
    outputs.swap_remove(0)
}

const PROGRAM: &str = r##"
type User(name: String)
type Box[A](value: A)

impl User: Show {
  fn show(self) { "User(#{self.name})" }
}

impl Box[A: Show]: Show {
  fn show(self) { "Box(#{Show.show(self.value)})" }
}

trait Greet: Show {
  fn hello(self) -> String

  fn loud(self) -> String { "#{Greet.hello(self)}! #{Show.show(self)}" }
}

impl User: Greet {
  fn hello(self) { "hi #{self.name}" }
}

impl Box[A: Show]: Greet {
  fn hello(self) { "box" }

  fn loud(self) { "BOX" }
}

trait Parse {
  fn parse(text: String) -> Self
}

impl User: Parse {
  fn parse(text) { User(name: text) }
}

fn describe[A: Greet](x: A) -> String { Greet.loud(x) }

fn main() {
  u = User(name: "ann")
  dbg(Show.show(u))
  dbg(Show.show(Box(value: Box(value: u))))
  dbg(describe(u))
  dbg(describe(Box(value: u)))
  p: User = Parse.parse("bob")
  dbg(p |> Show.show)
  f = Show.show
  dbg(List.map([u, p], f))
}
"##;

#[test]
fn calls_reach_the_impl_for_the_type() {
    assert_eq!(
        stderr_of("impls", PROGRAM),
        "\"User(ann)\"\n\"Box(Box(User(ann)))\"\n\"hi ann! User(ann)\"\n\"BOX\"\n\"User(bob)\"\n[\"User(ann)\", \"User(bob)\"]\n"
    );
}

const BUILT_IN: &str = r##"
fn label[A: Show](x: A) -> String { Show.show(x) }
fn same[A: Eq](x: A, y: A) -> Bool { Eq.eq(x, y) }
fn cmp[A: Ord](x: A, y: A) -> Ordering { Ord.compare(x, y) }

fn main() {
  dbg(label(1))
  dbg(label("a\"b"))
  dbg(label(1.5))
  dbg(label(True))
  dbg(label(()))
  dbg(label((1, "a", [Some("x"), None])))
  dbg(label([[1, 2], []]))
  dbg(same((1, "a"), (1, "a")))
  dbg(same([1, 2], [1, 3]))
  dbg(same(Some(0.0), Some(-0.0)))
  dbg(same((), ()))
  dbg(cmp([1, 2], [1, 2, 0]))
  dbg(cmp((2, "a"), (1, "b")))
  dbg(cmp((1, "a"), (1, "a")))
  dbg(cmp(False, True))
  dbg(cmp("b", "a"))
  dbg(cmp(None, Some(1)))
  dbg(cmp(Some(3), Some(3)))
}
"##;

#[test]
fn built_in_types_have_impls() {
    // Primitive types and tuples have the compiler's impls, List and Option those in core
    // (spec 10.6). A String inside a value is quoted.
    assert_eq!(
        stderr_of("built_in", BUILT_IN),
        "\"1\"\n\"a\\\"b\"\n\"1.5\"\n\"True\"\n\"()\"\n\"(1, \\\"a\\\", [Some(\\\"x\\\"), None])\"\n\"[[1, 2], []]\"\n\
         True\nFalse\nTrue\nTrue\nLess\nGreater\nEqual\nLess\nGreater\nLess\nEqual\n"
    );
}

/// Derived impls (spec 10.5): fields go through their own impls (Email's written Show and Eq),
/// a String field is quoted, Ord orders constructors as declared and then fields from the first,
/// and a generic type's impl follows its type argument.
const DERIVED: &str = r##"
type Email(value: String)

impl Email: Show {
  fn show(self) { "<#{self.value}>" }
}

impl Email: Eq {
  fn eq(self, other) { True }
}

type User(id: Int, name: String, email: Email)
  derive Eq, Show

type Point(x: Int, y: Int)
  derive Eq, Ord, Show

enum Tree[A] {
  Leaf
  Node(Tree[A], A, Tree[A])
}
  derive Eq, Ord, Show

enum Pair {
  Both(Bool, (Int, String))
}
  derive Eq, Ord, Show

fn largest[A: Ord](a: A, b: A) -> A {
  Ord.compare(a, b) match {
    Less -> b
    _ -> a
  }
}

fn main() {
  u = User(id: 1, name: "a\"b", email: Email(value: "x@y"))
  dbg(Show.show(u))
  dbg(Eq.eq(u, User(id: 1, name: "a\"b", email: Email(value: "z@y"))))
  dbg(Eq.eq(u, User(id: 2, name: "a\"b", email: Email(value: "x@y"))))
  dbg(Ord.compare(Point(x: 1, y: 2), Point(x: 1, y: 3)))
  dbg(Ord.compare(Point(x: 2, y: 0), Point(x: 1, y: 3)))
  dbg(Ord.compare(Point(x: 1, y: 2), Point(x: 1, y: 2)))
  t = Node(Leaf, 2, Node(Leaf, 3, Leaf))
  dbg(Show.show(t))
  dbg(Ord.compare(Leaf, t))
  dbg(Ord.compare(t, Node(Leaf, 2, Leaf)))
  dbg(Eq.eq(t, t))
  dbg(Show.show(Node(Leaf, u, Leaf)))
  dbg(Eq.eq(Node(Leaf, u, Leaf), Leaf))
  dbg(Show.show(largest(Point(x: 1, y: 2), Point(x: 0, y: 5))))
  dbg(Ord.compare(Both(False, (1, "b")), Both(True, (0, "a"))))
  dbg(Ord.compare(Both(True, (1, "b")), Both(True, (1, "a"))))
  dbg(Show.show(Both(True, (1, "b"))))
}
"##;

#[test]
fn derived_impls_follow_the_structure() {
    assert_eq!(
        stderr_of("derived", DERIVED),
        r#""User(id: 1, name: \"a\\\"b\", email: <x@y>)"
True
False
Less
Greater
Equal
"Node(Leaf, 2, Node(Leaf, 3, Leaf))"
Less
Greater
True
"Node(Leaf, User(id: 1, name: \"a\\\"b\", email: <x@y>), Leaf)"
False
"Point(x: 1, y: 2)"
Less
Greater
"Both(True, (1, \"b\"))"
"#
    );
}

/// Derived impls reach the core impls of List and Option fields: Option orders `None` first.
#[test]
fn derived_impls_use_the_core_impls_of_fields() {
    let main = r#"
type Bag(items: List[Int], tag: Option[String])
  derive Eq, Ord, Show

fn main() {
  a = Bag(items: [1, 2], tag: Some("x"))
  dbg(Show.show(a))
  dbg(Ord.compare(a, Bag(items: [1, 3], tag: None)))
  dbg(Ord.compare(Bag(items: [1], tag: None), Bag(items: [1], tag: Some("a"))))
  dbg(Eq.eq(a, a))
}
"#;
    assert_eq!(
        stderr_of("derived_fields", main),
        "\"Bag(items: [1, 2], tag: Some(\\\"x\\\"))\"\nLess\nLess\nTrue\n"
    );
}

const OPERATORS: &str = r##"
type User(name: String)
type Pt(x: Int, y: Int)
  derive Eq, Show
type Ver(major: Int)

impl User: Show {
  fn show(self) { "<#{self.name}>" }
}

impl User: Eq {
  fn eq(self, other) { True }
}

impl Ver: Eq {
  fn eq(self, other) { self.major == other.major }
}

impl Ver: Ord {
  fn compare(self, other) { Ord.compare(self.major, other.major) }
}

fn biggest[A: Ord](x: A, y: A) -> A { if x < y { y } else { x } }
fn say[A: Show](x: A) -> String { "it is #{x}" }

fn main() {
  u = User(name: "ann")
  dbg("#{u} #{[u]} #{(u, 1)} #{Some(u)}")
  dbg("#{Pt(x: 1, y: 2)} #{[Pt(x: 1, y: 2)]}")
  dbg(u == User(name: "bob"))
  dbg([u] != [User(name: "bob")])
  dbg(Pt(x: 1, y: 2) == Pt(x: 1, y: 2))
  dbg(Ver(major: 1) < Ver(major: 2))
  dbg(Ver(major: 3) >= Ver(major: 2))
  dbg([1, 2] < [1, 3])
  dbg((1, "b") > (1, "a"))
  dbg(biggest(Ver(major: 1), Ver(major: 5)).major)
  dbg(biggest("a", "b"))
  dbg(say(u))
  dbg(say("x"))
  dbg(1.5 < 2.0)
  assert u == User(name: "x")
}
"##;

#[test]
fn operators_and_interpolation_go_through_the_impls() {
    // Values with impls written in source go through them; the rest keeps the structure
    // (spec 10.3, 10.6).
    assert_eq!(
        stderr_of("operators", OPERATORS),
        "\"<ann> [<ann>] (<ann>, 1) Some(<ann>)\"\n\"Pt(x: 1, y: 2) [Pt(x: 1, y: 2)]\"\n\
         True\nFalse\nTrue\nTrue\nTrue\nTrue\nTrue\n5\n\"b\"\n\"it is <ann>\"\n\"it is x\"\nTrue\n"
    );
}

#[test]
fn an_assert_through_an_impl_shows_both_sides() {
    // `==` and `<` through impls still show both sides with inspect (spec 13.2), here by the
    // derived Show (`pub`, as the assert alone does not use it, W0303).
    let main = r##"
pub type Ver(major: Int, note: String) derive Show

impl Ver: Eq {
  fn eq(self, other) { self.major == other.major }
}

impl Ver: Ord {
  fn compare(self, other) { Ord.compare(self.major, other.major) }
}

fn main() {
  assert Ver(major: 1, note: "a") == Ver(major: 1, note: "b")
  assert [Ver(major: 1, note: "a")] < [Ver(major: 2, note: "b")]
  assert [Ver(major: 1, note: "a")] != [Ver(major: 1, note: "b")]
}
"##;
    let expected = "defect: assertion failed: [Ver(major: 1, note: \"a\")] != [Ver(major: 1, note: \"b\")]\n  left: [Ver(major: 1, note: \"a\")]\n right: [Ver(major: 1, note: \"b\")]\n";
    let targets: &[Target] = if c_backend().is_some() {
        &[Target::Js, Target::C]
    } else {
        &[Target::Js]
    };
    for &target in targets {
        let (analysis, output) = run_on("assert", main, target);
        assert!(analysis.diagnostics.is_empty(), "{target}");
        let output = output.expect("the program ran");
        assert_ne!(output.code, 0, "{target}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            expected,
            "{target}"
        );
    }
}

/// The derives and impls of std types (#337). Duration's Show is written in std, so
/// interpolation goes through it instead of the structure.
#[test]
fn std_types_have_impls() {
    let main = r##"
import Std.Duration.{Duration}
import Std.Instant
import Std.Http.Request.{Request, Get}
import Std.Cause.{Fail}

fn main() {
  d = Duration.seconds(2)
  dbg("#{d} #{d == Duration.milliseconds(2000)} #{d < Duration.seconds(3)}")
  dbg("#{Instant.from_unix_milliseconds(0)} #{Instant.from_unix_milliseconds(10) < Instant.from_unix_milliseconds(20)}")
  r = Request(method: Get, url: "u", headers: [], body: "")
  dbg("#{r} #{r == r} #{Fail} #{Fail == Fail}")
}
"##;
    assert_eq!(
        stderr_of("std_types", main),
        "\"2000ms True True\"\n\"1970-01-01T00:00:00.000Z True\"\n\"Request(method: Get, url: \\\"u\\\", headers: [], body: \\\"\\\") True Fail True\"\n"
    );
}

/// The debug display (spec 10.6, #362). `inspect` and `dbg` of a type with Show go through its
/// impl, a String quoted at the top level. `dbg` of a type without Show gives its constructed
/// form, the parts with Show through Show, functions as `<fn>`, and the contents of an opaque
/// type. Whether a type satisfies Show follows the impl's constraints: `impl Wrap[A: Eq]: Show`
/// needs the argument's Eq.
const INSPECT: &str = r##"
type Name(value: String)

impl Name: Show {
  fn show(self) { "<#{self.value}>" }
}

impl Name: Eq {
  fn eq(self, other) { self.value == other.value }
}

type Box[A](value: A)

impl Box[A: Show]: Show {
  fn show(self) { "Box #{Show.show(self.value)}" }
}

type Wrap[A](value: A)

impl Wrap[A: Eq]: Show {
  fn show(self) { "wrap" }
}

type Plain(n: Name, f: fn(Int) -> Int)

pub opaque type Secret(name: Name, code: Int)

fn debug[A](x: A) -> A { dbg(x) }

fn main() {
  dbg(Name(value: "a"))
  dbg([Some(Name(value: "b"))])
  dbg((1, Box(value: Name(value: "c"))))
  dbg(Wrap(value: 1))
  dbg(Wrap(value: Name(value: "w")))
  dbg(Wrap(value: fn(x: Int) { x }))
  dbg(Plain(n: Name(value: "d"), f: fn(x) { x }))
  debug([Plain(n: Name(value: "e"), f: fn(x) { x })])
  dbg(Wrap(value: Plain(n: Name(value: "f"), f: fn(x) { x })))
  dbg(Secret(name: Name(value: "g"), code: 7))
  dbg(inspect(Name(value: "h")))
  dbg(inspect([Box(value: "i")]))
  dbg("s")
}
"##;

#[test]
fn the_debug_display_uses_show_impls() {
    assert_eq!(
        stderr_of("inspect", INSPECT),
        r#"<a>
[Some(<b>)]
(1, Box <c>)
wrap
wrap
Wrap(value: <fn>)
Plain(n: <d>, f: <fn>)
[Plain(n: <e>, f: <fn>)]
Wrap(value: Plain(n: <f>, f: <fn>))
Secret(name: <g>, code: 7)
"<h>"
"[Box i]"
"s"
"#
    );
}

/// The targets to run on: JS, and C if a C compiler is there.
fn targets() -> &'static [Target] {
    if c_backend().is_some() {
        &[Target::Js, Target::C]
    } else {
        &[Target::Js]
    }
}

/// The stderr of a program that ends with `code`, the same on every target.
fn failure_of(name: &str, main: &str, code: i32) -> String {
    let mut outputs = Vec::new();
    for &target in targets() {
        let (analysis, output) = run_on(name, main, target);
        assert!(
            analysis.diagnostics.is_empty(),
            "{target}: {:?}",
            analysis.diagnostics
        );
        let output = output.expect("the program ran");
        assert_eq!(output.code, code, "{target}");
        outputs.push(String::from_utf8(output.stderr).unwrap());
    }
    if let [js, c] = outputs.as_slice() {
        assert_eq!(js, c, "JS and C differ");
    }
    outputs.swap_remove(0)
}

const NAME: &str = r##"
type Name(value: String)

impl Name: Show {
  fn show(self) { "<#{self.value}>" }
}

impl Name: Eq {
  fn eq(self, other) { self.value == other.value }
}
"##;

#[test]
fn a_failed_assert_shows_the_sides_through_show() {
    let main = format!(
        "{NAME}\nfn main() {{\n  assert [Name(value: \"a\")] == [Name(value: \"b\")]\n}}\n"
    );
    assert_eq!(
        failure_of("assert_show", &main, 101),
        "defect: assertion failed: [Name(value: \"a\")] == [Name(value: \"b\")]\n  left: [<a>]\n right: [<b>]\n"
    );
}

/// The sides of a failed assert are shown only if their type satisfies Show (spec 13.2), here
/// decided by the constraint of the impl (`Wrap[A: Ord]`). Otherwise only the text.
#[test]
fn a_failed_assert_without_show_gives_only_the_text() {
    let decls = "type Hidden(n: Int) derive Eq\n\ntype Wrap[A](value: A) derive Eq\n\nimpl Wrap[A: Ord]: Show {\n  fn show(self) { \"w\" }\n}\n\nfn same[A: Eq](a: A, b: A) { assert a == b }\n\nfn same_wraps(a: Wrap[Int], b: Wrap[Int]) -> Bool { a == b }\n\nfn same_hidden(a: Hidden, b: Hidden) -> Bool { a == b }\n";
    let cases = [
        (
            "assert [Wrap(value: Hidden(n: 1))] == [Wrap(value: Hidden(n: 2))]",
            "defect: assertion failed: [Wrap(value: Hidden(n: 1))] == [Wrap(value: Hidden(n: 2))]\n",
        ),
        (
            "same(Hidden(n: 1), Hidden(n: 2))",
            "defect: assertion failed: a == b\n",
        ),
        (
            "same(Wrap(value: 1), Wrap(value: 2))",
            "defect: assertion failed: a == b\n  left: w\n right: w\n",
        ),
    ];
    for (i, (body, expected)) in cases.into_iter().enumerate() {
        let main = format!("{decls}\nfn main() {{\n  {body}\n}}\n");
        assert_eq!(failure_of(&format!("assert_text{i}"), &main, 101), expected);
    }
}

/// An error reaching `main` is shown by its Show, derived (`error ... derive Show`) or written,
/// the parts through their impls; an error without Show by its name only (spec 7.1, 14.3).
#[test]
fn errors_reaching_main_are_shown_by_their_show() {
    let decls = format!(
        "{NAME}\nerror DbError(message: String) derive Show\n\nerror Retry(n: Int)\n\nerror Bad(name: Name)\n  derive Show\n\nerror Custom(n: Int)\n\nimpl Custom: Show {{\n  fn show(self) {{ \"custom #{{self.n}}\" }}\n}}\n\nfn raise[E](e: E) -> () fails E {{ fail e }}\n"
    );
    let cases = [
        (
            "fail DbError(message: \"down\")",
            "error: DbError(message: \"down\")\n",
        ),
        ("fail Retry(n: 1)", "error: Retry\n"),
        (
            "fail Bad(name: Name(value: \"x\"))",
            "error: Bad(name: <x>)\n",
        ),
        ("raise(Custom(n: 3))", "error: custom 3\n"),
    ];
    for (i, (body, expected)) in cases.into_iter().enumerate() {
        let main = format!(
            "{decls}\nfn main() -> () fails DbError | Retry | Bad | Custom {{\n  {body}\n}}\n"
        );
        assert_eq!(failure_of(&format!("error{i}"), &main, 1), expected);
    }
}

/// Std.Map and Std.Set (spec 15.2, 18.1 #36): key order whatever the order of insertion, values
/// compared and shown by their contents, and a tree that stays balanced through many insertions
/// and removals.
#[test]
fn maps_and_sets_follow_key_order() {
    let main = r##"
import Std.Map.{Map}
import Std.Set.{Set}

fn main() {
  m = Map.from_list([(3, "c"), (1, "a"), (2, "b"), (1, "z")])
  dbg("#{m} #{Map.get(m, 1)} #{Map.get(m, 9)} #{Map.size(m)}")
  dbg("#{Map.remove(m, 2)} #{Map.update(m, 4, fn(v) { Some("d") })} #{Map.update(m, 3, fn(v) { None })}")
  dbg("#{Map.union(Map.from_list([(1, "x")]), m)} #{Map.filter(m, fn(k, v) { k != 2 })}")
  dbg("#{Map.fold(m, from: 0, f: fn(acc, k, v) { acc + k })} #{Map.values(Map.map_values(m, fn(v) { "#{v}!" }))}")
  dbg("#{m == Map.from_list([(2, "b"), (1, "z"), (3, "c")])} #{m == Map.remove(m, 3)}")
  big = List.fold(List.range(0, 2000), from: Map.empty(), f: fn(acc, i) { Map.insert(acc, (i * 7919) % 2000, i) })
  rest = List.fold(List.range(0, 1500), from: big, f: fn(acc, i) { Map.remove(acc, i) })
  dbg("#{Map.size(big)} #{Map.size(rest)} #{Map.keys(rest) == List.range(1500, 2000)}")
  s = Set.from_list([3, 1, 2, 3])
  dbg("#{s} #{Set.contains(s, 2)} #{Set.remove(s, 2)} #{Set.union(s, Set.from_list([5]))}")
}
"##;
    assert_eq!(
        stderr_of("maps", main),
        r#""Map.from_list([(1, \"z\"), (2, \"b\"), (3, \"c\")]) Some(\"z\") None 3"
"Map.from_list([(1, \"z\"), (3, \"c\")]) Map.from_list([(1, \"z\"), (2, \"b\"), (3, \"c\"), (4, \"d\")]) Map.from_list([(1, \"z\"), (2, \"b\")])"
"Map.from_list([(1, \"x\"), (2, \"b\"), (3, \"c\")]) Map.from_list([(1, \"z\"), (3, \"c\")])"
"6 [\"z!\", \"b!\", \"c!\"]"
"True False"
"2000 500 True"
"Set.from_list([1, 2, 3]) True Set.from_list([1, 3]) Set.from_list([1, 2, 3, 5])"
"#
    );
}
