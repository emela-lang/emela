//! Emits suspension and resumption (spec 8.4, 14.5, 16.5).
//!
//! Suspending functions and lambdas (`Function::suspends`, `Expr::Lambda::suspends`) become
//! generators, and suspending calls (`Expr::Call::suspends`) are chained with `yield*`. A call to a
//! suspend fn operation is `yield $perform(h.op, [args])`, and an `@external suspend fn` is
//! `yield $await(promise)`; both send the request up to the runtime driver (`$start`) that steps
//! the outermost generator. The driver makes `resume`, calls the operation's implementation, and
//! continues with the resumed value.
//!
//! ```js
//! function* greet(ev) {
//!   const clock = ev.Clock;
//!   (yield $perform(clock.sleep, [1000]));   // a suspend fn operation
//!   return (yield* fetch_name());            // a suspending function
//! }
//! ```
//!
//! Suspending calls can also appear in a non-suspending function (R is Immediate) inside a `with`
//! of a handler that resumes immediately (the InstantClock of spec 8.7). There the generator is run
//! to completion on the spot with `$sync`, and suspend fn operations are called with
//! `$syncPerform`. If it is never resumed, `$Interrupted` is thrown (spec 14.5).
//!
//! Functions without suspending calls are emitted unchanged. A `$Fail` comes back as the
//! generator's return value just the same, and self tail call loops (spec 16.3) need no `yield*`
//! because `Recur` has replaced the call.

use emela_core::Callee;

use super::Emitter;

impl Emitter<'_> {
    /// A call of `f` with evaluated `args` (JS expressions). `suspends` is
    /// `Expr::Call::suspends`.
    pub(super) fn suspending_call(
        &mut self,
        callee: &Callee,
        f: &str,
        args: &[String],
        suspends: bool,
    ) -> String {
        let call = format!("{f}({})", args.join(", "));
        match callee {
            Callee::Direct(id) => {
                let target = &self.m.functions[*id];
                assert_eq!(
                    target.suspends, suspends,
                    "invalid IR: the call to `{}` disagrees with the function's suspends flag",
                    target.name
                );
                if !suspends {
                    return call;
                }
                if !self.generator {
                    return format!("{}({call})", self.rt("$sync"));
                }
                format!("(yield* {call})")
            }
            Callee::Op { effect, op, .. } if self.m.effects[*effect].ops[*op].suspend => {
                let def = &self.m.effects[*effect];
                assert!(
                    suspends,
                    "invalid IR: a call to the suspend fn operation `{}.{}` is marked as not suspending",
                    def.name, def.ops[*op].name
                );
                if !self.generator {
                    let perform = self.rt("$syncPerform");
                    return format!("{perform}({f}, [{}])", args.join(", "));
                }
                let perform = self.rt("$perform");
                format!("(yield {perform}({f}, [{}]))", args.join(", "))
            }
            // Function values and fn operations may turn out not to suspend; `$maybe` tells them
            // apart.
            Callee::Indirect(_) | Callee::Op { .. } => {
                if !suspends {
                    return call;
                }
                if !self.generator {
                    return format!("{}({call})", self.rt("$sync"));
                }
                let maybe = self.rt("$maybe");
                format!("(yield* {maybe}({call}))")
            }
        }
    }
}
