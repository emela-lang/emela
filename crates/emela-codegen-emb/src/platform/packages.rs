//! Bundled package construction from explicit immutable profile data.
use super::contract::*;
use crate::artifact::ABI_VERSION;
fn base(id: &str) -> PlatformPackage {
    PlatformPackage {
        id: id.into(),
        abi_version: ABI_VERSION,
        services: [
            "runtime.lifecycle",
            "runtime.defect",
            "runtime.debug",
            "runtime.tasks",
        ]
        .map(str::to_owned)
        .into(),
        external_symbols: Default::default(),
        isr_safe_symbols: Default::default(),
        supports_isr: false,
        files: vec![],
        sources: vec![],
        compile_flags: vec![],
        link_flags: vec![],
    }
}
pub fn native() -> PlatformPackage {
    let mut package = base("native");
    package
        .services
        .extend(["service.clock".into(), "service.console".into()]);
    package.files.push(PackageFile {
        path: "native.c".into(),
        contents: include_bytes!("native.c").to_vec(),
    });
    package.sources.push("native.c".into());
    package.files.push(PackageFile{path:"emela_native.h".into(),contents:b"#ifndef EMELA_NATIVE_H\n#define EMELA_NATIVE_H\n/* Called from the main thread during idle; do not reenter the scheduler. */\nvoid emela_native_set_poll(void (*poll)(void));\n#endif\n".to_vec()});
    package
        .compile_flags
        .extend(["-Wall".into(), "-Werror".into()]);
    package.link_flags.push("-lm".into());
    package
}
/// Build an MCU package without inspecting environment, tools, or the filesystem.
/// These packages include their own reset/main routines: use ConnectOptions { main:false,.. }.
pub fn board(id: &str) -> Result<PlatformPackage, String> {
    let bsp = crate::boards::package(id).ok_or_else(|| format!("unknown board {id}"))?;
    let mut package = base(bsp.id);
    package.supports_isr = true;
    package.services.extend([
        "service.clock".into(),
        "service.console".into(),
        "service.gpio".into(),
        "service.uart".into(),
    ]);
    package.files = bsp
        .assets
        .iter()
        .filter(|asset| !asset.name.ends_with(".py") && !asset.name.ends_with(".md"))
        .map(|asset| PackageFile {
            path: asset.name.into(),
            contents: asset.contents.to_vec(),
        })
        .collect();
    if bsp.id == "esp32c3" {
        package.sources = vec!["board.c".into(), "startup.S".into()];
        package.compile_flags = [
            "-march=rv32imc_zicsr_zifencei",
            "-mabi=ilp32",
            "-ffreestanding",
            "-fno-builtin",
            "-fno-pic",
            "-msmall-data-limit=0",
            "-ffunction-sections",
            "-fdata-sections",
        ]
        .map(str::to_owned)
        .into();
        package.link_flags = [
            "-nostartfiles",
            "--specs=nosys.specs",
            "-Tplatform/firmware.ld",
            "-Wl,--gc-sections",
            "-Wl,--no-relax",
            "-Wl,-Map,out/firmware.map",
            "-lm",
            "-lgcc",
        ]
        .map(str::to_owned)
        .into();
    } else {
        let read = |name: &str| -> Result<serde_json::Value, String> {
            serde_json::from_slice(
                &package
                    .files
                    .iter()
                    .find(|file| file.path == name)
                    .ok_or_else(|| format!("missing board asset {name}"))?
                    .contents,
            )
            .map_err(|e| e.to_string())
        };
        let profile = read("profile.json")?;
        let chip = read("chip.json")?;
        let number = |v: &serde_json::Value, key: &str| {
            v[key]
                .as_u64()
                .ok_or_else(|| format!("invalid board profile {key}"))
        };
        let text = |key: &str| {
            profile[key]
                .as_str()
                .ok_or_else(|| format!("invalid board profile {key}"))
        };
        let uart = &profile["console_uart"];
        let masks = chip["gpio_pin_masks"]
            .as_array()
            .ok_or("invalid GPIO masks")?;
        let vectors = chip["external_vectors"]
            .as_array()
            .ok_or("invalid vectors")?;
        let values = vec![
            ("GPIO_PORT_COUNT", masks.len().to_string()),
            (
                "GPIO_PIN_MASKS",
                format!(
                    "{{{}}}",
                    masks
                        .iter()
                        .map(|mask| mask.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ),
            ("CONSOLE_UART_ID", number(uart, "id")?.to_string()),
            ("CONSOLE_UART_BASE", number(uart, "base")?.to_string()),
            ("CONSOLE_UART_APB", number(uart, "apb")?.to_string()),
            ("CONSOLE_UART_RCC_BIT", number(uart, "rcc_bit")?.to_string()),
            ("CONSOLE_TX_PIN", number(uart, "tx_pin")?.to_string()),
            ("CONSOLE_RX_PIN", number(uart, "rx_pin")?.to_string()),
            ("CONSOLE_UART_AF", number(uart, "af")?.to_string()),
            ("EXTERNAL_IRQ_COUNT", vectors.len().to_string()),
        ];
        let header = format!(
            "#ifndef EMELA_STM32F4_BOARD_CONFIG_H\n#define EMELA_STM32F4_BOARD_CONFIG_H\n{}\n#endif\n",
            values
                .iter()
                .map(|(key, value)| format!("#define EMELA_{key} {value}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let vectors = vectors
            .iter()
            .map(|name| {
                name.as_str()
                    .map_or("    .word 0".into(), |name| format!("    vector {name}"))
            })
            .collect::<Vec<String>>()
            .join("\n")
            + "\n";
        let linker = package
            .files
            .iter()
            .find(|file| file.path == "firmware.ld.in")
            .ok_or("missing linker template")?;
        let mut linker = String::from_utf8(linker.contents.clone()).map_err(|e| e.to_string())?;
        for (key, value) in [
            ("FLASH_ORIGIN", number(&profile, "flash_origin")?),
            ("FLASH_BYTES", number(&chip, "flash_bytes")?),
            ("RAM_ORIGIN", number(&profile, "ram_origin")?),
            ("RAM_BYTES", number(&chip, "ram_bytes")?),
            ("STACK_BYTES", number(&profile, "stack_bytes")?),
            ("GUARD_BYTES", number(&profile, "guard_bytes")?),
        ] {
            linker = linker.replace(&format!("@{key}@"), &format!("0x{value:x}"));
        }
        for (path, contents) in [
            ("board_config.h", header),
            ("board_vectors.inc", vectors),
            ("firmware.ld", linker),
        ] {
            package.files.push(PackageFile {
                path: path.into(),
                contents: contents.into_bytes(),
            });
        }
        package.sources = ["runtime.c", "validation_stack.c", "startup.S"]
            .map(str::to_owned)
            .into();
        package.compile_flags = vec![
            format!("-mcpu={}", text("cpu")?),
            "-mthumb".into(),
            format!("-mfpu={}", text("fpu")?),
            format!("-mfloat-abi={}", text("float_abi")?),
            "-ffunction-sections".into(),
            "-fdata-sections".into(),
        ];
        package.link_flags = [
            "-nostartfiles",
            "--specs=nano.specs",
            "--specs=nosys.specs",
            "-Tplatform/firmware.ld",
            "-Wl,--gc-sections",
            "-Wl,--wrap=main",
            "-Wl,-Map,out/firmware.map",
            "-lm",
        ]
        .map(str::to_owned)
        .into();
    }
    Ok(package)
}
