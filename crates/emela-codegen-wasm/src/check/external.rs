//! External functions with `@external(wasm, ...)`.

use super::*;

use emela_core::Type;
use emela_lir::Function;

/// An external function with `@external(wasm, ...)`: an uncoded diagnostic for a `suspend fn`
/// (E0428 was retired when the C target learned to run them), E0427 for callbacks.
pub(super) fn external(m: &Module, f: &Function, found: &mut Found) {
    let name = display_name(&f.name);
    if f.external
        .as_ref()
        .is_some_and(|e| e.binding("wasm").is_some())
    {
        found.report(
            WasmErrorKind::Unsupported,
            format!(
                "external function `{name}` has a `wasm` import, which the WASI 0.3 component target does not support yet"
            ),
        );
    }
    if f.suspends {
        found.report(
            WasmErrorKind::SuspendExternal,
            format!(
                "external function `{name}` is a `suspend fn`, which target `wasm` does not support yet"
            ),
        );
    }
    let params = f.params.iter().map(|&p| (f.vars[p].ty, "takes"));
    for (ty, verb) in params.chain([(f.ret, "returns")]) {
        if let Type::Fn(_) = m.core.ty(ty) {
            found.report(
                WasmErrorKind::Callback,
                format!(
                    "external function `{name}` {verb} a callback, which target `wasm` does not support yet"
                ),
            );
        }
    }
}
