//! What the C target does not support yet, found on the Flow IR before the Heap IR passes (which
//! leave suspending functions unconverted).

use emela_core::{Builtin, Type};
use emela_lir::{Callee, Const, Function, Module, Operand, Stmt, Value};
use rustc_hash::FxHashSet;

use crate::{CError, CErrorKind};

/// Every construct reachable from the entry (the module holds only reachable functions after
/// monomorphisation) that the C target cannot compile yet.
pub(crate) fn unsupported(m: &Module) -> Vec<CError> {
    let mut errors = Vec::new();
    let mut seen = FxHashSet::default();
    let mut report = |kind, message: String| {
        if seen.insert(message.clone()) {
            errors.push(CError::new(kind, message));
        }
    };
    for (_, f) in m.functions.iter() {
        if let Some(ext) = &f.external {
            if ext.binding("c").is_none() {
                continue;
            }
            let name = display_name(&f.name);
            if f.suspends {
                report(
                    CErrorKind::SuspendExternal,
                    format!(
                        "external function `{name}` is a `suspend fn`, which target `c` does not support yet"
                    ),
                );
            }
            let tys = f.params.iter().map(|&p| f.vars[p].ty).chain([f.ret]);
            for ty in tys {
                if let Type::Fn(_) = m.core.ty(ty) {
                    let takes = if ty == f.ret { "returns" } else { "takes" };
                    report(
                        CErrorKind::Callback,
                        format!(
                            "external function `{name}` {takes} a callback, which target `c` does not support yet"
                        ),
                    );
                }
            }
            continue;
        }
        if f.suspends {
            report(CErrorKind::Unsupported, suspends(&f.name));
        }
        for (_, stmt) in f.stmts.iter() {
            let Stmt::Let { value, .. } = stmt else {
                continue;
            };
            if let Some(message) = value_unsupported(m, f, value) {
                report(CErrorKind::Unsupported, message);
            }
        }
    }
    errors
}

fn suspends(name: &str) -> String {
    format!(
        "`{}` may suspend, and target `c` does not support suspending code yet",
        display_name(name)
    )
}

fn value_unsupported(m: &Module, f: &Function, value: &Value) -> Option<String> {
    let graphemes = |what: &str| {
        format!(
            "{what} needs the Unicode grapheme cluster tables, which target `c` does not support yet"
        )
    };
    match value {
        Value::Fiber { op, .. } => Some(format!(
            "`{}` (fibers and their values) is not supported on target `c` yet",
            fiber_name(*op)
        )),
        // A `suspend fn` operation called where nothing may suspend: its implementation runs in
        // place (spec 8.7), which needs the suspend machinery too.
        Value::Call {
            callee: Callee::Op { effect, op, .. },
            ..
        } if m.core.effects[*effect].ops[*op].suspend => {
            let effect = &m.core.effects[*effect];
            Some(format!(
                "`{}.{}` is a `suspend fn` operation, and target `c` does not support suspending code yet",
                display_name(&effect.name),
                effect.ops[*op].name
            ))
        }
        Value::Closure(c) if c.suspends => Some(format!(
            "a closure in `{}` may suspend, and target `c` does not support suspending code yet",
            display_name(&f.name)
        )),
        Value::Builtin { op, args } => match op {
            Builtin::StringLength => Some(graphemes("`String.length`")),
            Builtin::StringChars => Some(graphemes("`String.chars`")),
            Builtin::StringSplit if matches!(args.get(1), Some(Operand::Const(Const::String(s))) if s.is_empty()) => {
                Some(graphemes("`String.split` with an empty separator"))
            }
            _ => None,
        },
        _ => None,
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
