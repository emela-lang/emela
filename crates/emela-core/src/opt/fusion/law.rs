//! The descriptors of the fusion laws: one identity, one contract and, if implemented, one body
//! per law (plan §3.4).
//!
//! A law's contract is data here instead of rustdoc only, so the registry can be inspected by the
//! tests and by the report that counts how often a law fires (plan §5 Track F). The prose of a
//! contract is the one in the module of the body ([`super::option`], [`super::list`],
//! [`super::string`]): this table repeats it so that a law that has no body yet can have its
//! contract reviewed before its implementation (D3–D4).
//!
//! [`LAWS`] is in pipeline order: the order of the implemented laws is the order the pipeline runs
//! them in, and the reserved laws sit where they will be inserted once they have a body (after the
//! range laws and `string_concat`). [`super::laws`] exposes the table.
//!
//! The pipeline laws of D3/PR-030 (`map_map`, `fold_map`, `filter_map`) come before the range
//! laws, because the call they build is the one `range_fold` and `range_map` fuse: a range pipeline
//! is then fused completely in one sweep. Their bodies are in [`super::list`], beside the range
//! laws they feed.

use crate::ir::Module;
use crate::opt::{CoreFns, PassStats};

/// What one law changed in one sweep: the sites it rewrote and the counters it filled.
pub(super) struct LawRun {
    /// Sites the law rewrote. A law that only declines sites reports zero.
    pub applied: u32,
    /// The counters of the pass, of which the law fills its own (and, once a law deopts, its
    /// `deopt`).
    pub stats: PassStats,
}

/// The body of a law: the pass it runs, with the number of rewrites made explicit.
///
/// A body fills its own counter and returns the site count with it, so the registry never reads a
/// counter to decide whether the sweep changed something. A later law may fill several counters;
/// its body decides what one application is.
pub(super) type LawFn = fn(&mut Module, &CoreFns) -> LawRun;

/// The identity of a fusion law.
///
/// The variants are stable: the flags, the tests and the registry order use them. The rule id (see
/// [`LawId::rule_id`]) is the name a contract and a report show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LawId {
    /// `Option.or(Array.get(a, i), d)` to the internal `ArrayGetOr`.
    OptionOr,
    /// `match Array.get(a, i) { Some(x) -> …; None -> … }` to a bounds test and a load.
    ArrayGetMatch,
    /// `List.map(List.map(xs, f), g)` to one traversal.
    MapMap,
    /// `List.fold(List.map(xs, f), init, g)` to one traversal.
    FoldMap,
    /// `List.map(List.filter(xs, p), f)` to one traversal.
    FilterMap,
    /// `List.fold(List.range(lo, hi), init, f)` to a loop over an integer counter.
    RangeFold,
    /// `Array.from_list(List.map(List.range(lo, hi), f))` to the internal `ArrayMapRange`.
    RangeMapArray,
    /// `List.map(List.range(lo, hi), f)` to a loop that does not build the range.
    RangeMap,
    /// A chain of `String.append` calls to one `concat` (D5/PR-032).
    StringConcat,
    /// An `Array` pipeline over a range. Reserved; no body yet (D4/PR-031).
    ArrayPipeline,
}

impl LawId {
    /// The rule id of the law, as its contract and the reports name it.
    pub const fn rule_id(self) -> &'static str {
        match self {
            Self::OptionOr => "option_or",
            Self::ArrayGetMatch => "array_get_match",
            Self::MapMap => "map_map",
            Self::FoldMap => "fold_map",
            Self::FilterMap => "filter_map",
            Self::RangeFold => "range_fold",
            Self::RangeMapArray => "range_map_array",
            Self::RangeMap => "range_map",
            Self::StringConcat => "string_concat",
            Self::ArrayPipeline => "array_pipeline",
        }
    }
}

