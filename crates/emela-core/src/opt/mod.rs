//! Optimisation of the Core IR: the target-neutral pipeline after lowering, and the δ stage on the
//! monomorphic module.
//!
//! The passes work on the target-neutral Core IR, so every target gets them. [`run`] is the fixed
//! sequence; there is no generic pass framework. A build turns all of them off together with
//! `--no-opt`, so the same source can be compiled with and without them and the observable
//! behaviour compared. [`run_with`] runs the same sequence with individual reduction rules turned
//! on or off ([`Rules`], [`Options`]).
//!
//! Every pass preserves the observable behaviour of the program: evaluation order, effects,
//! failures, the order and count of the releases of `with`, and drop points. It may change memory
//! use and when a program runs out of memory (spec `[effect.handler]`, `[target.native]`). A pass
//! that cannot keep a contract gives up on that expression instead of changing it.
//!
//! # Pipeline
//!
//! [`run_with`] calls the passes in this order. A later pass may rely on an earlier one having run,
//! and says so in its rustdoc. The passes in [`reduce`] have the shape
//! `fn(&mut Module) -> RuleStats` and fill only their own counter; the older passes here have the
//! shape `fn(&mut Module) -> PassStats`; the fusion laws have the shape
//! `fn(&mut Module, &CoreFns) -> PassStats` and run through [`fusion::run`]. [`run_with`] merges
//! all of them into one [`Stats`].
//!
//! | Pass | Counter | Added by |
//! |---|---|---|
//! | `reduce::zeta` | [`Stats::zeta`] | ζ reduction (A2/PR-011) |
//! | `loop::unswitch` | [`Stats::hoist`] | Loop unswitching (A9/PR-018) |
//! | `fusion::run`: `option_or` | [`Stats::option_or`] | Option fusion |
//! | `fusion::run`: `array_get_match` | [`Stats::array_get_match`] | Option fusion |
//! | `fusion::run`: `range_fold` | [`Stats::range_fold`] | range fusion |
//! | `fusion::run`: `range_map_array` | [`Stats::map_range`] | range fusion |
//! | `fusion::run`: `range_map` | [`Stats::range_map`] | range fusion |
//! | `reduce::delta` | [`Stats::delta`] | δ reduction (A8/PR-017) |
//! | `reduce::beta` | [`Stats::beta`] | β reduction (A3/PR-012) |
//! | `reduce::iota` | [`Stats::iota`] | ι reduction (A5/PR-014) |
//! | `reduce::fold` | [`Stats::constant`] | constant folding (A6/PR-015) |
//! | `reduce::algebra` | [`Stats::algebra`] | algebraic reduction (A7/PR-016) |
//! | `reduce::eta` | [`Stats::eta`] | η reduction (A4/PR-013) |
//!
//! The positions are chosen for the patterns each pass exposes:
//!
//! - **ζ first.** Lowering and the previous sweep's inlining leave chains of `let` bindings;
//!   propagating copies and removing dead trivial bindings first is what lets every later pass see
//!   the names and constructors inside them. ζ only shrinks, so it cannot hide a fusion pattern.
//!   It is also what removes the `let` copies inlining leaves before the unswitch looks for a
//!   variable bound outside the loop.
//! - **The unswitch before the fusions.** It matches the bounds test the Option fusion built on
//!   the previous sweep, and runs before that fusion again so it sees the test before the fusion
//!   can reshape it. It runs before δ so that it sees the loop before a callee's body is copied
//!   into it.
//! - **The fusions before δ.** A fusion matches the call the source wrote (`List.fold`,
//!   `Option.or`); expanding those functions first would dissolve the pattern. `range_map_array`
//!   comes before `range_map`, which would otherwise rewrite the `List.map` inside the
//!   `Array.from_list` it is looking for. The order of the laws is the registry order of
//!   [`fusion::laws`], not a second list in this pipeline.
//! - **δ, then β.** δ expands the direct calls; β applies the linear lambdas that survived the
//!   expansion and the ones a copy brought in. β runs after δ for that reason, and both run after
//!   the fusions.
//! - **ι, then constant folding and algebra.** δ and β expose a known constructor and a constant
//!   condition; ι removes the match, and folding and the algebraic laws simplify the values and
//!   conditions it leaves in the same sweep.
//! - **η last.** The range fusion copies its lambda into a loop; running η before it would turn
//!   that lambda into a function value and make the loop's step an indirect call. The algebraic
//!   laws also remove forwarding bodies, which leaves more wrapper lambdas for η to see.
//!
//! δ is the only rule that may grow the program, so it carries a budget; every shrinking rule runs
//! before it, and a body δ copies is simplified on the next sweep.
//!
//! # Status of the reduction kernel
//!
//! A0/PR-002 added the rules and the order above; each rule landed with its own pull request
//! (A2–A8). A10/PR-019 turns every implemented rule on by default, so [`run`] and
//! [`run_with`] with [`Options::default`] run the whole pipeline. A rule whose flag is off is
//! skipped entirely and contributes nothing to [`Stats`], not even a deopt.
//!
//! `bounds` and `cse` stay off in [`Rules::default`]: their rules do not exist yet (A11/PR-040,
//! A12/PR-041) and [`Rules::all`] turns them on for the tests that will fill them.
//!
//! # Fusion laws
//!
//! The fusions are library laws, not reductions of the λ-calculus: each replaces a call of a core
//! function by the operation it stands for, and each has a contract (Rule ID, Theory, Equation,
//! Preconditions, Postconditions, Preserves, Deopt, Budget, Measures) in [`fusion::laws`]. The
//! pipeline calls [`fusion::run`] once in the place of the five fusion passes it used to call
//! directly; the registry order is the pipeline order, and the per-sweep budget is
//! [`fusion::MAX_FUSIONS_PER_SWEEP`]. See [`fusion`] for the flags, the reserved laws and the
//! budget's deopt rule.
//!
//! # Feature flags
//!
//! [`Rules`] has one flag per reduction rule of [`reduce`], plus reserved flags for the later
//! bounds-check and CSE rules. [`fusion::Laws`] has one flag per fusion law. [`Options`] holds both
//! and is the argument of [`run_with`]; [`run`] keeps its old signature and uses
//! [`Options::default`].
//!
//! [`Rules::default`] turns every implemented reduction rule on: `zeta`, `beta`, `eta`, `iota`,
//! `delta`, `fold` and `algebra`. `bounds` and `cse` are off, because their rules do not exist yet;
//! [`Rules::all`] turns every flag on and [`Rules::none`] turns every flag off. `loop::unswitch` is
//! a loop transformation, not a reduction rule, and has no flag: it runs whatever the flags say.
//! The flags are a programmatic API only: no command-line flag is added.
//!
//! [`fusion::Laws::default`] turns on the five implemented fusion laws and leaves the reserved ones
//! off. A disabled law is skipped entirely and contributes nothing to [`Stats`], not even a deopt;
//! [`fusion::Laws::none`] therefore turns the fusions off without touching the rest of the
//! pipeline. The reserved laws have no body and never run, whatever their flag says.
//!
//! # Fixed point
//!
//! [`run_with`] applies the table repeatedly, in the same order, until a whole sweep changes
//! nothing, with a cap of [`MAX_PASSES`] sweeps. The cap is only a guard: in debug builds
//! [`run_with`] asserts that the sweep converged, so a pass that never settles fails the tests
//! instead of stopping quietly. [`Stats::passes`] records how many sweeps changed the module,
//! counting one when the first sweep already changed nothing. A rule's [`Stats::deopt`] is not a
//! change and does not keep the sweep going.
//!
//! One sweep is not enough, because the order itself creates work for earlier passes:
//!
//! ```emel
//! fn span() -> List[Int] {
//!   List.range(0, 3)
//! }
//!
//! fn main() {
//!   dbg(List.fold(span(), 0, |a, b| a + b))
//! }
//! ```
//!
//! The range fusion looks for `List.fold(List.range(…), …)`. It does not see one in the first
//! sweep, because the argument is the call `span()`; δ replaces the call by its body, but δ runs
//! after the fusions. The next sweep finds `List.fold(List.range(0, 3), …)` and rewrites it.
//! Repeating the whole table keeps the order that the fusions need and still finds the patterns δ
//! exposes. A pattern that appears only by substituting an argument into a parameter is not exposed
//! at all: δ binds each argument to a fresh local first, so the pattern has to be in the callee's
//! body. A module that is already at the fixed point takes one sweep, which changes nothing.
//!
//! # After monomorphisation
//!
//! A build checks the module and monomorphizes it after [`run`] (`crate::mono`), and then runs one
//! further stage on the monomorphic module: δ reduction alone, through
//! `delta_after_monomorphization`. Nothing else runs there. This is not a second pipeline and it
//! has no fixed point of its own; the stage is deliberately δ only, with the rule's existing
//! contract, budget and counter.
//!
//! The reason is the rule's `set_params` precondition. Before monomorphisation a callee whose E or
//! R still mentions a set type parameter is left as a call, because its evidence and type
//! arguments do not substitute with [`crate::ir::TypeTable::subst`] alone. A small higher-order
//! wrapper such as `fn apply(f, x) { f(x) }` is exactly such a callee. After monomorphisation its
//! copy has no type or set parameters and no unresolved dispatch, so the same rule can copy it.
//! The WASM backend has no inliner of its own, so a call that survives the Core IR stays a call
//! there.
//!
//! The stage's invariant is that its input is monomorphic and that its [`CoreFns`] map is rebuilt
//! for the output function ids: each output function is followed through
//! `Monomorphized::origins` to the input function it copies, and then through the identity map
//! lowering built. A stale or empty map would let δ copy a reserved bundled collection function
//! (`List.fold`, `List.reverse`, ...) into every caller. See `delta_after_monomorphization` and
//! [`reduce::delta`] for the stage and the contract.
//!
//! # Pass contract
//!
//! A reduction rule's rustdoc carries the nine lines of plan §3.4:
//!
//! ```text
//! Rule ID: the name of the rule
//! Theory: the theory it comes from
//! Equation: the rewrite, as an equation
//! Preconditions: when the rule applies
//! Postconditions: what the rule guarantees
//! Preserves: evaluation order, effects, failure edges, suspension points, drop points, evidence
//! Deopt: leave the expression alone when a precondition fails or an analysis answers `Unknown`
//! Budget: the size or cost limit, if the rule may grow the program
//! Measures: the counter and the benches it targets
//! ```
//!
//! An analysis carries the six lines of plan §3.4 (Analysis ID, Domain / lattice, Transfer,
//! Soundness, Deopt (unknown), Consumers). The older passes in this module keep the five-line form
//! they were written with:
//!
//! ```text
//! Preconditions: when the pass applies
//! Postconditions: what it guarantees (the invariants `verify` can show)
//! Preserves: evaluation order, effects, failure edges, drop points, evidence
//! Deopt: leave the expression alone when it reaches `Any`, a container, a closure capture, a
//!        saved frame or an external call
//! Measures: the benches and allocation counts it targets
//! ```
//!
//! In debug builds [`run_with`] checks the module with [`crate::verify`] after every pass of every
//! sweep, so a rewrite that breaks typing fails where it was made.

