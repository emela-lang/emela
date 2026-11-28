//! Emits the fiber operations (spec chapter 11).
//!
//! Each operation is a call of a runtime function. Operations that wait (the run-time scope of a
//! group or a background, join, `Deferred.await`, acquire, offer, take) are generators or requests
//! handled by the scheduler, chained in generators:
//!
//! ```js
//! const $t0 = (yield* $scope((function* (s$1) { … })));   // with tasks = Task.Group
//! const a$2 = $fork(s$1, (function* () { … }));            // tasks.start(f)
//! const v$3 = (yield $join(a$2));                          // a.join()
//! ```
//!
//! A request can also appear in a function that does not suspend. The built-in handlers Deferred,
//! Semaphore and Queue do not require Async, so `with d = Deferred { d.await() }` leaves R
//! Immediate (`[effect.with]`, `[task.deferred]`) and the function is not a generator (issue
//! #942). No other fiber can use a capability made there, so the request either is served on the
//! spot or is never served. `$syncWait` runs it in place and throws `$Interrupted` when it is not
//! served (`[dyn.suspend]`), as `$syncPerform` does for a suspend fn operation.
//!
//! `Exit.attempt` follows the rules of a function value call: `$attempt` returns a generator only
//! when `f` does.

use emela_core::{
    CAUSE_DIE, CAUSE_FAIL, EXIT_FAILURE, EXIT_SUCCESS, EnumId, Expr, FiberOp, VariantFields,
};

use super::Emitter;

impl Emitter<'_> {
    /// The JS expression of a fiber operation. A failure is not checked here.
    pub(super) fn fiber(&mut self, op: FiberOp, args: &[Expr]) -> String {
        assert_eq!(
            args.len(),
            op.arity(),
            "invalid IR: wrong number of arguments to fiber operation {}",
            op.name()
        );
        match op {
            FiberOp::Attempt { exit, .. } => self.check_exit(exit),
            FiberOp::CauseKind { kind } => self.check_cause_kind(kind),
            _ => {}
        }
        let es: Vec<&Expr> = args.iter().collect();
        let vs = self.values_in_order(&es);
        let vs = vs.join(", ");
        let (f, waits) = match op {
            // A background passes the flag after the body (`$scope`).
            FiberOp::Scope { .. } => ("$scope", Wait::Generator),
            FiberOp::Fork => ("$fork", Wait::No),
            FiberOp::Deferred => ("$deferred", Wait::No),
            FiberOp::Join => ("$join", Wait::Request),
            FiberOp::Attempt { suspends, .. } => {
                let call = format!("{}({vs})", self.rt("$attempt"));
                if !suspends {
                    return call;
                }
                if !self.generator {
                    return format!("{}({call})", self.rt("$sync"));
                }
                return format!("(yield* {}({call}))", self.rt("$maybe"));
            }
            FiberOp::Complete => ("$complete", Wait::No),
            FiberOp::DeferredAwait => ("$deferredAwait", Wait::Request),
            FiberOp::Poll => ("$poll", Wait::No),
            FiberOp::Semaphore => ("$semaphore", Wait::No),
            FiberOp::Queue => ("$queue", Wait::No),
            FiberOp::Acquire => ("$acquire", Wait::Request),
            FiberOp::TryAcquire => ("$tryAcquire", Wait::No),
            FiberOp::Release => ("$release", Wait::No),
            FiberOp::Available => ("$available", Wait::No),
            FiberOp::Offer => ("$offer", Wait::Request),
            FiberOp::Take => ("$take", Wait::Request),
            FiberOp::TryOffer => ("$tryOffer", Wait::No),
            FiberOp::QueuePoll => ("$queuePoll", Wait::No),
            FiberOp::Size => ("$size", Wait::No),
            FiberOp::Close => ("$close", Wait::No),
            FiberOp::CauseKind { .. } => ("$causeKind", Wait::No),
            FiberOp::CauseToString => ("$causeToString", Wait::No),
            FiberOp::CauseRaise { .. } => ("$causeRaise", Wait::No),
        };
        // A background passes the flag after the body (`$scope`).
        let call = match op {
            FiberOp::Scope {
                background: true, ..
            } => format!("{}({vs}, true)", self.rt(f)),
            _ => format!("{}({vs})", self.rt(f)),
        };
        match waits {
            Wait::No => call,
            Wait::Generator => {
                // A group or a background requires Async, so R is never Immediate here.
                assert!(
                    self.generator,
                    "invalid IR: fiber operation {} outside a suspending function",
                    op.name()
                );
                format!("(yield* {call})")
            }
            Wait::Request if self.generator => format!("(yield {call})"),
            Wait::Request => format!("{}({call})", self.rt("$syncWait")),
        }
    }

    /// The runtime builds `Exit` values as `{ $tag: 0 | 1, $0: … }`.
    fn check_exit(&self, exit: EnumId) {
        let def = &self.m.enums[exit];
        let ok = def.variants.len() == 2
            && [EXIT_SUCCESS, EXIT_FAILURE]
                .iter()
                .all(|&k| def.variants[k].fields == VariantFields::Positional(1));
        assert!(ok && !def.error, "invalid IR: Exit has the wrong shape");
    }

    /// The runtime returns `CauseKind` values as `{ $tag: k }`.
    fn check_cause_kind(&self, kind: EnumId) {
        let def = &self.m.enums[kind];
        let ok = def.variants.len() == 2
            && [CAUSE_FAIL, CAUSE_DIE]
                .iter()
                .enumerate()
                .all(|(i, &k)| i == k && def.variants[k].fields.is_empty());
        assert!(
            ok && !def.error,
            "invalid IR: CauseKind has the wrong shape"
        );
    }
}

#[derive(PartialEq, Eq)]
enum Wait {
    /// Does not suspend.
    No,
    /// A runtime generator, chained with `yield*`.
    Generator,
    /// A request to the scheduler, sent with `yield`.
    Request,
}
