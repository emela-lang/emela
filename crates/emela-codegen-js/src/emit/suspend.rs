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
//! Each suspending call in a generator spends the running fiber's budget first and yields to the
//! other fibers when it runs out (spec 11.1): `($spend() && (yield $pause()), (yield* f(x)))`.
//! Functions that do not suspend (R is Immediate) get nothing, so they run in one go.
//!
//! The yield is inserted here, not by lowering: the `suspends` marks already decide where it
//! goes, so the output alone is enough and the IR stays free of scheduling details. Fiber
//! operations (join, ...) and the suspending operations of `Expr::Fiber` get no yield in front:
//! they are suspension points themselves, and cheap when they return at once. Measured on an
//! Apple M3 with node 22 (2026-10-13), the overhead was within the noise of the measurement (at
//! most about 10%) both for a loop of 10 million suspending calls and for 2 million suspend fn
//! operations resumed in place.
//!
//! Why generators and not CPS: functions without suspending calls are emitted unchanged. A
//! `$Fail` comes back as the generator's return value just the same, and self tail call loops
//! (spec 16.3) need no `yield*` because `Recur` has replaced the call.

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
            Callee::Direct { f: id, .. } => {
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
                self.paused(format!("(yield* {call})"))
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
                self.paused(format!("(yield {perform}({f}, [{}]))", args.join(", ")))
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
                self.paused(format!("(yield* {maybe}({call}))"))
            }
        }
    }

    /// A suspending call `call` in a generator, preceded by the yield for fairness (spec 11.1):
    /// the call spends the fiber's budget, and the fiber yields to the others when it runs out.
    fn paused(&mut self, call: String) -> String {
        let spend = self.rt("$spend");
        let pause = self.rt("$pause");
        format!("({spend}() && (yield {pause}()), {call})")
    }
}