use rustc_hash::FxHashMap;

use crate::ir::{Expr, FnId, Lit, Module};

pub mod analysis;
pub mod fusion;
pub mod r#loop;
pub mod reduce;

/// The result of one reduction rule over one module.
///
/// A rule that applies its equation `n` times reports `applied: n`; a rule that examines a site and
/// leaves it alone because a precondition or an analysis failed reports `deopt`. A deopt is not a
/// rewrite: it never keeps the fixed-point loop going and it does not make a run non-empty.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RuleStats {
    /// Sites the rule rewrote.
    pub applied: u32,
    /// Sites the rule left alone (a failed precondition, or an analysis that answered `Unknown`).
    pub deopt: u32,
}

impl RuleStats {
    /// A rule result with `applied` rewrites and no deopt.
    pub const fn applied(applied: u32) -> Self {
        Self { applied, deopt: 0 }
    }
}

/// The reduction rules [`run_with`] may run, one flag per rule.
///
/// [`Rules::default`] turns on every implemented rule, which is the default pipeline. `bounds` and
/// `cse` are reserved for A11/PR-040 and A12/PR-041; no pass reads them yet, so they stay off.
/// [`Rules::all`] turns every flag on, which the per-rule tests use, and [`Rules::none`] turns
/// every flag off. The unswitch of [`r#loop`] and the fusion laws of [`fusion`] are not gated here;
/// the unswitch has no flag, and the laws have their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// ζ reduction: `let x = v in e → e[x := v]` and the dead trivial let.
    pub zeta: bool,
    /// β reduction: a linear `let f = λx.e in C[f v]`.
    pub beta: bool,
    /// η reduction: `λx. f x → f`.
    pub eta: bool,
    /// ι reduction: a match of a known constructor.
    pub iota: bool,
    /// δ reduction: expansion of a direct call (inlining), within a budget.
    pub delta: bool,
    /// Constant folding.
    pub fold: bool,
    /// Algebraic reduction of Bool and integer operations.
    pub algebra: bool,
    /// Bounds-check elimination. Reserved; no rule reads it yet (A11/PR-040).
    pub bounds: bool,
    /// Common subexpression elimination. Reserved; no rule reads it yet (A12/PR-041).
    pub cse: bool,
}

