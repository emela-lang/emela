//! Source-bound implementation and test references. Their existence is not a correctness claim.
use crate::definitions;
#[path = "test_inventory.rs"]
mod test_inventory;
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::spanned::Spanned;

#[derive(Clone, Deserialize)]
struct Owner {
    path: String,
    symbol: String,
    role: String,
    #[serde(default)]
    lower_symbol: Option<String>,
    #[serde(default)]
    phase_owners: Vec<PhaseOwner>,
}

#[derive(Clone, Deserialize)]
struct PhaseOwner {
    path: String,
    symbol: String,
}

#[derive(Clone, Debug, Serialize)]
struct SourceEntry {
    path: String,
    symbol: String,
    line: usize,
    end_line: usize,
    body_sha256: String,
}

#[derive(Debug, Serialize)]
struct Record {
    id: String,
    kind: String,
    relation: &'static str,
    owner_id: String,
    role: String,
    review_status: &'static str,
    observed_definition_sha256: String,
    entry: Option<SourceEntry>,
    phase_entries: Vec<SourceEntry>,
    tests: Vec<test_inventory::TestEntry>,
    test_relation: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Issue {
    pub id: String,
    pub kind: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
struct Inventory {
    schema_version: u32,
    inventory_kind: &'static str,
    production_enabled: bool,
    definition_inventory_sha256: String,
    summary: BTreeMap<&'static str, usize>,
    records: Vec<Record>,
    issues: Vec<Issue>,
}

fn root() -> PathBuf {
    definitions::crate_dir().to_owned()
}

fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn functions(root: &Path, path: &str) -> Result<BTreeMap<String, SourceEntry>, String> {
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("entry path must stay inside the crate: {path}"));
    }
    let source = fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
    let parsed = syn::parse_file(&source).map_err(|e| format!("{path}: {e}"))?;
    let mut result = BTreeMap::new();
    let mut insert = |symbol: String, signature: &syn::Signature, block: &syn::Block| {
        let span = block.span();
        let normalized = format!(
            "{} {}",
            signature.to_token_stream(),
            block.to_token_stream()
        );
        result.insert(
            symbol.clone(),
            SourceEntry {
                path: path.into(),
                symbol,
                line: signature.span().start().line,
                end_line: span.end().line,
                body_sha256: hash(normalized),
            },
        );
    };
    for item in parsed.items {
        match item {
            syn::Item::Fn(f) => insert(f.sig.ident.to_string(), &f.sig, &f.block),
            syn::Item::Impl(i) => {
                let syn::Type::Path(owner) = &*i.self_ty else {
                    continue;
                };
                let owner = owner.path.segments.last().unwrap().ident.to_string();
                for item in i.items {
                    if let syn::ImplItem::Fn(f) = item {
                        insert(format!("{owner}::{}", f.sig.ident), &f.sig, &f.block);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

fn inspect(root: &Path, definitions: &Value) -> Result<Inventory, String> {
    let owners: BTreeMap<String, Owner> = serde_json::from_str(
        &fs::read_to_string(root.join("inventory/owners.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let containers: BTreeMap<String, Owner> = serde_json::from_str(
        &fs::read_to_string(root.join("inventory/containers.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let items = definitions["items"]
        .as_array()
        .ok_or("missing definition items")?;
    let mut by_id = BTreeMap::new();
    for item in items {
        let id = item["id"].as_str().ok_or("missing definition ID")?;
        if by_id.insert(id.to_owned(), item).is_some() {
            return Err(format!("duplicate definition: {id}"));
        }
    }
    let expected: BTreeSet<_> = items
        .iter()
        .filter(|i| i["kind"] == "variant")
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    let actual: BTreeSet<_> = owners.keys().cloned().collect();
    let declarations: BTreeSet<_> = items
        .iter()
        .filter(|i| matches!(i["kind"].as_str(), Some("enum" | "struct" | "type_alias")))
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    let mut issues = Vec::new();
    for id in expected.difference(&actual) {
        issues.push(Issue {
            id: id.clone(),
            kind: "missing_owner",
            message: "variant entries differ: missing variant owner".into(),
        });
    }
    for id in actual.difference(&expected) {
        issues.push(Issue {
            id: id.clone(),
            kind: "orphan_owner",
            message: "variant entries differ: orphan variant owner".into(),
        });
    }
    for id in declarations.difference(&containers.keys().cloned().collect()) {
        issues.push(Issue {
            id: id.clone(),
            kind: "missing_declaration",
            message: "missing declaration responsibility".into(),
        });
    }
    for id in containers.keys().filter(|id| !declarations.contains(*id)) {
        issues.push(Issue {
            id: id.clone(),
            kind: "orphan_declaration",
            message: "orphan declaration responsibility".into(),
        });
    }
    let mut cache = BTreeMap::new();
    for owner in owners.values().chain(containers.values()) {
        cache
            .entry(owner.path.clone())
            .or_insert_with(|| functions(root, &owner.path));
    }
    let mut endpoints = BTreeSet::new();
    for (id, owner) in &owners {
        if !endpoints.insert((&owner.path, &owner.symbol)) {
            issues.push(Issue {
                id: id.clone(),
                kind: "reused_entry",
                message: format!("variant entry reused: {}::{}", owner.path, owner.symbol),
            });
        }
    }
    let mut records = Vec::new();
    for (id, item) in &by_id {
        let mut owner_id = id.clone();
        let owner = loop {
            if let Some(owner) = owners.get(&owner_id).or_else(|| containers.get(&owner_id)) {
                break Some(owner);
            }
            // A missing variant/declaration never inherits a parent's responsibility.
            if expected.contains(&owner_id) || declarations.contains(&owner_id) {
                break None;
            }
            let Some(parent) = by_id.get(&owner_id).and_then(|i| i["parent"].as_str()) else {
                break None;
            };
            owner_id = parent.into();
        };
        let relation = if owner.is_none() {
            "missing_owner"
        } else if owner_id == *id && item["kind"] == "variant" {
            "variant_entry"
        } else if owner_id == *id {
            "shared_declaration"
        } else if expected.contains(&owner_id) {
            "member_of_variant"
        } else {
            "member_of_declaration"
        };
        let mut entry = None;
        let mut phases = Vec::new();
        if let Some(owner) = owner {
            match &cache[&owner.path] {
                Ok(available) => {
                    entry = available.get(&owner.symbol).cloned();
                    if entry.is_none() {
                        issues.push(Issue {
                            id: owner_id.clone(),
                            kind: "missing_entry",
                            message: format!("missing entry: {}::{}", owner.path, owner.symbol),
                        });
                    }
                    for phase in &owner.phase_owners {
                        let found = functions(root, &phase.path)
                            .ok()
                            .and_then(|functions| functions.get(&phase.symbol).cloned());
                        if let Some(found) = found {
                            phases.push(found);
                        } else {
                            issues.push(Issue {
                                id: owner_id.clone(),
                                kind: "missing_phase",
                                message: format!(
                                    "missing stage owner {}::{}",
                                    phase.path, phase.symbol
                                ),
                            });
                        }
                    }
                    if relation == "variant_entry" {
                        let mut names = vec![
                            "lower",
                            "emit_function",
                            "emit_suspend",
                            "emit_enter",
                            "emit_leave",
                            "emit_finish",
                            "string_behavior",
                            "string_bound",
                            "result_ownership",
                            "needs_heap",
                            "is_known_empty",
                            "require_runtime",
                        ];
                        let specific;
                        if let Some(name) = id.strip_prefix("emela_lir::Callee::") {
                            specific = format!("emit_suspend_{}", name.to_lowercase());
                            names = vec![&specific];
                        } else if id == "emela_lir::Value::Call" {
                            names.extend([
                                "emit_suspend_direct",
                                "emit_suspend_indirect",
                                "emit_suspend_op",
                            ]);
                        }
                        for name in names.into_iter().chain(owner.lower_symbol.as_deref()) {
                            if name != owner.symbol
                                && let Some(found) = available.get(name)
                                && !phases.iter().any(|p| p.symbol == found.symbol)
                            {
                                phases.push(found.clone());
                            }
                        }
                    }
                }
                Err(message) => issues.push(Issue {
                    id: owner_id.clone(),
                    kind: "missing_entry",
                    message: format!("missing entry: {message}"),
                }),
            }
        }
        records.push(Record {
            id: id.clone(),
            kind: item["kind"].as_str().unwrap().into(),
            relation,
            owner_id,
            role: owner.map_or("missing", |o| o.role.as_str()).into(),
            review_status: "unreviewed",
            observed_definition_sha256: item["definition_sha256"].as_str().unwrap().into(),
            entry,
            phase_entries: phases,
            tests: vec![],
            test_relation: "not_independently_required",
        });
    }
    issues.sort();
    issues.dedup();
    let audit_config: Value = serde_json::from_slice(
        &fs::read(root.join("tools/flow-implementation-routes.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let audit_owners: BTreeSet<_> = audit_config["targets"]
        .as_array()
        .ok_or("missing audit targets")?
        .iter()
        .map(|t| t["owner"].as_str().unwrap())
        .collect();
    let primary = expected
        .iter()
        .filter(|id| audit_owners.contains(id.rsplit_once("::").unwrap().0))
        .count();
    let missing = records
        .iter()
        .filter(|r| r.kind == "variant" && r.entry.is_none())
        .count();
    Ok(Inventory {
        schema_version: 3,
        inventory_kind: "backend_implementation_ownership",
        production_enabled: true,
        definition_inventory_sha256: hash(serde_json::to_vec(definitions).unwrap()),
        summary: BTreeMap::from([
            ("definition_records", items.len()),
            ("variant_entries", expected.intersection(&actual).count()),
            ("primary_audit_variants", primary),
            ("additional_metadata_variants", expected.len() - primary),
            (
                "input_rejected_variants",
                owners.values().filter(|o| o.role == "reject").count(),
            ),
            (
                "fields",
                items.iter().filter(|i| i["kind"] == "field").count(),
            ),
            (
                "shared_declarations",
                declarations
                    .intersection(&containers.keys().cloned().collect())
                    .count(),
            ),
            ("missing_entries", missing),
            ("orphan_entries", actual.difference(&expected).count()),
        ]),
        records,
        issues,
    })
}

fn require_complete(inventory: &Inventory) -> Result<(), String> {
    if inventory.issues.is_empty() {
        return Ok(());
    }
    Err(inventory
        .issues
        .iter()
        .map(|i| format!("{}: {}", i.id, i.message))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn collect(root: &Path, definitions: &Value) -> Result<Inventory, String> {
    let inventory = inspect(root, definitions)?;
    require_complete(&inventory)?;
    Ok(inventory)
}

fn load_definitions() -> Result<Value, String> {
    let crate_dir = definitions::crate_dir();
    let repo = definitions::repository_root();
    let definitions: Value = serde_json::from_slice(
        &fs::read(crate_dir.join("docs/generated/flow-ir-inventory.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    for source in definitions["sources"]
        .as_array()
        .ok_or("missing definition sources")?
    {
        let path = source["path"].as_str().ok_or("missing definition path")?;
        if hash(fs::read(repo.join(path)).map_err(|e| e.to_string())?)
            != source["sha256"].as_str().unwrap()
        {
            return Err(format!(
                "definition inventory is stale: {path}; regenerate the definition inventory first"
            ));
        }
    }
    Ok(definitions)
}

fn markdown(inventory: &Inventory, crate_dir: &Path, out_dir: &Path) -> String {
    let mut out = String::from(
        "# Flow IR・本体実装・テストの対応表\n\n自動生成。通常の公開APIが使う入口と、明示的に関連付けた実在テストを記録する。入口・テストの存在は正しさや網羅性の保証ではない。\n\n",
    );
    out.push_str(&format!("定義 **{}** レコード、variant専用入口 **{}**、共有宣言 **{}**、field **{}**。欠落 **{}**、定義外ID **{}**。fieldと共有宣言を操作の実装数へ加算しない。\n\n",inventory.summary["definition_records"], inventory.summary["variant_entries"],inventory.summary["shared_declarations"],inventory.summary["fields"],inventory.summary["missing_entries"],inventory.summary["orphan_entries"]));
    out.push_str(&format!("従来の監査対象 **{}** variant ＋ 追加のmetadata **{}** variant。入力拒否として登録したvariantは **{}**。\n\n",inventory.summary["primary_audit_variants"],inventory.summary["additional_metadata_variants"],inventory.summary["input_rejected_variants"]));
    out.push_str(&format!("登録済みの関連テスト: **{}** variant / **{}** 個の異なるテスト。宣言・fieldの参照は親variantとの関係を表示する。\n\n", inventory.summary["variants_with_registered_tests"], inventory.summary["distinct_registered_tests"]));
    out.push_str("## 未追従・参照切れ\n\n");
    if inventory.issues.is_empty() {
        out.push_str("なし。\n");
    }
    for issue in &inventory.issues {
        out.push_str(&format!(
            "- `{}` / {}: {}\n",
            issue.id, issue.kind, issue.message
        ));
    }
    out.push_str("\n| 定義ID | 区分 | 担当ID | 入口・担当箇所 | 追加の段階 | 関連テスト |\n| --- | --- | --- | --- | --- | --- |\n");
    let link = |entry: &SourceEntry| {
        format!(
            "[{}]({}#L{})",
            entry.symbol,
            definitions::relative_path(out_dir, &crate_dir.join(&entry.path)),
            entry.line
        )
    };
    for item in &inventory.records {
        out.push_str(&format!(
            "| `{}` | {} / {} | `{}` | {} | {} | {} |\n",
            item.id,
            item.relation,
            item.role,
            item.owner_id,
            item.entry
                .as_ref()
                .map(link)
                .unwrap_or_else(|| "未追従".into()),
            item.phase_entries
                .iter()
                .map(link)
                .collect::<Vec<_>>()
                .join(" / "),
            item.tests
                .iter()
                .map(|test| format!(
                    "[{}::{}]({}#L{}){}",
                    test.reference.target,
                    test.reference.name,
                    definitions::relative_path(out_dir, &crate_dir.join(&test.path)),
                    test.line,
                    if test.ignored { " (ignored)" } else { "" }
                ))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    out
}

#[allow(dead_code)] // Also compiled as a helper of the backend integration tests.
pub fn check_tests(crate_dir: &Path, definitions: &Value) -> Result<(), String> {
    let required = definitions["items"]
        .as_array()
        .ok_or("missing definition items")?
        .iter()
        .filter(|item| item["kind"] == "variant")
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect();
    test_inventory::load(crate_dir, &required)?;
    Ok(())
}

pub struct Report {
    pub markdown: String,
    pub json: String,
    pub issues: Vec<Issue>,
}

pub fn generate(crate_dir: &Path, definitions: &Value, out_dir: &Path) -> Result<Report, String> {
    let mut inventory = inspect(crate_dir, definitions)?;
    let required: BTreeSet<_> = inventory
        .records
        .iter()
        .filter(|r| r.kind == "variant")
        .map(|r| r.id.clone())
        .collect();
    let associations: test_inventory::Associations = serde_json::from_slice(
        &fs::read(crate_dir.join("inventory/tests.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let catalog = match test_inventory::discover(crate_dir) {
        Ok(catalog) => catalog,
        Err(message) => {
            inventory.issues.push(Issue {
                id: "test_catalog".into(),
                kind: "test_catalog",
                message,
            });
            test_inventory::Catalog::new()
        }
    };
    for id in &required {
        let selected = associations
            .get(id)
            .cloned()
            .map(|tests| BTreeMap::from([(id.clone(), tests)]))
            .unwrap_or_default();
        if let Err(message) =
            test_inventory::validate(&BTreeSet::from([id.clone()]), &selected, &catalog)
        {
            inventory.issues.push(Issue {
                id: id.clone(),
                kind: "missing_tests",
                message,
            });
        }
    }
    for id in associations.keys().filter(|id| !required.contains(*id)) {
        inventory.issues.push(Issue {
            id: id.clone(),
            kind: "orphan_tests",
            message: "orphan test association".into(),
        });
    }
    for record in &mut inventory.records {
        let references: BTreeSet<_> = if let Some(tests) = associations.get(&record.owner_id) {
            record.test_relation = if record.kind == "variant" {
                "direct_variant"
            } else {
                "inherited_from_variant"
            };
            tests.iter().cloned().collect()
        } else if record.kind == "variant" {
            record.test_relation = "missing_tests";
            BTreeSet::new()
        } else {
            let prefix = format!("{}::", record.owner_id);
            let related = associations
                .iter()
                .filter(|(id, _)| id.starts_with(&prefix))
                .flat_map(|(_, tests)| tests.iter().cloned())
                .collect::<BTreeSet<_>>();
            if !related.is_empty() {
                record.test_relation = "related_variants";
            }
            related
        };
        record.tests = references
            .iter()
            .filter_map(|r| catalog.get(r).cloned())
            .collect();
    }
    inventory.summary.insert(
        "variants_with_registered_tests",
        required
            .iter()
            .filter(|id| {
                associations.get(*id).is_some_and(|tests| {
                    tests
                        .iter()
                        .any(|r| catalog.get(r).is_some_and(|entry| !entry.ignored))
                })
            })
            .count(),
    );
    inventory.summary.insert(
        "distinct_registered_tests",
        associations
            .values()
            .flatten()
            .filter(|r| catalog.contains_key(*r))
            .collect::<BTreeSet<_>>()
            .len(),
    );
    inventory.issues.sort();
    inventory.issues.dedup();
    Ok(Report {
        markdown: markdown(&inventory, crate_dir, out_dir),
        json: serde_json::to_string_pretty(&inventory).map_err(|e| e.to_string())? + "\n",
        issues: inventory.issues,
    })
}

fn render() -> Result<(String, String), String> {
    let report = generate(
        &root(),
        &load_definitions()?,
        &root().join("docs/generated"),
    )?;
    if !report.issues.is_empty() {
        return Err(report
            .issues
            .iter()
            .map(|i| format!("{}: {}", i.id, i.message))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    Ok((report.markdown, report.json))
}

#[test]
fn backend_inventory_matches_all_definition_ids_sources_and_tests() {
    let (md, json) = render().unwrap();
    for (name, expected) in [
        ("flow-backend-inventory.md", md),
        ("flow-backend-inventory.json", json),
    ] {
        assert_eq!(
            fs::read_to_string(root().join("docs/generated").join(name)).unwrap(),
            expected,
            "stale backend inventory; run tools/backend-inventory.sh"
        );
    }
}

#[test]
#[ignore = "writes the explicitly requested backend ownership artifacts"]
fn regenerate_backend_inventory() {
    let (md, json) = render().unwrap();
    fs::create_dir_all(root().join("docs/generated")).unwrap();
    fs::write(root().join("docs/generated/flow-backend-inventory.md"), md).unwrap();
    fs::write(
        root().join("docs/generated/flow-backend-inventory.json"),
        json,
    )
    .unwrap();
}

#[test]
fn newly_defined_variants_cannot_silently_inherit_another_entry() {
    let mut definitions = load_definitions().unwrap();
    definitions["items"].as_array_mut().unwrap().push(serde_json::json!({"id":"emela_lir::Value::NewVariant","kind":"variant","parent":"emela_lir::Value","definition_sha256":"new"}));
    assert!(
        collect(&root(), &definitions)
            .unwrap_err()
            .contains("variant entries differ")
    );
}

#[cfg(test)]
fn fixture() -> (tempfile::TempDir, Value) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("inventory")).unwrap();
    fs::create_dir_all(dir.path().join("tools")).unwrap();
    fs::write(
        dir.path().join("tools/flow-implementation-routes.json"),
        r#"{"targets":[{"owner":"fixture::Op"}]}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("backend.rs"),
        "fn dispatch() { a(); b(); } fn a() {} fn b() {}",
    )
    .unwrap();
    fs::write(
        dir.path().join("inventory/owners.json"),
        r#"{
      "fixture::Op::A":{"path":"backend.rs","symbol":"a","role":"emit"},
      "fixture::Op::B":{"path":"backend.rs","symbol":"b","role":"emit"}}
    "#,
    )
    .unwrap();
    fs::write(
        dir.path().join("inventory/containers.json"),
        r#"{
      "fixture::Op":{"path":"backend.rs","symbol":"dispatch","role":"schema"}}
    "#,
    )
    .unwrap();
    let definitions = serde_json::json!({"items":[
      {"id":"fixture::Op","kind":"enum","definition_sha256":"enum"},
      {"id":"fixture::Op::A","kind":"variant","parent":"fixture::Op","definition_sha256":"a"},
      {"id":"fixture::Op::B","kind":"variant","parent":"fixture::Op","definition_sha256":"b"}
    ]});
    (dir, definitions)
}

#[test]
fn missing_reused_and_orphan_entries_are_reported() {
    let (dir, definitions) = fixture();
    collect(dir.path(), &definitions).unwrap();
    fs::write(dir.path().join("backend.rs"), "fn dispatch() {} fn a() {}").unwrap();
    assert!(
        collect(dir.path(), &definitions)
            .unwrap_err()
            .contains("missing entry")
    );
    let config_path = dir.path().join("inventory/owners.json");
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["fixture::Op::B"]["symbol"] = Value::String("a".into());
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(
        collect(dir.path(), &definitions)
            .unwrap_err()
            .contains("entry reused")
    );
    config["fixture::Op::Removed"] = config["fixture::Op::A"].clone();
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(
        collect(dir.path(), &definitions)
            .unwrap_err()
            .contains("orphan")
    );
}

#[test]
fn new_fields_remain_unreviewed_members_of_their_owner() {
    let (dir, mut definitions) = fixture();
    definitions["items"].as_array_mut().unwrap().push(serde_json::json!({
        "id":"fixture::Op::A.new_field","kind":"field","parent":"fixture::Op::A","definition_sha256":"new"
    }));
    let inventory = collect(dir.path(), &definitions).unwrap();
    let field = inventory
        .records
        .iter()
        .find(|r| r.id.ends_with(".new_field"))
        .unwrap();
    assert_eq!(field.relation, "member_of_variant");
    assert_eq!(field.owner_id, "fixture::Op::A");
    assert_eq!(field.review_status, "unreviewed");
    assert_eq!(inventory.summary["variant_entries"], 2);
}

#[test]
fn missing_variants_remain_visible_in_partial_reports() {
    let (dir, mut definitions) = fixture();
    definitions["items"].as_array_mut().unwrap().push(serde_json::json!({
        "id":"fixture::Op::Added","kind":"variant","parent":"fixture::Op","definition_sha256":"new"
    }));
    let inventory = inspect(dir.path(), &definitions).unwrap();
    assert_eq!(inventory.summary["missing_entries"], 1);
    let added = inventory
        .records
        .iter()
        .find(|r| r.id == "fixture::Op::Added")
        .unwrap();
    assert!(added.entry.is_none());
    assert_eq!(added.relation, "missing_owner");
    assert!(require_complete(&inventory).is_err());
}
