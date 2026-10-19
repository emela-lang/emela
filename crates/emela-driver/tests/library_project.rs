//! Library-only directory inputs are checks, not executable builds (#266).

use std::path::{Path, PathBuf};

use emela_driver::{
    Analysis, Backend, BuildOutput, CoreModule, Diagnostic, Input, Lower, MemoryFiles, Output,
    RunOptions, Target, Typecheck, build, check, run,
};

/// Either hook being called for a library-only project is a bug, including `finish` (which may
/// invoke a native compiler). This does not depend on any installed backend tools.
struct NoBackend;

impl Backend<CoreModule> for NoBackend {
    fn emit(&mut self, _: &CoreModule, _: &Analysis) -> Result<BuildOutput, Vec<Diagnostic>> {
        panic!("library-only build must not emit");
    }

    fn finish(&mut self, _: &Path, _: PathBuf) -> Result<PathBuf, Vec<Diagnostic>> {
        panic!("library-only build must not finish");
    }
}

fn options(name: &str) -> RunOptions {
    RunOptions {
        out_dir: std::env::temp_dir().join(format!(
            "emela-library-project-{}-{name}",
            std::process::id()
        )),
        node: "/nonexistent/node".into(),
        args: Vec::new(),
        output: Output::Capture,
    }
}

#[test]
fn directory_library_selection_does_not_change_explicit_file_inputs() {
    let fs = MemoryFiles::new([
        ("app/Pome.toml", ""),
        ("app/src/lib.emel", "pub fn f() -> Int { 1 }"),
        ("app/src/nested/util.emel", ""),
    ]);
    for path in ["app", "app/src", "app/src/nested"] {
        let input = Input::resolve(&fs, Path::new(path)).unwrap();
        assert_eq!(input.entry.as_deref(), Some(Path::new("/app/src/lib.emel")));
        assert!(input.library_only, "{path}");
    }
    let file = Input::resolve(&fs, Path::new("app/src/lib.emel")).unwrap();
    assert!(!file.library_only);
    assert!(!file.single_file);

    let standalone = MemoryFiles::new([("lib.emel", "")]);
    let file = Input::resolve(&standalone, Path::new("lib.emel")).unwrap();
    assert!(!file.library_only);
    assert!(file.single_file);

    let binary = MemoryFiles::new([
        ("app/Pome.toml", ""),
        ("app/src/main.emel", ""),
        ("app/src/lib.emel", ""),
    ]);
    let input = Input::resolve(&binary, Path::new("app")).unwrap();
    assert_eq!(
        input.entry.as_deref(),
        Some(Path::new("/app/src/main.emel"))
    );
    assert!(!input.library_only);
}

#[test]
fn library_build_only_typechecks_and_never_calls_backend_hooks() {
    let fs = MemoryFiles::new([
        ("app/Pome.toml", ""),
        // Showing a capability value is type-correct but not supported by lowering. A check-
        // only build must not lower exports or produce a backend-dependent error.
        // Generic interpolation is supported via monomorphization, so it cannot serve as
        // the unsupported expression here.
        (
            "app/src/lib.emel",
            "import Std.Console.{Console, SystemConsole}\n\npub fn show() -> String {\n  with SystemConsole {\n    c = use Console\n    \"#{c}\"\n  }\n}\n",
        ),
    ]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    for target in [Target::Js, Target::C, Target::Wasm] {
        let options = options(&format!("check-only-{}", target.name()));
        let (analysis, entry) = build(
            &fs,
            &input,
            &mut Lower::default().with_target(target),
            &mut NoBackend,
            &options.out_dir,
        );
        assert!(
            analysis.diagnostics.is_empty(),
            "{:?}",
            analysis.diagnostics
        );
        assert!(entry.is_none());
        assert!(!options.out_dir.exists());
    }

    // Explicit files retain normal build semantics, including lowering diagnostics.
    let file = Input::resolve(&fs, Path::new("app/src/lib.emel")).unwrap();
    let (analysis, _) = build(
        &fs,
        &file,
        &mut Lower::default(),
        &mut NoBackend,
        &options("explicit-lowering").out_dir,
    );
    assert!(analysis.has_errors());
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.message.contains("showing a capability value"))
    );
}

#[test]
fn library_run_is_e0230_even_if_lib_defines_main() {
    for (name, source) in [
        ("no-main", "pub fn f() -> Int { 1 }\n"),
        ("with-main", "fn main() { panic(\"must not run\") }\n"),
        (
            "effectful-main",
            "effect Logger { fn info(msg: String) -> () }\nfn main() { (use Logger).info(\"not an executable boundary\") }\n",
        ),
    ] {
        let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/lib.emel", source)]);
        let input = Input::resolve(&fs, Path::new("app")).unwrap();
        for target in [Target::Js, Target::C, Target::Wasm] {
            let options = options(&format!("run-{name}-{}", target.name()));
            let mut called = 0;
            let (analysis, output) = run(
                &fs,
                &input,
                &mut Lower::default().with_target(target),
                &mut NoBackend,
                &options,
                |_| called += 1,
            );
            assert_eq!(called, 1);
            assert!(output.is_none());
            assert_eq!(analysis.diagnostics.len(), 1);
            assert_eq!(analysis.diagnostics[0].code, Some("E0230"));
            assert_eq!(
                analysis.diagnostics[0].message,
                "entry `src/lib.emel` has no `main` function"
            );
            assert!(!options.out_dir.exists());
        }
    }
}

#[test]
fn library_build_and_run_do_not_hide_type_errors() {
    let fs = MemoryFiles::new([
        ("app/Pome.toml", ""),
        ("app/src/lib.emel", "pub fn f() -> Int { \"wrong\" }\n"),
    ]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let (checked, _) = check(&fs, &input, &mut Typecheck::default());
    let options = options("type-error");
    let (built, entry) = build(
        &fs,
        &input,
        &mut Lower::default(),
        &mut NoBackend,
        &options.out_dir,
    );
    let (ran, output) = run(
        &fs,
        &input,
        &mut Lower::default(),
        &mut NoBackend,
        &options,
        |_| {},
    );
    assert!(entry.is_none());
    assert!(output.is_none());
    assert!(!options.out_dir.exists());
    for analysis in [checked, built, ran] {
        assert_eq!(analysis.diagnostics.len(), 1, "{:?}", analysis.diagnostics);
        assert_eq!(analysis.diagnostics[0].code, Some("E0301"));
    }
}

#[test]
fn projects_with_neither_entry_still_report_e0209() {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/util.emel", "")]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    assert!(input.entry.is_none());
    assert!(!input.library_only);
    let options = options("no-entry");
    let (built, entry) = build(
        &fs,
        &input,
        &mut Lower::default(),
        &mut NoBackend,
        &options.out_dir,
    );
    let (ran, output) = run(
        &fs,
        &input,
        &mut Lower::default(),
        &mut NoBackend,
        &options,
        |_| {},
    );
    assert!(entry.is_none());
    assert!(output.is_none());
    for analysis in [built, ran] {
        assert_eq!(analysis.diagnostics.len(), 1);
        assert_eq!(analysis.diagnostics[0].code, Some("E0209"));
    }
    assert!(!options.out_dir.exists());
}
