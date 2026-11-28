//! Diagnostics of reading a project or a file: the module tree, imports, the bundled core,
//! syntax errors, the license and the experimental settings.

use std::path::Path;

use emela_driver::{
    Analysis, Checked, CoreSource, Diagnostic, FileId, Frontend, Input, LexOnly, Location,
    MemoryFiles, ParseOnly, Parsed, Resolve, SourceDb, SourceFile, Span, Typecheck, check, render,
};
use emela_resolve::{Import, ModuleId, ModuleName};
use emela_types::{InferCtx, Prim, Ty, TyCons};
use line_index::{TextRange, TextSize};

use super::check_files;

#[test]
fn collects_every_broken_file() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", "let x = \"abc\nlet y = $\n"),
            ("app/src/http/client.emel", "let n = 3px\n"),
            ("app/src/http/Server.emel", "let ok = 1\n"),
            ("app/src/My-Lib/util.emel", "let ok = 1\n"),
            ("app/src/json.emel", "let fine = 1\n"),
        ],
        "app",
        &mut LexOnly,
    );
    assert_eq!(analysis.error_count(), 5);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn clean_project_has_no_output() {
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", "let x = 1\n")],
        "app",
        &mut LexOnly,
    );
    assert!(analysis.diagnostics.is_empty());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    assert!(analysis.entry.is_some());
}

#[test]
fn invalid_entry_name() {
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/Main.emel", "")],
        "app/src/Main.emel",
        &mut LexOnly,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// A stand-in parser that lexes and reads only `import A.B` lines, to check import checks
/// through the plug-in point.
struct ImportLines {
    checked: bool,
}

impl Frontend for ImportLines {
    type Program = Vec<ModuleId>;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        // Lexer diagnostics are the frontend's job.
        let mut parsed = LexOnly.parse(module, file, source);
        let mut offset = 0;
        for line in source.text().split_inclusive('\n') {
            if let Some(path) = line.trim_end().strip_prefix("import ") {
                parsed.imports.push(Import {
                    path: ModuleName::parse(path),
                    range: TextRange::at(
                        TextSize::new(offset),
                        TextSize::new(line.trim_end().len() as u32),
                    ),
                });
            }
            offset += line.len() as u32;
        }
        parsed
    }

    fn check(&mut self, _: &Analysis, order: &[ModuleId]) -> Checked<Vec<ModuleId>> {
        self.checked = true;
        Checked {
            program: Some(order.to_vec()),
            diagnostics: Vec::new(),
        }
    }
}

#[test]
fn single_file_sees_only_its_directory() {
    let mut frontend = ImportLines { checked: false };
    let analysis = check_files(
        &[
            (
                "home/src/tools/main.emel",
                "import Util\nimport Deep.Thing\n",
            ),
            ("home/src/tools/util.emel", "let x = $\n"),
            ("home/src/tools/Bad.emel", ""),
            // Subdirectories and ancestors are not looked at.
            ("home/src/tools/deep/thing.emel", "let y = $\n"),
            ("home/src/other.emel", "let z = $\n"),
        ],
        "home/src/tools/main.emel",
        &mut frontend,
    );
    let names: Vec<String> = analysis
        .modules
        .iter()
        .map(|(_, m)| m.name.to_string())
        .collect();
    assert_eq!(names, ["Main", "Util"]);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn import_graph_through_frontend() {
    let mut frontend = ImportLines { checked: false };
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", "import Http.Client\nimport Missing\n"),
            ("app/src/http/client.emel", "import Json\n"),
            ("app/src/json.emel", "import Http.Client\n"),
        ],
        "app",
        &mut frontend,
    );
    // With a cycle there is no dependency order, so the type checker is not called.
    assert!(!frontend.checked);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn check_runs_in_dependency_order() {
    let mut frontend = ImportLines { checked: false };
    let fs = MemoryFiles::new([
        ("Pome.toml", ""),
        ("src/main.emel", "import Json\n"),
        ("src/json.emel", ""),
    ]);
    let input = Input::resolve(&fs, Path::new(".")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut frontend);
    assert!(analysis.diagnostics.is_empty());
    let names: Vec<String> = program
        .unwrap()
        .iter()
        .map(|&id| analysis.module(id).name.to_string())
        .collect();
    assert_eq!(names, ["Json", "Main"]);
}

