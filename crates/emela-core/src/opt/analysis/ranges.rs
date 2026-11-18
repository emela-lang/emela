//! Ranges of integer locals, including induction variables.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Ranges
//! Domain / lattice: an inclusive integer interval over the values of one local, with a bottom
//!                   and a top. `Bottom` has no value (an unreachable point); `Top` has every
//!                   value and is the element a consumer must treat as "no answer". The order is
//!                   Bottom <= every interval; [lo, hi] <= [lo', hi'] iff lo' <= lo and hi <= hi';
//!                   every interval <= Top. The join is the hull (the smallest interval that
//!                   contains both operands), and Top is absorbing. The meet is the intersection,
//!                   and Bottom is absorbing.
//! Transfer: a literal gives a point; `Add`, `Sub`, `Mul`, `Div` and `Rem` are evaluated on the
//!           bounds in i128 and kept only when the exact result is within the width of `Int`
//!           (32-bit) or `Int64` (64-bit); an operation whose result may wrap, or may divide by
//!           zero, gives Top. Comparisons give the Bool interval [0, 1]. A construct the analysis
//!           does not model gives Top.
//! Soundness: a bounded interval contains every value the local can hold at that point, under the
//!            two's-complement wrap of `Int` and `Int64` and the defects of division. A narrowing
//!            is only made when the bounds prove it.
//! Deopt (unknown): a construct outside the transfer gives `Interval::Top`, and a consumer treats a
//!                  Top local exactly like `Fact::Unknown`: it keeps the bounds check or the
//!                  branch it wanted to remove, and never assumes an index is in bounds.
//! Consumers: bounds-check elimination (A11/PR-040), induction variables and loop unswitching.
//! ```
//!
//! # Domain
//!
//! An [`Interval`] is one of three things:
//!
//! - [`Interval::Bottom`]: the empty set. It is the value of an expression that never produces one
//!   (`fail`, a `match` with no arms, an `if` whose condition has no value). A join with Bottom
//!   is the other operand.
//! - `Interval::Range { lo, hi }`: the set `{ lo, ..., hi }`, inclusive, with `lo <= hi`.
//! - [`Interval::Top`]: every value of the local's type. Top means the analysis could not bound
//!   the local; it is not the full interval of a type written as `Range`, because consumers need
//!   one element to deopt on.
//!
//! The bounds are `i128` so that a bound of `Int64` and a bound of `Int` share a representation,
//! and so that a bound arithmetic is done before it is narrowed to the operation's width.
//!
//! The order (via [`PartialOrd`]) is the usual interval order:
//!
//! ```text
//! Bottom <= [lo, hi] <= Top
//! [lo, hi] <= [lo', hi']  iff  lo' <= lo and hi <= hi'
//! ```
//!
//! Two bounded intervals that overlap but are not nested are incomparable. The join
//! ([`Interval::join`]) is the hull, not the intersection: the analysis over-approximates, and a
//! value from either path is a value of the joined local. The meet ([`Interval::meet`]) is the
//! intersection, used only to narrow a local with a condition that is known to hold on one branch.
//!
//! # Transfer
//!
//! [`of_expr`] walks one expression with an [`Env`] of the locals bound so far and returns the
//! interval of the expression's value. The transfer is:
//!
//! - `Lit(Int(n))`, `Lit(Int64(n))`: the point `[n, n]`. `Lit(Bool(b))` is `[b, b]` (Bool is 0 or
//!   1). Float, String and Unit are Top: no consumer asks for their range.
//! - `Var(x)`: the binding of `x` in the environment, or Top when `x` is unbound (a parameter or a
//!   local the walk did not reach).
//! - `Add`, `Sub`, `Mul`: the bounds are combined exactly in `i128`. The result is kept only when
//!   every value of the exact result fits the operation's width (`Int`: 32-bit, `Int64`: 64-bit).
//!   If the result may wrap, the answer is Top: the analysis never reports a wrapped interval,
//!   because a wrap makes the result jump to the other end of the type and no smaller interval
//!   describes it.
//! - `Div`, `Rem`: Top when the divisor interval contains 0 (the operation is then a defect) or
//!   when the result may not fit the width (`Int::MIN / -1` wraps). A quotient is bounded by the
//!   four corner quotients; a remainder is bounded by `max(|divisor|) - 1` with the sign of the
//!   dividend, which is a sound bound for a divisor range wider than one value.
//! - `Neg`: `[-hi, -lo]`, or Top when the result may not fit the width (`-Int::MIN` spells
//!   `Int::MIN` again through the wrap, but the analysis does not claim a bound it cannot prove).
//! - Comparisons (`Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`): the Bool interval `[0, 1]`.
//! - `If`: both branches are transferred in environments narrowed by the condition, and their
//!   results and environments are joined. `Match` and `Escape` join their arms the same way.
//! - `Let`: the value's interval is bound to the local and the body is transferred; a local bound
//!   to a literal or to another local therefore keeps that range.
//! - `Loop`: see below.
//! - `Recur`: the value of a back edge is not a value of the loop expression; it is Bottom. The
//!   arguments are still transferred, so their ranges are recorded.
//! - Every other construct: its children are transferred (so their ranges are recorded) and the
//!   result is Top. A `fail` is Bottom, because it produces no value.
//!
//! # Loops
//!
//! `Expr::Loop { vars, inits, body }` binds `vars` to `inits` and evaluates `body`; a `Recur` in
//! tail position starts the next iteration with its arguments bound to `vars`. The analysis starts
//! from the intervals of the `inits` and joins, for each variable, the interval of every `Recur`
//! argument computed in the current environment. The environment of a round is a superset of the
//! one before it, so the sequence is monotone. A loop runs at most [`LOOP_ROUNDS`] rounds before
//! it must have reached a fixed point; a variable that is still growing then is widened to Top and
//! recorded as Top, and one more round gives the result. The cap makes the analysis terminate on a
//! loop whose counter has no provable bound (`i = i + 1` with no guard); the deopt is Top, never a
//! wrong interval.
//!
//! An `If` in the loop body narrows its branches with the condition, using the bound the condition
//! gives (for example `i < hi` with `hi` known narrows the then-branch to `i <= hi - 1`). That is
//! what lets a bounded loop reach a fixed point in a few rounds instead of widening.
//!
//! # Induction variables
//!
//! [`inductions`] reports an [`Induction`] for a loop variable when every one of its `Recur`
//! arguments is `Var(x) + const` (or `const + Var(x)`, or `Var(x) - const`) with the same constant,
//! and its initial interval is bounded. `iterations` is `Some(n)` only when the loop body is a
//! `let*`-wrapped `if` whose continuation condition compares the variable with a point bound that
//! does not depend on the loop, the step is not 0, there is exactly one `Recur` site, and the
//! counter does not wrap before the guard fails. Otherwise `iterations` is `None` and only the
//! step is promised. A variable whose step is not constant gets no entry at all.
//!
//! # Status
//!
//! B5/PR-024 implements the interval domain, the wrap-aware transfer, the loop fixed point and the
//! induction variables. [`analyze`] returns `Known` for a well-formed module: the analysis is
//! total over the IR, and an unknown value is `Interval::Top` inside the result, not
//! `Fact::Unknown`. A consumer still deopts on Top.

