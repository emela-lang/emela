//! `emela pome` on a project directory, against the fake proxy and checksum database.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use emela_pome::Lock;
use emela_pome::project::{PomeError, Project};

const APP: &str = "github.com/me/app";

fn app(dir: &Path, deps: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Pome.toml"),
        format!("# My app.\n{}", pome(APP, "v0.1.0", deps)),
    )
    .unwrap();
}

fn lock(dir: &Path) -> Vec<String> {
    let lock = Lock::parse(&fs::read_to_string(dir.join("Pome.lock")).unwrap()).unwrap();
    lock.pomes
        .iter()
        .map(|p| format!("{}@{}", p.path, p.version))
        .collect()
}

// router v0.1.0 and v0.2.0 (which needs d v1.2.0); d v1.1.0, v1.2.0, v1.3.0.
fn registry() -> Registry {
    let r = Registry::new();
    let add = |name: &str, version: &str, deps: &str| {
        let m = pome(name, version, deps);
        r.add(
            name,
            version,
            &[
                ("Pome.toml", &m),
                ("src/lib.emel", "pub fn f() -> Int { 1 }\n"),
            ],
            true,
        );
    };
    add("github.com/acme/d", "v1.1.0", "");
    add("github.com/acme/d", "v1.2.0", "");
    add("github.com/acme/d", "v1.3.0", "");
    add(
        "github.com/acme/router",
        "v0.1.0",
        "\"github.com/acme/d\" = \"v1.1.0\"\n",
    );
    add(
        "github.com/acme/router",
        "v0.2.0",
        "\"github.com/acme/d\" = \"v1.2.0\"\n",
    );
    r.proxy.borrow_mut().insert(
        format!("{PROXY}/github.com/acme/router/@latest"),
        br#"{"version":"v0.2.0","commit":"cccccccccccccccccccccccccccccccccccccccc"}"#.to_vec(),
    );
    r.proxy.borrow_mut().insert(
        format!("{PROXY}/github.com/acme/d/@latest"),
        br#"{"version":"v1.3.0","commit":"cccccccccccccccccccccccccccccccccccccccc"}"#.to_vec(),
    );
    r
}

#[test]
fn add_writes_the_requirement_and_the_lock() {
    let d = dirs();
    let r = registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(&dir, "");

    let mut project = Project::find(&dir.join("src")).unwrap();
    let outcome = project
        .add(&f, "github:acme/router@v0.1.0", false, false)
        .unwrap();
    assert_eq!(
        outcome
            .requirements
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["+ github.com/acme/router v0.1.0"]
    );
    assert_eq!(
        lock(&dir),
        ["github.com/acme/d@v1.1.0", "github.com/acme/router@v0.1.0"]
    );
    let toml = fs::read_to_string(dir.join("Pome.toml")).unwrap();
    assert!(
        toml.starts_with("# My app.\n") && toml.contains("\"github.com/acme/router\" = \"v0.1.0\""),
        "{toml}"
    );

    // Without a version: the latest. The transitive d follows router's requirement.
    project
        .add(&f, "github.com/acme/router", false, false)
        .unwrap();
    assert_eq!(
        lock(&dir),
        ["github.com/acme/d@v1.2.0", "github.com/acme/router@v0.2.0"]
    );
}

#[test]
fn update_raises_and_remove_drops() {
    let d = dirs();
    let r = registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(
        &dir,
        "\"github.com/acme/router\" = \"v0.1.0\"\n\"github.com/acme/d\" = \"v1.1.0\"\n",
    );
    let mut project = Project::find(&dir).unwrap();
    project.install(&f).unwrap();
    assert_eq!(
        lock(&dir),
        ["github.com/acme/d@v1.1.0", "github.com/acme/router@v0.1.0"]
    );

    let outcome = project.update(&f, Some("github.com/acme/router")).unwrap();
    assert_eq!(
        outcome
            .build_list
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "  github.com/acme/d v1.1.0 -> v1.2.0",
            "  github.com/acme/router v0.1.0 -> v0.2.0"
        ]
    );
    project.update(&f, None).unwrap();
    assert_eq!(
        lock(&dir),
        ["github.com/acme/d@v1.3.0", "github.com/acme/router@v0.2.0"]
    );

    project.remove(&f, "github.com/acme/router").unwrap();
    assert_eq!(lock(&dir), ["github.com/acme/d@v1.3.0"]);
    assert!(matches!(
        project.remove(&f, "github.com/acme/router"),
        Err(PomeError::Usage(_))
    ));
}

