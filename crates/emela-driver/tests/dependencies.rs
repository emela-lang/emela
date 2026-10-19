//! Dependencies (spec 4.7): modules of the Pomes a project depends on, under root names.

use std::path::Path;

use emela_driver::{
    Dependencies, DependencyPome, DependencyRef, Input, MemoryFiles, Target, Typecheck, check,
    check_dependency_targets,
};

// The project depends on quince (github.com/acme/quince); quince depends on http-kit, which it
// calls `Web`. Both the project and quince have a module `Util`.
const APP: &str = "import Quince\nimport Quince.Router\nimport Util\n\nfn main() -> Int { Quince.answer() + Router.route() + Util.one() }\n";

fn files(app: &str, router: &str) -> MemoryFiles {
    MemoryFiles::new([
        ("app/Pome.toml", ""),
        ("app/src/main.emel", app),
        ("app/src/util.emel", "pub fn one() -> Int { 1 }\n"),
        ("deps/quince/Pome.toml", ""),
        (
            "deps/quince/src/lib.emel",
            "import Router\n\npub fn answer() -> Int { Router.route() }\n",
        ),
        ("deps/quince/src/main.emel", "this is not used\n"),
        ("deps/quince/src/router.emel", router),
        ("deps/quince/src/util.emel", "pub fn two() -> Int { 2 }\n"),
        ("deps/http-kit/Pome.toml", ""),
        ("deps/http-kit/src/lib.emel", "pub fn get() -> Int { 3 }\n"),
        (
            "deps/http-kit/src/client.emel",
            "pub fn send() -> Int { 4 }\n",
        ),
    ])
}

const ROUTER: &str = "import Util\nimport Web\nimport Web.Client\n\npub fn route() -> Int { Util.two() + Web.get() + Client.send() }\n";

/// The compiler's own version: a dependency written for it gets no W0203, whatever the
/// workspace version is.
const COMPILER: &str = env!("CARGO_PKG_VERSION");

fn pome(
    name: &str,
    dir: &str,
    module: Option<&str>,
    emela: &str,
    dependencies: Vec<DependencyRef>,
) -> DependencyPome {
    DependencyPome {
        name: name.into(),
        dir: dir.into(),
        last_element: name.rsplit('/').next().unwrap().into(),
        module: module.map(Into::into),
        emela: Some(emela.into()),
        targets: None,
        version: "v1.0.0".into(),
        dependencies,
    }
}

fn dependencies(project_module: Option<&str>, quince_emela: &str) -> Dependencies {
    Dependencies {
        pomes: vec![
            pome(
                "github.com/acme/quince",
                "/deps/quince",
                None,
                quince_emela,
                vec![DependencyRef {
                    pome: 1,
                    module: Some("Web".into()),
                }],
            ),
            pome(
                "github.com/acme/http-kit",
                "/deps/http-kit",
                None,
                COMPILER,
                vec![],
            ),
        ],
        project: vec![DependencyRef {
            pome: 0,
            module: project_module.map(Into::into),
        }],
        dev: Vec::new(),
        manifest: "/app/Pome.toml".into(),
        allow_external: Vec::new(),
    }
}

fn diagnostics(fs: &MemoryFiles, deps: Dependencies) -> Vec<String> {
    let mut input = Input::resolve(fs, Path::new("app")).unwrap();
    input.dependencies = deps;
    let (analysis, _) = check(fs, &input, &mut Typecheck::default());
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{} {}", d.code.unwrap_or("-"), d.message))
        .collect()
}

#[test]
fn imports_dependencies_by_their_root_names() {
    assert_eq!(
        diagnostics(&files(APP, ROUTER), dependencies(None, COMPILER)),
        Vec::<String>::new()
    );
}

#[test]
fn hides_dependencies_of_dependencies() {
    let app = "import Web\nimport HttpKit\n\nfn main() -> Int { 1 }\n";
    let found = diagnostics(&files(app, ROUTER), dependencies(None, COMPILER));
    assert_eq!(
        found,
        [
            "E0205 undefined module `Web`",
            "E0205 undefined module `HttpKit`"
        ]
    );
}

#[test]
fn renames_a_dependency_on_the_depending_side() {
    let app =
        "import Quin\nimport Quin.Router\n\nfn main() -> Int { Quin.answer() + Router.route() }\n";
    assert_eq!(
        diagnostics(&files(app, ROUTER), dependencies(Some("Quin"), COMPILER)),
        Vec::<String>::new()
    );
}

#[test]
fn checks_root_names() {
    let check = |module: &str| {
        diagnostics(
            &files("fn main() -> Int { 1 }\n", ROUTER),
            dependencies(Some(module), COMPILER),
        )
    };
    assert!(
        check("Util")[0].starts_with("E0204 the root name `Util`"),
        "{:?}",
        check("Util")
    );
    assert!(check("Std")[0].starts_with("E0203"), "{:?}", check("Std"));
    assert!(check("List")[0].starts_with("E0203"), "{:?}", check("List"));
    assert!(
        check("lower")[0].starts_with("E0235"),
        "{:?}",
        check("lower")
    );
}

