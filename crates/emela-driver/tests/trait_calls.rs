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
            run(&fs, &input, &mut frontend, &mut c, &options, |_| {})
        }
        _ => run(&fs, &input, &mut frontend, &mut CoreJs, &options, |_| {}),
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

#[test]
fn a_type_without_an_impl_is_not_supported_yet() {
    // A derived type satisfies Show for the type checker before its impl is generated.
    let main = "type Pt(x: Int)\n  derive Show\n\nfn label[A: Show](x: A) -> String { Show.show(x) }\nfn main() { dbg(label(Pt(x: 1))) }\n";
    let (analysis, output) = run_on("missing", main, Target::Js);
    assert!(output.is_none());
    let messages: Vec<&str> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["not supported yet: `Show.show` for `Pt`, which has no impl"]
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
    // `==` and `<` through impls still show both sides with inspect (spec 13.2).
    let main = r##"
type Ver(major: Int, note: String)

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
