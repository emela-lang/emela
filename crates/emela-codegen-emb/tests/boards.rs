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
        for required in ["build.py", "startup.S", "README.md"] {
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