#[test]
fn allows_external_only_when_the_project_says_so() {
    let router = "@external(js, \"./x.mjs\", \"f\")\nfn f() -> Int use {}\n\npub fn route() -> Int { f() }\n";
    let app = "import Quince\n\nfn main() -> Int { Quince.answer() }\n";
    let found = diagnostics(&files(app, router), dependencies(None, COMPILER));
    assert!(
        found.iter().any(|d| d
            .starts_with("E0233 `github.com/acme/quince` uses `@external`, but it is not allowed")),
        "{found:?}"
    );

    let mut allowed = dependencies(None, COMPILER);
    allowed.allow_external = vec!["github.com/acme/quince".into()];
    assert_eq!(
        diagnostics(&files(app, router), allowed.clone()),
        Vec::<String>::new()
    );

    // Allowed, but a `pub` external fn would let the host be reached without an effect.
    let public = "@external(js, \"./x.mjs\", \"f\")\npub fn f() -> Int use {}\n\npub fn route() -> Int { f() }\n";
    let found = diagnostics(&files(app, public), allowed.clone());
    assert!(found.iter().any(|d| d.starts_with("E0237")), "{found:?}");

    // An allowed Pome that is not depended on.
    allowed.allow_external.push("github.com/acme/old".into());
    let found = diagnostics(&files(app, router), allowed);
    assert_eq!(
        found,
        ["W0204 `github.com/acme/old` is allowed in [external] but not depended on"]
    );
}

#[test]
fn checks_the_targets_of_dependencies() {
    let mut deps = dependencies(None, COMPILER);
    deps.pomes[1].targets = Some(vec!["js".into()]);
    assert!(check_dependency_targets(&deps, Target::Js).is_empty());
    let found = check_dependency_targets(&deps, Target::Wasm);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, Some("E0238"));
    assert_eq!(
        found[0].message,
        "`github.com/acme/http-kit` supports only js, not wasm"
    );
    assert_eq!(found[0].notes[0], "required by `github.com/acme/quince`");
}

#[test]
fn compares_the_compiler_version() {
    let found = diagnostics(&files(APP, ROUTER), dependencies(None, "99.0.0"));
    assert!(
        found[0].starts_with("E0234 `github.com/acme/quince` needs emela 99.0.0"),
        "{found:?}"
    );
    let found = diagnostics(&files(APP, ROUTER), dependencies(None, "0.1.0"));
    assert!(found[0].starts_with("W0203"), "{found:?}");
}

// The project has a test root with a helper module; brix is a `[dev-dependencies]` Pome.
fn with_test_root(main: &str, extra: &[(&str, &str)]) -> (MemoryFiles, Dependencies) {
    let mut files = vec![
        ("app/Pome.toml", ""),
        ("app/src/main.emel", main),
        (
            "app/src/sort.emel",
            "pub fn sort(xs: List[Int]) -> List[Int] { xs }\n",
        ),
        (
            "app/test/support.emel",
            "import Brix\n\npub fn sample() -> List[Int] { Brix.sample() }\n",
        ),
        (
            "app/test/sort_test.emel",
            "import Sort\nimport Support\n\n@test\nfn keeps_the_length() {\n  assert List.length(Sort.sort(Support.sample())) == 3\n}\n",
        ),
        ("deps/brix/Pome.toml", ""),
        (
            "deps/brix/src/lib.emel",
            "pub fn sample() -> List[Int] { [3, 1, 2] }\n",
        ),
    ];
    files.extend_from_slice(extra);
    let fs = MemoryFiles::new(files);
    let deps = Dependencies {
        pomes: vec![pome(
            "github.com/emela-lang/brix",
            "/deps/brix",
            None,
            COMPILER,
            vec![],
        )],
        dev: vec![DependencyRef {
            pome: 0,
            module: None,
        }],
        manifest: "/app/Pome.toml".into(),
        ..Dependencies::default()
    };
    (fs, deps)
}

fn test_diagnostics(fs: &MemoryFiles, deps: Dependencies, tests: bool) -> Vec<String> {
    let mut input = Input::resolve(fs, Path::new("app")).unwrap();
    if tests {
        input = input.with_tests(fs);
    }
    input.dependencies = deps;
    let (analysis, _) = check(fs, &input, &mut Typecheck::default());
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{} {}", d.code.unwrap_or("-"), d.message))
        .collect()
}

#[test]
fn the_test_root_sees_the_dev_dependencies() {
    let (fs, deps) = with_test_root(
        "import Sort\n\nfn main() -> Int { List.length(Sort.sort([1])) }\n",
        &[],
    );
    assert_eq!(
        test_diagnostics(&fs, deps.clone(), true),
        Vec::<String>::new()
    );
    // Without the tests (build and run), the test root is not read at all.
    assert_eq!(test_diagnostics(&fs, deps, false), Vec::<String>::new());
}

#[test]
fn the_source_root_cannot_import_test_modules_or_dev_dependencies() {
    let main = "import Brix\nimport Support\n\nfn main() -> Int { 1 }\n";
    let (fs, deps) = with_test_root(main, &[]);
    assert_eq!(
        test_diagnostics(&fs, deps, true),
        [
            "E0239 `Brix` can only be imported under `test/`",
            "E0239 `Support` can only be imported under `test/`",
        ]
    );
}

#[test]
fn a_test_module_shares_the_names_of_the_source_root() {
    let (fs, deps) = with_test_root(
        "fn main() -> Int { 1 }\n",
        &[
            ("app/test/sort.emel", "pub fn other() -> Int { 1 }\n"),
            ("app/test/lib.emel", ""),
        ],
    );
    let found = test_diagnostics(&fs, deps, true);
    assert!(
        found
            .iter()
            .any(|d| d.starts_with("E0204 duplicate module `Sort`"))
            && found.contains(&"E0203 module name `Lib` is reserved".to_string()),
        "{found:?}"
    );
}
