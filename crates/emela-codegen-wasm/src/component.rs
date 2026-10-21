//! Wraps the emitted core module into a WASI 0.3 (p3) component.
//!
//! The core module is written by `emit`; this module embeds the WIT world
//! (exporting `wasi:cli/run@0.3.0` and importing stdout/stderr/exit) into it and
//! lets `wit-component` build the component. The WIT files are compiled into
//! the binary, so an installed compiler does not need them on disk.

use std::sync::OnceLock;

use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::Resolve;

const RANDOM: &str = include_str!("../wit/deps/random.wit");
const CLOCKS: &str = include_str!("../wit/deps/clocks.wit");
const SOCKETS: &str = include_str!("../wit/deps/sockets.wit");
const FILESYSTEM: &str = include_str!("../wit/deps/filesystem.wit");
const CLI: &str = include_str!("../wit/deps/cli.wit");
const WORLD: &str = include_str!("../wit/emela.wit");

/// The world a compiled program is wrapped in.
fn resolve() -> Result<(Resolve, wit_parser::WorldId), String> {
    let mut resolve = Resolve::default();
    // The dependencies are pushed in dependency order: `sockets` and
    // `filesystem` use `clocks`, and `wasi:cli` uses all of them.
    for (path, source) in [
        ("deps/random.wit", RANDOM),
        ("deps/clocks.wit", CLOCKS),
        ("deps/sockets.wit", SOCKETS),
        ("deps/filesystem.wit", FILESYSTEM),
        ("deps/cli.wit", CLI),
    ] {
        resolve
            .push_str(path, source)
            .map_err(|e| format!("cannot load the vendored WIT `{path}`: {e}"))?;
    }
    let pkg = resolve
        .push_str("emela.wit", WORLD)
        .map_err(|e| format!("cannot load the command world: {e}"))?;
    let world = resolve
        .select_world(&[pkg], Some("emela"))
        .map_err(|e| format!("cannot select the command world: {e}"))?;
    Ok((resolve, world))
}

/// The vendored WIT is the same for every compilation, so it is parsed once. The resolved
/// `Resolve` is borrowed; `wrap` does not need a mutable copy.
fn resolved() -> Result<&'static (Resolve, wit_parser::WorldId), String> {
    static RESOLVED: OnceLock<Result<(Resolve, wit_parser::WorldId), String>> = OnceLock::new();
    RESOLVED.get_or_init(resolve).as_ref().map_err(Clone::clone)
}

/// Wraps `core` (the encoded core module) into a component.
pub(crate) fn wrap(core: &[u8]) -> Result<Vec<u8>, String> {
    let (resolve, world) = resolved()?;
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
