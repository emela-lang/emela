//! The optimisation switch (`--no-opt`) must not change what a program observes: for the same
//! source, stdout, stderr and the exit code are the same with and without the IR optimisation
//! passes. Only memory use and when a program runs out of memory may differ (implementation
//! defined, spec `[effect.handler]`).
//!
//! Two kinds of test:
//!
//! - Corpus tests build and run the same program both ways on C (with the runtime's leak check,
//!   `-DEMELA_LEAK_CHECK`, which makes a leak an observable difference) and on WASM for a few
//!   programs. `crates/emela-codegen-c/tests/programs/*.emel` is the bulk; `examples/*` are
//!   projects. By default a representative subset runs; set `EMELA_OPT_DIFF_FULL=1` to run every
//!   codegen program. The law tests of `tests/laws` are behind `--ignored`.
//! - Mutation tests (T02, so that T11 and T12 have something to check): a program written as the
//!   optimisation's input and the same program with the expansion written by hand. Both forms
//!   give the same output with optimisation on, which is what an optimisation must preserve.
//!
//! Needs a C compiler (`EMELA_CC` or `cc`) and, for the WASM tests, wasmtime; without them the
//! tests are skipped with a message.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use emela_driver::{
    Analysis, CoreC, CoreWasm, Input, Lower, LowerTests, OsFs, Output, Reporter, RunOptions,
    RunOutput, Target, TestOptions, TestOutcome, cc_command, run, test, wasmtime_program,
};
use emela_pome::fetch::Fetcher;
use emela_pome::fetch::config::Config;
use emela_pome::fetch::transport::Http;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Whether the C compiler starts; prints why a test is skipped otherwise.
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

/// The C backend with the runtime's leak check compiled in, as `parity.rs` builds it.
fn c_backend(optimize: bool) -> CoreC {
    let (cc, mut flags) = cc_command();
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    CoreC::with_cc(cc, &flags).with_optimize(optimize)
}

/// Whether wasmtime starts; prints why the WASM tests are skipped otherwise.
fn have_wasmtime() -> bool {
    let wasmtime = wasmtime_program();
    let ok = Command::new(&wasmtime)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start wasmtime `{}`", wasmtime.display());
    }
    ok
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-opt-diff-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

type Outcome = (i32, String, String);

/// Builds and runs the program at `path` on the native target, with the optimisation passes on or
/// off.
///
/// `path` is a source file or a project directory. The result is the program's exit code, stdout
/// and stderr, or the diagnostics that stopped it.
fn run_native_program(
    path: &Path,
    optimize: bool,
    name: &str,
    c: &mut CoreC,
) -> Result<Outcome, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{name}-{}", flag(optimize))),
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default()
        .with_target(Target::Native)
        .with_optimize(optimize);
    let (analysis, output) = run(&OsFs, &input, &mut frontend, c, &options, |_, _| {});
    let _ = std::fs::remove_dir_all(&options.out_dir);
    outcome(analysis, output)
}

/// [`run_native_program`] on the WASM target.
fn run_wasm_program(path: &Path, optimize: bool, name: &str) -> Result<Outcome, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{name}-{}", flag(optimize))),
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default()
        .with_target(Target::Wasm)
        .with_optimize(optimize);
    let (analysis, output) = run(
        &OsFs,
        &input,
        &mut frontend,
        &mut CoreWasm::default().with_optimize(optimize),
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    outcome(analysis, output)
}

/// The observable result of a build and run, or the diagnostics that stopped it.
fn outcome(analysis: Analysis, output: Option<RunOutput>) -> Result<Outcome, Box<Analysis>> {
    match output {
        Some(RunOutput {
            code,
            stdout,
            stderr,
        }) if !analysis.has_errors() => Ok((
            code,
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )),
        _ => Err(Box::new(analysis)),
    }
}

/// The test results of the project at `path`, with the optimisation passes on or off.
///
/// Reads the test root and the dependencies of the project (`Pome.toml` and `Pome.lock`), as
/// `emela test` does: `tests/laws` imports brix.
fn run_tests(
    path: &Path,
    optimize: bool,
    name: &str,
    c: &mut CoreC,
) -> Result<Vec<(String, TestOutcome)>, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let mut input = input.with_tests(&OsFs);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config = Config::from_env(|k| std::env::var(k).ok(), home)
        .unwrap_or_else(|e| panic!("the dependencies of {} cannot be read: {e}", path.display()));
    let fetcher = Fetcher::new(config, Http::new());
    input.dependencies = emela_driver::project_dependencies(&OsFs, &input.base, &fetcher, true)
        .unwrap_or_else(|e| panic!("the dependencies of {} cannot be read: {e}", path.display()));
    let options = TestOptions {
        out_dir: out_dir(&format!("{name}-tests-{}", flag(optimize))),
        js_host: emela_driver::js_host_program(),
        filter: None,
        output: Output::Capture,
        reporter: Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &OsFs,
        &input,
        &mut LowerTests::default()
            .with_target(Target::Native)
            .with_optimize(optimize),
        c,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    match report {
        Some(report) if !analysis.has_errors() => Ok(report
            .results
            .into_iter()
            .map(|r| (r.name, r.outcome))
            .collect()),
        _ => Err(Box::new(analysis)),
    }
}

