//! Optimisation of the Core IR, between lowering and monomorphisation.
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
//! shape `fn(&mut Module) -> PassStats`. [`run_with`] merges both into one [`Stats`].
//!
//! | Pass | Counter | Added by |
//! |---|---|---|
//! | `reduce::zeta` | [`Stats::zeta`] | ζ reduction (A2/PR-011) |
//! | `reduce::beta` | [`Stats::beta`] | β reduction (A3/PR-012) |
//! | `hoist::hoist` | [`Stats::hoist`] | Loop hoisting |
//! | `option::option_or` | [`Stats::option_or`] | Option fusion |
//! | `option::array_get_match` | [`Stats::array_get_match`] | Option fusion |
//! | `range::fold` | [`Stats::range_fold`] | range fusion |
//! | `range::map_array` | [`Stats::map_range`] | range fusion |
//! | `range::map` | [`Stats::range_map`] | range fusion |
//! | `inline::inline` | [`Stats::inline`] | inlining |
//! | `reduce::iota` | [`Stats::iota`] | ι reduction (A5/PR-014) |
//! | `reduce::fold` | [`Stats::constant`] | constant folding (A6/PR-015) |
//! | `reduce::algebra` | [`Stats::algebra`] | algebraic reduction (A7/PR-016) |
//! | `reduce::eta` | [`Stats::eta`] | η reduction (A4/PR-013) |
//! | `reduce::delta` | [`Stats::delta`] | δ reduction (A8/PR-017) |
//!
//! The positions are chosen for the patterns each pass exposes:
//!
//! - **ζ and β first.** Both reduce `let` bindings. ζ propagates trivial values so the structural
//!   passes see the names and constructors without an intermediate binding, and β removes a
//!   linear lambda call before the inliner has to. The reductions only shrink, so running them
//!   before the fusions cannot hide a fusion pattern.
//! - **The hoisting and the fusions keep their previous place.** Hoisting runs before the Option
//!   fusion because it matches the `match Array.get` that the fusion replaces by a bounds test; the
//!   fusions run before inlining because they match the calls the source wrote, and inlining a
//!   function such as `List.fold` first would dissolve the call the range fusion looks for.
//!   `range::map_array` comes before `range::map`, which would otherwise rewrite the `List.map`
//!   inside the `Array.from_list` it is looking for.
//! - **Inlining stays after the fusions**, in the position it had before the reduction kernel was
//!   added. The new rules run after it, so they simplify the body it exposes in the same sweep
//!   instead of waiting for the next one.
//! - **ι, then constant folding and algebra.** Inlining and β expose a known constructor and a
//!   constant condition; ι removes the match, and folding and the algebraic laws simplify the
//!   values and conditions it leaves.
//! - **η after the algebraic laws**, which remove forwarding bodies and so leave more wrapper
//!   lambdas for it to see.
//! - **δ last**, because it is the only rule that may grow the program. Everything that shrinks
//!   runs first, and a body δ copies is simplified on the next sweep.
//!
//! # Status of the reduction kernel
//!
//! A0/PR-002 adds the rules and the order above, but every rule in [`reduce`] is a no-op: it
//! returns [`RuleStats::default`] and rewrites nothing. The pipeline skips a rule whose flag is
//! off, and [`Options::default`] has every new rule off, so a default run still calls only the
//! older passes. The skeleton therefore changes no output, no counter of a rewrite, and no sweep
//! count. `subst` exposes the substitution API with a conservative body; A1/PR-010 completes it.
//!
//! # Feature flags
//!
//! [`Rules`] has one flag per reduction rule of [`reduce`], plus reserved flags for the later
//! bounds-check and CSE rules. [`Options`] holds the flags and is the argument of [`run_with`];
//! [`run`] keeps its old signature and uses [`Options::default`], which keeps every new rule off.
//! The flags are a programmatic API only: no command-line flag is added in this skeleton.
//!
//! A new rule starts off so that a pull request which only fills an empty module cannot change the
//! default pipeline on its own. The integrator flips the defaults on at A10/PR-019, after the
//! rules and their tests have landed, as plan §6.4(4) and §9.1 require. [`Rules::all`] is
//! available for the per-rule tests, which turn on exactly the rule under test.
//!
//! The older passes (`hoist`, `option`, `range`, `inline`) do not have a flag yet; A10/PR-019
//! unifies their gating with the reduction rules. `Rules::none()` therefore still runs those
//! older passes.
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
//! sweep, because the argument is the call `span()`; inlining `span` replaces the call by its
//! body, but inlining runs after the fusions. The next sweep finds `List.fold(List.range(0, 3), …)`
//! and rewrites it. Repeating the whole table keeps the order that the fusions need and still finds
//! the patterns inlining exposes. A pattern that appears only by substituting an argument into a
//! parameter is not exposed at all: inlining binds each argument to a fresh local first, so the
//! pattern has to be in the callee's body. A module that is already at the fixed point takes one
//! sweep, which changes nothing.
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
mod hoist;
mod inline;
pub mod r#loop;
mod option;
mod range;
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
/// A new rule is off in [`Rules::default`] until its implementation and tests have landed; the
/// integrator turns the defaults on at A10/PR-019. [`Rules::all`] turns every rule on, which the
/// per-rule tests use. `bounds` and `cse` are reserved for A11/PR-040 and A12/PR-041; no pass reads
/// them yet. The existing passes of this module (`hoist`, `option`, `range`, `inline`) are not
/// gated here yet; A10/PR-019 unifies the gating, so [`Rules::none`] still runs them.
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
    /// Every new rule off. The default pipeline is the one before the reduction kernel existed;
    /// A10/PR-019 flips each rule on after its implementation and tests have landed.
    fn default() -> Self {
        Self::none()
    }
}

