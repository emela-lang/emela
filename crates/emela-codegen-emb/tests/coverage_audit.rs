#[path = "../examples/support/flow_audit_inventory.rs"]
mod inventory;

#[test]
fn audit_inventory_matches_current_flow_and_builtin_enum_definitions() {
    inventory::check(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
}

#[test]
fn audit_parser_ignores_comments_attributes_and_nested_payloads() {
    let source = r#"
// enum Example { Wrong }
pub enum Example {
    /// Ignore documentation, commas, and { braces }.
    #[cfg(any(feature = "hidden", test))]
    First { fields: Vec<(i32, i32)>, callback: fn() },
    /* Nested /* comment */ remains a comment. */
    Second(&'static [u8; 2]),
    Third = 3,
}
"#;
    let expected = ["First", "Second", "Third"]
        .map(str::to_owned)
        .into_iter()
        .collect();
    assert_eq!(
        inventory::enum_variants(source, "Example").unwrap(),
        expected
    );
}

#[test]
fn audit_inventory_rejects_missing_extra_duplicated_and_malformed_entries() {
    let expected = ["First", "Second"].map(str::to_owned).into_iter().collect();
    let entry = |name: &str| {
        serde_json::json!({
            "id": format!("fixture::Example::{name}"),
            "kind": "variant",
            "parent": "fixture::Example"
        })
    };
    let complete = serde_json::json!({"items": [entry("First"), entry("Second")]});
    inventory::check_inventory("fixture::Example", &expected, &complete).unwrap();
    let mut wrong_parent = complete.clone();
    wrong_parent["items"][1]["parent"] = "other::Example".into();
    let mut malformed_id = complete.clone();
    malformed_id["items"][1]["id"] = "other::Example::Second".into();
    for invalid in [
        serde_json::json!({"items": [entry("First")]}),
        serde_json::json!({"items": [entry("First"), entry("Second"), entry("Third")]}),
        serde_json::json!({"items": [entry("First"), entry("First"), entry("Second")]}),
        serde_json::json!({"items": [entry("First"), entry("")]}),
        serde_json::json!({}),
        wrong_parent,
        malformed_id,
    ] {
        assert!(inventory::check_inventory("fixture::Example", &expected, &invalid).is_err());
    }
}
