//! Build targets (spec chapter 16) and the default target of a project (`[build].target` in
//! `Pome.toml`, spec 4.7).

use std::path::Path;

use crate::code;
use crate::diagnostic::{Diagnostic, Location};
use crate::pipeline::{Input, PROJECT_FILE};
use crate::source::FileSystem;

/// What build, run and test compile to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    /// JS run on node (Tier 1).
    #[default]
    Js,
    /// C compiled with cc to a native executable (Tier 2).
    C,
    /// WebAssembly (WASI) run with `wasmtime` (Tier 1).
    Wasm,
    /// Freestanding C source for microcontrollers, linked by the board's build (spec 16.6).
    CEmbedded,
}

impl Target {
    /// The names `--target` and `[build].target` accept.
    pub const NAMES: [&'static str; 4] = ["js", "c", "wasm", "c-embedded"];

    pub fn parse(name: &str) -> Option<Target> {
        match name {
            "js" => Some(Target::Js),
            "c" => Some(Target::C),
            "wasm" => Some(Target::Wasm),
            "c-embedded" => Some(Target::CEmbedded),
            _ => None,
        }
    }

    /// The name `--target` and `[build].target` use.
    pub fn name(self) -> &'static str {
        match self {
            Target::Js => "js",
            Target::C => "c",
            Target::Wasm => "wasm",
            Target::CEmbedded => "c-embedded",
        }
    }
}

impl Target {
    /// The name used in `@external(<name>, ...)` (spec 12.1): `embedded` for `c-embedded`, whose
    /// external functions are not those of the host C target.
    pub fn external_name(self) -> &'static str {
        match self {
            Target::CEmbedded => "embedded",
            t => t.name(),
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The project's default target, `[build].target` of its `Pome.toml`; `None` if it has none (or
/// for a single file). An unknown target is E0232. Other contents of the file are not read here.
pub fn project_target(fs: &dyn FileSystem, input: &Input) -> Result<Option<Target>, Diagnostic> {
    if input.single_file {
        return Ok(None);
    }
    let path = input.base.join(PROJECT_FILE);
    let Ok(text) = fs.read_to_string(&path) else {
        return Ok(None);
    };
    target_in(&text, &path)
}

/// `[build].target` in the text of a `Pome.toml` at `path`. A file that is not TOML, or whose
/// `target` is not a string, has no target as far as this is concerned.
fn target_in(text: &str, path: &Path) -> Result<Option<Target>, Diagnostic> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Ok(None);
    };
    let Some(name) = table
        .get("build")
        .and_then(|build| build.get("target"))
        .and_then(|target| target.as_str())
    else {
        return Ok(None);
    };
    match Target::parse(name) {
        Some(target) => Ok(Some(target)),
        None => Err(
            Diagnostic::error(format!("unknown target `{name}` in `[build].target`"))
                .with_code(code::UNKNOWN_BUILD_TARGET)
                .with_note(format!("expected one of {}", quoted(&Target::NAMES)))
                .at(Location::Path(path.to_owned())),
        ),
    }
}

/// The embedded C target's settings in the project's `Pome.toml`:
/// `[embedded.memory] string_capacity` (default 256, at most 512). A value out of range is an
/// error in the settings.
pub fn embedded_options(
    fs: &dyn FileSystem,
    input: &Input,
) -> Result<emela_codegen_c_embedded::Options, Diagnostic> {
    let mut options = emela_codegen_c_embedded::Options::default();
    if input.single_file {
        return Ok(options);
    }
    let path = input.base.join(PROJECT_FILE);
    let Ok(text) = fs.read_to_string(&path) else {
        return Ok(options);
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return Ok(options);
    };
    let capacity = table
        .get("embedded")
        .and_then(|e| e.get("memory"))
        .and_then(|m| m.get("string_capacity"));
    if let Some(value) = capacity {
        let max = emela_codegen_c_embedded::Options::MAX_STRING_CAPACITY;
        match value.as_integer() {
            Some(n) if (1..=i64::from(max)).contains(&n) => options.string_capacity = n as u16,
            _ => {
                return Err(Diagnostic::error(format!(
                    "`[embedded.memory] string_capacity` must be an integer from 1 to {max}"
                ))
                .at(Location::Path(path)));
            }
        }
    }
    let setting_error =
        |message: String| Diagnostic::error(message).at(Location::Path(path.clone()));
    if let Some(value) = table
        .get("embedded")
        .and_then(|e| e.get("memory"))
        .and_then(|m| m.get("heap_bytes"))
    {
        match value.as_integer() {
            Some(n) if (0..=i64::from(u32::MAX)).contains(&n) => options.heap_bytes = n as u32,
            _ => {
                return Err(setting_error(
                    "`[embedded.memory] heap_bytes` must be a non-negative integer".into(),
                ));
            }
        }
    }
    if let Some(board) = table.get("build").and_then(|b| b.get("board")) {
        let Some(file) = board.as_str() else {
            return Err(setting_error(
                "`[build] board` must be the path of a board definition file".into(),
            ));
        };
        let board_path = input.base.join(file);
        let text = fs.read_to_string(&board_path).map_err(|e| {
            setting_error(format!(
                "cannot read the board definition `{}`: {e}",
                board_path.display()
            ))
        })?;
        options.board = Some(board_def(&text).map_err(|message| {
            Diagnostic::error(format!("invalid board definition: {message}"))
                .at(Location::Path(board_path.clone()))
        })?);
    }
    if let Some(interrupts) = table.get("embedded").and_then(|e| e.get("gpio_interrupts")) {
        let Some(interrupts) = interrupts.as_table() else {
            return Err(setting_error(
                "`[embedded.gpio_interrupts]` must be a table of `logical pin = \"function name\"`"
                    .into(),
            ));
        };
        for (pin, handler) in interrupts {
            let (Ok(pin), Some(handler)) = (pin.parse::<i32>(), handler.as_str()) else {
                return Err(setting_error(format!(
                    "`{pin}` in `[embedded.gpio_interrupts]` must be `logical pin = \"function name\"`"
                )));
            };
            options
                .gpio_interrupts
                .push(emela_codegen_c_embedded::GpioInterrupt {
                    pin,
                    handler: handler.to_owned(),
                });
        }
    }
    Ok(options)
}