/// `on` or `off`, for the names of output directories and failures.
fn flag(optimize: bool) -> &'static str {
    if optimize { "on" } else { "off" }
}

fn messages(analysis: &Analysis) -> Vec<String> {
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect()
}

/// One comparison of the two modes: `None` when they agree.
fn compare_program(path: &Path, name: &str, off: &mut CoreC, on: &mut CoreC) -> Option<String> {
    let without = run_native_program(path, false, name, off);
    let with = run_native_program(path, true, name, on);
    match (without, with) {
        (Ok(without), Ok(with)) if without == with => None,
        (Ok(without), Ok(with)) => Some(format!(
            "{name}: --no-opt gives {without:?}\nthe default gives {with:?}"
        )),
        (Err(analysis), _) => Some(format!(
            "{name} does not build with --no-opt: {:?}",
            messages(&analysis)
        )),
        (_, Err(analysis)) => Some(format!(
            "{name} does not build by default: {:?}",
            messages(&analysis)
        )),
    }
}

/// [`compare_program`] on the WASM target.
fn compare_wasm(path: &Path, name: &str) -> Option<String> {
    let without = run_wasm_program(path, false, name);
    let with = run_wasm_program(path, true, name);
    match (without, with) {
        (Ok(without), Ok(with)) if without == with => None,
        (Ok(without), Ok(with)) => Some(format!(
            "{name}: --no-opt gives {without:?}\nthe default gives {with:?}"
        )),
        (Err(analysis), _) => Some(format!(
            "{name} does not build with --no-opt: {:?}",
            messages(&analysis)
        )),
        (_, Err(analysis)) => Some(format!(
            "{name} does not build by default: {:?}",
            messages(&analysis)
        )),
    }
}

/// Calls `f` on every item from as many threads as there are CPUs, each with its own pair of C
/// backends, and returns the failures in the order of `items`.
fn in_parallel<T: Sync>(
    items: &[T],
    f: impl Fn(&T, &mut CoreC, &mut CoreC) -> Option<String> + Sync,
) -> Vec<String> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, Option<String>)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut off = c_backend(false);
                    let mut on = c_backend(true);
                    let mut results = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        results.push((i, f(item, &mut off, &mut on)));
                    }
                    results
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().filter_map(|(_, r)| r).collect()
}

/// Every program of emela-codegen-c's tests (`crates/emela-codegen-c/tests/programs/*.emel`).
fn codegen_programs() -> Vec<PathBuf> {
    let dir = root().join("crates/emela-codegen-c/tests/programs");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    programs
}

/// The programs run by default: a small mixed set. `EMELA_OPT_DIFF_FULL=1` runs every program of
/// the corpus instead, which takes long enough to keep out of the usual test run.
const DEFAULT_PROGRAMS: &[&str] = &[
    "array",
    "array_map_range",
    "call_order",
    "closures",
    "data",
    "defect_release",
    "each",
    "effects",
    "errors",
];

fn full() -> bool {
    std::env::var("EMELA_OPT_DIFF_FULL").is_ok_and(|v| v == "1")
}

fn selected_programs() -> Vec<PathBuf> {
    let programs = codegen_programs();
    if full() {
        return programs;
    }
    programs
        .into_iter()
        .filter(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy();
            DEFAULT_PROGRAMS.contains(&stem.as_ref())
        })
        .collect()
}

/// The same codegen program gives the same stdout, stderr and exit code with `--no-opt` and by
/// default. `EMELA_OPT_DIFF_FULL=1` runs the whole corpus.
#[test]
fn codegen_programs_are_the_same_with_and_without_optimisation() {
    if !have_cc() {
        return;
    }
    let programs = selected_programs();
    assert!(!programs.is_empty());
    let failures = in_parallel(&programs, |path, off, on| {
        let name = path.file_stem().unwrap().to_string_lossy();
        compare_program(path, &format!("program-{name}"), off, on)
    });
    report(&failures);
}

/// The example projects give the same stdout, stderr and exit code with `--no-opt` and by
/// default.
#[test]
fn examples_are_the_same_with_and_without_optimisation() {
    if !have_cc() {
        return;
    }
    let examples = root().join("examples");
    let names = ["hello", "errors", "traits"];
    let failures = in_parallel(&names, |name, off, on| {
        compare_program(&examples.join(name), name, off, on)
    });
    report(&failures);
}