/// One fusion law: its identity, its contract (plan §3.4) and its body, if it has one.
///
/// The contract lines are the fields from [`Law::theory`] to [`Law::measures`]. A law whose
/// [`Law::implemented`] is false is registered for a later pull request: it has no body, so no
/// flag can make it run.
#[derive(Debug, Clone, Copy)]
pub struct Law {
    /// The identity the flags and the reports use.
    pub id: LawId,
    /// The body, or `None` while the law is reserved.
    run: Option<LawFn>,
    /// Where the equation comes from.
    pub theory: &'static str,
    /// The rewrite, as an equation.
    pub equation: &'static str,
    /// When the law applies.
    pub preconditions: &'static str,
    /// What the law guarantees.
    pub postconditions: &'static str,
    /// What the rewrite keeps: order, effects, failures, suspension points, drop points, evidence.
    pub preserves: &'static str,
    /// What the law leaves alone.
    pub deopt: &'static str,
    /// The size or cost limit of the law.
    pub budget: &'static str,
    /// The counter and the benchmarks the law targets.
    pub measures: &'static str,
}

impl Law {
    /// The rule id of the law (see [`LawId::rule_id`]).
    pub const fn rule_id(&self) -> &'static str {
        self.id.rule_id()
    }

    /// Whether the equation has a body. A reserved law is never run, whatever its flag says.
    pub const fn implemented(&self) -> bool {
        self.run.is_some()
    }

    /// The body, or `None` for a reserved law.
    pub(super) fn body(&self) -> Option<LawFn> {
        self.run
    }
}

