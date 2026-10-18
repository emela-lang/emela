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
    let outcome = project.add(&f, "github:acme/router@v0.1.0").unwrap();
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
    project.add(&f, "github.com/acme/router").unwrap();
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
    assert!(project.add(&f, "github.com/acme/none@v1.0.0").is_err());
    assert!(
        matches!(project.add(&f, "github.com/acme/router@v2.0.0"), Err(PomeError::Usage(m)) if m.contains("/v2"))
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
