use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn write(root: &Path, path: &str, text: impl AsRef<[u8]>) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let crate_path = "crates/emela-codegen-emb";
    write(
        root,
        "crates/emela-lir/src/ir.rs",
        "pub enum Op { A, Added }",
    );
    write(root, "crates/emela-lir/src/repr.rs", "");
    let names = [
        "FiberOp",
        "Type",
        "FnType",
        "FnSets",
        "SetSummary",
        "EnumDef",
        "VariantDef",
        "VariantFields",
        "CtorRef",
        "EffectDef",
        "OpDef",
        "External",
        "ExternalBinding",
        "HostRef",
        "HostModule",
    ];
    write(
        root,
        "crates/emela-core/src/ir.rs",
        names
            .iter()
            .map(|name| format!("pub struct {name};\n"))
            .collect::<String>(),
    );
    write(
        root,
        "crates/emela-core/src/intrinsics.rs",
        "pub enum Builtin {}",
    );
    write(
        root,
        &format!("{crate_path}/src/contract/ir.rs"),
        "pub(crate) enum Op { A }",
    );
    write(
        root,
        &format!("{crate_path}/src/lib.rs"),
        "compile_error!(\"this backend deliberately does not build\");",
    );
    write(
        root,
        &format!("{crate_path}/src/dispatch.rs"),
        "fn dispatch(op: Op) { match op { Op::A => a() } } fn a() {}",
    );
    write(
        root,
        &format!("{crate_path}/tests/probe.rs"),
        "#[test] fn present() {}",
    );
    write(
        root,
        &format!("{crate_path}/inventory/owners.json"),
        json!({"emela_lir::Op::A":{"path":"src/dispatch.rs","symbol":"a","role":"emit"}})
            .to_string(),
    );
    let mut containers =
        json!({"emela_lir::Op":{"path":"src/dispatch.rs","symbol":"dispatch","role":"schema"}});
    for name in names.into_iter().chain(["Builtin"]) {
        containers[format!("emela_core::{name}")] = containers["emela_lir::Op"].clone();
    }
    write(
        root,
        &format!("{crate_path}/inventory/containers.json"),
        containers.to_string(),
    );
    write(
        root,
        &format!("{crate_path}/inventory/tests.json"),
        json!({"emela_lir::Op::A":[{"target":"probe","name":"present"}]}).to_string(),
    );
    write(root, &format!("{crate_path}/tools/flow-implementation-routes.json"), json!({"schema_version":1,"targets":[{"owner":"emela_lir::Op","representation_owner":"crate::contract::ir::Op","contexts":["common"]}],"routes":[{"id":"dispatch","path":format!("{crate_path}/src/dispatch.rs"),"symbol":"dispatch","owner":"emela_lir::Op","representation_owner":"crate::contract::ir::Op","selector":"match","expression":"op","contexts":["common"],"role":"emit","note":"fixture"}],"exclusions":[]}).to_string());
    dir
}

fn run(root: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_emela-emb-inventory"))
        .args(["all", "--root"])
        .arg(root)
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn reports_are_saved_before_failure_even_when_backend_code_does_not_build() {
    let dir = fixture();
    let result = run(dir.path(), &[]);
    assert_eq!(result.status.code(), Some(1));
    let docs = dir.path().join("crates/emela-codegen-emb/docs/generated");
    assert!(docs.is_dir(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read_dir(&docs).unwrap().count(), 8);
    let backend: Value =
        serde_json::from_slice(&fs::read(docs.join("flow-backend-inventory.json")).unwrap())
            .unwrap();
    assert_eq!(backend["summary"]["missing_entries"], 1);
    assert!(
        fs::read_to_string(docs.join("flow-backend-inventory.md"))
            .unwrap()
            .contains("欠落 **1**")
    );
    let coverage: Value =
        serde_json::from_slice(&fs::read(docs.join("flow-coverage.json")).unwrap()).unwrap();
    assert_eq!(coverage["summary"]["missing_evidence"], 1);
    assert_eq!(
        coverage["representation_gaps"][0]["missing_variants"],
        json!(["Added"])
    );
    let before: Vec<_> = fs::read_dir(&docs)
        .unwrap()
        .map(|p| {
            let p = p.unwrap().path();
            (p.clone(), fs::read(p).unwrap())
        })
        .collect();
    let checked = run(dir.path(), &["--check"]);
    assert_eq!(
        checked.status.code(),
        Some(1),
        "gaps must still fail after regeneration"
    );
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(
        fs::read_to_string(dir.path().join("crates/emela-codegen-emb/src/lib.rs"))
            .unwrap()
            .contains("compile_error!")
    );
}

#[test]
fn check_does_not_create_reports_or_hide_stale_documents() {
    let dir = fixture();
    let docs = dir.path().join("crates/emela-codegen-emb/docs/generated");
    let result = run(dir.path(), &["--check"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(!docs.exists());
    let _ = run(dir.path(), &[]);
    fs::write(docs.join("flow-coverage.md"), "stale").unwrap();
    let result = run(dir.path(), &["--check"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("stale generated files"));
    assert_eq!(
        fs::read_to_string(docs.join("flow-coverage.md")).unwrap(),
        "stale"
    );
}

#[test]
fn missing_test_source_files_are_recorded_without_discarding_backend_reports() {
    let dir = fixture();
    write(
        dir.path(),
        "crates/emela-codegen-emb/src/lib.rs",
        "mod removed;",
    );
    let result = run(dir.path(), &[]);
    assert_eq!(result.status.code(), Some(1));
    let docs = dir.path().join("crates/emela-codegen-emb/docs/generated");
    assert_eq!(fs::read_dir(&docs).unwrap().count(), 8);
    let report: Value =
        serde_json::from_slice(&fs::read(docs.join("flow-backend-inventory.json")).unwrap())
            .unwrap();
    assert!(
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["kind"] == "test_catalog")
    );
}
