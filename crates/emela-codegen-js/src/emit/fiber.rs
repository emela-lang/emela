//! Emits the fiber operations (spec chapter 11).
//!
//! Each operation is a call of a runtime function. Operations that wait (`Fiber.scope`, join,
//! await, interrupt, `Deferred.await`) are generators or requests handled by the scheduler, so
//! they only appear in generators:
//!
//! ```js
//! const $t0 = (yield* $scope((function* (s$1) { … })));   // Fiber.scope
//! const a$2 = $fork(s$1, (function* () { … }));            // s.fork(f)
//! const v$3 = (yield $join(a$2));                          // Fiber.join(a)
//! ```
//!
//! `Fiber.attempt` follows the rules of a function value call: `$attempt` returns a generator only
//! when `f` does.

use emela_core::{
    CAUSE_DIE, CAUSE_FAIL, CAUSE_INTERRUPT, EXIT_FAILURE, EXIT_SUCCESS, EnumId, Expr, FiberOp,
    VariantFields,
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
            FiberOp::Await { exit } | FiberOp::Attempt { exit, .. } => self.check_exit(exit),
            FiberOp::CauseKind { kind } => self.check_cause_kind(kind),
            _ => {}
        }
        let es: Vec<&Expr> = args.iter().collect();
        let vs = self.values_in_order(&es);
        let vs = vs.join(", ");
        let (f, waits) = match op {
            FiberOp::Scope { .. } => ("$scope", Wait::Generator),
            FiberOp::Fork => ("$fork", Wait::No),
            FiberOp::Deferred => ("$deferred", Wait::No),
            FiberOp::Join => ("$join", Wait::Request),
            FiberOp::Await { .. } => ("$fiberAwait", Wait::Request),
            FiberOp::Interrupt => ("$interrupt", Wait::Request),
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
            FiberOp::CauseKind { .. } => ("$causeKind", Wait::No),
            FiberOp::CauseToString => ("$causeToString", Wait::No),
            FiberOp::CauseRaise { .. } => ("$causeRaise", Wait::No),
        };
        let call = format!("{}({vs})", self.rt(f));
        if waits != Wait::No {
            // These require Async, so R is never Immediate here.
            assert!(
                self.generator,
                "invalid IR: fiber operation {} outside a suspending function",
                op.name()
            );
        }
        match waits {
            Wait::No => call,
            Wait::Generator => format!("(yield* {call})"),
            Wait::Request => format!("(yield {call})"),
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
        let ok = def.variants.len() == 3
            && [CAUSE_FAIL, CAUSE_DIE, CAUSE_INTERRUPT]
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