use std::cmp::Ordering;

use rustc_hash::FxHashMap;

use crate::ir::{BinOp, Expr, FnId, Lit, Local, Module, OpTy, Pat, TypeId, UnOp};

use super::{Analysis, AnalysisId, Fact};

/// How many rounds a loop may take before its unstable variables are widened to Top.
///
/// A loop with a provable bound (its guard narrows the counter) reaches a fixed point in a couple
/// of rounds. A loop without one grows by one each round and would otherwise never stop; after
/// this many rounds the analysis deopts to Top instead of running for an unbounded time.
pub const LOOP_ROUNDS: usize = 16;

/// An inclusive integer interval, with a bottom and a top.
///
/// See the module documentation for the lattice, the order and the join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interval {
    /// No value: an unreachable point.
    Bottom,
    /// The set `{ lo, ..., hi }`, inclusive, with `lo <= hi`.
    Range {
        /// The smallest value, inclusive.
        lo: i128,
        /// The largest value, inclusive.
        hi: i128,
    },
    /// Every value of the local's type: the analysis could not bound it.
    Top,
}

impl Default for Interval {
    /// The conservative default: no bound.
    fn default() -> Self {
        Interval::Top
    }
}

impl Interval {
    /// No value.
    pub const fn bottom() -> Self {
        Interval::Bottom
    }

    /// Every value.
    pub const fn top() -> Self {
        Interval::Top
    }

    /// The interval `[lo, hi]`, or [`Interval::Bottom`] when `lo > hi` (the set is empty).
    pub fn range(lo: i128, hi: i128) -> Self {
        if lo <= hi {
            Interval::Range { lo, hi }
        } else {
            Interval::Bottom
        }
    }

    /// The point `[v, v]`.
    pub const fn point(v: i128) -> Self {
        Interval::Range { lo: v, hi: v }
    }

    /// Whether this is [`Interval::Bottom`].
    pub fn is_bottom(self) -> bool {
        matches!(self, Interval::Bottom)
    }

    /// Whether this is [`Interval::Top`].
    pub fn is_top(self) -> bool {
        matches!(self, Interval::Top)
    }

    /// The smallest value, or `None` when the interval is Top or Bottom.
    pub fn lo(self) -> Option<i128> {
        match self {
            Interval::Range { lo, .. } => Some(lo),
            Interval::Bottom | Interval::Top => None,
        }
    }

    /// The largest value, or `None` when the interval is Top or Bottom.
    pub fn hi(self) -> Option<i128> {
        match self {
            Interval::Range { hi, .. } => Some(hi),
            Interval::Bottom | Interval::Top => None,
        }
    }

    /// The point `v`, if this interval is exactly that point.
    pub fn as_point(self) -> Option<i128> {
        match self {
            Interval::Range { lo, hi } if lo == hi => Some(lo),
            _ => None,
        }
    }

    /// Whether `v` is in the interval. Top contains every value.
    pub fn contains(self, v: i128) -> bool {
        match self {
            Interval::Bottom => false,
            Interval::Top => true,
            Interval::Range { lo, hi } => lo <= v && v <= hi,
        }
    }