#[test]
fn a_failed_add_changes_nothing() {
    let d = dirs();
    let r = registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(&dir, "");
    let before = fs::read_to_string(dir.join("Pome.toml")).unwrap();
    let mut project = Project::find(&dir).unwrap();
    assert!(
        project
            .add(&f, "github.com/acme/none@v1.0.0", false, false)
            .is_err()
    );
    assert!(
        matches!(project.add(&f, "github.com/acme/router@v2.0.0", false, false), Err(PomeError::Usage(m)) if m.contains("/v2"))
    );
    assert_eq!(fs::read_to_string(dir.join("Pome.toml")).unwrap(), before);
    assert!(!dir.join("Pome.lock").exists());
}

#[test]
fn replace_uses_the_local_directory_and_leaves_it_out_of_the_lock() {
    let d = dirs();
    let r = registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    let local = d.repos.join("router");
    fs::create_dir_all(&local).unwrap();
    fs::write(
        local.join("Pome.toml"),
        pome(
            "github.com/acme/router",
            "v0.3.0",
            "\"github.com/acme/d\" = \"v1.3.0\"\n",
        ),
    )
    .unwrap();
    app(
        &dir,
        "\"github.com/acme/router\" = \"v0.1.0\"\n\n[replace]\n\"github.com/acme/router\" = { path = \"../router\" }\n",
    );

    let mut project = Project::find(&dir).unwrap();
    project.install(&f).unwrap();
    assert_eq!(lock(&dir), ["github.com/acme/d@v1.3.0"]);
    let tree = project.list(&f).unwrap();
    assert!(
        tree.contains("└── github.com/acme/router v0.1.0 => ../router"),
        "{tree}"
    );
}

#[test]
fn list_shows_the_tree_and_verify_finds_changed_files() {
    let d = dirs();
    let r = registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(
        &dir,
        "\"github.com/acme/router\" = \"v0.1.0\"\n\"github.com/acme/d\" = \"v1.2.0\"\n",
    );
    let mut project = Project::find(&dir).unwrap();
    project.install(&f).unwrap();
    assert_eq!(
        project.list(&f).unwrap(),
        "github.com/me/app\n\
         ├── github.com/acme/d v1.2.0\n\
         └── github.com/acme/router v0.1.0\n    \
             └── github.com/acme/d v1.1.0 -> v1.2.0\n"
    );

    assert!(project.verify(&f).unwrap().is_empty());
    let file = f
        .cache()
        .dir(&p("github.com/acme/d"), &v("v1.2.0"))
        .join("src/lib.emel");
    let _ = std::process::Command::new("chmod")
        .args(["-R", "u+w"])
        .arg(f.cache().root())
        .status();
    fs::write(&file, "tampered\n").unwrap();
    let problems = project.verify(&f).unwrap();
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0].starts_with("github.com/acme/d v1.2.0"),
        "{problems:?}"
    );
}

#[test]
fn install_reuses_the_lock_without_asking_the_checksum_database() {
    let d = dirs();
    let r = registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(&dir, "\"github.com/acme/d\" = \"v1.1.0\"\n");
    Project::find(&dir).unwrap().install(&f).unwrap();

    // Another machine with the same lock and an empty cache.
    let other = dirs();
    let f2 = fetcher(&r, &other.cache, &other.repos);
    let lookups = r.requested(&format!("{SUMDB}/lookup/"));
    let outcome = Project::find(&dir).unwrap().install(&f2).unwrap();
    assert!(outcome.build_list.is_empty());
    assert_eq!(r.requested(&format!("{SUMDB}/lookup/")), lookups);
}

#[test]
fn needs_a_project() {
    let d = dirs();
    fs::create_dir_all(&d.repos).unwrap();
    assert!(matches!(
        Project::find(&d.repos),
        Err(PomeError::NoProject(_))
    ));
}

