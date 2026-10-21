//! What the WASM target does not support yet, found on the Flow IR before the Heap IR passes.
//!
//! The checks are split by what they inspect: the whole module, the suspension points and the
//! collected diagnostics here, external functions in `external`, and values (fibers, builtins) in
//! `value`.

mod external;
mod value;

use emela_lir::{Callee, Closure, Module, SeqId, Stmt, Value, VarId};
use rustc_hash::{FxHashMap, FxHashSet};

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
                external(m, f, &mut found);
            }
            continue;
        }
        // A function whose type says it may suspend (because it uses an effect with a suspend
        // operation, say) but whose body does not actually suspend needs no state machine. Only
        // the suspension points themselves are reported below.
        for (_, stmt) in f.stmts.iter() {
            if let Stmt::Let { value, .. } = stmt {
                value_unsupported(m, value, &mut found);
            }
        }
        // Suspending calls are looked for in the function's own statements and in the bodies of
        // closures it actually calls (directly or through a captured variable). An unused
        // handler operation such as `SystemClock.sleep` is only constructed, so it does not
        // report even though its body calls a suspending external.
        if let Some(body) = f.body {
            let closures = closures_of(f);
            let mut visited = FxHashSet::default();
            suspending_calls(m, f, body, &closures, &mut visited, &mut found);
        }
    }
    found.finish()
}

/// Every closure definition in `f`'s arenas, by the variable it is bound to.
fn closures_of(f: &emela_lir::Function) -> FxHashMap<VarId, &Closure> {
    let mut closures = FxHashMap::default();
    for (_, stmt) in f.stmts.iter() {
        if let Stmt::Let {
            dst: Some(dst),
            value: Value::Closure(closure),
            ..
        } = stmt
        {
            closures.insert(*dst, closure.as_ref());
        }
    }
    closures
}

/// Reports a suspending call in the sequence `seq`, following the control-flow statements and the
/// bodies of closures that are called through a local variable. `visited` stops recursive
/// closures from being walked repeatedly.
fn suspending_calls(
    m: &Module,
    f: &emela_lir::Function,
    seq: SeqId,
    closures: &FxHashMap<VarId, &Closure>,
    visited: &mut FxHashSet<SeqId>,
    found: &mut Found,
) {
    if !visited.insert(seq) {
        return;
    }
    let body = &f.seqs[seq];
    for &stmt in &body.stmts {
        match &f.stmts[stmt] {
            Stmt::Let { value, .. } => {
                suspending_value(m, value, found);
                if let Value::Call {
                    callee: Callee::Indirect(var),
                    ..
                } = value
                    && let Some(closure) = closures.get(var)
                {
                    suspending_calls(m, f, closure.body, closures, visited, found);
                }
            }
            Stmt::If { then, else_, .. } => {
                suspending_calls(m, f, *then, closures, visited, found);
                suspending_calls(m, f, *else_, closures, visited, found);
            }
            Stmt::Switch { cases, default, .. } => {
                for case in cases {
                    suspending_calls(m, f, case.body, closures, visited, found);
                }
                if let Some(default) = default {
                    suspending_calls(m, f, *default, closures, visited, found);
                }
            }
            Stmt::Loop { body, .. } | Stmt::Block { body, .. } => {
                suspending_calls(m, f, *body, closures, visited, found);
            }
            Stmt::Protect { body, cleanup, .. } => {
                suspending_calls(m, f, *body, closures, visited, found);
                suspending_calls(m, f, *cleanup, closures, visited, found);
            }
            Stmt::Dup(_) | Stmt::Drop(_) => {}
        }
    }
}

fn suspending_value(m: &Module, value: &Value, found: &mut Found) {
    // A direct call to a `suspend fn` external function has no state machine either. (A call
    // through a function value follows its type, which over-approximates: using an effect with a
    // suspend operation makes closures "may suspend" even when they only call a non-suspending
    // operation such as `Clock.now`. Those calls are emitted on the fast path; an actually
    // suspending external is a stub that raises a defect.)
    if let Value::Call {
        callee: Callee::Direct(fid),
        ..
    } = value
        && m.functions[*fid].external.is_some()
        && m.functions[*fid].suspends
    {
        found.suspending(format!(
            "a call to `{}`, a `suspend fn` external",
            display_name(&m.functions[*fid].name)
        ));
    }
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