    /// The hull: the smallest interval that contains both operands. Top is absorbing and Bottom is
    /// the identity.
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Interval::Top, _) | (_, Interval::Top) => Interval::Top,
            (Interval::Bottom, x) | (x, Interval::Bottom) => x,
            (Interval::Range { lo: a, hi: b }, Interval::Range { lo: c, hi: d }) => {
                Interval::range(a.min(c), b.max(d))
            }
        }
    }

    /// The intersection. Bottom is absorbing and Top is the identity.
    pub fn meet(self, other: Self) -> Self {
        match (self, other) {
            (Interval::Bottom, _) | (_, Interval::Bottom) => Interval::Bottom,
            (Interval::Top, x) | (x, Interval::Top) => x,
            (Interval::Range { lo: a, hi: b }, Interval::Range { lo: c, hi: d }) => {
                Interval::range(a.max(c), b.min(d))
            }
        }
    }

    /// This interval intersected with `[lo, hi]`.
    fn clip(self, lo: i128, hi: i128) -> Self {
        self.meet(Interval::range(lo, hi))
    }
}

impl PartialOrd for Interval {
    /// The interval order documented on the module. Two overlapping intervals that are not nested
    /// are incomparable, so this is partial, not total.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Interval::Bottom, Interval::Bottom) | (Interval::Top, Interval::Top) => {
                Some(Ordering::Equal)
            }
            (Interval::Bottom, _) => Some(Ordering::Less),
            (_, Interval::Bottom) => Some(Ordering::Greater),
            (Interval::Top, _) => Some(Ordering::Greater),
            (_, Interval::Top) => Some(Ordering::Less),
            (Interval::Range { lo: a, hi: b }, Interval::Range { lo: c, hi: d }) => {
                if a == c && b == d {
                    Some(Ordering::Equal)
                } else if c <= a && b <= d {
                    Some(Ordering::Less)
                } else if a <= c && d <= b {
                    Some(Ordering::Greater)
                } else {
                    None
                }
            }
        }
    }
}

/// The width of an integer operation, from its [`OpTy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntWidth {
    /// `Int`: 32-bit, wrapping.
    I32,
    /// `Int64`: 64-bit, wrapping.
    I64,
}

impl IntWidth {
    /// The width of an operand type, or `None` for a type whose arithmetic the analysis does not
    /// model (Float, Bool, String, Structural).
    fn of_op(ty: OpTy) -> Option<Self> {
        match ty {
            OpTy::Int => Some(IntWidth::I32),
            OpTy::Int64 => Some(IntWidth::I64),
            OpTy::Float | OpTy::Bool | OpTy::String | OpTy::Structural => None,
        }
    }

    /// The width of a local's type, for an induction variable.
    fn of_type(ty: TypeId) -> Option<Self> {
        if ty == TypeId::INT {
            Some(IntWidth::I32)
        } else if ty == TypeId::INT64 {
            Some(IntWidth::I64)
        } else {
            None
        }
    }

    /// The smallest and largest value of the width.
    fn bounds(self) -> (i128, i128) {
        match self {
            IntWidth::I32 => (i32::MIN as i128, i32::MAX as i128),
            IntWidth::I64 => (i64::MIN as i128, i64::MAX as i128),
        }
    }
}

/// The ranges of the locals of one function.
///
/// The map is the hull of the intervals seen at every binding and read of a local, so it over-
/// approximates the local at any one point. A local that is absent is Top.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FnRanges {
    /// The range of each local the analysis could bound.
    pub ranges: Ranges,
    /// The induction variables of the loops in the function.
    pub inductions: Vec<Induction>,
}

impl FnRanges {
    /// The range of `local`: Top when the map has no entry.
    pub fn range(&self, local: Local) -> Interval {
        self.ranges.get(&local).copied().unwrap_or(Interval::Top)
    }
}

/// The range of each local the analysis could bound. An absent local is Top.
pub type Ranges = FxHashMap<Local, Interval>;

/// The ranges of every function of a module, keyed by function.
pub type ModuleRanges = FxHashMap<FnId, FnRanges>;

/// A loop counter whose every back edge updates it by the same constant.
///
/// See the module documentation for when an entry is reported and when `iterations` is `Some`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Induction {
    /// The loop variable.
    pub var: Local,
    /// The interval of the initial value.
    pub init: Interval,
    /// The constant added on every back edge.
    pub step: i128,
    /// The number of back edges the loop's guard allows from `init`, when it proves one.
    ///
    /// This is exact for an evaluation that reaches the guard on every iteration and ends by
    /// leaving the loop normally. An evaluation that fails or defects earlier ends the loop
    /// sooner, so a consumer treats it as the count of a complete loop.
    pub iterations: Option<u64>,
}

/// The bindings of the locals bound so far.
///
/// A local that is not bound is Top. A [`Local`] is bound at most once in a module
/// ([`crate::ir`]), so joining two branch environments is a union with the hull on a key both
/// sides hold; a local bound on one side only stays with that side's interval, because it is out
/// of scope after the join.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    bindings: FxHashMap<Local, Interval>,
}

impl Env {
    /// The empty environment: every local is Top.
    pub fn new() -> Self {
        Self::default()
    }

    /// The interval of `local`, or Top when it is not bound.
    pub fn get(&self, local: Local) -> Interval {
        self.bindings.get(&local).copied().unwrap_or(Interval::Top)
    }

    /// Binds `local` to `interval`.
    pub fn set(&mut self, local: Local, interval: Interval) {
        self.bindings.insert(local, interval);
    }

    /// Joins another environment into this one: the hull on a key both hold, the other side's
    /// interval on a key only it holds.
    fn join_into(&mut self, other: &Env) {
        for (&local, &interval) in &other.bindings {
            match self.bindings.get(&local) {
                Some(&mine) => {
                    self.bindings.insert(local, mine.join(interval));
                }
                None => {
                    self.bindings.insert(local, interval);
                }
            }
        }
    }
}