// cf uses `@external` (v0.2.0 in one more file); uses-cf depends on cf and has none itself.
fn external_registry() -> Registry {
    let r = registry();
    let kv = "@external(js, \"./kv.mjs\", \"get\")\nfn get() -> Int\n";
    let cf1 = pome("github.com/acme/cf", "v0.1.0", "");
    r.add(
        "github.com/acme/cf",
        "v0.1.0",
        &[
            ("Pome.toml", &cf1),
            ("src/kv.emel", kv),
            ("src/kv.mjs", "export const get = () => 1;\n"),
        ],
        true,
    );
    let cf2 = pome("github.com/acme/cf", "v0.2.0", "");
    r.add(
        "github.com/acme/cf",
        "v0.2.0",
        &[
            ("Pome.toml", &cf2),
            ("src/kv.emel", kv),
            ("src/d1.emel", kv),
            ("src/kv.mjs", ""),
        ],
        true,
    );
    r.proxy.borrow_mut().insert(
        format!("{PROXY}/github.com/acme/cf/@latest"),
        br#"{"version":"v0.2.0","commit":"cccccccccccccccccccccccccccccccccccccccc"}"#.to_vec(),
    );
    let uses = pome(
        "github.com/acme/uses-cf",
        "v1.0.0",
        "\"github.com/acme/cf\" = \"v0.1.0\"\n",
    );
    r.add(
        "github.com/acme/uses-cf",
        "v1.0.0",
        &[
            ("Pome.toml", &uses),
            ("src/lib.emel", "pub fn f() -> Int { 1 }\n"),
        ],
        true,
    );
    r
}

#[test]
fn add_stops_at_external_until_allowed() {
    let d = dirs();
    let r = external_registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(&dir, "");
    let before = fs::read_to_string(dir.join("Pome.toml")).unwrap();
    let mut project = Project::find(&dir).unwrap();

    let err = project
        .add(&f, "github.com/acme/cf@v0.1.0", false, false)
        .unwrap_err();
    assert!(
        matches!(&err, PomeError::External(m) if m.contains("github.com/acme/cf v0.1.0 uses @external:\n  src/kv.emel\n")),
        "{err}"
    );
    assert_eq!(fs::read_to_string(dir.join("Pome.toml")).unwrap(), before);
    assert!(!dir.join("Pome.lock").exists());

    let outcome = project
        .add(&f, "github.com/acme/cf@v0.1.0", true, false)
        .unwrap();
    assert_eq!(outcome.allowed, [p("github.com/acme/cf")]);
    let toml = fs::read_to_string(dir.join("Pome.toml")).unwrap();
    assert!(
        toml.contains("[external]\nallow = [\"github.com/acme/cf\"]"),
        "{toml}"
    );
    let lock = fs::read_to_string(dir.join("Pome.lock")).unwrap();
    assert!(lock.contains("external = [\"src/kv.emel\"]"), "{lock}");

    // A new version reaching the host in one more file is reported, not refused.
    let outcome = project.update(&f, None).unwrap();
    assert_eq!(outcome.external.len(), 1);
    assert_eq!(
        (
            outcome.external[0].added.clone(),
            outcome.external[0].removed.len()
        ),
        (vec!["src/d1.emel".to_string()], 0)
    );
}

#[test]
fn allow_external_covers_the_pomes_an_add_brings_in() {
    let d = dirs();
    let r = external_registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(&dir, "");
    let mut project = Project::find(&dir).unwrap();
    assert!(matches!(
        project.add(&f, "github.com/acme/uses-cf@v1.0.0", false, false),
        Err(PomeError::External(_))
    ));
    let outcome = project
        .add(&f, "github.com/acme/uses-cf@v1.0.0", true, false)
        .unwrap();
    // uses-cf has no `@external` of its own; cf, which it brings in, is allowed.
    assert_eq!(outcome.allowed, [p("github.com/acme/cf")]);
}

#[test]
fn install_refuses_external_that_is_not_allowed() {
    let d = dirs();
    let r = external_registry();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(&dir, "\"github.com/acme/cf\" = \"v0.1.0\"\n");
    let err = Project::find(&dir).unwrap().install(&f).unwrap_err();
    assert!(
        matches!(&err, PomeError::External(m) if m.contains("--allow-external github.com/acme/cf")),
        "{err}"
    );
}

