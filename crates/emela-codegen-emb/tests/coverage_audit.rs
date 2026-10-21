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
fn audit_inventory_rejects_missing_extra_duplicated_and_undocumented_entries() {
    let expected = ["First", "Second"].map(str::to_owned).into_iter().collect();
    let complete = "<!-- flow-audit:Example First Second -->\n`Example::First` `Example::Second`";
    inventory::check_document("Example", &expected, complete).unwrap();
    for invalid in [
        complete.replace("First Second -->", "First -->"),
        complete.replace("First Second -->", "First Second Third -->"),
        complete.replace("First Second -->", "First First Second -->"),
        complete.replace("`Example::Second`", ""),
        format!("{complete}\n{complete}"),
    ] {
        assert!(inventory::check_document("Example", &expected, &invalid).is_err());
    }
}

#[test]
fn audit_test_references_must_name_existing_rust_tests() {
    let names = ["existing_test"].map(str::to_owned).into_iter().collect();
    let document =
        "| variant | 試験 |\n| --- | --- |\n| `Value::Const` | `fixture::existing_test` |";
    assert_eq!(
        inventory::check_test_references(document, &names).unwrap(),
        1
    );
    assert!(
        inventory::check_test_references(&document.replace("existing_test", "old_test"), &names)
            .is_err()
    );
    assert!(inventory::check_test_references("no references", &names).is_err());
}