#[test]
fn type_errors() {
    let mut sources = SourceDb::new();
    let file = sources.add(
        "src/main.emel",
        "let a: Int = \"s\"\nlet b = [1] == [\"s\"]\nlet f = |x| x(x)\n",
    );
    let span = |start: u32, end: u32| Span::new(file, TextRange::new(start.into(), end.into()));
    let cons = TyCons::new();
    let mut ctx = InferCtx::new();
    let int = Ty::Prim(Prim::Int);
    let string = Ty::Prim(Prim::String);

    let top = ctx.unify(&int, &string).unwrap_err();
    let nested = ctx
        .unify(&Ty::list(int.clone()), &Ty::list(string.clone()))
        .unwrap_err();
    let var = ctx.new_var();
    let infinite = ctx
        .unify(&var, &Ty::func([var.clone()], int.clone()))
        .unwrap_err();

    let diagnostics = vec![
        Diagnostic::from_type_error(&top, &cons, span(13, 16)),
        Diagnostic::from_type_error(&nested, &cons, span(25, 36)),
        Diagnostic::from_type_error(&infinite, &cons, span(50, 54)),
    ];
    insta::assert_snapshot!(render(&diagnostics, &sources, false));
}

#[test]
fn labels_warnings_and_locations() {
    let mut sources = SourceDb::new();
    let file = sources.add("src/main.emel", "let x = 1\nlet x = 2\n");
    let diagnostics = vec![
        Diagnostic::warning("`x` shadows an earlier binding")
            .with_span(Span::new(file, TextRange::new(14.into(), 15.into())))
            .with_label(
                Span::new(file, TextRange::new(4.into(), 5.into())),
                "the earlier `x` is here",
            )
            .with_note("rename it, or use the earlier `x`"),
        Diagnostic::error("a problem with the whole file").at(Location::File(file)),
        Diagnostic::error("cannot read directory").at(Location::Path("src/secret".into())),
        Diagnostic::error("`node` not found").with_note("install Node.js"),
    ];
    insta::assert_snapshot!(render(&diagnostics, &sources, false));
}

#[test]
fn color_output_has_escapes() {
    let mut sources = SourceDb::new();
    let file = sources.add("a.emel", "$");
    let diagnostics = [Diagnostic::error("認識できない文字")
        .with_span(Span::new(file, TextRange::new(0.into(), 1.into())))];
    assert!(render(&diagnostics, &sources, true).contains('\x1b'));
    assert!(!render(&diagnostics, &sources, false).contains('\x1b'));
}

#[test]
fn syntax_errors_in_every_file() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import Json\nfn main() {\n  let x = (1,)\n}\n",
            ),
            ("app/src/json.emel", "pub fn parse(s: String -> Int { 1 }\n"),
            ("app/src/http/client.emel", "enum Method {}\nlet n = 3px\n"),
        ],
        "app",
        &mut ParseOnly::new(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn lexical_errors_are_reported_once() {
    let files = [
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "fn main() {\n  \"abc\n}\nconst y = $\n",
        ),
    ];
    // Only lexer diagnostics (E1001 to E1009).
    let codes = |analysis: &Analysis| -> Vec<&'static str> {
        analysis
            .diagnostics
            .iter()
            .filter_map(|d| d.code)
            .filter(|c| *c < "E1101")
            .collect()
    };
    let parsed = check_files(&files, "app", &mut ParseOnly::new());
    assert_eq!(codes(&parsed), ["E1001", "E1005"]);
    let lexed = check_files(&files, "app", &mut LexOnly);
    assert_eq!(codes(&lexed), ["E1001", "E1005"]);
}