/// The range analysis.
pub struct RangesAnalysis;

impl Analysis for RangesAnalysis {
    type Output = Fact<ModuleRanges>;

    const ID: AnalysisId = AnalysisId::Ranges;

    /// The ranges of every function of `m`.
    ///
    /// Always `Known`: the walk is total over the IR and an unbounded value is `Interval::Top`
    /// inside the result. A consumer treats Top and `Fact::Unknown` the same way.
    fn analyze(&self, m: &Module) -> Self::Output {
        let mut all = ModuleRanges::default();
        for (f, _) in m.functions.iter() {
            if let Fact::Known(ranges) = of_function(m, f) {
                all.insert(f, ranges);
            }
        }
        Fact::Known(all)
    }
}

/// The ranges of `m`.
pub fn analyze(m: &Module) -> Fact<ModuleRanges> {
    RangesAnalysis.analyze(m)
}

/// The interval of `e`, evaluated with `env` for the locals it reads.
///
/// The walk records nothing: it answers for one expression. Use [`of_function`] for the ranges of
/// every local.
pub fn of_expr<'a>(m: &'a Module, e: &'a Expr, env: &Env) -> Interval {
    let mut ctx = Ctx::new(m, env.clone());
    ctx.transfer(e)
}

/// The ranges of the locals of `f` and its induction variables.
pub fn of_function(m: &Module, f: FnId) -> Fact<FnRanges> {
    let mut ctx = Ctx::new(m, Env::new());
    let function = &m.functions[f];
    for &param in &function.params {
        ctx.bind(param, Interval::Top);
    }
    ctx.transfer(&function.body);
    Fact::Known(FnRanges {
        ranges: ctx.ranges,
        inductions: ctx.inductions,
    })
}

/// The induction variables of the loops of `f`.
///
/// `Known` with one entry per recognised induction variable; a loop whose step is not constant
/// contributes nothing. See [`Induction`] for when `iterations` is `Some`.
pub fn inductions(m: &Module, f: FnId) -> Fact<Vec<Induction>> {
    match of_function(m, f) {
        Fact::Known(ranges) => Fact::Known(ranges.inductions),
        Fact::Unknown => Fact::Unknown,
    }
}

/// One `Recur` site of the loop being transferred, with the intervals of its arguments in the
/// environment of the round.
struct RecurSite<'a> {
    args: Vec<&'a Expr>,
    intervals: Vec<Interval>,
}

/// The state of one [`of_expr`] walk.
struct Ctx<'a> {
    m: &'a Module,
    env: Env,
    ranges: Ranges,
    inductions: Vec<Induction>,
    /// One frame per enclosing `Loop`; a `Recur` appends to the innermost.
    recur_stack: Vec<Vec<RecurSite<'a>>>,
}

impl<'a> Ctx<'a> {
    fn new(m: &'a Module, env: Env) -> Self {
        Ctx {
            m,
            env,
            ranges: Ranges::default(),
            inductions: Vec::new(),
            recur_stack: Vec::new(),
        }
    }

    /// Binds `local` and records its range.
    fn bind(&mut self, local: Local, interval: Interval) {
        self.env.set(local, interval);
        self.record(local, interval);
    }

    /// Records `interval` as a possible range of `local`. Top adds nothing: the entry would only
    /// say what a missing entry already says.
    fn record(&mut self, local: Local, interval: Interval) {
        if interval.is_top() {
            return;
        }
        let entry = self
            .ranges
            .entry(local)
            .or_insert(Interval::Bottom)
            .join(interval);
        self.ranges.insert(local, entry);
    }

    /// Forgets the range of `local`: it is unknown, not bounded by what was seen.
    fn deopt(&mut self, local: Local) {
        self.env.set(local, Interval::Top);
        self.ranges.insert(local, Interval::Top);
    }

    /// The interval of `e` in the current environment, without recording.
    fn interval(&self, e: &'a Expr) -> Interval {
        of_expr(self.m, e, &self.env)
    }

    /// The interval of one expression, recording the ranges of the locals it binds and reads.
    fn transfer(&mut self, e: &'a Expr) -> Interval {
        match e {
            Expr::Lit(lit) => literal(lit),
            Expr::Var(local) => {
                let interval = self.env.get(*local);
                self.record(*local, interval);
                interval
            }
            Expr::Fn { .. } => Interval::Top,
            Expr::Let { var, value, body } => {
                let interval = self.transfer(value);
                self.bind(*var, interval);
                self.transfer(body)
            }
            Expr::If { cond, then, else_ } => self.transfer_if(cond, then, else_),
            Expr::Match { scrutinee, arms } => self.transfer_match(scrutinee, arms),
            Expr::Escape { body, arms } => {
                let mut result = self.transfer(body);
                // The error value is not an Int or Int64 the analysis knows.
                for arm in arms {
                    self.bind_pattern(&arm.pat, Interval::Top);
                    if let Some(guard) = &arm.guard {
                        self.transfer(guard);
                    }
                    result = result.join(self.transfer(&arm.body));
                }
                result
            }
            Expr::Binary { op, ty, lhs, rhs } => {
                let l = self.transfer(lhs);
                let r = self.transfer(rhs);
                binary(*op, *ty, l, r)
            }
            Expr::Unary { op, ty, operand } => {
                let v = self.transfer(operand);
                unary(*op, *ty, v)
            }
            Expr::Lambda { params, body, .. } => {
                let outer = self.env.clone();
                for &param in params {
                    self.bind(param, Interval::Top);
                }
                self.transfer(body);
                self.env = outer;
                Interval::Top
            }
            Expr::Loop { vars, inits, body } => self.transfer_loop(vars, inits, body),
            Expr::Recur(args) => {
                let intervals: Vec<Interval> = args.iter().map(|a| self.transfer(a)).collect();
                if let Some(frame) = self.recur_stack.last_mut() {
                    frame.push(RecurSite {
                        args: args.iter().collect(),
                        intervals,
                    });
                }
                // The value of a back edge is not the value of the loop expression.
                Interval::Bottom
            }
            Expr::Fail(inner) => {
                self.transfer(inner);
                Interval::Bottom
            }
            Expr::With { binds, body } => {
                for b in binds {
                    let interval = self.transfer(&b.value);
                    self.bind(b.var, interval);
                    if let Some(release) = &b.release {
                        self.transfer(&release.body);
                    }
                }
                self.transfer(body)
            }
            // Every construct the analysis does not model: transfer the children so their ranges
            // are recorded, and answer Top for the value.
            _ => {
                e.for_each_child(&mut |child| {
                    self.transfer(child);
                });
                Interval::Top
            }
        }
    }