impl Rules {
    /// Every flag on.
    pub const fn all() -> Self {
        Self {
            zeta: true,
            beta: true,
            eta: true,
            iota: true,
            delta: true,
            fold: true,
            algebra: true,
            bounds: true,
            cse: true,
        }
    }

    /// Every flag off. The passes without a flag still run (see the type documentation).
    pub const fn none() -> Self {
        Self {
            zeta: false,
            beta: false,
            eta: false,
            iota: false,
            delta: false,
            fold: false,
            algebra: false,
            bounds: false,
            cse: false,
        }
    }
}

impl Default for Rules {
    /// Every implemented rule on; `bounds` and `cse` reserved and off.
    ///
    /// This is the default pipeline (A10/PR-019). [`Rules::all`] differs only in the two reserved
    /// flags.
    fn default() -> Self {
        Self {
            zeta: true,
            beta: true,
            eta: true,
            iota: true,
            delta: true,
            fold: true,
            algebra: true,
            bounds: false,
            cse: false,
        }
    }
}

/// The options of [`run_with`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Which reduction rules to run. [`Rules::default`] (every implemented rule on) by default.
    pub rules: Rules,
    /// Which fusion laws to run. [`fusion::Laws::default`] (the implemented laws on) by default,
    /// so `Options::default()` is the pipeline from before the registry.
    pub laws: fusion::Laws,
}