#[test]
fn real_imports_are_checked() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import Http.Client.{get, Response}\nimport Missing.Thing\n",
            ),
            ("app/src/http/client.emel", "## 送る\nimport Json\n"),
            ("app/src/json.emel", "import Http.Client.{get}\n"),
            ("app/src/selfish.emel", "import Selfish\n"),
        ],
        "app",
        &mut ParseOnly::new(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn broken_import_is_not_resolved() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import http\nimport Nope.\nimport Json\n",
            ),
            ("app/src/json.emel", ""),
        ],
        "app",
        &mut ParseOnly::new(),
    );
    let codes: Vec<_> = analysis.diagnostics.iter().filter_map(|d| d.code).collect();
    assert_eq!(codes, ["E1101", "E1109"]);
}

#[test]
fn single_file_reads_only_what_the_entry_imports() {
    let mut frontend = ParseOnly::new();
    let analysis = check_files(
        &[
            ("tools/main.emel", "import Util\nfn main() { 1 }\n"),
            ("tools/util.emel", "import Strings\nfn f( { 1 }\n"),
            ("tools/strings.emel", "const s = \"x\n"),
            // Not imported from anywhere: never read, no diagnostics.
            ("tools/scratch.emel", "fn broken( {\nconst z = $\n"),
            ("tools/Bad.emel", ""),
            ("tools/http_2.emel", ""),
            ("tools/http2.emel", ""),
        ],
        "tools/main.emel",
        &mut frontend,
    );
    // The module table is always complete.
    let names: Vec<String> = analysis
        .modules
        .iter()
        .map(|(_, m)| m.name.to_string())
        .collect();
    assert_eq!(names, ["Http2", "Main", "Scratch", "Strings", "Util"]);
    // Only the three reached modules were read and parsed.
    let read: Vec<String> = analysis
        .modules
        .iter()
        .filter(|(id, _)| frontend.parse_of(*id).is_some())
        .map(|(_, m)| m.name.to_string())
        .collect();
    assert_eq!(read, ["Main", "Strings", "Util"]);
    assert_eq!(analysis.files.iter().count(), 3);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn single_file_reports_duplicates_it_reaches() {
    let analysis = check_files(
        &[
            ("tools/main.emel", "import Http2\n"),
            ("tools/http_2.emel", ""),
            ("tools/http2.emel", ""),
        ],
        "tools/main.emel",
        &mut ParseOnly::new(),
    );
    let codes: Vec<_> = analysis.diagnostics.iter().filter_map(|d| d.code).collect();
    assert_eq!(codes, ["E2003"]);
}

#[test]
fn project_reads_everything() {
    let mut frontend = ParseOnly::new();
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", ""),
            ("app/src/unused.emel", "fn f( {\n"),
        ],
        "app/src/main.emel",
        &mut frontend,
    );
    assert_eq!(analysis.files.iter().count(), 2);
    // In a project, diagnostics are reported even for modules the entry does not import.
    let unused = analysis.sources.file_id("/app/src/unused.emel".as_ref());
    assert!(analysis.has_errors());
    assert!(analysis.diagnostics.iter().all(|d| {
        matches!(d.location, Some(Location::Span(span)) if Some(span.file) == unused)
    }));
}

/// A small core used in place of the bundled core sources.
fn test_core() -> Vec<CoreSource> {
    vec![
        CoreSource {
            module: "String".into(),
            text: "@intrinsic\npub fn length(s: String) -> Int\n".into(),
        },
        CoreSource {
            module: "Prelude".into(),
            text: "pub fn identity[A](x: A) -> A { x }\n".into(),
        },
    ]
}

#[test]
fn core_sources_resolve_as_modules() {
    let mut frontend = Resolve::default().with_core(test_core());
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "fn main() -> Int {\n  identity(String.length(\"abc\"))\n}\n\n@intrinsic\nfn mine() -> Int\n",
            ),
            // A source module named like a core module is hidden and gets a reserved name error.
            (
                "app/src/string.emel",
                "pub fn length(s: String) -> Int { 0 }\n",
            ),
        ],
        "app",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn single_file_reads_core_sources() {
    let mut frontend = Resolve::default().with_core(test_core());
    let analysis = check_files(
        &[(
            "home/main.emel",
            "fn main() -> Int { String.length(\"a\") }\n",
        )],
        "home/main.emel",
        &mut frontend,
    );
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    let names: Vec<String> = analysis
        .files
        .iter()
        .map(|(id, _)| analysis.module(id).name.to_string())
        .collect();
    assert!(names.contains(&"String".to_owned()), "{names:?}");
}

