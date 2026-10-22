//! What the WASM target does not support yet, found on the Flow IR before the Heap IR passes.
//!
//! The checks are split by what they inspect: the whole module and the collected diagnostics
//! here, external functions in `external`, and values (fibers, suspending calls, builtins) in
//! `value`.

mod external;
mod value;

use emela_lir::{Module, Stmt};

use crate::{WasmError, WasmErrorKind};

use self::external::external;
use self::value::value_unsupported;

/// Every construct reachable from the entry (the module holds only reachable functions after
/// monomorphisation) that the WASM target cannot compile yet. Suspending code and fibers are
/// reported once each, naming where they are reached.
pub(crate) fn unsupported(m: &Module) -> Vec<WasmError> {
    let mut found = Found::default();
    for (_, f) in m.functions.iter() {
        if let Some(ext) = &f.external {
            if ext.binding("wasm").is_some() {
                external(f, &mut found);
            }
            continue;
        }
        if f.suspends {
            found.suspending(format!("`{}`", display_name(&f.name)));
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
    /// Where suspending code is reached.
    suspending: Vec<String>,
    /// The fiber operations used.
    fibers: Vec<&'static str>,
}

impl Found {
    fn report(&mut self, kind: WasmErrorKind, message: String) {
        if !self.errors.iter().any(|e| e.message == message) {
            self.errors.push(WasmError::new(kind, message));
        }
    }

    fn suspending(&mut self, what: String) {
        if !self.suspending.contains(&what) {
            self.suspending.push(what);
        }
    }

    fn finish(mut self) -> Vec<WasmError> {
        if !self.suspending.is_empty() {
            let message = format!(
                "the entry reaches suspending code, which target `wasm` does not support yet: {}",
                list(&self.suspending)
            );
            self.errors
                .insert(0, WasmError::new(WasmErrorKind::Unsupported, message));
        }
        if !self.fibers.is_empty() {
            let names: Vec<String> = self.fibers.iter().map(|f| format!("`{f}`")).collect();
            let message = format!(
                "fibers are not supported on target `wasm` yet, and the entry uses {}",
                list(&names)
            );
            self.errors
                .push(WasmError::new(WasmErrorKind::Unsupported, message));
        }
        self.errors
    }
}

/// `a, b, c` with at most 5 items, then the count of the rest.
fn list(items: &[String]) -> String {
    const MAX: usize = 5;
    let mut out = items[..items.len().min(MAX)].join(", ");
    if items.len() > MAX {
        out.push_str(&format!(" and {} more", items.len() - MAX));
    }
    out
}

/// The source name of an IR function: without the suffixes of generated functions (`$closure0`)
/// and monomorphized copies (`[Int]`).
pub(crate) fn display_name(name: &str) -> &str {
    name.split(['$', '[']).next().unwrap_or(name)
}