    /// `if cond then else_`: narrow both branches by the condition and join their results.
    fn transfer_if(&mut self, cond: &'a Expr, then: &'a Expr, else_: &'a Expr) -> Interval {
        if self.transfer(cond).is_bottom() {
            return Interval::Bottom;
        }
        let base = self.env.clone();
        self.refine(cond, true);
        let then_interval = self.transfer(then);
        let then_env = std::mem::replace(&mut self.env, base.clone());
        self.refine(cond, false);
        let else_interval = self.transfer(else_);
        let else_env = std::mem::replace(&mut self.env, base);
        self.env.join_into(&then_env);
        self.env.join_into(&else_env);
        then_interval.join(else_interval)
    }

    /// `match scrutinee { arms }`: bind each arm's pattern and join the arms.
    fn transfer_match(&mut self, scrutinee: &'a Expr, arms: &'a [crate::ir::Arm]) -> Interval {
        let scrutinee_interval = self.transfer(scrutinee);
        let base = self.env.clone();
        let mut result = Interval::Bottom;
        let mut arm_envs = Vec::with_capacity(arms.len());
        for arm in arms {
            self.env = base.clone();
            if let Expr::Var(local) = scrutinee
                && let Some(value) = literal_point(&arm.pat)
            {
                self.env.set(*local, Interval::point(value));
            }
            self.bind_pattern(&arm.pat, scrutinee_interval);
            if let Some(guard) = &arm.guard {
                self.transfer(guard);
            }
            result = result.join(self.transfer(&arm.body));
            arm_envs.push(std::mem::replace(&mut self.env, base.clone()));
        }
        self.env = base;
        for env in &arm_envs {
            self.env.join_into(env);
        }
        if arms.is_empty() {
            Interval::Bottom
        } else {
            result
        }
    }

    /// Binds the locals a pattern binds. A nested field or element is Top: the analysis does not
    /// split a value's interval.
    fn bind_pattern(&mut self, pat: &'a Pat, interval: Interval) {
        match pat {
            Pat::Wild | Pat::Lit(_) => {}
            Pat::Bind(local) => self.bind(*local, interval),
            Pat::Ctor { fields, .. } | Pat::Tuple(fields) => {
                for field in fields {
                    self.bind_pattern(field, Interval::Top);
                }
            }
            Pat::List { elems, rest } => {
                for elem in elems {
                    self.bind_pattern(elem, Interval::Top);
                }
                if let Some(rest) = rest {
                    self.bind_pattern(rest, Interval::Top);
                }
            }
        }
    }

    /// Narrows the current environment with the comparison `cond`, for the branch where it is
    /// `truth`. Only a bound on a `Var` is used, and only when the other operand has a bound.
    fn refine(&mut self, cond: &'a Expr, truth: bool) {
        let Some((op, lhs, rhs, width)) = comparison(cond) else {
            return;
        };
        let op = if truth { op } else { negate(op) };
        let (lo, hi) = width.bounds();
        let left = self.interval(lhs);
        let right = self.interval(rhs);
        match op {
            BinOp::Lt => {
                self.narrow_hi(lhs, right.hi().map(|v| v - 1), lo, hi);
                self.narrow_lo(rhs, left.lo().map(|v| v + 1), lo, hi);
            }
            BinOp::Le => {
                self.narrow_hi(lhs, right.hi(), lo, hi);
                self.narrow_lo(rhs, left.lo(), lo, hi);
            }
            BinOp::Gt => {
                self.narrow_lo(lhs, right.lo().map(|v| v + 1), lo, hi);
                self.narrow_hi(rhs, left.hi().map(|v| v - 1), lo, hi);
            }
            BinOp::Ge => {
                self.narrow_lo(lhs, right.lo(), lo, hi);
                self.narrow_hi(rhs, left.hi(), lo, hi);
            }
            BinOp::Eq => {
                self.narrow_meet(lhs, right, lo, hi);
                self.narrow_meet(rhs, left, lo, hi);
            }
            BinOp::Ne
            | BinOp::And
            | BinOp::Or
            | BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem => {}
        }
    }