#[test]
fn bundled_core_resolves_cleanly() {
    let mut frontend = Resolve::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            "fn main() -> Int {\n  xs = List.map([1, 2], |x| x + 1)\n  n = Option.or(List.head(xs), 0)\n  String.length(\"#{n}\")\n}\n",
        )],
        "home/main.emel",
        &mut frontend,
    );
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

#[test]
fn bundled_core_typechecks_cleanly() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            "fn main() {\n  xs = List.map([1, 2], |x| x + 1)\n  n = Option.or(List.head(xs), 0)\n  String.length(\"#{n}\")\n  ()\n}\n",
        )],
        "home/main.emel",
        &mut frontend,
    );
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

#[test]
fn internal_fns_are_core_only() {
    // `@internal` fns can be called from core modules but not from source modules (E2205), and
    // `@internal` cannot be written in source modules (E2206). The bundled core has no `@internal`
    // fn, so a test core module is added.
    let mut core = CoreSource::bundled();
    core.push(CoreSource {
        module: "Secret".into(),
        text: "@internal\npub fn key() -> Int { 42 }\n\npub fn hint() -> Int { key() - 1 }\n"
            .into(),
    });
    core.push(CoreSource {
        module: "Friend".into(),
        text: "pub fn peek() -> Int { Secret.key() }\n".into(),
    });
    let mut frontend = Typecheck::new(Resolve::default().with_core(core));
    let analysis = check_files(
        &[(
            "home/main.emel",
            "\
fn main() {
  k = Secret.key()
  f = Secret.key
  dbg(Secret.hint() + Friend.peek())
  ()
}

@internal
fn mine() -> Int { 1 }
",
        )],
        "home/main.emel",
        &mut frontend,
    );
    let codes: Vec<_> = analysis.diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, [Some("E2205"), Some("E2205"), Some("E2206")]);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn std_and_core_module_files_are_reserved() {
    // Files named like core modules, and `std.emel` and `std/`, cannot sit directly under the
    // source root (spec 4.1, E2002). Std modules live under `Std`, so `instant.emel` is allowed
    // (spec 15.2).
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import Instant\n\nfn main() { Instant.f() }\n",
            ),
            ("app/src/instant.emel", "pub fn f() -> Int { 1 }\n"),
            ("app/src/list.emel", "pub fn f() -> Int { 1 }\n"),
            ("app/src/std.emel", "pub fn f() -> Int { 1 }\n"),
            ("app/src/std/console.emel", "pub fn f() -> Int { 1 }\n"),
        ],
        "app",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// `[pome.manifest]`: a `[pome].license` that is not an SPDX license expression is W2303 at the
/// value, and the check goes on.
#[test]
fn license_that_is_not_an_spdx_expression() {
    let manifest = |license: &str| {
        format!(
            "[pome]\nname = \"github.com/acme/app\"\nversion = \"0.1.0\"\nemela = \"0.27.0\"\nlicense = {license}\n"
        )
    };
    let check_manifest = |manifest: &str| {
        check_files(
            &[
                ("app/Pome.toml", manifest),
                ("app/src/main.emel", "let x = 1\n"),
            ],
            "app",
            &mut LexOnly,
        )
    };

    let analysis = check_manifest(&manifest("\"MIT/Apache-2.0\""));
    assert_eq!(analysis.error_count(), 0);
    insta::assert_snapshot!(
        "license_not_an_expression",
        render(&analysis.diagnostics, &analysis.sources, false)
    );

    let analysis = check_manifest(&manifest("[\"MIT\", \"Apache-2.0\"]"));
    assert_eq!(analysis.error_count(), 0);
    insta::assert_snapshot!(
        "license_not_a_string",
        render(&analysis.diagnostics, &analysis.sources, false)
    );

    for clean in [
        manifest("\"MIT OR Apache-2.0\""),
        manifest("\"LicenseRef-Acme-Internal\""),
        // Omitted: the Pome declares no license, which is not reported when building.
        "[pome]\nname = \"github.com/acme/app\"\nversion = \"0.1.0\"\nemela = \"0.27.0\"\n".into(),
        // `license` is read only in `[pome]`.
        "[build]\nlicense = 1\n".into(),
        String::new(),
    ] {
        let analysis = check_manifest(&clean);
        assert!(
            analysis.diagnostics.is_empty(),
            "{clean:?}: {:?}",
            analysis.diagnostics
        );
    }
}

