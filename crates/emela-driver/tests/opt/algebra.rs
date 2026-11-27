//! A driver-level regression test for the empty-string guard of the algebraic reduction.
//!
//! The rule does not fold `if True { "" } else { "," }` into `""`, because the native target
//! rejects a `String.split` whose separator is the constant empty string at compile time
//! (`[target.native.unsupported]`), while a separator that is empty at run time is a run-time
//! defect (or a split by character on ASCII text). Folding the `if` would move that behaviour from
//! run time to compile time, so the program below must still build and run.
//!
//! The per-rule flags are off by default until A10/PR-019, and the driver has no switch for a
//! single rule. [`AlgebraOnly`] therefore lowers with the passes off and applies exactly the
//! algebraic rule to the module before the C backend sees it. The same driver path then builds and
//! runs the program, which is what makes the failure a compile error and not only an IR difference.
//!
//! Needs a C compiler (`EMELA_CC` or `cc`); the test is skipped with a message without one.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_core::opt::{CoreFns, Options, Rules, run_with};
use emela_driver::{
    Analysis, Checked, CoreC, CoreSource, FileId, Frontend, Input, Lower, MemoryFiles, Output,
    Parsed, RunOptions, RunOutput, SourceFile, Target, cc_command, run,
};
use emela_resolve::ModuleId;

/// The driver frontend with only the algebraic rule on.
struct AlgebraOnly(Lower);

impl Frontend for AlgebraOnly {
    type Program = emela_core::Module;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        self.0.parse(module, file, source)
    }

    fn core_sources(&self) -> Vec<CoreSource> {
        self.0.core_sources()
    }

    fn has_entry_main(&self) -> Option<bool> {
        self.0.has_entry_main()
    }

    fn is_http_service(&self) -> bool {
        self.0.is_http_service()
    }

    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<Self::Program> {
        let checked = self.0.check(analysis, order);
        let program = checked.program.map(|mut m| {
            run_with(
                &mut m,
                &CoreFns::default(),
                Options {
                    rules: Rules {
                        algebra: true,
                        ..Rules::none()
                    },
                    ..Options::default()
                },
            );
            m
        });
        Checked {
            program,
            diagnostics: checked.diagnostics,
        }
    }
}

fn out_dir(name: &str) -> PathBuf {
    // `CARGO_TARGET_TMPDIR` is under the target directory, not the system temporary directory,
    // which a user's quota can fill while the test suites of several worktrees run at once.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "emela-driver-algebra-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Whether the C compiler starts; prints why the test is skipped otherwise.
fn have_cc() -> bool {
    let (cc, flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!(
            "skipping: cannot start the C compiler `{}` (set EMELA_CC)",
            cc.display()
        );
    }
    ok
}

/// Builds and runs `source` for the native target with exactly the algebraic rule on.
fn native_run(name: &str, source: &str) -> (Analysis, Option<RunOutput>) {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", source)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        js_host: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = AlgebraOnly(
        Lower::default()
            .with_target(Target::Native)
            .with_optimize(false),
    );
    let (analysis, output) = run(
        &fs,
        &input,
        &mut frontend,
        &mut CoreC::default(),
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    (analysis, output)
}

/// The program that uses `if True { "" } else { "," }` as a separator. It must build and split the
/// ASCII text by character, which is what an empty separator does at run time.
const RUNTIME_SEPARATOR: &str = "\
fn main() {
  sep = if True { \"\" } else { \",\" }
  dbg(String.join(String.split(\"abc\", sep), \"|\"))
  ()
}
";

/// The same program with the empty separator written as a literal. The native target rejects it
/// at compile time, which is exactly why the `if` above must not be folded into it.
const LITERAL_SEPARATOR: &str = "\
fn main() {
  dbg(String.join(String.split(\"abc\", \"\"), \"|\"))
  ()
}
";

/// `if True { "" } else { "," }` is not folded, so the program builds and keeps the run-time
/// behaviour: on ASCII text an empty separator splits by character. Written as a literal, the
/// same separator is a compile-time error on the native target, so a fold would turn this test
/// into a build failure.
#[test]
fn an_empty_separator_behind_an_if_still_builds_and_runs() {
    if !have_cc() {
        return;
    }
    let (analysis, output) = native_run("runtime-separator", RUNTIME_SEPARATOR);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let output = output.expect("the program builds and runs");
    assert_eq!(output.code, 0);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("a|b|c"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let (analysis, output) = native_run("literal-separator", LITERAL_SEPARATOR);
    assert!(
        output.is_none(),
        "the literal empty separator must not build"
    );
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.message.contains("empty separator")),
        "{:?}",
        analysis.diagnostics
    );
}