/// The fusion laws in pipeline order, the implemented ones in the order the pipeline runs them and
/// the reserved ones where they will be inserted.
pub(super) const LAWS: &[Law] = &[
    Law {
        id: LawId::OptionOr,
        run: Some(super::option::option_or),
        theory: "Option fusion: Array.get makes an Option only for the caller to take the element \
                 or a default, and the two can be one operation (plan §2.2).",
        equation: "Option.or(Array.get(a, i), d) → Array.get_or(a, i, d)",
        preconditions: "The callee is the core Option.or; the call neither fails nor suspends; the \
                        Array.get's element type is the call's single type argument and is known to \
                        have a representation.",
        postconditions: "The result is the element at i, or d when i is out of range. a, i and d \
                         are evaluated once each, in that order; d is evaluated even when i is in \
                         range. No Some is made.",
        preserves: "Evaluation order, effects, failures, suspension points and drop points. The \
                    Some the original call made is not observable.",
        deopt: "Any other call shape (an indirect call, a user's or, another element type), an \
                element type that may have no representation, and a failing or suspending callee.",
        budget: "None: the rewrite replaces one call by one builtin and does not grow the \
                 expression.",
        measures: "Stats::option_or; the matmul, fannkuch and spectral benchmarks.",
    },
    Law {
        id: LawId::ArrayGetMatch,
        run: Some(super::option::array_get_match),
        theory: "Option fusion: a match on Array.get tests only the bounds, so the Option can be \
                 removed by a bounds test and an unchecked load (plan §2.2).",
        equation: "match Array.get(a, i) { Some(x) -> e1; None -> e2 } → \
                   let a0 = a; let i0 = i; \
                   if i0 >= 0 && i0 < Array.size(a0) \
                   { let x = Array.load_unchecked(a0, i0); e1 } else { e2 } \
                   (Some(_) binds no local)",
        preconditions: "The scrutinee is a direct Array.get(a, i); the arms are exactly one \
                        Some(x) or Some(_) and one None, unguarded, in either order; the element \
                        type is known to have a representation.",
        postconditions: "a and i are evaluated once each, in that order, before the test; the Some \
                         arm's body sees the element; the None arm's body runs when i < 0 or \
                         i >= Array.size(a). The Some wrapper is not made.",
        preserves: "Evaluation order, effects, failures of the scrutinee and both bodies, \
                    suspension points and drop points.",
        deopt: "A match on a let-bound scrutinee (let t = Array.get(..); match t) is not fused yet; \
                neither is any other pattern shape, a guarded arm, or an element type that may have \
                no representation.",
        budget: "None: the bounds test and the load replace one call and one match; the \
                 expression does not grow with the site count.",
        measures: "Stats::array_get_match; matmul (most of its Some allocations).",
    },
    Law {
        id: LawId::MapMap,
        run: Some(super::list::map_map),
        theory: "Map fusion: a producer map and a consumer map compose into one traversal (plan \
                 §2.2, deforestation of the pipeline a list function declares).",
        equation: "List.map(List.map(xs, f), g) → List.map(xs, λx. g(f x))",
        preconditions: "The callee of the outer call is the core List.map, and the outer call's \
                        first argument is a direct call of the core List.map; both calls are in the \
                        inner and outer shape of the equation and neither fails nor suspends. f and \
                        g are a lambda literal or a reference to a top-level function, their \
                        instantiated types are pure (no failure, no suspension, no evidence table), \
                        and their bodies may neither use an effect nor fail nor suspend. At most \
                        one of f and g may reach a defect or diverge. The site is within the \
                        code-size budget.",
        postconditions: "The result is List.map's list over xs with each element replaced by \
                         g(f(x)), in the order of xs. f and g are called once per element, f before \
                         g, in that order; no element of the intermediate list is made. The outer \
                         map keeps its type arguments for xs and its result and its evidence \
                         argument, so the fused call is the same call the source's outer map was.",
        preserves: "Evaluation order, effects, failures, suspension points and drop points. f and g \
                    are proved quiet (an effect, a failure or a suspension in either deopts), so \
                    the per-element interleaving `g(f x)` has the same observable behaviour as the \
                    two traversals; at most one of them may stop the program (a defect or a \
                    divergence), so the first stop is the same call of the same function. xs is \
                    still evaluated first and the evidence argument last; a lambda literal is made \
                    once before the call instead of before its own traversal, which is not \
                    observable because making a lambda has no effect. The inner call's evidence \
                    argument (a local or `()`) is dropped with the inner call.",
        deopt: "A call that is not this shape (a user's map, an indirect call, fewer or more type \
                arguments or arguments); a function value that is neither a lambda literal nor a \
                reference to a top-level function (a parameter or a computed value, whose effects \
                are unknown); a `dbg` or any other effect, failure or suspension in f or g; a site \
                over the code-size budget. A deopt at a nested-map site is counted in \
                Stats::deopt.",
        budget: "The composed lambda is at most the two function expressions' nodes plus \
                 COMPOSE_OVERHEAD (the parameter, the two calls and their callees), and one \
                 function body gains at most MAX_COMPOSED_PER_BODY composed lambdas from this law \
                 in one sweep. A site over either limit is left alone.",
        measures: "Stats::map_map; the map pipelines of the benchmarks (the intermediate list's \
                   cons cells, halved).",
    },
    Law {
        id: LawId::FoldMap,
        run: Some(super::list::fold_map),
        theory: "Fold fusion: the map that feeds a fold is fused into the fold's step (plan §2.2), \
                 so the mapped list is never built.",
        equation: "List.fold(List.map(xs, f), init, g) → \
                   List.fold(xs, init, λa, x. g(a, f x))",
        preconditions: "The callee of the outer call is the core List.fold and its first argument \
                        is a direct call of the core List.map; both calls are in the shape of the \
                        equation and neither fails nor suspends. f and g are a lambda literal or a \
                        reference to a top-level function, their instantiated types are pure (no \
                        failure, no suspension, no evidence table), and their bodies may neither use \
                        an effect nor fail nor suspend. Among f, g and init, at most one may stop \
                        the program: f and g at a defect or a divergence, init at any effect, \
                        failure, suspension, defect or divergence. The site is within the code-size \
                        budget.",
        postconditions: "The result is the last accumulator of folding g(acc, f(x)) over xs, or \
                         init when xs is empty. f and g are called once per element, f before g, in \
                         the order of xs; no element of the intermediate list is made. The fold \
                         keeps its accumulator type, its type arguments and its evidence argument.",
        preserves: "Evaluation order, effects, failures, suspension points and drop points. f and g \
                    are proved quiet, so the per-element interleaving `g(a, f x)` has the same \
                    observable behaviour as the map followed by the fold. init moves from after \
                    the whole map to the fold's first argument; that is not observable because at \
                    most one of init, f and g may produce an event or stop the program, so no \
                    event of another one can be overtaken. xs is still evaluated first and the \
                    evidence argument last. The inner map call's evidence argument (a local or \
                    `()`) is dropped with the inner call.",
        deopt: "A call that is not this shape (a user's fold or map, an indirect call, other type \
                arguments or arguments); a function value that is neither a lambda literal nor a \
                reference to a top-level function; a `dbg` or any other effect, failure or \
                suspension in f or g; a site over the code-size budget. A deopt at a nested site is \
                counted in Stats::deopt.",
        budget: "The composed lambda is at most the two function expressions' nodes plus \
                 COMPOSE_OVERHEAD (the two parameters, the two calls and their callees), and one \
                 function body gains at most MAX_COMPOSED_PER_BODY composed lambdas from this law \
                 in one sweep. A site over either limit is left alone.",
        measures: "Stats::fold_map; the fold pipelines of the benchmarks (the intermediate list's \
                   cons cells).",
    },
    Law {
        id: LawId::FilterMap,
        run: Some(super::list::filter_map),
        theory: "Filter and map fusion: the filter that feeds a map becomes one fold that tests and \
                 maps each element (plan §2.2), the shape the library's own filter and map build.",
        equation: "List.map(List.filter(xs, p), f) → \
                   List.reverse(List.fold(xs, [], λacc, x. if p(x) { [f(x), ..acc] } else { acc }))",
        preconditions: "The callee of the outer call is the core List.map and its first argument \
                        is a direct call of the core List.filter; both calls are in the shape of the \
                        equation and neither fails nor suspends; a non-suspending copy of the core \
                        List.fold and the core List.reverse are in the module. p and f are a lambda \
                        literal or a reference to a top-level function, their instantiated types \
                        are pure (no failure, no suspension, no evidence table), and their bodies \
                        may neither use an effect nor fail nor suspend. At most one of p and f may \
                        reach a defect or diverge. The site is within the code-size budget.",
        postconditions: "The result is the list of f(x) for each element x of xs for which p(x) \
                         holds, in the order of xs. For each element, p is called first and f is \
                         called only when p holds, in the order of xs. The filter's list and the \
                         map's work are one traversal: the fold builds the result in reverse and \
                         List.reverse puts it in order.",
        preserves: "Evaluation order, effects, failures, suspension points and drop points. p and f \
                    are proved quiet, so testing and mapping each element in one traversal has the \
                    same observable behaviour as the filter followed by the map; at most one of them \
                    may stop the program (a defect or a divergence), so the first stop is the same. \
                    xs is still evaluated first and the evidence argument last. The filter call's \
                    evidence argument (a local or `()`) is dropped with the filter call.",
        deopt: "A call that is not this shape (a user's filter or map, an indirect call, other type \
                arguments or arguments); a function value that is neither a lambda literal nor a \
                reference to a top-level function; a `dbg` or any other effect, failure or \
                suspension in p or f; a module with only a suspending List.fold or no List.reverse; \
                a site over the code-size budget. A deopt at a filter-map site is counted in \
                Stats::deopt.",
        budget: "The composed step is at most the two function expressions' nodes plus \
                 COMPOSE_OVERHEAD (the parameters, the test, the cons and the calls), and one \
                 function body gains at most MAX_COMPOSED_PER_BODY composed steps from this law in \
                 one sweep. A site over either limit is left alone.",
        measures: "Stats::filter_map; the filter-then-map pipelines of the benchmarks (the \
                   filter's list and the reverse it would make).",
    },
    Law {
        id: LawId::RangeFold,
        run: Some(super::list::fold),
        theory: "Range fusion: List.range is only consumed by the fold, so the range's list can be \
                 replaced by an integer counter (plan §2.2).",
        equation: "List.fold(List.range(lo, hi), init, f) → \
                   let lo0 = lo; let hi0 = hi; let init0 = init; let f0 = f; \
                   loop i = lo0..hi0 with accumulator init0 calling f0 (the internal loop node)",
        preconditions: "The callee is the core List.fold with its four type arguments and four \
                        arguments (xs, from, f, ev); xs is a direct List.range(lo, hi); the element \
                        type is Int; the range neither fails nor suspends.",
        postconditions: "f is called once for each i in lo..hi (max(0, hi - lo) times), in \
                         ascending order, with the accumulator and i; the result is the last \
                         accumulator, or init when the range is empty. lo, hi, init and f are \
                         evaluated once each, in that order, before the loop; f is evaluated even \
                         when the range is empty. A lambda literal as f is copied in rather than \
                         called.",
        preserves: "Evaluation order, the calls of f and their failures, effects and suspension \
                    points, and drop points. Only the range's cons cells are no longer made, and \
                    the closure of an inlined lambda is not made.",
        deopt: "A call that is not this shape (a user's fold or range, another element type, a \
                let-bound range, failing or suspending marks on the range).",
        budget: "None beyond the loop node it adds: the range's list and recursion are removed.",
        measures: "Stats::range_fold; nqueens, matmul and spectral (the cons cells of their \
                   ranges).",
    },
    Law {
        id: LawId::RangeMapArray,
        run: Some(super::list::map_array),
        theory: "Range and array fusion: the range and the mapped list only feed Array.from_list, \
                 so the array can be made in one traversal (plan §2.2).",
        equation: "Array.from_list(List.map(List.range(lo, hi), f)) → Array.map_range(lo, hi, f)",
        preconditions: "The argument is a direct List.map(List.range(lo, hi), f) of the core \
                        List.map with its four type arguments and three arguments (xs, f, ev); the \
                        element type is Int; the map's result type is the array's element type, is \
                        neither () nor Never; f's type is a pure function of one Int (no failure, \
                        no suspension, no evidence table), so the map call neither fails nor \
                        suspends.",
        postconditions: "The result is an array of max(0, hi - lo) elements whose element i is \
                         f(lo + i), in ascending order. lo, hi and f are evaluated once each, in \
                         that order; f is evaluated even when the range is empty. Neither the \
                         range's cons cells nor the mapped list are made, and the array is made \
                         once.",
        preserves: "Evaluation order, the calls of f and their failures, effects and suspension \
                    points, and drop points. A defect is not part of f's type: it stops the loop at \
                    the first one, and the array, made before the first call, is released with the \
                    elements already written.",
        deopt: "A call that is not this shape; an effectful, failing or suspending f (the internal \
                builtin has no failure edge and takes no evidence table); an element type without \
                a representation.",
        budget: "The array is made once; neither the range nor the mapped list is built.",
        measures: "Stats::map_range; matmul, spectral and k-nucleotide (the mapped list's cons \
                   cells and the second traversal Array.from_list made).",
    },
    Law {
        id: LawId::RangeMap,
        run: Some(super::list::map),
        theory: "Range fusion: List.range is only consumed by the map, so the range's list can be \
                 replaced by an integer counter (plan §2.2).",
        equation: "List.map(List.range(lo, hi), f) → \
                   let lo0 = lo; let hi0 = hi; let f0 = f; \
                   loop building the result in reverse, then List.reverse (the internal loop node)",
        preconditions: "The callee is the core List.map with its four type arguments and three \
                        arguments (xs, f, ev); xs is a direct List.range(lo, hi); the element type \
                        is Int; the range neither fails nor suspends.",
        postconditions: "f is called once for each i in lo..hi, in ascending order, and the result \
                         is List.map's list. The list is built in reverse and List.reverse is \
                         called once at the end. lo, hi and f are evaluated once each, in that \
                         order, before the loop; f is evaluated even when the range is empty. A \
                         lambda literal as f is copied in rather than called.",
        preserves: "Evaluation order, the calls of f and their failures, effects and suspension \
                    points, and drop points. The range's cons cells, the recursion of the range \
                    and the closure of an inlined lambda are no longer made; the result list and \
                    the reverse still are.",
        deopt: "A call that is not this shape, as in range_fold. An \
                Array.from_list(List.map(List.range(…), f)) is range_map_array's case, which this \
                law sees only when that fusion gave up (an effectful f, or an element type \
                without a representation).",
        budget: "The result list and the reverse are still made; the range's cells, its recursion \
                 and the closure of an inlined lambda are not.",
        measures: "Stats::range_map; the list forms of the range fusion.",
    },
    Law {
        id: LawId::StringConcat,
        run: Some(super::string::concat),
        theory: "String builder fusion: a chain of String.append calls makes each intermediate \
                 string only for the next append to copy it, so the chain can become one concat \
                 (plan §2.2, D5 and E7).",
        equation: "String.append(…String.append(a, b)…) → concat([a, b, …]) \
                   (the internal Expr::Concat that interpolation already lowers to, which the LIR \
                   builds as one RuntimeOp::Concat)",
        preconditions: "Every leaf of the append tree is a String, the tree has at least three \
                        leaves, and not every leaf is the empty string literal. The leaves are \
                        collected left to right, which is the order the appends evaluate them in.",
        postconditions: "The result has the bytes of the leaves in order, and the intermediate \
                         strings are not made: the runtime adds up the lengths, allocates one \
                         String and copies each leaf into it. All leaves are evaluated before the \
                         one allocation.",
        preserves: "Evaluation order, effects, failures, suspension points and drop points of the \
                    leaves, and the byte content of the result. A leaf is evaluated exactly once, \
                    in the order it had in the append tree.",
        deopt: "A single append or a chain of two leaves (one append already allocates once), and a \
                chain whose leaves are all the empty string literal: lowering it would make the \
                literal empty string, and the native target rejects a constant empty String.split \
                separator at compile time while a separator empty at run time is a defect or a \
                split by character. A dynamic chain is not matched.",
        budget: "One concat per site: the intermediate appends and their allocations are removed. \
                 The per-sweep budget is that of the registry.",
        measures: "Stats::string_concat; one String per site in the C allocation report (the form \
                   the law builds is the one a_three_part_interpolation_allocates_one_string \
                   pins).",
    },
    Law {
        id: LawId::ArrayPipeline,
        run: None,
        theory: "Array pipeline fusion: a range filtered and mapped into an array becomes one pass \
                 over the array (plan §2.2 and D4).",
        equation: "Array.from_list(List.map(List.filter(List.range(lo, hi), p), f)) → \
                   Array.filter_map_range(lo, hi, p, f)",
        preconditions: "The filter and the map are direct calls of the core List functions over a \
                        direct List.range(lo, hi); p and f are pure functions without evidence \
                        tables; the element type has a representation; the site stays within the \
                        budget.",
        postconditions: "The array has one element f(i) for each i in lo..hi for which p holds, in \
                         ascending order; neither the range nor an intermediate list is made.",
        preserves: "Evaluation order, the calls of p and f and their failures, effects and \
                    suspension points, and drop points.",
        deopt: "An effectful, failing or suspending p or f, an element type without a \
                representation, an unknown callee, or an over-budget site.",
        budget: "One array, built once, within the per-sweep budget (PR-031).",
        measures: "A reserved Stats counter; the array pipelines of the benchmarks (PR-031).",
    },
];