/// `[pome.manifest]`: an unknown name in `[experimental]` is W2304 at the key, whatever its value,
/// and a value that is not a boolean is W2305 at the value. The check goes on.
#[test]
fn experimental_settings_of_the_project() {
    let manifest = |table: &str| {
        format!(
            "[pome]\nname = \"github.com/acme/app\"\nversion = \"0.1.0\"\nemela = \"0.29.0\"\n\n[experimental]\n{table}"
        )
    };
    let check_manifest = |manifest: &str| {
        check_files(
            &[
                ("app/Pome.toml", manifest),
                ("app/src/main.emel", "let x = 1\n"),
            ],
            "app",
            &mut LexOnly,
        )
    };

    let analysis = check_manifest(&manifest("from-the-future = true\nalso-unknown = false\n"));
    assert_eq!(analysis.error_count(), 0);
    insta::assert_snapshot!(
        "experimental_unknown_feature",
        render(&analysis.diagnostics, &analysis.sources, false)
    );

    let analysis = check_manifest(&manifest("from-the-future = \"yes\"\n"));
    assert_eq!(analysis.error_count(), 0);
    insta::assert_snapshot!(
        "experimental_not_a_boolean",
        render(&analysis.diagnostics, &analysis.sources, false)
    );

    for clean in [
        manifest(""),
        // `[experimental]` is a table of its own; a key of that name elsewhere is not read.
        "[build]\nexperimental = 1\n".into(),
        // Without a `[pome]` (a project without dependencies may have an empty `Pome.toml`).
        String::new(),
    ] {
        let analysis = check_manifest(&clean);
        assert!(
            analysis.diagnostics.is_empty(),
            "{clean:?}: {:?}",
            analysis.diagnostics
        );
    }
}

/// `[cmd.build]`: a flag `--experimental-<name>` that names no known experimental feature is not
/// an error but W2304, without a location. The same name twice is reported once. In single-file
/// mode the flags are read too.
#[test]
fn experimental_flags() {
    let fs = MemoryFiles::new([
        ("app/Pome.toml", ""),
        ("app/src/main.emel", "let x = 1\n"),
        ("single/main.emel", "let x = 1\n"),
    ]);
    for path in ["app", "single/main.emel"] {
        let mut input = Input::resolve(&fs, Path::new(path)).unwrap();
        input.experimental = vec!["from-the-future".into(), "from-the-future".into()];
        let (analysis, _) = check(&fs, &input, &mut LexOnly);
        assert_eq!(analysis.error_count(), 0, "{path}");
        insta::assert_snapshot!(
            "experimental_unknown_flag",
            render(&analysis.diagnostics, &analysis.sources, false)
        );
    }
}

/// A diagnostic produced by an experimental feature carries the note
/// ``= note: experimental feature `name` `` (`[intro.experimental]`).
#[test]
fn experimental_feature_note() {
    let diagnostic = Diagnostic::warning("something the feature checks")
        .with_code("W0000")
        .from_experimental_feature("some-feature");
    insta::assert_snapshot!(render(&[diagnostic], &SourceDb::new(), false), @r"
    warning[W0000]: something the feature checks
       = note: experimental feature `some-feature`

    1 warning
    ");
}

/// A single file has no `Pome.toml` of its own, so there is no license to check.
#[test]
fn single_file_does_not_read_a_license() {
    let analysis = check_files(
        &[
            ("dir/Pome.toml", "[pome]\nlicense = \"MIT/Apache-2.0\"\n"),
            ("other/main.emel", "let x = 1\n"),
        ],
        "other/main.emel",
        &mut LexOnly,
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
}
