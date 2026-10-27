//! Board support packages bundled with the compiler.

/// A file installed into the selected board's build directory.
pub struct BoardAsset {
    pub name: &'static str,
    pub contents: &'static [u8],
}

/// A board's reusable build, startup, and peripheral implementation.
pub struct BoardPackage {
    pub id: &'static str,
    pub assets: &'static [BoardAsset],
}

macro_rules! asset {
    ($board:literal, $name:literal) => {
        BoardAsset {
            name: $name,
            contents: include_bytes!(concat!("../boards/", $board, "/", $name)),
        }
    };
}

macro_rules! stm32f4_assets {
    ($profile:literal, $chip:literal) => {
        &[
            asset!("stm32/stm32f4", "build.py"),
            asset!("stm32/common", "stm32_common.py"),
            asset!("stm32/stm32f4", "startup.S"),
            asset!("stm32/stm32f4", "firmware.ld.in"),
            asset!("stm32/stm32f4", "runtime.c"),
            asset!("stm32/stm32f4", "validation_stack.c"),
            asset!("stm32/stm32f4", "validation_stack.h"),
            asset!("stm32/stm32f4", "README.md"),
            BoardAsset {
                name: "profile.json",
                contents: include_bytes!(concat!(
                    "../boards/stm32/stm32f4/boards/",
                    $profile,
                    ".json"
                )),
            },
            BoardAsset {
                name: "chip.json",
                contents: include_bytes!(concat!("../boards/stm32/stm32f4/chips/", $chip, ".json")),
            },
        ]
    };
}

/// Board identifiers accepted by [`package`].
pub const BOARD_NAMES: &[&str] = &[
    "stm32f446",
    "stm32f407vg",
    "stm32f412zg",
    "stm32f429zi",
    "stm32f446re",
    "stm32f4-discovery",
    "nucleo-f446re",
    "esp32c3",
];

/// Returns a package without depending on the compiler's source checkout at runtime.
pub fn package(id: &str) -> Option<BoardPackage> {
    let (id, assets): (_, &'static [BoardAsset]) = match id {
        "stm32f446" => ("stm32f446", stm32f4_assets!("stm32f446", "stm32f446re")),
        "stm32f407vg" => ("stm32f407vg", stm32f4_assets!("stm32f407vg", "stm32f407vg")),
        "stm32f412zg" => ("stm32f412zg", stm32f4_assets!("stm32f412zg", "stm32f412zg")),
        "stm32f429zi" => ("stm32f429zi", stm32f4_assets!("stm32f429zi", "stm32f429zi")),
        "stm32f446re" => ("stm32f446re", stm32f4_assets!("stm32f446re", "stm32f446re")),
        "stm32f4-discovery" => (
            "stm32f4-discovery",
            stm32f4_assets!("stm32f4-discovery", "stm32f407vg"),
        ),
        "nucleo-f446re" => (
            "nucleo-f446re",
            stm32f4_assets!("nucleo-f446re", "stm32f446re"),
        ),
        "esp32c3" => (
            "esp32c3",
            &[
                asset!("esp32c3", "build.py"),
                asset!("esp32c3", "board.c"),
                asset!("esp32c3", "startup.S"),
                asset!("esp32c3", "firmware.ld"),
                asset!("esp32c3", "README.md"),
            ],
        ),
        _ => return None,
    };
    Some(BoardPackage { id, assets })
}