/// A few programs give the same stdout, stderr and exit code on WASM with `--no-opt` and by
/// default.
#[test]
fn wasm_programs_are_the_same_with_and_without_optimisation() {
    if !have_wasmtime() {
        return;
    }
    let dir = root().join("crates/emela-codegen-c/tests/programs");
    let mut failures = Vec::new();
    for name in ["array", "array_map_range", "core_bool"] {
        let path = dir.join(format!("{name}.emel"));
        if let Some(failure) = compare_wasm(&path, &format!("wasm-{name}")) {
            failures.push(failure);
        }
    }
    report(&failures);
}

/// The pairs of mutation tests: the first file is written as the optimisation's input, the second
/// is the same program with the expansion written by hand.
const PAIRS: &[(&str, &str)] = &[
    ("option_fused.emel", "option_expanded.emel"),
    ("range_fused.emel", "range_expanded.emel"),
    (
        "array_map_range_fused.emel",
        "array_map_range_expanded.emel",
    ),
    ("effectful_fused.emel", "effectful_expanded.emel"),
    ("failing_fused.emel", "failing_expanded.emel"),
    ("with_release_fused.emel", "with_release_expanded.emel"),
    ("inline_fused.emel", "inline_expanded.emel"),
    ("rc_release_fused.emel", "rc_release_expanded.emel"),
];

/// An optimisation must keep the output of the program it rewrites: each form gives the same
/// output with the passes on and off, and the optimised input gives the same output as its hand
/// written expansion.
#[test]
fn mutation_pairs_keep_the_output_of_their_expansion() {
    if !have_cc() {
        return;
    }
    let dir = root().join("crates/emela-driver/tests/programs/opt");
    assert!(dir.join("option_fused.emel").exists());
    let pairs: Vec<(&str, &str)> = PAIRS.to_vec();
    let failures = in_parallel(&pairs, |&(fused, expanded), off, on| {
        let stem = fused.trim_end_matches(".emel");
        let name = format!("opt-{stem}-fused");
        let expanded_name = format!("opt-{stem}-expanded");
        let fused_off = run_native_program(&dir.join(fused), false, &name, off);
        let fused_on = run_native_program(&dir.join(fused), true, &name, on);
        let expanded_off = run_native_program(&dir.join(expanded), false, &expanded_name, off);
        let expanded_on = run_native_program(&dir.join(expanded), true, &expanded_name, on);
        match (fused_off, fused_on, expanded_off, expanded_on) {
            (Ok(fused_off), Ok(fused_on), Ok(expanded_off), Ok(expanded_on))
                if fused_off == fused_on
                    && expanded_off == expanded_on
                    && fused_on == expanded_on =>
            {
                None
            }
            (Err(analysis), _, _, _) => Some(format!(
                "{stem}: the input does not build with --no-opt: {:?}",
                messages(&analysis)
            )),
            (_, Err(analysis), _, _) => Some(format!(
                "{stem}: the input does not build by default: {:?}",
                messages(&analysis)
            )),
            (_, _, Err(analysis), _) => Some(format!(
                "{stem}: the expansion does not build with --no-opt: {:?}",
                messages(&analysis)
            )),
            (_, _, _, Err(analysis)) => Some(format!(
                "{stem}: the expansion does not build by default: {:?}",
                messages(&analysis)
            )),
            (Ok(fused_off), Ok(fused_on), Ok(expanded_off), Ok(expanded_on)) => Some(format!(
                "{stem}: the input with --no-opt gives {fused_off:?}\nthe input by default gives {fused_on:?}\nthe expansion with --no-opt gives {expanded_off:?}\nthe expansion by default gives {expanded_on:?}"
            )),
        }
    });
    report(&failures);
}

/// The law tests of `tests/laws` (the Pome with brix) give the same results with `--no-opt` and by
/// default. Heavy: run with `cargo test -p emela-driver --test opt_diff -- --ignored`.
#[test]
#[ignore = "heavy: the law tests of tests/laws"]
fn law_tests_are_the_same_with_and_without_optimisation() {
    if !have_cc() {
        return;
    }
    let path = root().join("tests/laws");
    let mut off = c_backend(false);
    let mut on = c_backend(true);
    let without = run_tests(&path, false, "laws", &mut off);
    let with = run_tests(&path, true, "laws", &mut on);
    match (without, with) {
        (Ok(without), Ok(with)) if without == with && !without.is_empty() => {}
        (Ok(without), Ok(with)) => {
            panic!("laws: --no-opt gives {without:?}\nthe default gives {with:?}")
        }
        (Err(analysis), _) => panic!("laws do not build with --no-opt: {:?}", messages(&analysis)),
        (_, Err(analysis)) => panic!("laws do not build by default: {:?}", messages(&analysis)),
    }
}

/// Fails with every difference found (written to stderr too, in case the panic message is lost).
fn report(failures: &[String]) {
    if !failures.is_empty() {
        let text = failures.join("\n\n");
        eprintln!("{text}");
        panic!("\n{text}");
    }
}