/// A board definition (`docs/design/board-definition.md`): `[board]` and `[logical_pins."<id>"]`.
fn board_def(text: &str) -> Result<emela_codegen_c_embedded::BoardDef, String> {
    let table = text
        .parse::<toml::Table>()
        .map_err(|e| format!("not valid TOML: {e}"))?;
    let board = table
        .get("board")
        .and_then(|b| b.as_table())
        .ok_or("no `[board]`")?;
    let text_of = |key: &str| board.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let mut pins = Vec::new();
    if let Some(logical) = table.get("logical_pins").and_then(|p| p.as_table()) {
        for (id, pin) in logical {
            let id: i32 = id
                .parse()
                .map_err(|_| format!("the logical pin ID `{id}` must be an integer"))?;
            let port = pin
                .get("port")
                .and_then(|p| p.as_str())
                .and_then(|p| {
                    let mut chars = p.chars();
                    let c = chars.next()?;
                    (chars.next().is_none() && c.is_ascii_uppercase()).then_some(c)
                })
                .ok_or(format!(
                    "`port` of the logical pin {id} must be one letter like `\"A\"`"
                ))?;
            let number = pin
                .get("pin")
                .and_then(|p| p.as_integer())
                .filter(|n| (0..16).contains(n))
                .ok_or(format!(
                    "`pin` of the logical pin {id} must be an integer from 0 to 15"
                ))?;
            pins.push(emela_codegen_c_embedded::LogicalPin {
                id,
                label: pin
                    .get("label")
                    .and_then(|l| l.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                port,
                pin: number as u8,
            });
        }
    }
    pins.sort_by_key(|p| p.id);
    let (mut uarts, mut i2cs, mut spis) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(routes) = table.get("peripheral_routes").and_then(|r| r.as_table()) {
        for (key, route) in routes {
            let Some(kind) = ["uart", "i2c", "spi"].into_iter().find(|k| {
                key.strip_prefix(k)
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            }) else {
                continue;
            };
            let int = |field: &str| {
                route.get(field).and_then(|v| v.as_integer()).ok_or(format!(
                    "`{field}` of `[peripheral_routes.{key}]` must be an integer"
                ))
            };
            let instance = route
                .get("instance")
                .and_then(|v| v.as_str())
                .ok_or(format!("`[peripheral_routes.{key}]` has no `instance`"))?;
            let mut route_pins = Vec::new();
            for role in emela_codegen_c_embedded::route_roles(kind) {
                let pin = int(&format!("{role}_pin"))?;
                if !pins.iter().any(|p| i64::from(p.id) == pin) {
                    return Err(format!(
                        "the logical pin {pin} of `[peripheral_routes.{key}]` is not in `[logical_pins]`"
                    ));
                }
                route_pins.push(((*role).to_owned(), pin as i32));
            }
            // `uart0` is the constructor `Uart0` of `UartId`.
            let mut id = key.clone();
            id[..1].make_ascii_uppercase();
            let r = emela_codegen_c_embedded::Route {
                id,
                instance: instance.to_owned(),
                pins: route_pins,
                af: int("alternate_function")? as u8,
            };
            match kind {
                "uart" => uarts.push(r),
                "i2c" => i2cs.push(r),
                _ => spis.push(r),
            }
        }
    }
    Ok(emela_codegen_c_embedded::BoardDef {
        id: text_of("id").unwrap_or_default(),
        name: text_of("name").unwrap_or_default(),
        hal: text_of("hal"),
        pins,
        uarts,
        i2cs,
        spis,
    })
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Result<Option<Target>, String> {
        target_in(text, Path::new("Pome.toml")).map_err(|d| format!("{:?} {}", d.code, d.message))
    }

    #[test]
    fn reads_build_target() {
        assert_eq!(target(""), Ok(None));
        assert_eq!(target("[pome]\nname = \"x\"\n"), Ok(None));
        assert_eq!(target("[build]\ntarget = \"c\"\n"), Ok(Some(Target::C)));
        assert_eq!(target("[build]\ntarget = \"js\"\n"), Ok(Some(Target::Js)));
        assert_eq!(
            target("[build]\ntarget = \"node\"\n"),
            Err("Some(\"E0232\") unknown target `node` in `[build].target`".into())
        );
    }
}