/// The options of [`run_with`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Which reduction rules to run. [`Rules::default`] (every new rule off) by default.
    pub rules: Rules,
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
    /// Parts of a loop that do not change between its iterations copied out of it, and a bounds
    /// test unswitched out of it.
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
    /// Calls inlined: direct calls replaced by a copy of the callee's body.
    pub inline: u32,
    /// ζ reductions: let bindings substituted away, including dead trivial lets.
    pub zeta: u32,
    /// β reductions: linear lambda calls reduced to a let binding.
    pub beta: u32,
    /// η reductions: forwarding lambdas removed.
    pub eta: u32,
    /// ι reductions: matches of a known constructor removed.
    pub iota: u32,
    /// δ reductions: direct calls expanded within the budget.
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
        self.inline += other.inline;
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
            && self.inline == 0
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
/// This is [`run_with`] with [`Options::default`] (every new reduction rule off). The sequence is
/// the order in the pipeline table above, applied until a whole sweep changes nothing (see the
/// fixed-point section). `core_fns` is what lowering recorded about the bundled core functions
/// (`CoreFn`), by which the fusions recognise `Option.or` and the `List` functions. With
/// `optimize` false the module is left untouched and `Stats` is empty.
pub fn run(m: &mut Module, core_fns: &CoreFns, optimize: bool) -> Stats {
    if !optimize {
        return Stats::default();
    }
    run_with(m, core_fns, Options::default())
}

/// Runs the Core IR optimisation passes with the reduction rules of `options`.
///
/// The sequence is the order in the pipeline table above. A rule whose flag is off is skipped
/// entirely, so its counter stays zero. The older passes without a flag still run; see [`Rules`].
/// Like [`run`], this applies the whole sweep until it changes nothing and records the sweep count
/// in [`Stats::passes`].
pub fn run_with(m: &mut Module, core_fns: &CoreFns, options: Options) -> Stats {
    let rules = options.rules;
    let mut stats = Stats::default();
    let mut changed_sweeps = 0;
    let mut converged = false;
    for _ in 0..MAX_PASSES {
        let mut sweep = Stats::default();
        // The reduction rules are no-ops in the A0/PR-002 skeleton; they still occupy their fixed
        // position, so a later pull request only fills a body.
        if rules.zeta {
            let done = reduce::zeta(m);
            sweep.zeta += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        if rules.beta {
            let done = reduce::beta(m);
            sweep.beta += done.applied;
            sweep.deopt += done.deopt;
            check(m);
        }
        sweep.merge(hoist::hoist(m));
        check(m);
        sweep.merge(option::option_or(m, core_fns));
        check(m);
        sweep.merge(option::array_get_match(m, core_fns));
        check(m);
        sweep.merge(range::fold(m, core_fns));
        check(m);
        sweep.merge(range::map_array(m, core_fns));
        check(m);
        sweep.merge(range::map(m, core_fns));
        check(m);
        // Inlining keeps its place after the fusions, for the reason in the pipeline docs.
        sweep.merge(inline::inline(m, core_fns));
        check(m);
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
        if rules.delta {
            let done = reduce::delta(m);
            sweep.delta += done.applied;
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
fn check(m: &Module) {
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
            inline: 5,
            ..PassStats::default()
        });

        assert_eq!(total.option_or, 1);
        assert_eq!(total.array_get_match, 2);
        assert_eq!(total.range_fold, 3);
        assert_eq!(total.range_map, 4);
        assert_eq!(total.inline, 5);
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

    /// Every new rule is off in `Default`, and `all` turns every one on.
    #[test]
    fn the_rule_flags_have_conservative_defaults() {
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

        assert_eq!(Rules::default(), none);
        assert_ne!(Rules::default(), all);
    }

    /// The empty rules do not change the convergence of the pipeline: an empty module takes one
    /// sweep that changes nothing, whether every new rule is on or off.
    #[test]
    fn empty_rules_keep_the_pipeline_at_a_fixed_point() {
        for rules in [Rules::all(), Rules::default()] {
            let mut m = Module::new();
            let stats = run_with(&mut m, &CoreFns::default(), Options { rules });
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
            inline: 1,
            ..Stats::default()
        });
        assert_eq!(total.passes, 5);
        assert_eq!(total.inline, 1);
    }
}
