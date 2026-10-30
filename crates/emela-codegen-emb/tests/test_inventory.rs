#[path = "support/test_inventory.rs"]
mod inventory;
use inventory::*;
use std::{collections::BTreeSet, path::Path};

#[test]
fn every_variant_names_existing_enabled_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("inventory/owners.json").is_file())
        .unwrap();
    let owners: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("inventory/owners.json")).unwrap())
            .unwrap();
    let required = owners.as_object().unwrap().keys().cloned().collect();
    load(root, &required).unwrap();
}

fn fixture() -> (tempfile::TempDir, Catalog) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests/nested")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "#[cfg(test)] mod tests { #[test] fn unit() {} }",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tests/probe.rs"),
        "mod nested { include!(\"nested/cases.rs\"); }",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tests/nested/cases.rs"),
        "#[test] fn present() {} #[test] #[ignore] fn ignored() {} fn helper() {}",
    )
    .unwrap();
    let catalog = discover(dir.path()).unwrap();
    (dir, catalog)
}

#[test]
fn discovery_resolves_targets_modules_includes_and_ignore_attributes() {
    let (_dir, catalog) = fixture();
    assert!(catalog.contains_key(&TestRef {
        target: "lib".into(),
        name: "tests::unit".into()
    }));
    assert!(
        !catalog[&TestRef {
            target: "probe".into(),
            name: "nested::present".into()
        }]
            .ignored
    );
    assert!(
        catalog[&TestRef {
            target: "probe".into(),
            name: "nested::ignored".into()
        }]
            .ignored
    );
    assert!(!catalog.keys().any(|test| test.name.ends_with("helper")));
}

#[test]
fn missing_orphan_renamed_duplicate_and_only_ignored_tests_fail() {
    let (_dir, catalog) = fixture();
    let required = BTreeSet::from(["Op::A".into()]);
    let reference = TestRef {
        target: "probe".into(),
        name: "nested::present".into(),
    };
    let valid = Associations::from([("Op::A".into(), vec![reference.clone()])]);
    validate(&required, &valid, &catalog).unwrap();
    for invalid in [
        Associations::new(),
        Associations::from([("Op::B".into(), vec![reference.clone()])]),
        Associations::from([("Op::A".into(), vec![])]),
        Associations::from([("Op::A".into(), vec![reference.clone(), reference.clone()])]),
        Associations::from([(
            "Op::A".into(),
            vec![TestRef {
                target: "lib".into(),
                name: reference.name.clone(),
            }],
        )]),
        Associations::from([(
            "Op::A".into(),
            vec![TestRef {
                target: "probe".into(),
                name: "nested::ignored".into(),
            }],
        )]),
    ] {
        assert!(validate(&required, &invalid, &catalog).is_err());
    }
    let mut renamed = catalog.clone();
    renamed.remove(&reference);
    assert!(
        validate(&required, &valid, &renamed)
            .unwrap_err()
            .contains("missing test")
    );
}

#[test]
fn associations_allow_shared_tests_and_multiple_tests_per_variant() {
    let (_dir, catalog) = fixture();
    let a = TestRef {
        target: "probe".into(),
        name: "nested::present".into(),
    };
    let b = TestRef {
        target: "lib".into(),
        name: "tests::unit".into(),
    };
    validate(
        &BTreeSet::from(["A".into(), "B".into()]),
        &Associations::from([("A".into(), vec![a.clone(), b]), ("B".into(), vec![a])]),
        &catalog,
    )
    .unwrap();
}

#[test]
#[ignore = "writes a source-discovered test catalogue for inventory maintenance"]
fn export_test_catalog() {
    let catalog = discover(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|path| path.join("inventory/owners.json").is_file())
            .unwrap(),
    )
    .unwrap();
    let path = std::env::var("EMELA_TEST_CATALOG").expect("set EMELA_TEST_CATALOG");
    std::fs::write(
        path,
        serde_json::to_string_pretty(&catalog.values().collect::<Vec<_>>()).unwrap(),
    )
    .unwrap();
}
