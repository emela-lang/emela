//! What the WASM target does not support yet, found on the Flow IR before the Heap IR passes.

use emela_core::{Builtin, Type};
use emela_lir::{Const, Function, Module, Operand, Stmt, Value};

use crate::{WasmError, WasmErrorKind};

/// Every construct reachable from the entry (the module holds only reachable functions after
/// monomorphisation) that the WASM target cannot compile yet. Only the Unicode grapheme-cluster
/// operations (`String.length` / `String.chars` / an empty-separator `String.split`), external
/// functions that take or return a callback (Wave 4 / E0427) and `suspend fn` externals bound to
/// something other than an internal host are left; suspending code, the state machines, fibers,
/// `Float` and the internal hosts are all supported.
///
/// A construct is reported once: the message is the key ([`Found::report`]), so two uses of the
/// same operation give one diagnostic (the same wording as the C target's check). The diagnostic
/// carries no source location or function name — the codegen only has the message.
pub(crate) fn unsupported(m: &Module) -> Vec<WasmError> {
    let mut found = Found::default();
    for (_, f) in m.functions.iter() {
        if let Some(ext) = &f.external {
            if ext.binding("wasm").is_some() {
                external(m, f, &mut found);
            }
            continue;
        }
        for (_, stmt) in f.stmts.iter() {
            if let Stmt::Let { value, .. } = stmt {
                value_unsupported(m, f, value, &mut found);
            }
        }
    }
    found.finish()
}

#[derive(Default)]
struct Found {
    errors: Vec<WasmError>,
}

impl Found {
    fn report(&mut self, kind: WasmErrorKind, message: String) {
        if !self.errors.iter().any(|e| e.message == message) {
            self.errors.push(WasmError::new(kind, message));
        }
    }

    fn finish(self) -> Vec<WasmError> {
        self.errors
    }
}

/// An external function with `@external(wasm, ...)`: E0427 for callbacks, and a `suspend fn` that
/// binds to anything but an internal host is rejected (the runtime function is what drives the
/// task ABI of spec 12.6; a plain import cannot suspend).
fn external(m: &Module, f: &Function, found: &mut Found) {
    let name = display_name(&f.name);
    if f.suspends {
        let binding = crate::emit::external::wasm_import(f);
        let internal = binding.as_ref().is_some_and(|(module, import)| {
            crate::emit::external::internal_host_name(module, import).is_some()
        });
        if !internal {
            let at = binding
                .map(|(module, import)| format!("the WASM import `{module}`.`{import}`"))
                .unwrap_or_else(|| "a WASM binding".to_owned());
            found.report(
                WasmErrorKind::SuspendExternal,
                format!(
                    "`suspend fn` external function `{name}` is bound to {at}, which is not an \
                     internal host; only an internal host can suspend (spec 12.6)"
                ),
            );
            return;
        }
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

fn value_unsupported(_m: &Module, _f: &Function, value: &Value, found: &mut Found) {
    let graphemes = |what: &str| {
        format!(
            "{what} needs the Unicode grapheme cluster tables, which target `wasm` does not support yet"
        )
    };
    if let Value::Builtin { op, args } = value {
        let message = match op {
            Builtin::StringLength => graphemes("`String.length`"),
            Builtin::StringChars => graphemes("`String.chars`"),
            Builtin::StringSplit if matches!(args.get(1), Some(Operand::Const(Const::String(s))) if s.is_empty()) => {
                graphemes("`String.split` with an empty separator")
            }
            _ => return,
        };
        found.report(WasmErrorKind::Unsupported, message);
    }
}

/// The source name of an IR function: without the suffixes of generated functions (`$closure0`)
/// and monomorphized copies (`[Int]`).
pub(crate) fn display_name(name: &str) -> &str {
    name.split(['$', '[']).next().unwrap_or(name)
}
