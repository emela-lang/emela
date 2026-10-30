use std::collections::BTreeSet;
use std::path::{Component, Path};

use emela_codegen_emb::boards::{BOARD_NAMES, package};

#[test]
fn bundled_boards_have_complete_flat_packages() {
    let identifiers: BTreeSet<_> = BOARD_NAMES.iter().copied().collect();
    assert_eq!(identifiers.len(), BOARD_NAMES.len());
    assert_eq!(identifiers.len(), 8);
    for &identifier in BOARD_NAMES {
        let board = package(identifier).expect("every advertised board has a package");
        assert_eq!(board.id, identifier);
        let names: BTreeSet<_> = board.assets.iter().map(|asset| asset.name).collect();
        assert_eq!(names.len(), board.assets.len());
        assert!(!names.contains("build_state.py"));
        for asset in board.assets {
            assert!(!asset.contents.is_empty(), "{identifier}: {}", asset.name);
            assert!(matches!(
                Path::new(asset.name)
                    .components()
                    .collect::<Vec<_>>()
                    .as_slice(),
                [Component::Normal(_)]
            ));
        }
        for required in ["build.py", "startup.S", "README.md", "artifact_manifest.py"] {
            assert!(names.contains(required), "{identifier}: {required}");
        }
        let required = if identifier == "esp32c3" {
            &["board.c", "firmware.ld"][..]
        } else {
            &[
                "stm32_common.py",
                "runtime.c",
                "validation_stack.c",
                "validation_stack.h",
                "firmware.ld.in",
                "profile.json",
                "chip.json",
            ][..]
        };
        for &name in required {
            assert!(names.contains(name), "{identifier}: {name}");
        }
    }
}

#[test]
fn unknown_board_names_do_not_select_a_default_package() {
    for identifier in ["", "stm32", "stm32f446/../esp32c3", "ESP32C3"] {
        assert!(package(identifier).is_none(), "{identifier}");
    }
}

#[test]
fn installed_bsp_reads_the_contract_without_the_compiler_checkout() {
    use emela_codegen_emb::{GeneratedFile, Options, compile_flow};
    use std::process::Command;

    let mut output = compile_flow(
        &emela_lir::Module::new(emela_core::Module::new()),
        &Options::default(),
    )
    .unwrap();
    output
        .files
        .retain(|file| file.path != emela_codegen_emb::artifact::ALIASES_HEADER);
    output.files.push(GeneratedFile {
        path: "emela_embedded_exports.h".into(),
        contents: "#include \"emela_embedded.h\"\n".into(),
    });
    output.refresh_manifest().unwrap();
    for identifier in ["stm32f446re", "esp32c3"] {
        let temp = tempfile::tempdir().unwrap();
        let bsp = temp.path().join("bsp");
        let generated = temp.path().join("generated");
        std::fs::create_dir(&bsp).unwrap();
        std::fs::create_dir(&generated).unwrap();
        for asset in package(identifier).unwrap().assets {
            std::fs::write(bsp.join(asset.name), asset.contents).unwrap();
        }
        for file in &output.files {
            std::fs::write(generated.join(&file.path), &file.contents).unwrap();
        }
        let result = Command::new("python3").args(["-B", "-c", "import sys; sys.path.insert(0, sys.argv[1]); from artifact_manifest import load_manifest; inputs = load_manifest(sys.argv[2], require_aliases=True); assert len(inputs['source']) == 1"])
            .arg(&bsp).arg(&generated).current_dir(temp.path()).env_remove("PYTHONPATH").output().unwrap();
        assert!(
            result.status.success(),
            "{identifier}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        // The installed entry point must reject an unsupported contract before
        // looking for a cross compiler, even outside the compiler checkout.
        std::fs::write(
            generated.join("emela_artifact.json"),
            "{\"schema_version\":99,\"abi_version\":1,\"files\":[]}",
        )
        .unwrap();
        let result = Command::new("python3")
            .arg("-B")
            .arg(bsp.join("build.py"))
            .arg("build")
            .arg("--generated-dir")
            .arg(&generated)
            .arg("--project-dir")
            .arg(temp.path())
            .arg("--output-dir")
            .arg(temp.path().join("firmware"))
            .current_dir(temp.path())
            .env_remove("PYTHONPATH")
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("unsupported schema/ABI"),
            "{identifier}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!temp.path().join("firmware/firmware.json").exists());
    }
}