    /// Restricts `e` (a `Var`) to a largest value of `hi`.
    fn narrow_hi(&mut self, e: &'a Expr, hi: Option<i128>, lo: i128, type_hi: i128) {
        let (Expr::Var(local), Some(hi)) = (e, hi) else {
            return;
        };
        let narrowed = self.env.get(*local).clip(lo, hi.min(type_hi));
        self.bind(*local, narrowed);
    }

    /// Restricts `e` (a `Var`) to a smallest value of `lo`.
    fn narrow_lo(&mut self, e: &'a Expr, lo: Option<i128>, type_lo: i128, hi: i128) {
        let (Expr::Var(local), Some(lo)) = (e, lo) else {
            return;
        };
        let narrowed = self.env.get(*local).clip(lo.max(type_lo), hi);
        self.bind(*local, narrowed);
    }

    /// Restricts `e` (a `Var`) to the intersection with `with`.
    fn narrow_meet(&mut self, e: &'a Expr, with: Interval, lo: i128, hi: i128) {
        let Expr::Var(local) = e else {
            return;
        };
        let narrowed = self.env.get(*local).meet(with).clip(lo, hi);
        self.bind(*local, narrowed);
    }

    /// `Loop { vars, inits, body }`: the fixed point of the initial values and the back edges.
    fn transfer_loop(&mut self, vars: &'a [Local], inits: &'a [Expr], body: &'a Expr) -> Interval {
        let pre_env = self.env.clone();
        let mut init_intervals = Vec::with_capacity(vars.len());
        for (var, init) in vars.iter().zip(inits) {
            let interval = self.transfer(init);
            self.bind(*var, interval);
            init_intervals.push(interval);
        }
        let mut loop_env = self.env.clone();
        self.recur_stack.push(Vec::new());

        let mut rounds = 0usize;
        let mut widened = false;
        let (result, sites) = loop {
            rounds += 1;
            if let Some(frame) = self.recur_stack.last_mut() {
                frame.clear();
            }
            let result = self.transfer(body);
            let sites = std::mem::take(
                self.recur_stack
                    .last_mut()
                    .expect("the frame pushed above is still there"),
            );

            let mut changed = false;
            let mut next = loop_env.clone();
            for (index, var) in vars.iter().enumerate() {
                let back = sites
                    .iter()
                    .filter_map(|site| site.intervals.get(index).copied())
                    .fold(Interval::Bottom, Interval::join);
                let joined = loop_env.get(*var).join(back);
                if joined != loop_env.get(*var) {
                    changed = true;
                }
                next.set(*var, joined);
            }
            if !changed {
                break (result, sites);
            }
            loop_env = next;
            if rounds >= LOOP_ROUNDS && !widened {
                // No fixed point after the cap: deopt every loop variable to Top. A Top variable
                // is absorbing, so the next round cannot change and the loop stops.
                widened = true;
                for var in vars {
                    self.deopt(*var);
                }
                loop_env = self.env.clone();
            }
            self.env = loop_env.clone();
        };

        self.recur_stack.pop();
        self.env = loop_env;
        self.record_inductions(vars, &init_intervals, &sites, body, &pre_env);
        result
    }

