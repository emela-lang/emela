#[allow(dead_code)]
#[path = "../examples/support/feature_inventory.rs"]
mod definitions;
#[allow(dead_code)]
#[path = "../examples/support/implementation_inventory.rs"]
mod implementation;

use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn route() -> Value {
    json!({"id":"dispatch", "path":"backend.rs", "symbol":"dispatch", "owner":"fixture::Op", "selector":"match", "expression":"op", "contexts":["sync"], "role":"emit", "note":"test fixture"})
}

fn fixture(ir: &str, backend: &str, route: Value, exclusions: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ir.rs"), ir).unwrap();
    fs::write(dir.path().join("backend.rs"), backend).unwrap();
    let config = json!({"schema_version":1,"targets":[{"owner":"fixture::Op","contexts":["sync","suspend"]}],"routes":[route],"exclusions":exclusions});
    fs::write(
        dir.path().join("routes.json"),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    dir
}

fn collect(
    dir: &Path,
) -> (
    implementation::ImplementationInventory,
    implementation::Coverage,
) {
    let definitions =
        definitions::collect(dir, vec![definitions::Scope::new("ir.rs", "fixture", None)]).unwrap();
    implementation::collect(dir, Path::new("routes.json"), &definitions).unwrap()
}

#[test]
fn extracts_only_selected_dispatch_and_preserves_guards_and_calls() {
    let mut selected = route();
    selected["binding"] = json!("expression");
    let source = r#"
        fn dispatch(op: Op) {
            let arity = match op { Op::A => 1, Op::B => 2, _ => 0 };
            let expression = match op {
                Op::A if ready() => { emit_a(); Op::B },
                other => unsupported(other),
            };
            let text = "Op::B"; // Op::B is not an implementation arm.
        }
    "#;
    let selected = serde_json::from_value(selected).unwrap();
    let found = implementation::extract(&selected, source).unwrap();
    assert_eq!(
        found.items.keys().map(String::as_str).collect::<Vec<_>>(),
        ["fixture::Op::A"]
    );
    let evidence = &found.items["fixture::Op::A"][0];
    assert!(evidence.calls.iter().any(|c| c == "emit_a"));
    assert_eq!(evidence.condition.as_deref(), Some("ready ()"));
    assert_eq!(found.fallbacks.len(), 1);
    assert_eq!(found.fallbacks[0].pattern, "other");
}

#[test]
fn wildcard_and_other_context_do_not_cover_new_variants() {
    let dir = fixture(
        "pub enum Op { A, B }",
        "fn dispatch(op: Op) { match op { Op::A => emit(), _ => ignore() } }",
        route(),
        json!([]),
    );
    let (_, before) = collect(dir.path());
    assert_eq!(before.summary["candidate_found"], 1);
    assert_eq!(before.summary["missing_evidence"], 3);
    fs::write(dir.path().join("ir.rs"), "pub enum Op { A, B, Added }").unwrap();
    let (_, after) = collect(dir.path());
    let added = after
        .items
        .iter()
        .find(|i| i.id == "fixture::Op::Added")
        .unwrap();
    assert!(
        added
            .contexts
            .iter()
            .all(|c| c.status == "missing_evidence")
    );
    assert_eq!(after.summary["missing_evidence"], 5);
    assert!(implementation::require_evidence(&after).is_err());
}

#[test]
fn newly_added_primary_enums_are_flagged_instead_of_silently_excluded() {
    let mut selected = route();
    selected["contexts"] = json!(["sync", "suspend"]);
    let dir = fixture(
        "pub enum Op { A }",
        "fn dispatch(op: Op) { match op { Op::A => emit() } }",
        selected,
        json!([]),
    );
    let (_, before) = collect(dir.path());
    implementation::require_evidence(&before).unwrap();
    fs::write(
        dir.path().join("ir.rs"),
        "pub enum Op { A } pub enum NewControl { Added }",
    )
    .unwrap();
    let (_, after) = collect(dir.path());
    assert_eq!(after.unconfigured_enums, ["fixture::NewControl"]);
    assert!(
        implementation::require_evidence(&after)
            .unwrap_err()
            .contains("unconfigured enum")
    );
}

#[test]
fn direct_errors_are_not_generation_and_exclusions_require_a_rejection_route() {
    let dir = fixture(
        "pub enum Op { A }",
        "fn dispatch(op: Op) { match op { Op::A => Err(problem()) } }",
        route(),
        json!([]),
    );
    let (implementation, coverage) = collect(dir.path());
    assert!(implementation.items[0].evidence[0].error_only);
    assert_eq!(coverage.summary["missing_evidence"], 2);
    let mut reject = route();
    reject["role"] = json!("reject");
    reject["contexts"] = json!(["input"]);
    let config = json!({"schema_version":1,"targets":[{"owner":"fixture::Op","contexts":["sync","suspend"]}],"routes":[reject],"exclusions":[{"id":"fixture::Op::A","route":"dispatch","reason":"invalid Flow input in fixture"}]});
    fs::write(dir.path().join("routes.json"), config.to_string()).unwrap();
    let (_, coverage) = collect(dir.path());
    assert_eq!(coverage.items[0].flow_input, "forbidden_by_config");
    assert_eq!(coverage.items[0].contexts.len(), 1);
    assert_eq!(coverage.items[0].contexts[0].status, "rejection_evidence");
    implementation::require_evidence(&coverage).unwrap();
}

#[test]
fn if_conditions_and_let_else_are_candidates_but_negative_conditions_are_not_inverted() {
    let mut selected = route();
    selected["selector"] = json!("if");
    let source = "fn dispatch(op: Op) { if matches!(op, Op::A | Op::B) { emit(); } if op != Op::C { return Err(unsupported()); } let text = \"Op::C\"; }";
    let found =
        implementation::extract(&serde_json::from_value(selected).unwrap(), source).unwrap();
    assert_eq!(found.items.len(), 2);
    assert!(!found.items.contains_key("fixture::Op::C"));
    let mut selected = route();
    selected["selector"] = json!("let_else");
    selected["role"] = json!("lower");
    let source =
        "fn dispatch(op: Op) { let Op::A { payload } = op else { return; }; rewrite(payload); }";
    let found =
        implementation::extract(&serde_json::from_value(selected).unwrap(), source).unwrap();
    assert_eq!(found.items["fixture::Op::A"][0].role, "lower_candidate");
    assert!(
        found.items["fixture::Op::A"][0]
            .calls
            .iter()
            .any(|c| c == "rewrite")
    );
}

#[test]
fn renamed_or_ambiguous_selectors_and_removed_required_calls_fail() {
    let selected: implementation::Route = serde_json::from_value(route()).unwrap();
    for source in [
        "fn renamed(op: Op) { match op { Op::A => emit() } }",
        "fn dispatch(op: Op) { match other { Op::A => emit() } }",
        "fn dispatch(op: Op) { match op { Op::A => emit() } match op { Op::B => emit() } }",
        "fn dispatch(op: Op) { match op { _ => ignore() } }",
    ] {
        assert!(
            implementation::extract(&selected, source).is_err(),
            "{source}"
        );
    }
    let mut selected = route();
    selected["required_calls"] = json!(["guard"]);
    assert!(
        implementation::extract(
            &serde_json::from_value(selected).unwrap(),
            "fn dispatch(op: Op) { match op { Op::A => emit() } }"
        )
        .is_err()
    );
}

#[test]
fn old_implementation_ids_are_orphans_and_candidates_never_become_reviewed() {
    let dir = fixture(
        "pub enum Op { A }",
        "fn dispatch(op: Op) { match op { Op::A => emit(), Op::Removed => emit() } }",
        route(),
        json!([]),
    );
    let (implementation, coverage) = collect(dir.path());
    assert_eq!(coverage.orphan_evidence, ["fixture::Op::Removed"]);
    assert!(
        implementation
            .items
            .iter()
            .all(|i| i.review_status == "unreviewed")
    );
    assert!(
        coverage
            .items
            .iter()
            .all(|i| i.review_status == "unreviewed")
    );
    assert!(implementation::require_evidence(&coverage).is_err());
    let removed = implementation
        .items
        .iter()
        .find(|i| i.id == "fixture::Op::Removed")
        .unwrap();
    assert!(removed.observed_definition_sha256.is_none());
}

#[test]
fn checked_in_implementation_and_comparison_match_all_backend_contexts() {
    let root = definitions::repository_root();
    let definitions = definitions::collect(root, definitions::default_scope()).unwrap();
    let (implementation, coverage) = implementation::collect(
        root,
        Path::new("crates/emela-codegen-emb/tools/flow-implementation-routes.json"),
        &definitions,
    )
    .unwrap();
    let out_dir = root.join("crates/emela-codegen-emb/docs/generated");
    definitions::save(
        &implementation::reports(&implementation, &coverage, root, &out_dir).unwrap(),
        &out_dir,
        true,
    )
    .unwrap();
    assert!(!implementation.fallbacks.is_empty());
    assert_eq!(
        coverage
            .summary
            .get("missing_evidence")
            .copied()
            .unwrap_or(0),
        0
    );
    assert!(
        coverage
            .out_of_scope
            .iter()
            .any(|id| id.ends_with(".suspends"))
    );
    implementation::require_evidence(&coverage).unwrap();
}

#[test]
fn representation_gaps_are_reported_without_preventing_coverage_generation() {
    let mut selected = route();
    selected["contexts"] = json!(["sync", "suspend"]);
    selected["representation_owner"] = json!("crate::contract::ir::Op");
    let dir = fixture(
        "pub enum Op { A, Added }",
        "fn dispatch(op: Op) { match op { Op::A => emit() } }",
        selected.clone(),
        json!([]),
    );
    fs::create_dir_all(dir.path().join("crates/emela-codegen-emb/src/contract")).unwrap();
    fs::write(
        dir.path()
            .join("crates/emela-codegen-emb/src/contract/ir.rs"),
        "pub(crate) enum Op { A }",
    )
    .unwrap();
    let config = json!({"schema_version":1,"targets":[{"owner":"fixture::Op","representation_owner":"crate::contract::ir::Op","contexts":["sync","suspend"]}],"routes":[selected],"exclusions":[]});
    fs::write(dir.path().join("routes.json"), config.to_string()).unwrap();
    let (implementation, coverage) = collect(dir.path());
    assert_eq!(coverage.summary["missing_evidence"], 2);
    let serialized = serde_json::to_value(&coverage).unwrap();
    assert_eq!(
        serialized["representation_gaps"][0]["missing_variants"],
        json!(["Added"])
    );
    let reports =
        implementation::reports(&implementation, &coverage, dir.path(), dir.path()).unwrap();
    assert!(
        reports
            .iter()
            .any(|(name, text)| *name == "flow-coverage.md" && text.contains("Added"))
    );
    assert!(implementation::require_evidence(&coverage).is_err());
}

#[test]
fn missing_dispatch_functions_are_reported_without_preventing_coverage_generation() {
    let dir = fixture("pub enum Op { A }", "fn renamed() {}", route(), json!([]));
    let (_, coverage) = collect(dir.path());
    assert_eq!(coverage.summary["missing_evidence"], 2);
    let serialized = serde_json::to_value(&coverage).unwrap();
    assert_eq!(serialized["route_errors"][0]["route"], "dispatch");
    assert!(implementation::require_evidence(&coverage).is_err());
}
