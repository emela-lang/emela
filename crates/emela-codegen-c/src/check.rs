//! What the C target does not support yet, found on the Flow IR before the Heap IR passes.

use emela_core::{Builtin, Type};
use emela_lir::{Const, Function, Module, Operand, Stmt, Value};

use crate::{CError, CErrorKind};

/// Every construct reachable from the entry (the module holds only reachable functions after
/// monomorphisation) that the C target cannot compile yet.
pub(crate) fn unsupported(m: &Module) -> Vec<CError> {
    let mut found = Found::default();
    for (_, f) in m.functions.iter() {
        if let Some(ext) = &f.external {
            if ext.binding("c").is_some() {
                external(m, f, &mut found);
            }
            continue;
        }
        for (_, stmt) in f.stmts.iter() {
            if let Stmt::Let { value, .. } = stmt {
                value_unsupported(value, &mut found);
            }
        }
    }
    found.errors
}

#[derive(Default)]
struct Found {
    errors: Vec<CError>,
}

impl Found {
    fn report(&mut self, kind: CErrorKind, message: String) {
        if !self.errors.iter().any(|e| e.message == message) {
            self.errors.push(CError::new(kind, message));
        }
    }
}

/// An external function with `@external(c, ...)`: E0428 for a `suspend fn`, E0427 for callbacks.
fn external(m: &Module, f: &Function, found: &mut Found) {
    let name = display_name(&f.name);
    if f.suspends {
        found.report(
            CErrorKind::SuspendExternal,
            format!(
                "external function `{name}` is a `suspend fn`, which target `c` does not support yet"
            ),
        );
    }
    let params = f.params.iter().map(|&p| (f.vars[p].ty, "takes"));
    for (ty, verb) in params.chain([(f.ret, "returns")]) {
        if let Type::Fn(_) = m.core.ty(ty) {
            found.report(
                CErrorKind::Callback,
                format!(
                    "external function `{name}` {verb} a callback, which target `c` does not support yet"
                ),
            );
        }
    }
}

fn value_unsupported(value: &Value, found: &mut Found) {
    let graphemes = |what: &str| {
        format!(
            "{what} needs the Unicode grapheme cluster tables, which target `c` does not support yet"
        )
    };
    let Value::Builtin { op, args } = value else {
        return;
    };
    let message = match op {
        Builtin::StringLength => graphemes("`String.length`"),
        Builtin::StringChars => graphemes("`String.chars`"),
        Builtin::StringSplit if matches!(args.get(1), Some(Operand::Const(Const::String(s))) if s.is_empty()) => {
            graphemes("`String.split` with an empty separator")
        }
        _ => return,
    };
    found.report(CErrorKind::Unsupported, message);
}

/// The source name of an IR function: without the suffixes of generated functions (`$closure0`)
/// and monomorphized copies (`[Int]`).
pub(crate) fn display_name(name: &str) -> &str {
    name.split(['$', '[']).next().unwrap_or(name)
}