/// A fn of the bundled core that a Core IR pass matches by identity, not by name.
///
/// Lowering records the IR function of every copy(`List.fold$suspend3` too) in the map it passes
/// to [`run`]. A user's fn with the same name is a different [`FnId`], so a pass never confuses
/// the two. Inlining excludes every function recorded here: the fusions match them, and copying
/// them into every caller would grow the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFn {
    /// `Option.or`, which the Option fusion matches.
    OptionOr,
    /// `List.fold`, which the range fusion matches.
    ListFold,
    /// `List.map`, which the range fusion matches.
    ListMap,
    /// `List.range`, which the range fusion removes.
    ListRange,
    /// `List.reverse`, which the `List.map` range fusion calls once at the end.
    ListReverse,
    /// Another function of the core `List` module. Inlining leaves collection functions as calls:
    /// the range fusion matches some of them, and copying them into every caller would grow the
    /// output for no gain.
    Collection,
}

/// The bundled core functions lowering identified, by IR function ([`CoreFn`]).
pub type CoreFns = FxHashMap<FnId, CoreFn>;

/// What the Core IR passes changed in one [`run`] or [`run_with`], for tests and the differential
/// harness.
///
/// The change counters are added with the passes; a pass fills only its own. An empty `Stats` says
/// that no pass changed anything, which is what a test of a pass must not see on the corpus it
/// runs on. [`Stats::passes`] records the sweeps instead of a change, so it is set even when
/// nothing changed; [`Stats::is_empty`] ignores it. [`Stats::deopt`] records decisions a rule
/// declined, which is also not a change, so it does not keep [`run_with`] looping either.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Bounds tests unswitched out of a loop by [`r#loop::unswitch`], with the load the test
    /// guards. The historical name; it no longer counts a `let` copied out of a loop, because copy
    /// propagation is ζ reduction.
    pub hoist: u32,
    /// `Option.or(Array.get(a, i), d)` rewritten to the internal `ArrayGetOr`.
    pub option_or: u32,
    /// `match Array.get(a, i) { Some(x) -> …; None -> … }` rewritten to a bounds test and a load.
    pub array_get_match: u32,
    /// `List.fold(List.range(lo, hi), init, f)` rewritten to a loop over an integer counter.
    pub range_fold: u32,
    /// `List.map(List.range(lo, hi), f)` rewritten to a loop that no longer builds the range's
    /// list (the result list is still built).
    pub range_map: u32,
    /// `Array.from_list(List.map(List.range(lo, hi), f))` rewritten to the internal
    /// `ArrayMapRange(lo, hi, f)`, which builds neither the range nor a list.
    pub map_range: u32,
    /// ζ reductions: let bindings substituted away, including dead trivial lets.
    pub zeta: u32,
    /// β reductions: linear lambda calls reduced to a let binding.
    pub beta: u32,
    /// η reductions: forwarding lambdas removed.
    pub eta: u32,
    /// ι reductions: matches of a known constructor removed.
    pub iota: u32,
    /// δ reductions: direct calls expanded within the budget. This is the counter the historical
    /// `inline` field became in A10/PR-019; one expansion is counted here once.
    pub delta: u32,
    /// Constant foldings: an operation on literals replaced by its result.
    pub constant: u32,
    /// Algebraic simplifications: a constant side of a Bool or integer operation removed.
    pub algebra: u32,
    /// Bounds checks removed. Reserved for A11/PR-040; zero until then.
    pub bounds: u32,
    /// Common subexpressions removed. Reserved for A12/PR-041; zero until then.
    pub cse: u32,
    /// Sites a rule examined and left alone: a failed precondition, or an analysis that answered
    /// `Unknown`. A deopt is not a rewrite and does not keep the fixed-point loop going.
    pub deopt: u32,
    /// How many sweeps of the pipeline were needed: the number of sweeps that changed the module,
    /// counted as one when the module was already at the fixed point. A pass leaves it zero;
    /// [`run_with`] sets it. Since it stops on the sweep that changes nothing, `n > 1` means `n`
    /// sweeps changed the module and the sweep after them confirmed the fixed point.
    pub passes: u32,
}

