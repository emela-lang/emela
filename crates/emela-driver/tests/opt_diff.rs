//! The optimisation switch (`--no-opt`) must not change what a program observes: for the same
//! source, stdout, stderr and the exit code are the same with and without the IR optimisation
//! passes. Only memory use and when a program runs out of memory may differ (implementation
//! defined, spec `[effect.handler]`).
//!
//! Three kinds of test:
//!
//! - Every program of `crates/emela-driver/tests/programs/opt/` is built and run both ways on
//!   every target: native C (with the runtime's leak check, `-DEMELA_LEAK_CHECK`, which makes a
//!   leak an observable difference), WASM and JS. The directory is the corpus of effect, fail,
//!   `with` release and fusion shapes; it is enumerated, so a fixture added there is picked up
//!   without editing this file. The WASM and JS tests are skipped with a message when wasmtime or
//!   the JS host is missing. The three codegen programs the WASM test used before are still run.
//!   The switch reaches the Core IR passes on every target, and the LIR passes (SRA, rc cleanup)
//!   on C and WASM, which are the targets that lower through the LIR; the JS backend emits from
//!   Core IR and has no LIR stage.
//! - Mutation tests (T02, so that T11 and T12 have something to check): each `*_fused.emel` is a
//!   program written as an optimisation's input and `*_expanded.emel` is the same program with
//!   the expansion written by hand. Both forms give the same output with optimisation on, which
//!   is what an optimisation must preserve, and the fused form changes when the pass runs.
//! - A corpus of the C codegen programs and the example projects, both ways on native C. By
//!   default a representative subset runs; set `EMELA_OPT_DIFF_FULL=1` to run every codegen
//!   program. The law tests of `tests/laws` are behind `--ignored`.
//!
//! Needs a C compiler (`EMELA_CC` or `cc`), wasmtime (`EMELA_WASMTIME` or `wasmtime`) and a JS
//! host (`EMELA_JS_HOST` or `node`); each target's test is skipped with a message when its
//! toolchain is missing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use emela_driver::{
    Analysis, CoreC, CoreJs, CoreWasm, Input, Lower, LowerTests, OsFs, Output, Reporter,
    RunOptions, RunOutput, Target, TestOptions, TestOutcome, cc_command, js_host_program, run,
    test, wasmtime_program,
};
use emela_pome::fetch::Fetcher;
use emela_pome::fetch::config::Config;
use emela_pome::fetch::transport::Http;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A target the differential harness builds for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Native,
    Wasm,
    Js,
}

impl Platform {
    /// The name in failures and in the output directories.
    fn name(self) -> &'static str {
        match self {
            Platform::Native => "native",
            Platform::Wasm => "wasm",
            Platform::Js => "js",
        }
    }

    /// The build target the platform lowers for.
    fn target(self) -> Target {
        match self {
            Platform::Native => Target::Native,
            Platform::Wasm => Target::Wasm,
            Platform::Js => Target::Js,
        }
    }

    /// Whether the toolchain that runs the output of this target starts; prints why a test is
    /// skipped otherwise.
    fn available(self) -> bool {
        match self {
            Platform::Native => have_cc(),
            Platform::Wasm => have_wasmtime(),
            Platform::Js => have_js_host(),
        }
    }
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

/// Whether the JS host starts; prints why the JS tests are skipped otherwise.
///
/// The version the driver requires (`[target]`) is checked by the build, not here: this only
/// separates "no JS host" from a program that does not build.
fn have_js_host() -> bool {
    let host = js_host_program();
    let ok = Command::new(&host)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!(
            "skipping: cannot start the JS host `{}` (set EMELA_JS_HOST)",
            host.display()
        );
    }
    ok
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-opt-diff-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

type Outcome = (i32, String, String);

/// Builds and runs the program at `path` on `platform`, with the optimisation passes on or off.
///
/// `path` is a source file or a project directory. The result is the program's exit code, stdout
/// and stderr, or the diagnostics that stopped it. `c` is the native backend, kept across
/// programs so its runtime is not built again for every one; the other targets make their
/// backend per program.
fn run_program(
    platform: Platform,
    path: &Path,
    optimize: bool,
    name: &str,
    c: &mut CoreC,
) -> Result<Outcome, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{}-{name}-{}", platform.name(), flag(optimize))),
        js_host: match platform {
            Platform::Wasm => wasmtime_program(),
            _ => js_host_program(),
        },
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default()
        .with_target(platform.target())
        .with_optimize(optimize);
    let (analysis, output) = match platform {
        Platform::Native => run(&OsFs, &input, &mut frontend, c, &options, |_, _| {}),
        Platform::Wasm => run(
            &OsFs,
            &input,
            &mut frontend,
            &mut CoreWasm::default().with_optimize(optimize),
            &options,
            |_, _| {},
        ),
        Platform::Js => run(
            &OsFs,
            &input,
            &mut frontend,
            &mut CoreJs,
            &options,
            |_, _| {},
        ),
    };
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
        js_host: js_host_program(),
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

