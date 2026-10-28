//! What the C target does not support yet, found on the Flow IR before the Heap IR passes.
//!
//! The check runs after monomorphisation from the entry, so it only sees what the entry reaches
//! (`[target.c.unsupported]`).

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

/// An external function with `@external(c, ...)`: E0427 for callbacks and E0443 for Bytes, whose
/// C form is not decided yet (`[ext.boundary]`). A `suspend fn` external function completes its
/// task later (spec 12.6).
fn external(m: &Module, f: &Function, found: &mut Found) {
    let name = display_name(&f.name);
    let params = f.params.iter().map(|&p| (f.vars[p].ty, "takes"));
    for (ty, verb) in params.chain([(f.ret, "returns")]) {
        match m.core.ty(ty) {
            Type::Fn(_) => found.report(
                CErrorKind::Callback,
                format!(
                    "external function `{name}` {verb} a callback, which target `c` does not support yet"
                ),
            ),
            Type::Bytes => found.report(
                CErrorKind::Bytes,
                format!(
                    "external function `{name}` {verb} `Bytes`, which target `c` does not support yet"
                ),
            ),
            _ => {}
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
