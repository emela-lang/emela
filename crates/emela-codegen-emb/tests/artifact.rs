//! Generator contract checks: no filesystem, C compiler, BSP, or board toolchain.
use emela_codegen_emb::artifact::{ALIASES_HEADER, FileRole, MANIFEST_FILE, Manifest};
use emela_codegen_emb::{GeneratedFile, Options, compile_flow};

fn fixture() -> Manifest {
    Manifest::from_json(include_str!("fixtures/artifact/emela_artifact.json")).unwrap()
}

#[test]
fn generator_lists_every_source_and_header_in_a_valid_manifest() {
    let output = compile_flow(
        &emela_lir::Module::new(emela_core::Module::new()),
        &Options::default(),
    )
    .unwrap();
    let json = &output
        .files
        .iter()
        .find(|file| file.path == MANIFEST_FILE)
        .unwrap()
        .contents;
    let manifest = Manifest::from_json(json).unwrap();
    assert_eq!(manifest, Manifest::from_files(&output.files).unwrap());
    assert_eq!(manifest.files.len() + 1, output.files.len());
    assert!(
        manifest
            .files
            .iter()
            .any(|file| file.role == FileRole::AliasesHeader)
    );
}

#[test]
fn adding_an_adapter_and_aliases_refreshes_one_complete_manifest() {
    let mut output = compile_flow(
        &emela_lir::Module::new(emela_core::Module::new()),
        &Options::default(),
    )
    .unwrap();
    for (path, contents) in [
        ("adapter/native.c", "/* native adapter */"),
        (ALIASES_HEADER, "/* aliases */"),
    ] {
        output.files.retain(|file| file.path != path);
        output.files.push(GeneratedFile {
            path: path.into(),
            contents: contents.into(),
        });
    }
    output.refresh_manifest().unwrap();
    output.refresh_manifest().unwrap();
    let manifests: Vec<_> = output
        .files
        .iter()
        .filter(|file| file.path == MANIFEST_FILE)
        .collect();
    assert_eq!(manifests.len(), 1);
    let manifest = Manifest::from_json(&manifests[0].contents).unwrap();
    assert_eq!(manifest, Manifest::from_files(&output.files).unwrap());
    assert!(
        manifest
            .files
            .iter()
            .any(|file| file.path == "adapter/native.c" && file.role == FileRole::Source)
    );
    assert!(
        manifest
            .files
            .iter()
            .any(|file| file.path == ALIASES_HEADER && file.role == FileRole::AliasesHeader)
    );
    let original = output.files.clone();
    output.files.push(GeneratedFile {
        path: "../outside.c".into(),
        contents: String::new(),
    });
    let invalid = output.files.clone();
    assert!(output.refresh_manifest().is_err());
    assert_eq!(
        output.files, invalid,
        "failed refresh must not remove the old manifest"
    );
    assert!(Manifest::from_files(&original).is_ok());
}

#[test]
fn rust_and_python_share_the_persisted_contract_fixtures() {
    let manifest = fixture();
    assert_eq!(
        Manifest::from_json(&manifest.to_json().unwrap()).unwrap(),
        manifest
    );
    assert_eq!(
        manifest
            .files
            .iter()
            .filter(|file| file.role == FileRole::Source)
            .count(),
        2
    );
    // Both readers use this corpus, so their validation rules cannot silently drift.
    let invalid: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/artifact-invalid.json")).unwrap();
    for case in invalid {
        let json = case["json"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| case["manifest"].to_string());
        assert!(
            Manifest::from_json(&json).is_err(),
            "accepted {}",
            case["name"]
        );
    }
}