/// One comparison on `platform`: `None` when the two modes agree.
fn compare_program(
    platform: Platform,
    path: &Path,
    name: &str,
    off: &mut CoreC,
    on: &mut CoreC,
) -> Option<String> {
    let without = run_program(platform, path, false, name, off);
    let with = run_program(platform, path, true, name, on);
    let where_ = format!("{} {name}", platform.name());
    match (without, with) {
        (Ok(without), Ok(with)) if without == with => None,
        (Ok(without), Ok(with)) => Some(format!(
            "{where_}: --no-opt gives {without:?}\nthe default gives {with:?}"
        )),
        (Err(analysis), _) => Some(format!(
            "{where_} does not build with --no-opt: {:?}",
            messages(&analysis)
        )),
        (_, Err(analysis)) => Some(format!(
            "{where_} does not build by default: {:?}",
            messages(&analysis)
        )),
    }
}

/// Calls `f` on every item from as many threads as there are CPUs, each with its own pair of C
/// backends (the native target uses them; the others make their backend per program), and returns
/// the failures in the order of `items`.
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

/// Every program of the differential corpus, fused and expanded forms alike
/// (`crates/emela-driver/tests/programs/opt/*.emel`).
///
/// The directory is read, so a fixture added there is compared without editing this file. The
/// fixtures are effect, fail and `with` release shapes, so this is where those are covered on
/// every target.
fn opt_programs() -> Vec<PathBuf> {
    let dir = opt_dir();
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty(), "no fixtures in {}", dir.display());
    programs
}

fn opt_dir() -> PathBuf {
    root().join("crates/emela-driver/tests/programs/opt")
}

/// The pairs of mutation tests: each `*_fused.emel` written as the optimisation's input with its
/// `*_expanded.emel` hand-written expansion, found by reading the directory.
///
/// Every `.emel` in the directory must belong to a pair: a fixture with no partner would be a
/// parity fixture, not a mutation pair, and does not belong here. The pair is only useful when a
/// pass changes the fused form, which the test comments of each pair record.
fn mutation_pairs() -> Vec<(PathBuf, PathBuf)> {
    let mut pairs: Vec<(PathBuf, PathBuf)> = opt_programs()
        .into_iter()
        .filter(|p| p.file_stem().unwrap().to_string_lossy().ends_with("_fused"))
        .map(|fused| {
            let stem = fused
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .trim_end_matches("_fused")
                .to_owned();
            let expanded = opt_dir().join(format!("{stem}_expanded.emel"));
            assert!(
                expanded.exists(),
                "{} has no `{stem}_expanded.emel` partner",
                fused.display()
            );
            (fused, expanded)
        })
        .collect();
    pairs.sort();
    let paired = pairs.len() * 2;
    assert_eq!(
        opt_programs().len(),
        paired,
        "every fixture in {} must be a `*_fused.emel` with its `*_expanded.emel`",
        opt_dir().display()
    );
    assert!(!pairs.is_empty());
    pairs
}

