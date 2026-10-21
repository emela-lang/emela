//! External functions with `@external(wasm, ...)`.

use super::*;

use emela_lir::Function;

/// An external function with `@external(wasm, ...)`. The WASI 0.3 component can only import the
/// interfaces of its WIT world and custom host WIT is not implemented, so every such function is
/// unsupported except a WASI 0.3 host the bundled `Std` modules use. It is reported once, not
/// again for being a `suspend fn` or taking a callback.
pub(super) fn external(m: &Module, f: &Function, found: &mut Found) {
    if crate::std_host::classify(m, f).is_some() {
        return;
    }
    let name = display_name(&f.name);
    found.report(
        WasmErrorKind::Unsupported,
        format!(
            "external function `{name}` has a `wasm` import, which is not a supported WASI 0.3 host function"
        ),
    );
}