    /// Reports the induction variables of a loop whose fixed point is known.
    fn record_inductions(
        &mut self,
        vars: &'a [Local],
        init_intervals: &[Interval],
        sites: &[RecurSite<'a>],
        body: &'a Expr,
        pre_env: &Env,
    ) {
        if sites.is_empty() {
            return;
        }
        for (index, var) in vars.iter().enumerate() {
            let init = init_intervals[index];
            if init.as_point().is_none() {
                continue;
            }
            let Some(step) = constant_step(sites, index, *var) else {
                continue;
            };
            let iterations = self.iteration_count(*var, init, step, sites, body, pre_env);
            self.inductions.push(Induction {
                var: *var,
                init,
                step,
                iterations,
            });
        }
    }

    /// The trip count of an induction variable, when the loop's guard proves it.
    fn iteration_count(
        &self,
        var: Local,
        init: Interval,
        step: i128,
        sites: &[RecurSite<'a>],
        body: &'a Expr,
        pre_env: &Env,
    ) -> Option<u64> {
        if step == 0 || sites.len() != 1 {
            return None;
        }
        let start = init.as_point()?;
        let width = IntWidth::of_type(self.m.locals[var].ty)?;
        let (min, max) = width.bounds();
        let (guard, recur_in_then) = loop_guard(body)?;
        let (op, lhs, rhs, _) = comparison(guard)?;

        let (var_on_left, bound) = if is_var(lhs, var) {
            (true, rhs)
        } else if is_var(rhs, var) {
            (false, lhs)
        } else {
            return None;
        };
        if super::uses::of_expr(bound, var) != 0 {
            // A bound that mentions the counter is not independent of it.
            return None;
        }
        let bound_interval = of_expr(self.m, bound, pre_env);
        let bound = bound_interval.as_point()?;

        // The condition under which the next iteration is taken.
        let mut op = if recur_in_then { op } else { negate(op) };
        if !var_on_left {
            op = swap(op);
        }

        let steps = if step > 0 {
            match op {
                BinOp::Lt => {
                    if start < bound {
                        ceil_div(bound - start, step)?
                    } else {
                        0
                    }
                }
                BinOp::Le => {
                    if start <= bound {
                        (bound - start) / step + 1
                    } else {
                        0
                    }
                }
                BinOp::Gt | BinOp::Ge => {
                    if start > bound || (op == BinOp::Ge && start == bound) {
                        // The guard holds while the counter grows: no iteration count.
                        return None;
                    }
                    0
                }
                _ => return None,
            }
        } else {
            let stride = -step;
            match op {
                BinOp::Gt => {
                    if start > bound {
                        ceil_div(start - bound, stride)?
                    } else {
                        0
                    }
                }
                BinOp::Ge => {
                    if start >= bound {
                        (start - bound) / stride + 1
                    } else {
                        0
                    }
                }
                BinOp::Lt | BinOp::Le => {
                    if start < bound || (op == BinOp::Le && start == bound) {
                        return None;
                    }
                    0
                }
                _ => return None,
            }
        };

        let steps: u64 = steps.try_into().ok()?;
        // The counter reaches its final value on the last back edge. If that value wraps, the
        // guard would see the wrapped value and the count would not be the number of back edges.
        let end = start + i128::from(steps) * step;
        if end < min || end > max {
            return None;
        }
        Some(steps)
    }
}

/// The interval of a literal.
fn literal(lit: &Lit) -> Interval {
    match lit {
        Lit::Int(v) => Interval::point(i128::from(*v)),
        Lit::Int64(v) => Interval::point(i128::from(*v)),
        Lit::Bool(v) => Interval::point(i128::from(*v)),
        Lit::Float(_) | Lit::String(_) | Lit::Unit => Interval::Top,
    }
}

/// The point a literal pattern matches, for refining the scrutinee.
fn literal_point(pat: &Pat) -> Option<i128> {
    match pat {
        Pat::Lit(Lit::Int(v)) => Some(i128::from(*v)),
        Pat::Lit(Lit::Int64(v)) => Some(i128::from(*v)),
        Pat::Lit(Lit::Bool(v)) => Some(i128::from(*v)),
        _ => None,
    }
}

/// The transfer of a binary operation.
fn binary(op: BinOp, ty: OpTy, l: Interval, r: Interval) -> Interval {
    match op {
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if l.is_bottom() || r.is_bottom() {
                Interval::Bottom
            } else {
                Interval::range(0, 1)
            }
        }
        BinOp::And | BinOp::Or => {
            if l.is_bottom() || r.is_bottom() {
                Interval::Bottom
            } else {
                Interval::range(0, 1)
            }
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            let Some(width) = IntWidth::of_op(ty) else {
                return Interval::Top;
            };
            arithmetic(op, width, l, r)
        }
    }
}

/// The exact bounds of an arithmetic operation on `width`, or Top when it may overflow or divide
/// by zero.
fn arithmetic(op: BinOp, width: IntWidth, l: Interval, r: Interval) -> Interval {
    if l.is_bottom() || r.is_bottom() {
        return Interval::Bottom;
    }
    let (Some(ll), Some(lh), Some(rl), Some(rh)) = (l.lo(), l.hi(), r.lo(), r.hi()) else {
        return Interval::Top;
    };
    let (min, max) = width.bounds();
    let (lo, hi) = match op {
        BinOp::Add => (ll + rl, lh + rh),
        BinOp::Sub => (ll - rh, lh - rl),
        BinOp::Mul => {
            let products = [ll * rl, ll * rh, lh * rl, lh * rh];
            (
                products.iter().copied().min().expect("four products"),
                products.iter().copied().max().expect("four products"),
            )
        }
        BinOp::Div => {
            if r.contains(0) {
                return Interval::Top;
            }
            let quotients = [
                ll.checked_div(rl),
                ll.checked_div(rh),
                lh.checked_div(rl),
                lh.checked_div(rh),
            ];
            let mut lo = i128::MAX;
            let mut hi = i128::MIN;
            for quotient in quotients {
                let Some(q) = quotient else {
                    return Interval::Top;
                };
                lo = lo.min(q);
                hi = hi.max(q);
            }
            (lo, hi)
        }
        BinOp::Rem => {
            if r.contains(0) {
                return Interval::Top;
            }
            // The remainder has the sign of the dividend and a magnitude below the largest
            // divisor. A corner-based bound is not sound for a divisor range wider than one value.
            let magnitude = rl.abs().max(rh.abs()) - 1;
            let (lo, hi) = if ll >= 0 {
                (0, magnitude)
            } else if lh <= 0 {
                (-magnitude, 0)
            } else {
                (-magnitude, magnitude)
            };
            (lo, hi)
        }
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => unreachable!("handled by `binary`"),
    };
    if lo < min || hi > max {
        return Interval::Top;
    }
    Interval::range(lo, hi)
}

/// The transfer of a unary operation.
fn unary(op: UnOp, ty: OpTy, operand: Interval) -> Interval {
    if operand.is_bottom() {
        return Interval::Bottom;
    }
    match op {
        UnOp::Neg => {
            let Some(width) = IntWidth::of_op(ty) else {
                return Interval::Top;
            };
            let (Some(lo), Some(hi)) = (operand.lo(), operand.hi()) else {
                return Interval::Top;
            };
            let (min, max) = width.bounds();
            let (neg_lo, neg_hi) = (-hi, -lo);
            if neg_lo < min || neg_hi > max {
                return Interval::Top;
            }
            Interval::range(neg_lo, neg_hi)
        }
        UnOp::Not => match (operand.lo(), operand.hi()) {
            (Some(lo), Some(hi)) => Interval::range(1 - hi, 1 - lo).clip(0, 1),
            _ => Interval::Top,
        },
    }
}

/// The comparison behind `cond` when it is an integer comparison, with the width of its operands.
fn comparison(cond: &Expr) -> Option<(BinOp, &Expr, &Expr, IntWidth)> {
    let Expr::Binary { op, ty, lhs, rhs } = cond else {
        return None;
    };
    if !matches!(
        op,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
    ) {
        return None;
    }
    Some((*op, lhs, rhs, IntWidth::of_op(*ty)?))
}

/// The comparison that holds when `op` does not.
fn negate(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Ge,
        BinOp::Le => BinOp::Gt,
        BinOp::Gt => BinOp::Le,
        BinOp::Ge => BinOp::Lt,
        BinOp::Eq => BinOp::Ne,
        BinOp::Ne => BinOp::Eq,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::And | BinOp::Or => {
            op
        }
    }
}

