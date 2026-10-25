//! `@export(wasm, ...)` written by a user.
//!
//! A component can only export the interfaces of its WIT world. The entry adapters of the
//! bundled `Std` are exposed by the runtime; a user-written `@export(wasm, ...)` is a compile
//! error ([`target.wasm.external`]). It is checked on Core IR before monomorphisation, so it is
//! reported even when the fn is not reachable from the entry.

use emela_core::Module;

use crate::{WasmError, WasmErrorKind};

/// The user's `@export(wasm, ...)` bindings. The bundled core's bindings are the entry adapters
/// the runtime implements.
pub(crate) fn user_exports(core: &Module) -> Vec<WasmError> {
    let mut errors = Vec::new();
    for (_, f) in core.functions.iter() {
        if f.exports.iter().any(|e| e.target == "wasm" && !e.bundled) {
            let name = super::display_name(&f.name);
            let message = format!(
                "exported function `{name}` has a `wasm` export, which the WASI 0.3 component target does not support yet"
            );
            if !errors.iter().any(|e: &WasmError| e.message == message) {
                errors.push(WasmError::new(WasmErrorKind::Unsupported, message));
            }
        }
    }
    errors
}