/// The programs run by default: a small mixed set. `EMELA_OPT_DIFF_FULL=1` runs every program of
/// the corpus instead, which takes long enough to keep out of the usual test run.
const DEFAULT_PROGRAMS: &[&str] = &[
    "array",
    "array_map_range",
    "array_map_range_defect",
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

/// Every program of the differential corpus gives the same stdout, stderr and exit code with
/// `--no-opt` and by default on native C, leak check included.
#[test]
fn opt_fixtures_are_the_same_with_and_without_optimisation_on_native() {
    if !Platform::Native.available() {
        return;
    }
    let programs = opt_programs();
    let failures = in_parallel(&programs, |path, off, on| {
        let name = path.file_stem().unwrap().to_string_lossy();
        compare_program(Platform::Native, path, &name, off, on)
    });
    report(&failures);
}

/// [`opt_fixtures_are_the_same_with_and_without_optimisation_on_native`] on WASM.
#[test]
fn opt_fixtures_are_the_same_with_and_without_optimisation_on_wasm() {
    if !Platform::Wasm.available() {
        return;
    }
    let programs = opt_programs();
    let failures = in_parallel(&programs, |path, off, on| {
        let name = path.file_stem().unwrap().to_string_lossy();
        compare_program(Platform::Wasm, path, &name, off, on)
    });
    report(&failures);
}

/// [`opt_fixtures_are_the_same_with_and_without_optimisation_on_native`] on JS.
///
/// The switch reaches the Core IR passes here. The LIR passes have no JS counterpart: the JS
/// backend emits from Core IR, so there is nothing for `--no-opt` to switch off after lowering.
#[test]
fn opt_fixtures_are_the_same_with_and_without_optimisation_on_js() {
    if !Platform::Js.available() {
        return;
    }
    let programs = opt_programs();
    let failures = in_parallel(&programs, |path, off, on| {
        let name = path.file_stem().unwrap().to_string_lossy();
        compare_program(Platform::Js, path, &name, off, on)
    });
    report(&failures);
}

/// The same codegen program gives the same stdout, stderr and exit code with `--no-opt` and by
/// default. `EMELA_OPT_DIFF_FULL=1` runs the whole corpus.
#[test]
fn codegen_programs_are_the_same_with_and_without_optimisation() {
    if !Platform::Native.available() {
        return;
    }
    let programs = selected_programs();
    assert!(!programs.is_empty());
    let failures = in_parallel(&programs, |path, off, on| {
        let name = path.file_stem().unwrap().to_string_lossy();
        compare_program(Platform::Native, path, &format!("program-{name}"), off, on)
    });
    report(&failures);
}

/// The example projects give the same stdout, stderr and exit code with `--no-opt` and by
/// default.
#[test]
fn examples_are_the_same_with_and_without_optimisation() {
    if !Platform::Native.available() {
        return;
    }
    let examples = root().join("examples");
    let names = ["hello", "errors", "traits"];
    let failures = in_parallel(&names, |name, off, on| {
        compare_program(Platform::Native, &examples.join(name), name, off, on)
    });
    report(&failures);
}

/// The codegen programs the WASM differential test ran before the opt corpus was covered: they
/// still give the same stdout, stderr and exit code on WASM with `--no-opt` and by default.
const WASM_PROGRAMS: &[&str] = &["array", "array_map_range", "core_bool"];

/// [`WASM_PROGRAMS`] are the same on WASM with `--no-opt` and by default.
#[test]
fn wasm_programs_are_the_same_with_and_without_optimisation() {
    if !Platform::Wasm.available() {
        return;
    }
    let dir = root().join("crates/emela-codegen-c/tests/programs");
    let programs: Vec<PathBuf> = WASM_PROGRAMS
        .iter()
        .map(|name| dir.join(format!("{name}.emel")))
        .collect();
    let failures = in_parallel(&programs, |path, off, on| {
        let name = path.file_stem().unwrap().to_string_lossy();
        compare_program(Platform::Wasm, path, &format!("program-{name}"), off, on)
    });
    report(&failures);
}

/// An optimisation must keep the output of the program it rewrites: the four combinations of the
/// two forms (the optimisation's input and its hand-written expansion) and the two modes give the
/// same output, and the optimised input gives the same output as its hand-written expansion.
#[test]
fn mutation_pairs_keep_the_output_of_their_expansion() {
    if !Platform::Native.available() {
        return;
    }
    let pairs = mutation_pairs();
    let failures = in_parallel(&pairs, |(fused, expanded), off, on| {
        let stem = fused
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches("_fused")
            .to_owned();
        let fused_off = run_program(Platform::Native, fused, false, &stem, off);
        let fused_on = run_program(Platform::Native, fused, true, &stem, on);
        let expanded_off = run_program(Platform::Native, expanded, false, &stem, off);
        let expanded_on = run_program(Platform::Native, expanded, true, &stem, on);
        match (fused_off, fused_on, expanded_off, expanded_on) {
            (Ok(fused_off), Ok(fused_on), Ok(expanded_off), Ok(expanded_on))
                if fused_off == fused_on
                    && expanded_off == expanded_on
                    && fused_on == expanded_on =>
            {
                None
            }
            (Err(analysis), _, _, _) => Some(format!(
                "native {stem}: the input does not build with --no-opt: {:?}",
                messages(&analysis)
            )),
            (_, Err(analysis), _, _) => Some(format!(
                "native {stem}: the input does not build by default: {:?}",
                messages(&analysis)
            )),
            (_, _, Err(analysis), _) => Some(format!(
                "native {stem}: the expansion does not build with --no-opt: {:?}",
                messages(&analysis)
            )),
            (_, _, _, Err(analysis)) => Some(format!(
                "native {stem}: the expansion does not build by default: {:?}",
                messages(&analysis)
            )),
            (Ok(fused_off), Ok(fused_on), Ok(expanded_off), Ok(expanded_on)) => Some(format!(
                "native {stem}: the input with --no-opt gives {fused_off:?}\nthe input by default gives {fused_on:?}\nthe expansion with --no-opt gives {expanded_off:?}\nthe expansion by default gives {expanded_on:?}"
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
    if !Platform::Native.available() {
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
