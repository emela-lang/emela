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

#[test]
fn a_type_without_an_impl_is_not_supported_yet() {
    // The built-in types satisfy Show for the type checker before their impls exist.
    let main = "fn label[A: Show](x: A) -> String { Show.show(x) }\nfn main() { dbg(label(1)) }\n";
    let (analysis, output) = run_on("missing", main, Target::Js);
    assert!(output.is_none());
    let messages: Vec<&str> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["not supported yet: `Show.show` for `Int`, which has no impl"]
    );
}
