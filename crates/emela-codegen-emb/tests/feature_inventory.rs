#[allow(dead_code)]
#[path = "../examples/support/feature_inventory.rs"]
mod inventory;

use std::fs;
use std::path::Path;

use inventory::{Scope, collect, extract, reports, save};

fn fixture_scope() -> Scope {
    Scope::new("fixture.rs", "fixture", None)
}

#[test]
fn extracts_payloads_attributes_docs_and_fields_without_lexical_false_positives() {
    let source = r###"
        // pub enum Wrong { Fake }
        /* nested /* enum Wrong { Fake } */ comment */
        pub enum Value {
            /// Call with `args`.
            #[cfg(any(feature = "hidden", test))]
            Call {
                /// Nested tuple and array types.
                args: Vec<(u8, [u8; 2])>,
                callback: for<'a> fn(&'a str) -> Option<u8>,
            },
            #[doc = r#"a | b, "quoted", { braces }"#]
            Pair(&'static str, u32),
            Constant = { let _ = r#"enum NotAType { Wrong }"#; 3 },
        }
        pub struct Function { pub suspends: bool }
        pub type Id = Option<Function>;
        enum Private { Ignored }
    "###;
    let entries = extract(&fixture_scope(), source).unwrap();
    let find = |id: &str| entries.iter().find(|i| i.id == id).unwrap();
    let variants: Vec<_> = entries
        .iter()
        .filter(|i| i.kind == "variant")
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(
        variants,
        [
            "fixture::Value::Call",
            "fixture::Value::Pair",
            "fixture::Value::Constant"
        ]
    );
    assert_eq!(find("fixture::Value::Call").docs, "Call with `args`.");
    assert!(find("fixture::Value::Call").attributes[0].contains("cfg"));
    assert!(
        find("fixture::Value::Call.args")
            .declaration
            .contains("[u8 ; 2]")
    );
    assert_eq!(
        find("fixture::Value::Call.args").parent.as_deref(),
        Some("fixture::Value::Call")
    );
    assert_eq!(find("fixture::Value::Pair.1").kind, "field");
    assert_eq!(find("fixture::Function.suspends").kind, "field");
    assert_eq!(find("fixture::Id").kind, "type_alias");
    assert!(
        !entries
            .iter()
            .any(|i| i.id.contains("Wrong") || i.id.contains("Private"))
    );
    let md = inventory::markdown(
        &inventory::Inventory {
            schema_version: 1,
            inventory_kind: "definitions",
            scope: vec![],
            sources: vec![],
            items: entries,
        },
        Path::new("/repo"),
        Path::new("/repo/docs"),
    );
    assert!(md.contains("a &#124; b"));
    assert!(md.contains("../fixture.rs#L"));
}

#[test]
fn new_declarations_and_fields_are_discovered_and_shape_changes_keep_their_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.rs");
    fs::write(&path, "pub enum Value { Call { suspends: bool } }").unwrap();
    let before = collect(dir.path(), vec![fixture_scope()]).unwrap();
    fs::write(
        &path,
        "pub enum Value { Call { suspends: u8, added: bool } } pub enum NewType { Added }",
    )
    .unwrap();
    let after = collect(dir.path(), vec![fixture_scope()]).unwrap();
    assert!(
        after
            .items
            .iter()
            .any(|i| i.id == "fixture::NewType::Added")
    );
    assert!(
        after
            .items
            .iter()
            .any(|i| i.id == "fixture::Value::Call.added")
    );
    for id in [
        "fixture::Value",
        "fixture::Value::Call",
        "fixture::Value::Call.suspends",
    ] {
        let a = before.items.iter().find(|i| i.id == id).unwrap();
        let b = after.items.iter().find(|i| i.id == id).unwrap();
        assert_ne!(a.definition_sha256, b.definition_sha256, "{id}");
    }
}

#[test]
fn fingerprints_ignore_comments_and_locations_but_keep_conditional_attributes() {
    let a = extract(
        &fixture_scope(),
        "/// old\npub enum E { /// old field\n A { x: u8 } }",
    )
    .unwrap();
    let b = extract(
        &fixture_scope(),
        "\n\n/// new\npub enum E { /// new field\n A { x: u8 } }",
    )
    .unwrap();
    assert_eq!(
        a.iter().map(|i| &i.definition_sha256).collect::<Vec<_>>(),
        b.iter().map(|i| &i.definition_sha256).collect::<Vec<_>>()
    );
    assert_ne!(a[0].source.line, b[0].source.line);
    let conditional = extract(
        &fixture_scope(),
        "pub enum E { #[cfg(feature = \"a\")] A { x: u8 } }",
    )
    .unwrap();
    assert_ne!(a[1].definition_sha256, conditional[1].definition_sha256);
}

#[test]
fn missing_selections_ambiguous_ids_and_unsupported_syntax_fail_explicitly() {
    let scope = Scope::new("fixture.rs", "fixture", Some(&["Missing"]));
    assert!(
        extract(&scope, "pub enum E { A }")
            .unwrap_err()
            .contains("Missing")
    );
    for source in [
        "pub enum E {",
        "pub union U { a: u32 }",
        "mod nested { pub enum E { A } }",
        "generate_types!();",
        "#[doc = include_str!(\"docs.md\")] pub enum E { A }",
        "#[cfg(a)] pub enum E { A } #[cfg(b)] pub enum E { B }",
    ] {
        assert!(extract(&fixture_scope(), source).is_err(), "{source}");
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("fixture.rs"), "pub enum E { A }").unwrap();
    assert!(collect(dir.path(), vec![fixture_scope(), fixture_scope()]).is_err());
}

#[test]
fn check_detects_stale_and_missing_files_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("not-created");
    let reports = vec![
        ("a.md", "expected\n".to_owned()),
        ("a.json", "{}\n".to_owned()),
    ];
    assert!(save(&reports, &missing, true).is_err());
    assert!(!missing.exists());
    save(&reports, dir.path(), false).unwrap();
    save(&reports, dir.path(), true).unwrap();
    fs::write(dir.path().join("a.md"), "changed").unwrap();
    fs::remove_file(dir.path().join("a.json")).unwrap();
    assert!(save(&reports, dir.path(), true).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.md")).unwrap(),
        "changed"
    );
    assert!(!dir.path().join("a.json").exists());
}

#[test]
fn checked_in_inventory_matches_the_declared_source_scope() {
    let root = inventory::repository_root();
    let out_dir = root.join("crates/emela-codegen-emb/docs/generated");
    let inventory = collect(root, inventory::default_scope()).unwrap();
    // The JSON distinguishes declaration presence from stage legality or implementation claims.
    let value = serde_json::to_value(&inventory).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["inventory_kind"], "definitions");
    assert!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.get("support").is_none())
    );
    save(
        &reports(&inventory, root, &out_dir).unwrap(),
        &out_dir,
        true,
    )
    .unwrap();
}
