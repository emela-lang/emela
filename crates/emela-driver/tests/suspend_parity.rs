//! Suspension behaviour that the JS, C and WASM targets must agree on (spec chapter 16, 14.4,
//! 14.5): a release that fails while the run unwinds as interrupted, a release that suspends, and
//! the order releases run in when a continuation is unwound.
//!
//! Needs node, wasmtime and a C compiler (`EMELA_CC` or `cc`); the tests are skipped with a
//! message when one of them cannot start.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreC, CoreJs, CoreWasm, Input, Lower, MemoryFiles, Output, RunOptions, Target,
    cc_command, node_program, run, wasmtime_program,
};

type Outcome = (i32, String, String);

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "emela-driver-suspend-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn have(program: &Path) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn messages(analysis: &Analysis) -> Vec<String> {
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect()
}

/// Runs `src` on one target.
fn run_on(name: &str, src: &str, target: Target) -> Result<Outcome, Vec<String>> {
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
        Target::C => {
            let (cc, flags) = cc_command();
            let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
            let mut backend = CoreC::with_cc(cc, &flags);
            run(&fs, &input, &mut frontend, &mut backend, &options, |_| {})
        }
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    match output {
        Some(out) if !analysis.has_errors() => Ok((
            out.code,
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )),
        _ => Err(messages(&analysis)),
    }
}

/// Runs `src` on all three targets and returns the common outcome, or `None` when a tool is
/// missing. A target that does not build fails the test.
fn compare(name: &str, src: &str) -> Option<Outcome> {
    let (cc, _) = cc_command();
    let tools = [
        (Target::Js, node_program()),
        (Target::C, cc),
        (Target::Wasm, wasmtime_program()),
    ];
    for (target, program) in &tools {
        if !have(program) {
            eprintln!(
                "skipping {name}: cannot start the {target} tool `{}`",
                program.display()
            );
            return None;
        }
    }
    let mut outcomes = Vec::new();
    for (target, _) in &tools {
        let outcome = run_on(name, src, *target).unwrap_or_else(|messages| {
            panic!("{name}: {target} did not build:\n{}", messages.join("\n"))
        });
        outcomes.push((*target, outcome));
    }
    let (first_target, first) = &outcomes[0];
    for (target, outcome) in &outcomes[1..] {
        assert_eq!(
            outcome, first,
            "{name}: {first_target} and {target} disagree:\n{first_target}: {first:?}\n\
             {target}: {outcome:?}",
        );
    }
    Some(first.clone())
}

/// A release that fails while the run unwinds as interrupted: the defect replaces the
/// interruption (spec 14.4), so the run ends as a defect, not as `interrupted`.
const RELEASE_DEFECT: &str = r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release {
    dbg("release res")
    x = 1 / 0
    dbg("never #{x}")
  }
}

fn main() -> () use { Async } {
  with Res {
    with Ignores {
      dbg("body #{(use Wait).wait()}")
    }
  }
  ()
}
"#;

/// A release that suspends and resumes in place: it was rejected when the release could not
/// suspend (spec 11.4).
const RELEASE_SUSPENDS: &str = r#"
effect Tick {
  suspend fn tick() -> Int
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(1) }
}

effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release {
    dbg("release res")
    dbg("tick #{(use Tick).tick()}")
  }
}

fn main() -> () use { Async } {
  with Ticks {
    with Res {
      with Ignores {
        dbg("body #{(use Wait).wait()}")
      }
    }
  }
  dbg("not reached")
  ()
}
"#;

/// A stalled run with a suspended operation implementation: the implementation's releases are
/// inside the caller's, so they run first (spec 14.2, 14.5).
const NESTED_RELEASES: &str = r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

effect Go {
  suspend fn go() -> Int
}

handler Slow: Go {
  suspend fn go(resume) {
    with InnerRes {
      dbg("impl body")
      (use Wait).wait()
      dbg("impl after wait")
      resume(1)
    }
  }
}

effect Name {
  fn name() -> String
}

handler OuterRes: Name {
  init { OuterRes }
  fn name() { "outer" }
  release { dbg("release outer") }
}

handler InnerRes: Name {
  init { InnerRes }
  fn name() { "inner" }
  release { dbg("release inner") }
}

fn caller() -> Int use { Go } {
  with OuterRes {
    v = (use Go).go()
    dbg("caller got #{v}")
    v
  }
}

fn main() -> () use { Async } {
  with Ignores {
    with Slow {
      dbg("main got #{caller()}")
    }
  }
  ()
}
"#;

#[test]
fn a_release_defect_replaces_an_interruption() {
    let Some((code, _stdout, stderr)) = compare("release-defect", RELEASE_DEFECT) else {
        return;
    };
    assert_eq!(code, 101, "{stderr}");
    assert!(
        stderr.contains("defect: division by zero"),
        "the release's defect must be the result: {stderr}"
    );
    assert!(
        !stderr.contains("interrupted"),
        "the interruption must not replace the release's defect: {stderr}"
    );
}

#[test]
fn a_release_that_suspends_runs() {
    let Some((code, _stdout, stderr)) = compare("release-suspends", RELEASE_SUSPENDS) else {
        return;
    };
    assert_eq!(code, 130, "{stderr}");
    assert!(
        stderr.contains("\"release res\"\n\"tick 1\"\n"),
        "the release and its in-place resume must run: {stderr}"
    );
    assert!(stderr.ends_with("interrupted\n"), "{stderr}");
    assert!(!stderr.contains("not reached"), "{stderr}");
}

#[test]
fn releases_run_from_the_inside() {
    let Some((code, _stdout, stderr)) = compare("nested-releases", NESTED_RELEASES) else {
        return;
    };
    assert_eq!(code, 130, "{stderr}");
    let inner = stderr.find("\"release inner\"").expect(&stderr);
    let outer = stderr.find("\"release outer\"").expect(&stderr);
    assert!(
        inner < outer,
        "the implementation's release is inside the caller's: {stderr}"
    );
    assert!(!stderr.contains("impl after wait"), "{stderr}");
}