impl Stats {
    /// Adds what `other` changed to these, and its sweeps to [`Stats::passes`].
    ///
    /// A pass returns a [`PassStats`] with `passes` zero, so merging the passes of one sweep leaves
    /// the sweep count alone; merging the results of two runs adds their sweeps.
    pub fn merge(&mut self, other: Stats) {
        self.hoist += other.hoist;
        self.option_or += other.option_or;
        self.array_get_match += other.array_get_match;
        self.range_fold += other.range_fold;
        self.range_map += other.range_map;
        self.map_range += other.map_range;
        self.zeta += other.zeta;
        self.beta += other.beta;
        self.eta += other.eta;
        self.iota += other.iota;
        self.delta += other.delta;
        self.constant += other.constant;
        self.algebra += other.algebra;
        self.bounds += other.bounds;
        self.cse += other.cse;
        self.deopt += other.deopt;
        self.passes += other.passes;
    }

    /// Whether no pass changed anything.
    ///
    /// [`Stats::passes`] is not a change: a sweep that changed nothing still counts as a sweep, so
    /// the `Stats` of a converged run is empty with `passes` of one or more. [`Stats::deopt`] is not
    /// a change either: a rule that declines every site did not rewrite the module.
    pub fn is_empty(&self) -> bool {
        self.hoist == 0
            && self.option_or == 0
            && self.array_get_match == 0
            && self.range_fold == 0
            && self.range_map == 0
            && self.map_range == 0
            && self.zeta == 0
            && self.beta == 0
            && self.eta == 0
            && self.iota == 0
            && self.delta == 0
            && self.constant == 0
            && self.algebra == 0
            && self.bounds == 0
            && self.cse == 0
    }
}

