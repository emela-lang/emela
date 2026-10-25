//! Wraps the emitted core module into a WASI 0.3 (p3) component.
//!
//! The core module is written by `emit`; this module embeds the WIT world
//! (exporting `wasi:cli/run@0.3.0` and declaring stdout/stderr/exit plus the
//! environment, random, system clock, monotonic clock and file system) into it and lets
//! `wit-component` build the component. The wrapper only carries the interfaces the core
//! module actually references, so an unused one is not an output import. The
//! WIT files are compiled into the binary, so an installed compiler does not
//! need them on disk.

use std::sync::OnceLock;

use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::Resolve;

const RANDOM: &str = include_str!("../wit/deps/random.wit");
const CLOCKS: &str = include_str!("../wit/deps/clocks.wit");
const SOCKETS: &str = include_str!("../wit/deps/sockets.wit");
const FILESYSTEM: &str = include_str!("../wit/deps/filesystem.wit");
const CLI: &str = include_str!("../wit/deps/cli.wit");
const HTTP: &str = include_str!("../wit/deps/http.wit");
const WORLD: &str = include_str!("../wit/emela.wit");
const SERVICE_WORLD: &str = include_str!("../wit/service.wit");

/// The optional interfaces to add to the command world. The runtime always uses stdout, stderr
/// and exit; the environment, random, system clock, monotonic clock and file system are only
/// imported by a program whose `Std` handlers use them, so a hello-style program does not carry
/// them. The monotonic clock is the sleep host alone; `TestClock` needs no host timer. The file
/// system is `SystemFileSystem` alone (the test handler is pure Emela).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OptionalImports {
    pub(crate) environment: bool,
    pub(crate) random: bool,
    pub(crate) clock: bool,
    pub(crate) monotonic_clock: bool,
    pub(crate) filesystem: bool,
    /// An HTTP service: the world of `wit/service.wit` instead of the command world.
    pub(crate) service: bool,
}

impl OptionalImports {
    fn bits(self) -> usize {
        usize::from(self.environment)
            | usize::from(self.random) << 1
            | usize::from(self.clock) << 2
            | usize::from(self.monotonic_clock) << 3
            | usize::from(self.filesystem) << 4
            | usize::from(self.service) << 5
    }
}

/// The marker in `wit/emela.wit` where the used optional imports are inserted.
const OPTIONAL_MARKER: &str = "  // @optional-imports";

fn world_source(optional: OptionalImports) -> Result<String, String> {
    let world = if optional.service {
        SERVICE_WORLD
    } else {
        WORLD
    };
    let (before, after) = world
        .split_once(OPTIONAL_MARKER)
        .ok_or_else(|| format!("the command world has no `{OPTIONAL_MARKER}` marker"))?;
    if after.contains(OPTIONAL_MARKER) {
        return Err(format!(
            "the command world has more than one `{OPTIONAL_MARKER}` marker"
        ));
    }
    let mut imports = String::new();
    if optional.environment {
        imports.push_str("  import wasi:cli/environment@0.3.0;\n");
    }
    if optional.random {
        imports.push_str("  import wasi:random/random@0.3.0;\n");
    }
    if optional.clock {
        imports.push_str("  import wasi:clocks/system-clock@0.3.0;\n");
    }
    if optional.monotonic_clock {
        imports.push_str("  import wasi:clocks/monotonic-clock@0.3.0;\n");
    }
    if optional.filesystem {
        imports.push_str("  import wasi:filesystem/types@0.3.0;\n");
        imports.push_str("  import wasi:filesystem/preopens@0.3.0;\n");
    }
    Ok(format!("{before}{}{after}", imports.trim_end()))
}

/// The world a compiled program is wrapped in.
fn resolve(optional: OptionalImports) -> Result<(Resolve, wit_parser::WorldId), String> {
    let mut resolve = Resolve::default();
    // The dependencies are pushed in dependency order: `sockets` and
    // `filesystem` use `clocks`, `wasi:cli` uses all of them, and `wasi:http` uses `wasi:cli`
    // and `wasi:clocks`.
    for (path, source) in [
        ("deps/random.wit", RANDOM),
        ("deps/clocks.wit", CLOCKS),
        ("deps/sockets.wit", SOCKETS),
        ("deps/filesystem.wit", FILESYSTEM),
        ("deps/cli.wit", CLI),
        ("deps/http.wit", HTTP),
    ] {
        resolve
            .push_str(path, source)
            .map_err(|e| format!("cannot load the vendored WIT `{path}`: {e}"))?;
    }
    let world = world_source(optional)?;
    let pkg = resolve
        .push_str("emela.wit", &world)
        .map_err(|e| format!("cannot load the command world: {e}"))?;
    let name = if optional.service { "service" } else { "emela" };
    let world = resolve
        .select_world(&[pkg], Some(name))
        .map_err(|e| format!("cannot select the command world: {e}"))?;
    Ok((resolve, world))
}

type ResolvedWorld = Result<(Resolve, wit_parser::WorldId), String>;

/// The vendored WIT is the same for every combination of optional imports, so each is parsed
/// once. The resolved `Resolve` is borrowed; `wrap` does not need a mutable copy.
fn resolved(optional: OptionalImports) -> Result<&'static (Resolve, wit_parser::WorldId), String> {
    static RESOLVED: OnceLock<[OnceLock<ResolvedWorld>; 64]> = OnceLock::new();
    let slots = RESOLVED.get_or_init(|| [const { OnceLock::new() }; 64]);
    slots[optional.bits()]
        .get_or_init(|| resolve(optional))
        .as_ref()
        .map_err(Clone::clone)
}

/// Wraps `core` (the encoded core module) into a component.
pub(crate) fn wrap(core: &[u8], optional: OptionalImports) -> Result<Vec<u8>, String> {
    let (resolve, world) = resolved(optional)?;
    let mut core = core.to_vec();
    embed_component_metadata(
        &mut core,
        resolve,
        *world,
        StringEncoding::UTF8,
        // Use the canonical `@0.3.0` names so the component matches the p3 WIT
        // the host implements.
        true,
    )
    .map_err(|e| format!("cannot embed the component metadata: {e}"))?;
    let mut encoder = ComponentEncoder::default();
    encoder
        .module(&core)
        .map_err(|e| format!("the core module does not match the command world: {e}"))?
        .validate(true);
    encoder
        .encode()
        .map_err(|e| format!("cannot encode the component: {e:#}"))
}