/// `op` with its operands exchanged.
fn swap(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Rem
        | BinOp::And
        | BinOp::Or => op,
    }
}

/// Whether `e` is a read of `local`.
fn is_var(e: &Expr, local: Local) -> bool {
    matches!(e, Expr::Var(v) if *v == local)
}

/// The constant a `Recur` argument adds to `var`, if it has the shape `var + c`, `c + var` or
/// `var - c`.
fn step_of(e: &Expr, var: Local) -> Option<i128> {
    let Expr::Binary { op, lhs, rhs, .. } = e else {
        return None;
    };
    match op {
        BinOp::Add if is_var(lhs, var) => constant(rhs),
        BinOp::Add if is_var(rhs, var) => constant(lhs),
        BinOp::Sub if is_var(lhs, var) => Some(-constant(rhs)?),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Rem
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => None,
    }
}

/// The value of an integer literal, possibly negated.
fn constant(e: &Expr) -> Option<i128> {
    match e {
        Expr::Lit(Lit::Int(v)) => Some(i128::from(*v)),
        Expr::Lit(Lit::Int64(v)) => Some(i128::from(*v)),
        Expr::Unary {
            op: UnOp::Neg,
            operand,
            ..
        } => Some(-constant(operand)?),
        _ => None,
    }
}

/// The common constant step of `index` across every `Recur` site, or `None` when a site is not
/// `var + const` or the steps differ.
fn constant_step(sites: &[RecurSite<'_>], index: usize, var: Local) -> Option<i128> {
    let mut step: Option<i128> = None;
    for site in sites {
        let argument = site.args.get(index)?;
        let found = step_of(argument, var)?;
        match step {
            None => step = Some(found),
            Some(previous) if previous == found => {}
            Some(_) => return None,
        }
    }
    step
}

/// The `if` whose one branch continues the loop, with whether that branch is the then-branch.
///
/// Only a `let*`-wrapped `if` is recognised: any other shape has no provable trip count. The
/// continuing branch must be the `Recur` itself (behind `let`s): a branch that reaches the back
/// edge only on some of its paths can also leave the loop early, and then the guard does not
/// determine how many back edges are taken.
fn loop_guard(body: &Expr) -> Option<(&Expr, bool)> {
    let mut current = body;
    while let Expr::Let { body, .. } = current {
        current = body;
    }
    let Expr::If { cond, then, else_ } = current else {
        return None;
    };
    if is_recur_tail(then) && !has_recur(else_) {
        Some((cond, true))
    } else if is_recur_tail(else_) && !has_recur(then) {
        Some((cond, false))
    } else {
        None
    }
}

/// Whether the value of `e` is a `Recur`, with only `let` bindings between it and the tail.
fn is_recur_tail(e: &Expr) -> bool {
    let mut current = e;
    while let Expr::Let { body, .. } = current {
        current = body;
    }
    matches!(current, Expr::Recur(_))
}

/// Whether `e` contains a `Recur` of this loop. A nested `Loop` has its own `Recur`s, so the walk
/// does not enter one.
fn has_recur(e: &Expr) -> bool {
    match e {
        Expr::Recur(_) => true,
        Expr::Loop { .. } => false,
        _ => {
            let mut found = false;
            e.for_each_child(&mut |child| found |= has_recur(child));
            found
        }
    }
}

/// `ceil(a / b)` for `a >= 0` and `b > 0`.
fn ceil_div(a: i128, b: i128) -> Option<i128> {
    debug_assert!(a >= 0 && b > 0);
    a.checked_add(b - 1)?.checked_div(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The join is the hull and is monotone in both operands.
    #[test]
    fn join_is_the_hull() {
        let a = Interval::range(1, 3);
        let b = Interval::range(2, 8);
        assert_eq!(a.join(b), Interval::range(1, 8));
        assert_eq!(a.join(Interval::Bottom), a);
        assert_eq!(a.join(Interval::Top), Interval::Top);
        assert!(a <= a.join(b));
        assert!(b <= a.join(b));
    }

    /// The order is the interval order, with Bottom and Top at the ends.
    #[test]
    fn the_order_is_partial() {
        assert!(Interval::Bottom < Interval::range(0, 0));
        assert!(Interval::range(0, 0) < Interval::range(-1, 1));
        assert!(Interval::range(-1, 1) < Interval::Top);
        assert_eq!(
            Interval::range(0, 2).partial_cmp(&Interval::range(1, 3)),
            None,
            "overlapping but not nested intervals are incomparable"
        );
    }
}