/// What one pass changed: the same counters as [`Stats`], of which the pass fills only its own.
/// [`run_with`] merges what every pass of a sweep returns.
pub type PassStats = Stats;

/// The most sweeps [`run_with`] applies. Reaching it means a pass is not idempotent, which debug
/// builds assert against; release builds stop here rather than loop for ever.
///
/// Inlining can only expose work for the fusions once, so two sweeps that change something and one
/// that confirms are enough; eight leaves room for a future pass without making a non-converging
/// one expensive.
const MAX_PASSES: u32 = 8;

/// Runs the Core IR optimisation passes on `m` if `optimize`, and returns what they changed.
///
/// This is [`run_with`] with [`Options::default`] (every implemented reduction rule on and the
/// implemented fusion laws on). The sequence is the order in the pipeline table above, applied until a whole
/// sweep changes nothing (see the fixed-point section). `core_fns` is what lowering recorded about
/// the bundled core functions (`CoreFn`), by which the fusions recognise `Option.or` and the
/// `List` functions. With `optimize` false the module is left untouched and `Stats` is empty.
pub fn run(m: &mut Module, core_fns: &CoreFns, optimize: bool) -> Stats {
    if !optimize {
        return Stats::default();
    }
    run_with(m, core_fns, Options::default())
}

/// Runs the Core IR optimisation passes with the reduction rules and fusion laws of `options`.
///
/// The sequence is the order in the pipeline table above. A rule whose flag is off is skipped
/// entirely, so its counter stays zero; a fusion law whose flag is off is skipped entirely too and
/// contributes nothing. The unswitch has no flag and always runs. Like [`run`], this
/// applies the whole sweep until it changes nothing and records the sweep count in
/// [`Stats::passes`].
pub fn run_with(m: &mut Module, core_fns: &CoreFns, options: Options) -> Stats {
    let rules = options.rules;
    let laws = options.laws;
    let mut stats = Stats::default();
    let mut changed_sweeps = 0;
    let mut converged = false;
    for _ in 0..MAX_PASSES {
        let mut sweep = Stats::default();
        // The reduction rules run in the table's order; a rule whose flag is off is skipped and
        // contributes nothing to the sweep.
        if rules.zeta {
            let done = reduce::zeta(m);
            sweep.zeta += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        sweep.merge(r#loop::unswitch(m));
        check(m);
        // The fusion laws run in the registry's order, which is the order of the table above.
        // `fusion::run` adds their counters and verifies after each law that runs.
        fusion::run(m, core_fns, laws, &mut sweep);
        // δ keeps the inliner's place after the fusions, for the reason in the pipeline docs. β
        // runs after it so that it applies the lambdas that survive the expansion and the ones a
        // copy brought in.
        if rules.delta {
            let done = reduce::delta(m, core_fns);
            sweep.delta += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        if rules.beta {
            let done = reduce::beta(m);
            sweep.beta += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        if rules.iota {
            let done = reduce::iota(m);
            sweep.iota += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        if rules.fold {
            let done = reduce::fold(m);
            sweep.constant += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        if rules.algebra {
            let done = reduce::algebra(m);
            sweep.algebra += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        if rules.eta {
            let done = reduce::eta(m);
            sweep.eta += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        stats.merge(sweep);
        if sweep.is_empty() {
            converged = true;
            break;
        }
        changed_sweeps += 1;
    }
    debug_assert!(
        converged,
        "the Core IR passes did not converge in {MAX_PASSES} sweeps (a compiler bug)"
    );
    // One sweep is enough to see that there is nothing to do; report that as one.
    stats.passes = changed_sweeps.max(1);
    stats
}

/// Runs δ reduction once on a monomorphic module: the stage after monomorphisation.
///
/// The input must be the module [`crate::mono`] produced, with `core_fns` rebuilt for its function
/// ids. This calls [`reduce::delta`] once with the rule's existing budget; there is no other
/// reduction, no fusion law and no fixed-point loop. The returned [`Stats`] fills only
/// [`Stats::delta`] and [`Stats::deopt`], which the caller merges with the counters of [`run`].
/// [`Stats::passes`] stays zero: it counts sweeps of [`run_with`], not stages.
///
/// δ keeps its contract and is not a different rule here; the only difference is that the module
/// is monomorphic, so a callee with no set type parameters (an effect-polymorphic wrapper before
/// it was copied) is eligible. The [`CoreFns`] map must be rebuilt for the monomorphic function
/// ids, or the reserved bundled functions would become candidates; see the module's
/// "After monomorphisation" section.
pub(crate) fn delta_after_monomorphization(m: &mut Module, core_fns: &CoreFns) -> Stats {
    let done = reduce::delta(m, core_fns);
    check(m);
    Stats {
        delta: done.applied,
        deopt: done.deopt,
        ..Stats::default()
    }
}

/// Applies `f` to the body of every function, with the module to allocate locals in.
///
/// A pass takes the body out of the function, so it can allocate locals in the module and rewrite
/// the body without borrowing the function.
pub(super) fn each_body(m: &mut Module, f: &mut impl FnMut(&mut Module, &mut Expr)) {
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        let mut body = std::mem::replace(&mut m.functions[id].body, Expr::Lit(Lit::Unit));
        f(m, &mut body);
        m.functions[id].body = body;
    }
}

/// Checks that a pass left the module well typed. A failure is a compiler bug: the pass broke its
/// postcondition, not the program.
///
/// The fusion registry calls this after each law it runs, at the points the five direct fusion
/// calls had before the registry.
pub(in crate::opt) fn check(m: &Module) {
    #[cfg(debug_assertions)]
    if let Err(errors) = crate::verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!(
            "a Core IR optimisation pass produced ill-typed IR (a compiler bug):\n{}",
            lines.join("\n")
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = m;
}

#[cfg(test)]
mod tests {
    use super::{CoreFns, Options, PassStats, RuleStats, Rules, Stats, run_with};
    use crate::ir::Module;

    /// The total is the sum of what each pass returned: a counter belongs to exactly one pass.
    #[test]
    fn merge_adds_each_pass_counter() {
        let mut total = Stats::default();
        assert!(total.is_empty());

        total.merge(Stats {
            option_or: 1,
            array_get_match: 2,
            ..PassStats::default()
        });
        total.merge(PassStats {
            range_fold: 3,
            range_map: 4,
            delta: 5,
            ..PassStats::default()
        });

        assert_eq!(total.option_or, 1);
        assert_eq!(total.array_get_match, 2);
        assert_eq!(total.range_fold, 3);
        assert_eq!(total.range_map, 4);
        assert_eq!(total.delta, 5);
        assert!(!total.is_empty());
    }

    /// Every new reduction counter is part of the total and of `is_empty`.
    #[test]
    fn merge_adds_the_new_reduction_counters() {
        let mut total = Stats::default();
        total.merge(Stats {
            zeta: 1,
            beta: 2,
            eta: 3,
            iota: 4,
            delta: 5,
            constant: 6,
            algebra: 7,
            bounds: 8,
            cse: 9,
            ..Stats::default()
        });
        assert_eq!(total.zeta, 1);
        assert_eq!(total.beta, 2);
        assert_eq!(total.eta, 3);
        assert_eq!(total.iota, 4);
        assert_eq!(total.delta, 5);
        assert_eq!(total.constant, 6);
        assert_eq!(total.algebra, 7);
        assert_eq!(total.bounds, 8);
        assert_eq!(total.cse, 9);
        assert!(!total.is_empty());
    }

    /// A deopt is not a rewrite: it does not make a run non-empty and it does not keep the
    /// fixed-point loop going.
    #[test]
    fn a_deopt_is_not_a_rewrite() {
        let stats = Stats {
            deopt: 3,
            ..Stats::default()
        };
        assert!(stats.is_empty());
    }

    /// `RuleStats` reports the two counters a rule fills.
    #[test]
    fn rule_stats_default_is_no_change() {
        assert_eq!(
            RuleStats::default(),
            RuleStats {
                applied: 0,
                deopt: 0
            }
        );
        assert_eq!(RuleStats::applied(4).applied, 4);
        assert_eq!(RuleStats::applied(4).deopt, 0);
    }

    /// `Default` turns every implemented rule on, and leaves only the reserved rules off; `all`
    /// turns every one on and `none` every one off.
    #[test]
    fn the_rule_flags_have_the_default_pipeline() {
        let all = Rules::all();
        assert!(all.zeta && all.beta && all.eta && all.iota && all.delta);
        assert!(all.fold && all.algebra && all.bounds && all.cse);

        let none = Rules::none();
        assert!(
            !(none.zeta
                || none.beta
                || none.eta
                || none.iota
                || none.delta
                || none.fold
                || none.algebra
                || none.bounds
                || none.cse)
        );

        assert_eq!(
            Rules::default(),
            Rules {
                bounds: false,
                cse: false,
                ..all
            }
        );
        assert_ne!(Rules::default(), none);
        assert_ne!(Rules::default(), all);
    }

    /// The empty rules do not change the convergence of the pipeline: an empty module takes one
    /// sweep that changes nothing, whether every new rule is on or off.
    #[test]
    fn empty_rules_keep_the_pipeline_at_a_fixed_point() {
        for rules in [Rules::all(), Rules::default()] {
            let mut m = Module::new();
            let stats = run_with(
                &mut m,
                &CoreFns::default(),
                Options {
                    rules,
                    ..Options::default()
                },
            );
            assert!(stats.is_empty(), "{stats:?}");
            assert_eq!(stats.passes, 1, "{stats:?}");
        }
    }

    /// Merging an empty pass changes no counter, so a pass that does nothing is invisible.
    #[test]
    fn merging_an_empty_pass_keeps_the_total() {
        let mut total = Stats {
            option_or: 7,
            ..Stats::default()
        };
        total.merge(Stats::default());
        assert_eq!(
            total,
            Stats {
                option_or: 7,
                ..Stats::default()
            }
        );
    }

    /// `passes` is a count of sweeps, not a change: an empty `Stats` with sweeps in it is empty.
    #[test]
    fn a_sweep_count_is_not_a_change() {
        let stats = Stats {
            passes: 2,
            ..Stats::default()
        };
        assert!(stats.is_empty());
    }

    /// Merging adds the sweeps of both runs, so a total can say how often the pipeline ran.
    #[test]
    fn merge_adds_sweeps() {
        let mut total = Stats {
            passes: 2,
            ..Stats::default()
        };
        total.merge(Stats {
            passes: 3,
            delta: 1,
            ..Stats::default()
        });
        assert_eq!(total.passes, 5);
        assert_eq!(total.delta, 1);
    }
}
