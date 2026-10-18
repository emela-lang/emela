//! Getting Pomes from a fake proxy and checksum database held in memory, and from local git
//! repositories for `direct`.

mod common;

use common::*;
use emela_pome::fetch::transport::{Response, Transport};
use emela_pome::fetch::{FetchError, Fetcher};
use emela_pome::mvs::build_list;

#[test]
fn downloads_from_the_proxy_and_checks_the_checksum_database() {
    let d = dirs();
    let registry = Registry::new();
    for i in 0..300 {
        registry.add(
            &format!("github.com/acme/filler{i}"),
            "v1.0.0",
            &[("Pome.toml", "x")],
            false,
        );
    }
    let x = pome("github.com/acme/x", "v1.2.0", "");
    let files = [
        ("Pome.toml", x.as_str()),
        ("src/lib.emel", "pub fn f() -> Int { 1 }\n"),
    ];
    registry.add("github.com/acme/x", "v1.2.0", &files, true);

    let fetcher = fetcher(&registry, &d.cache, &d.repos);
    let locked = fetcher
        .download(&p("github.com/acme/x"), &v("v1.2.0"))
        .unwrap();
    assert_eq!(locked.commit, "c".repeat(40));
    let dir = fetcher.cache().dir(&p("github.com/acme/x"), &v("v1.2.0"));
    assert_eq!(
        std::fs::read_to_string(dir.join("src/lib.emel")).unwrap(),
        files[1].1
    );
    assert!(
        std::fs::metadata(dir.join("src/lib.emel"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert!(fetcher.cache().checkpoint_file("sum.example.test").exists());

    // Building from the lock uses the cache and does not ask anyone.
    let before = registry.requests.borrow().len();
    assert_eq!(fetcher.ensure(&locked).unwrap(), dir);
    assert_eq!(registry.requests.borrow().len(), before);
    assert_eq!(
        fetcher.rehash(&locked.path, &locked.version).unwrap(),
        Some(locked.hash.clone())
    );
}

#[test]
fn refuses_content_that_differs_from_the_checksum_database() {
    let d = dirs();
    let registry = Registry::new();
    let x = pome("github.com/acme/x", "v1.0.0", "");
    registry.add(
        "github.com/acme/x",
        "v1.0.0",
        &[("Pome.toml", &x), ("src/lib.emel", "good\n")],
        false,
    );
    registry.serve(
        "github.com/acme/x",
        "v1.0.0",
        &[("Pome.toml", &x), ("src/lib.emel", "evil\n")],
    );
    let fetcher = fetcher(&registry, &d.cache, &d.repos);
    let err = fetcher
        .download(&p("github.com/acme/x"), &v("v1.0.0"))
        .unwrap_err();
    assert!(matches!(err, FetchError::Verify(_)), "{err}");
    assert!(
        !fetcher
            .cache()
            .dir(&p("github.com/acme/x"), &v("v1.0.0"))
            .exists()
    );
}

#[test]
fn falls_back_to_direct_when_the_proxy_does_not_have_it() {
    let d = dirs();
    let registry = Registry::new();
    let y = pome("github.com/acme/y", "v0.1.0", "");
    let files: &[(&str, &str)] = &[
        ("Pome.toml", &y),
        ("src/lib.emel", "pub fn g() -> Int { 2 }\n"),
    ];
    git_repo(&d.repos, "github.com/acme/y", &[("v0.1.0", files)]);
    registry.add("github.com/acme/y", "v0.1.0", files, false);

    let fetcher = fetcher(&registry, &d.cache, &d.repos);
    assert_eq!(
        fetcher.versions(&p("github.com/acme/y")).unwrap(),
        [v("v0.1.0")]
    );
    let locked = fetcher
        .download(&p("github.com/acme/y"), &v("v0.1.0"))
        .unwrap();
    assert_eq!(locked.commit.len(), 40);
    assert_eq!(registry.requested(&format!("{SUMDB}/lookup/")), 1);
}

#[test]
fn takes_a_pome_in_a_directory_of_its_repository() {
    let d = dirs();
    let registry = Registry::new();
    let pkg = pome("github.com/acme/mono/pkg", "v1.0.0", "");
    let files: &[(&str, &str)] = &[
        ("Pome.toml", &pkg),
        ("src/lib.emel", "pub fn g() -> Int { 2 }\n"),
    ];
    // The repository has another Pome at its root, and the directory's tags are prefixed.
    let root = pome("github.com/acme/mono", "v3.0.0", "");
    git_repo(
        &d.repos,
        "github.com/acme/mono",
        &[
            ("v1.0.0", &[("Pome.toml", &root)]),
            (
                "pkg/v1.0.0",
                &[("pkg/Pome.toml", &pkg), ("pkg/src/lib.emel", files[1].1)],
            ),
        ],
    );
    registry.add("github.com/acme/mono/pkg", "v1.0.0", files, false);

    let fetcher = fetcher(&registry, &d.cache, &d.repos);
    let path = p("github.com/acme/mono/pkg");
    assert_eq!(fetcher.versions(&path).unwrap(), [v("v1.0.0")]);
    fetcher.download(&path, &v("v1.0.0")).unwrap();
    let dir = fetcher.cache().dir(&path, &v("v1.0.0"));
    assert!(dir.join("src/lib.emel").is_file() && !dir.join("pkg").exists());
}

#[test]
fn takes_private_pomes_directly_without_the_checksum_database() {
    let d = dirs();
    let registry = Registry::new();
    let z = pome("github.com/private/z", "v1.0.0", "");
    git_repo(
        &d.repos,
        "github.com/private/z",
        &[("v1.0.0", &[("Pome.toml", &z)])],
    );
    let fetcher = fetcher(&registry, &d.cache, &d.repos);
    fetcher
        .download(&p("github.com/private/z"), &v("v1.0.0"))
        .unwrap();
    assert!(
        registry.requests.borrow().is_empty(),
        "{:?}",
        registry.requests.borrow()
    );
}

#[test]
fn stops_at_a_proxy_failure_other_than_not_found() {
    let d = dirs();
    let registry = Registry::new();
    registry
        .proxy
        .borrow_mut()
        .insert(format!("{PROXY}/github.com/acme/w/@v/list"), b"x".to_vec());
    struct Failing<'a>(&'a Registry);
    impl Transport for Failing<'_> {
        fn get(&self, url: &str) -> Result<Response, String> {
            if url.starts_with(PROXY) {
                Ok(Response {
                    status: 502,
                    body: b"bad gateway".to_vec(),
                })
            } else {
                self.0.get(url)
            }
        }
    }
    let config = fetcher(&registry, &d.cache, &d.repos).config().clone();
    let fetcher = Fetcher::new(config, Failing(&registry));
    assert!(matches!(
        fetcher.versions(&p("github.com/acme/w")),
        Err(FetchError::Network(_))
    ));
}

#[test]
fn notices_a_checksum_database_that_forked() {
    let d = dirs();
    let x = pome("github.com/acme/x", "v1.0.0", "");
    let files: &[(&str, &str)] = &[("Pome.toml", &x)];

    let honest = Registry::new();
    honest.add("github.com/acme/a", "v1.0.0", &[("Pome.toml", "a")], false);
    honest.add("github.com/acme/x", "v1.0.0", files, true);
    fetcher(&honest, &d.cache, &d.repos)
        .download(&p("github.com/acme/x"), &v("v1.0.0"))
        .unwrap();

    // The same key signs a tree with a different history to someone who has seen the first one.
    let forked = Registry::new();
    forked.add("github.com/acme/b", "v1.0.0", &[("Pome.toml", "b")], false);
    forked.add("github.com/acme/x", "v1.0.0", files, false);
    forked.add("github.com/acme/y", "v1.0.0", files, true);
    let err = fetcher(&forked, &d.cache, &d.repos)
        .download(&p("github.com/acme/y"), &v("v1.0.0"))
        .unwrap_err();
    assert!(
        matches!(&err, FetchError::Verify(m) if m.contains("not consistent")),
        "{err}"
    );
}

#[test]
fn checks_the_lock_when_building() {
    let d = dirs();
    let registry = Registry::new();
    let x = pome("github.com/acme/x", "v1.0.0", "");
    registry.add("github.com/acme/x", "v1.0.0", &[("Pome.toml", &x)], true);
    let fetcher = fetcher(&registry, &d.cache, &d.repos);
    let mut locked = fetcher
        .download(&p("github.com/acme/x"), &v("v1.0.0"))
        .unwrap();
    let other = dirs();
    let fresh = self::fetcher(&registry, &other.cache, &other.repos);
    locked.hash = emela_pome::H1::parse("h1:+Thj6XlK+eABClH7yw779GrFFl/umwEs9PO0k3JjISY=").unwrap();
    assert!(matches!(fresh.ensure(&locked), Err(FetchError::Verify(_))));
}

#[test]
fn selects_versions_over_the_proxy() {
    let d = dirs();
    let registry = Registry::new();
    let add = |name: &str, version: &str, deps: &str| {
        let m = pome(name, version, deps);
        registry.add(name, version, &[("Pome.toml", &m)], true);
    };
    add(
        "github.com/acme/app-dep",
        "v1.0.0",
        "\"github.com/acme/d\" = \"v1.1.0\"\n",
    );
    add("github.com/acme/d", "v1.1.0", "");
    add("github.com/acme/d", "v1.3.0", "");
    add("github.com/acme/d/v2", "v2.0.0", "");
    let fetcher = fetcher(&registry, &d.cache, &d.repos);
    let root = [
        (p("github.com/acme/app-dep"), v("v1.0.0")),
        (p("github.com/acme/d/v2"), v("v2.0.0")),
    ];
    let list = build_list(&root, &mut &fetcher).unwrap();
    let got: Vec<String> = list.iter().map(|(p, v)| format!("{p}@{v}")).collect();
    assert_eq!(
        got,
        [
            "github.com/acme/app-dep@v1.0.0",
            "github.com/acme/d@v1.1.0",
            "github.com/acme/d/v2@v2.0.0"
        ]
    );
    assert_eq!(
        fetcher.versions(&p("github.com/acme/d")).unwrap(),
        [v("v1.1.0"), v("v1.3.0")]
    );
}
