//! What the WASM target does not support yet, found on the Flow IR before the Heap IR passes.

use emela_core::{Builtin, Type};
use emela_lir::{Const, Function, Module, Operand, Stmt, Value};

use crate::{WasmError, WasmErrorKind};

/// Every construct reachable from the entry (the module holds only reachable functions after
/// monomorphisation) that the WASM target cannot compile yet. The Unicode tables are reported once
/// each, naming where they are reached. Suspending code, the state machines and fibers are
/// supported.
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

/// An external function with `@external(wasm, ...)`: E0427 for callbacks. (A `suspend fn` external
/// function is supported.)
fn external(m: &Module, f: &Function, found: &mut Found) {
    let name = display_name(&f.name);
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
