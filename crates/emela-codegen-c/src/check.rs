//! What the C target does not support yet, found on the Flow IR before the Heap IR passes.

use emela_core::{Builtin, Type};
use emela_lir::{Callee, Const, Function, Module, Operand, Stmt, Value};

use crate::{CError, CErrorKind};

/// Every construct reachable from the entry (the module holds only reachable functions after
/// monomorphisation) that the C target cannot compile yet. Suspending code and fibers are
/// reported once each, naming where they are reached.
pub(crate) fn unsupported(m: &Module) -> Vec<CError> {
    let mut found = Found::default();
    for (_, f) in m.functions.iter() {
        if let Some(ext) = &f.external {
            if ext.binding("c").is_some() {
                external(m, f, &mut found);
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
    errors: Vec<CError>,
    /// Where suspending code is reached.
    suspending: Vec<String>,
    /// The fiber operations used.
    fibers: Vec<&'static str>,
}

impl Found {
    fn report(&mut self, kind: CErrorKind, message: String) {
        if !self.errors.iter().any(|e| e.message == message) {
            self.errors.push(CError::new(kind, message));
        }
    }

    fn suspending(&mut self, what: String) {
        if !self.suspending.contains(&what) {
            self.suspending.push(what);
        }
    }

    fn finish(mut self) -> Vec<CError> {
        if !self.suspending.is_empty() {
            let message = format!(
                "the entry reaches suspending code, which target `c` does not support yet: {}",
                list(&self.suspending)
            );
            self.errors
                .insert(0, CError::new(CErrorKind::Unsupported, message));
        }
        if !self.fibers.is_empty() {
            let names: Vec<String> = self.fibers.iter().map(|f| format!("`{f}`")).collect();
            let message = format!(
                "fibers are not supported on target `c` yet, and the entry uses {}",
                list(&names)
            );
            self.errors
                .push(CError::new(CErrorKind::Unsupported, message));
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

fn value_unsupported(m: &Module, f: &Function, value: &Value, found: &mut Found) {
    let graphemes = |what: &str| {
        format!(
            "{what} needs the Unicode grapheme cluster tables, which target `c` does not support yet"
        )
    };
    match value {
        Value::Fiber { op, .. } => {
            let name = fiber_name(*op);
            if !found.fibers.contains(&name) {
                found.fibers.push(name);
            }
        }
        // A `suspend fn` operation called where nothing may suspend: its implementation runs in
        // place (spec 8.7), which needs the suspend machinery too.
        Value::Call {
            callee: Callee::Op { effect, op, .. },
            ..
        } if m.core.effects[*effect].ops[*op].suspend => {
            let effect = &m.core.effects[*effect];
            found.suspending(format!(
                "`{}.{}` (a `suspend fn` operation)",
                display_name(&effect.name),
                effect.ops[*op].name
            ));
        }
        Value::Closure(c) if c.suspends => {
            found.suspending(format!("a closure in `{}`", display_name(&f.name)));
        }
        Value::Builtin { op, args } => {
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
        _ => {}
    }
}

fn fiber_name(op: emela_core::FiberOp) -> &'static str {
    use emela_core::FiberOp::*;
    match op {
        Scope { .. } => "Fiber.scope",
        Fork => "Fiber.fork",
        Deferred => "Deferred",
        Join => "Fiber.join",
        Await { .. } => "Fiber.await",
        Interrupt => "Fiber.interrupt",
        Attempt { .. } => "Fiber.attempt",
        Complete => "Deferred.complete",
        DeferredAwait => "Deferred.await",
        Poll => "Deferred.poll",
        CauseKind { .. } => "Cause.kind",
        CauseToString => "Cause.to_string",
        CauseRaise => "Cause.raise",
    }
}

/// The source name of an IR function: without the suffixes of generated functions (`$closure0`)
/// and monomorphized copies (`[Int]`).
pub(crate) fn display_name(name: &str) -> &str {
    name.split(['$', '[']).next().unwrap_or(name)
}
