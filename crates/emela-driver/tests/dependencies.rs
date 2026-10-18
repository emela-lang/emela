//! Dependencies (spec 4.7): modules of the Pomes a project depends on, under root names.

use std::path::Path;

use emela_driver::{
    Dependencies, DependencyPome, DependencyRef, Input, MemoryFiles, Typecheck, check,
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
                "0.21.0",
                vec![],
            ),
        ],
        project: vec![DependencyRef {
            pome: 0,
            module: project_module.map(Into::into),
        }],
        manifest: "/app/Pome.toml".into(),
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
        diagnostics(&files(APP, ROUTER), dependencies(None, "0.21.0")),
        Vec::<String>::new()
    );
}

#[test]
fn hides_dependencies_of_dependencies() {
    let app = "import Web\nimport HttpKit\n\nfn main() -> Int { 1 }\n";
    let found = diagnostics(&files(app, ROUTER), dependencies(None, "0.21.0"));
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
        diagnostics(&files(app, ROUTER), dependencies(Some("Quin"), "0.21.0")),
        Vec::<String>::new()
    );
}

#[test]
fn checks_root_names() {
    let check = |module: &str| {
        diagnostics(
            &files("fn main() -> Int { 1 }\n", ROUTER),
            dependencies(Some(module), "0.21.0"),
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
fn rejects_external_in_a_dependency() {
    let router =
        "@external(js, \"./x.mjs\", \"f\")\nfn f() -> Int\n\npub fn route() -> Int { f() }\n";
    let found = diagnostics(
        &files(
            "import Quince\n\nfn main() -> Int { Quince.answer() }\n",
            router,
        ),
        dependencies(None, "0.21.0"),
    );
    assert!(
        found.iter().any(|d| d.starts_with(
            "E0233 `@external` cannot be used in a dependency (`github.com/acme/quince`)"
        )),
        "{found:?}"
    );
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
