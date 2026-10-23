//! External functions with `@external(wasm, ...)`.

use super::*;

use emela_lir::Function;

/// An external function with `@external(wasm, ...)`. The WASI 0.3 component can only import the
/// interfaces of its WIT world and custom host WIT is not implemented, so every such function is
/// unsupported except the bundled `Std` externals the runtime implements; one written in a
/// program is unsupported even when it names the same WASI function. It is reported once, not
/// again for being a `suspend fn` or taking a callback.
pub(super) fn external(f: &Function, found: &mut Found) {
    if crate::std_host::classify(f).is_some() {
        return;
    }
    let name = display_name(&f.name);
    found.report(
        WasmErrorKind::Unsupported,
        format!(
            "external function `{name}` has a `wasm` import, which the WASI 0.3 component target does not support yet"
        ),
    );
}