// brix v0.2.0 (only for tests) needs d v1.3.0 and has its own dev-dependency, which is never got.
fn registry_with_brix() -> Registry {
    let r = registry();
    let m = format!(
        "{}\n[dev-dependencies]\n\"github.com/acme/nowhere\" = \"v1.0.0\"\n",
        pome(
            "github.com/emela-lang/brix",
            "v0.2.0",
            "\"github.com/acme/d\" = \"v1.3.0\"\n\"github.com/acme/only-tests\" = \"v1.0.0\"\n"
        )
    );
    r.add(
        "github.com/emela-lang/brix",
        "v0.2.0",
        &[
            ("Pome.toml", &m),
            ("src/lib.emel", "pub fn f() -> Int { 1 }\n"),
        ],
        true,
    );
    let m = pome("github.com/acme/only-tests", "v1.0.0", "");
    r.add(
        "github.com/acme/only-tests",
        "v1.0.0",
        &[
            ("Pome.toml", &m),
            ("src/lib.emel", "pub fn f() -> Int { 1 }\n"),
        ],
        true,
    );
    r
}

fn dev_lock(dir: &Path) -> Vec<String> {
    let lock = Lock::parse(&fs::read_to_string(dir.join("Pome.lock")).unwrap()).unwrap();
    lock.pomes
        .iter()
        .map(|p| {
            format!(
                "{}@{}{}",
                p.path,
                p.version,
                if p.dev { " dev" } else { "" }
            )
        })
        .collect()
}

#[test]
fn dev_dependencies_share_the_selection_and_are_marked() {
    let d = dirs();
    let r = registry_with_brix();
    let f = fetcher(&r, &d.cache, &d.repos);
    let dir = d.repos.join("app");
    app(&dir, "\"github.com/acme/router\" = \"v0.1.0\"\n");
    let mut project = Project::find(&dir).unwrap();
    let outcome = project
        .add(&f, "github.com/emela-lang/brix@v0.2.0", false, true)
        .unwrap();
    assert_eq!(
        outcome
            .requirements
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["+ github.com/emela-lang/brix v0.2.0 (dev)"]
    );
    // brix raises d for the build too; only brix and what only it needs are marked.
    assert_eq!(
        dev_lock(&dir),
        [
            "github.com/acme/d@v1.3.0",
            "github.com/acme/only-tests@v1.0.0 dev",
            "github.com/acme/router@v0.1.0",
            "github.com/emela-lang/brix@v0.2.0 dev",
        ]
    );
    let toml = fs::read_to_string(dir.join("Pome.toml")).unwrap();
    assert!(
        toml.ends_with("[dev-dependencies]\n\"github.com/emela-lang/brix\" = \"v0.2.0\"\n"),
        "{toml}"
    );
    assert!(
        project
            .list(&f)
            .unwrap()
            .contains("github.com/emela-lang/brix v0.2.0 (dev)")
    );

    assert!(matches!(
        project.add(&f, "github.com/acme/router@v0.1.0", false, true),
        Err(PomeError::Usage(m)) if m.contains("[dependencies]")
    ));

    // Without --dev, it moves to [dependencies].
    project
        .add(&f, "github.com/emela-lang/brix@v0.2.0", false, false)
        .unwrap();
    let toml = fs::read_to_string(dir.join("Pome.toml")).unwrap();
    assert!(
        toml.ends_with("\"github.com/emela-lang/brix\" = \"v0.2.0\"\n")
            && !toml.contains("dev-dependencies"),
        "{toml}"
    );
    assert_eq!(
        dev_lock(&dir),
        [
            "github.com/acme/d@v1.3.0",
            "github.com/acme/only-tests@v1.0.0",
            "github.com/acme/router@v0.1.0",
            "github.com/emela-lang/brix@v0.2.0",
        ]
    );

    project.remove(&f, "github.com/emela-lang/brix").unwrap();
    assert_eq!(
        dev_lock(&dir),
        ["github.com/acme/d@v1.1.0", "github.com/acme/router@v0.1.0"]
    );
}
