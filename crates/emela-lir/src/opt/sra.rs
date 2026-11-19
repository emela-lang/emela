//! Scalar replacement of tuples and records: an aggregate that stays inside its function becomes
//! its fields.
//!
//! A tuple or a record of scalars that is built and read in the same function, and whose value
//! does not escape (see [`super::escape`]), needs no allocation: each read of a field returns a
//! value that was already evaluated when the aggregate was built, and the aggregate itself is
//! never observed. Replacing the reads with those field values removes the allocation and the
//! aggregate object, which is most of what an `Option`, a `(Int, Int)` or a record costs in the
//! benchmarks.
//!
//! A record here is a [`Value::Make`] of an enum with exactly one variant, which is neither an
//! error enum nor `@external`; such a variant with fields is held as a reference. Its reads are
//! [`Value::Field`] and [`SwitchOn::Tag`] switches. Nested records are not decomposed: the inner
//! one is a reference field of the outer, and it is used by the outer construction.
//!
//! Closure records ([`Value::MakeClosure`], [`Value::EnvGet`] and a call through
//! [`Callee::Indirect`](crate::ir::Callee::Indirect)) are not records of this rule and are not
//! decomposed here. Replacing a closure needs the closure devirtualisation of E5/PR-037: the pass
//! must turn an indirect call into a direct one, or keep the closure's function table entry
//! reachable. Records and closures are the two halves of C5/PR-043; the flag
//! [`LirRules::sra_records`](super::LirRules::sra_records) covers records only, and the closure
//! half is [`super::closure_sra`] behind
//! [`LirRules::sra_closures`](super::LirRules::sra_closures).
//!
//! # Contract: scalar tuples
//!
//! Preconditions: the module is at [`Stage::ClosuresConverted`]; the pass runs in
//! [`super::run_mid`].
//!
//! Postconditions: a replaced tuple has no construction, no `tuple_get` and no variable left; the
//! module still verifies.
//!
//! Preserves: evaluation order, effects, failures, drop points, the evidence tables, and the
//! order of construction of the tuple's fields (they are operands, evaluated where they were).
//!
//! Deopt: the pass leaves a tuple alone when
//!
//! - its value escapes ([`super::escape::escaping`]): it is returned, failed with, stored in a
//!   list or another container, captured by a closure, boxed into `Any`, passed to a call or
//!   builtin, or live across a suspension point;
//! - a field is a reference (first version: only `I32`, `I64` and `F64` fields are replaced, so
//!   that no reference-counting ownership moves);
//! - a use is not a `tuple_get` (a loop variable, a container, another aggregate, a terminator);
//! - a variable field is never read through a `tuple_get` whose destination is used: removing the
//!   tuple would make the field's definition unreachable, and that definition may have effects;
//! - the tuple has no read at all, so removing it could drop the effect of a field's definition;
//! - the construction has a defect landing or a suspension edge (`on_defect` / `on_suspend`);
//! - a `tuple_get` of it has a defect landing or a suspension edge.
//!
//! A tuple nested inside another tuple is a reference field of the outer one, and the inner one is
//! used by the outer construction, so the first version replaces neither. That is a safe deopt,
//! not a wrong answer.
//!
//! Measures: `Stats::sra_tuples`, the tuples replaced.
//!
//! # Contract: scalar records
//!
//! ```text
//! Rule ID: SRA record (C5/PR-043)
//! Theory: scalar replacement of aggregates, behind the escape analysis of B3.
//! Equation: let r = K(a, b) in C[field r K.x, field r K.y, switch tag r { K -> e }]
//!           -> let a' = a, b' = b in C[a', b', e]      (K is the only variant; no allocation)
//! Preconditions: the construction is a `let` of a `make` of a single-variant, non-error,
//!                non-external enum, without a defect landing or a suspension edge; the escape
//!                analysis answers `Known` and the record is not in its set; every field is
//!                `i32`, `i64` or `f64`; every use is a field read or a tag switch; each variable
//!                field is read through a read whose destination is used.
//! Postconditions: the construction is gone, no `field` of it is left and no tag switch on it is
//!                  left; the module still verifies.
//! Preserves: evaluation order, effects, failures, defect and suspension edges, drop points and
//!            the evidence tables. A field value is never moved: a variable field keeps its
//!            existing definition where it was, and a constant field is bound where it was read.
//! Deopt: the pass leaves a record alone when the escape analysis answers `Unknown` or the record
//!        escapes (returned, failed with, stored in a container, captured, passed on, boxed, live
//!        across a suspension), a field is a reference (a nested record included), a use is not a
//!        field read or a tag switch, a variable field has no read whose destination is used (its
//!        definition may have effects), a field read has a defect landing or a suspension edge,
//!        the construction has one, or the tag switch has no arm for the variant. A record with
//!        no use at all is left alone as well.
//! Budget: none; the rewrite removes a construction and reads and adds nothing.
//! Measures: `Stats::sra_records` (records replaced) and `Stats::sra_deopts` (candidates left
//!           alone, one per `make` bound by a `let`).
//! ```
//!
//! A field whose value is a constant becomes `dst = const` at each read, as for a tuple; a field
//! whose value is a variable is that variable, so its existing definition stays where it was and
//! the field evaluation order is unchanged.
//!
//! The tag switch is replaced by a block holding the arm for the single variant (its case, or its
//! default when it has no case). The verifier requires a tag switch without a default to cover
//! every constructor; a record has exactly one, so the other arms are unreachable and are dropped
//! with the switch.

use rustc_hash::{FxHashMap, FxHashSet};

use super::Stats;
use super::escape;
use crate::heap::compact;
use crate::ir::{
    CaseKey, Const, FnId, Function, LabelData, Module, Operand, SeqId, Stmt, StmtId, SwitchOn,
    Term, Value, VarId,
};
use crate::repr::Repr;
use emela_core::CtorRef;

/// Replaces every non-escaping scalar tuple of every function, and, when `records` is set, every
/// non-escaping scalar record.
///
/// The functions are visited in arena order and the aggregates in statement order, so the result
/// does not depend on the iteration order of a hash map. A changed function is rebuilt with
/// `heap::compact`, which drops the removed statements and variables and numbers the rest in the
/// order met.
pub(crate) fn run(m: &mut Module, records: bool) -> Stats {
    let mut stats = Stats::default();
    // The record rule uses the shared escape analysis behind its API. A module the analysis cannot
    // answer for makes every record deopt; that is why the answer is an `Option` here and not an
    // empty set.
    let record_escapes = if records {
        super::analysis::escape::analyze(m).known()
    } else {
        None
    };
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in functions {
        let f = &m.functions[id];
        if f.body.is_none() {
            continue;
        }
        let escaping = escape::escaping(m, f);
        let uses = uses(f);
        let record_escape = record_escapes.as_ref().and_then(|sets| sets.get(&id));
        let plan = plan(m, f, &escaping, &uses, records, record_escape);
        stats.sra_deopts += plan.deopts;
        if plan.tuples == 0 && plan.records == 0 {
            continue;
        }
        let mut f = f.clone();
        plan.apply(&mut f);
        m.functions[id] = compact(&f);
        stats.sra_tuples += plan.tuples;
        stats.sra_records += plan.records;
    }
    stats
}

/// An aggregate to replace: remove its construction, and either substitute each read or bind it to
/// the field constant; replace a tag switch by its known arm.
#[derive(Default)]
struct Plan {
    /// The `dst = tuple(..)` and `dst = make(..)` statements to remove.
    constructions: FxHashSet<StmtId>,
    /// The `dst = tuple_get t#i` / `dst = field r K.i` statements removed because their
    /// destination is substituted.
    reads: FxHashSet<StmtId>,
    /// Read destination → the field variable that replaces every use of it.
    substitution: FxHashMap<VarId, VarId>,
    /// Reads kept as `dst = const`, with the field constant.
    constants: Vec<(StmtId, Const)>,
    /// Tag switches replaced by the arm of the known variant.
    switches: Vec<(StmtId, SeqId)>,
    /// Tuples replaced, the counter's contribution.
    tuples: u32,
    /// Records replaced, the counter's contribution.
    records: u32,
    /// Records left alone, the counter's contribution.
    deopts: u32,
}

impl Plan {
    fn apply(&self, f: &mut Function) {
        for (stmt, value) in &self.constants {
            let Stmt::Let { value: at, .. } = &mut f.stmts[*stmt] else {
                unreachable!("a field read is a let")
            };
            *at = Value::Const(value.clone());
        }

        // Resolve chains such as `y -> x` and `x -> a`, which arise when one aggregate's field is
        // a read of another aggregate. Every chain ends at a field variable (a definition that is
        // not a replaced read), so this terminates.
        let mut substitution = FxHashMap::default();
        for &from in self.substitution.keys() {
            let mut to = from;
            let mut steps = 0;
            while let Some(&next) = self.substitution.get(&to) {
                to = next;
                steps += 1;
                assert!(
                    steps <= self.substitution.len(),
                    "a cycle in the SRA substitution"
                );
            }
            substitution.insert(from, to);
        }

        // A tag switch on a single-variant record always takes one arm. A block takes the place of
        // the switch: it keeps the switch's destination, and its body is the arm, so the arm's
        // `yield` still gives the statement's value. The other arms are unreachable and `compact`
        // drops them.
        for (stmt, arm) in &self.switches {
            let dst = f.stmts[*stmt].dst();
            let label = f.labels.alloc(LabelData {
                hint: "sra".to_owned(),
            });
            f.stmts[*stmt] = Stmt::Block {
                dst,
                label,
                body: *arm,
            };
        }

        for (_, stmt) in f.stmts.iter_mut() {
            rewrite_stmt(stmt, &substitution);
        }
        for (_, seq) in f.seqs.iter_mut() {
            rewrite_term(&mut seq.term, &substitution);
        }

        for (_, seq) in f.seqs.iter_mut() {
            seq.stmts
                .retain(|s| !self.constructions.contains(s) && !self.reads.contains(s));
        }
    }
}

/// What the function does with each variable: its reads, its tag switches, and every other use.
#[derive(Default)]
struct Uses {
    /// `(the read statement, the field index)` for every `tuple_get` with this variable as base.
    tuple_reads: Vec<(StmtId, usize)>,
    /// `(the read statement, the field index, the constructor)` for every `field` read.
    field_reads: Vec<(StmtId, usize, CtorRef)>,
    /// The tag switches whose scrutinee is this variable.
    tag_switches: Vec<StmtId>,
    /// Uses other than a read or a tag switch of the variable.
    other: usize,
}

impl Uses {
    /// Whether the variable is used at all. SRA asks this of a read's destination: a read whose
    /// destination is unused does not keep the field it reads alive.
    fn is_used(&self) -> bool {
        self.other + self.tuple_reads.len() + self.field_reads.len() + self.tag_switches.len() > 0
    }
}

/// Classifies every use in the function. Every statement is in some sequence, so walking the
/// sequence arena (and not only the body) sees them all; the result is conservative for a
/// statement left unreachable by an earlier pass.
fn uses(f: &Function) -> FxHashMap<VarId, Uses> {
    let mut uses: FxHashMap<VarId, Uses> = FxHashMap::default();
    for (_, seq) in f.seqs.iter() {
        for &id in &seq.stmts {
            stmt_uses(f, id, &mut uses);
        }
        term_uses(&seq.term, &mut uses);
    }
    uses
}

fn get(uses: &mut FxHashMap<VarId, Uses>, v: VarId) -> &mut Uses {
    uses.entry(v).or_default()
}

fn operand_use(uses: &mut FxHashMap<VarId, Uses>, o: &Operand) {
    if let Operand::Var(v) = o {
        get(uses, *v).other += 1;
    }
}

fn operands_use(uses: &mut FxHashMap<VarId, Uses>, os: &[Operand]) {
    for o in os {
        operand_use(uses, o);
    }
}

fn stmt_uses(f: &Function, id: StmtId, uses: &mut FxHashMap<VarId, Uses>) {
    match &f.stmts[id] {
        Stmt::Let {
            value, on_suspend, ..
        } => {
            match value {
                Value::TupleGet { base, index } => {
                    get(uses, *base).tuple_reads.push((id, *index));
                }
                Value::Field { base, ctor, index } => {
                    get(uses, *base).field_reads.push((id, *index, *ctor));
                }
                _ => value.for_each_var(|v| get(uses, v).other += 1),
            }
            if let Some(suspend) = on_suspend {
                for &v in &suspend.saved {
                    get(uses, v).other += 1;
                }
            }
        }
        Stmt::If { cond, .. } => operand_use(uses, cond),
        Stmt::Switch { on, .. } => match on {
            SwitchOn::Tag(v) => get(uses, *v).tag_switches.push(id),
            SwitchOn::Lit(v) => get(uses, *v).other += 1,
        },
        Stmt::Loop { inits, .. } => operands_use(uses, inits),
        Stmt::Block { .. } | Stmt::Protect { .. } => {}
        Stmt::Dup(v) | Stmt::Drop(v) => get(uses, *v).other += 1,
    }
}

fn term_uses(t: &Term, uses: &mut FxHashMap<VarId, Uses>) {
    match t {
        Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
            if let Some(v) = v {
                operand_use(uses, v);
            }
        }
        Term::Fail(v) | Term::Defect(v, _) => operand_use(uses, v),
        Term::Recur(args) => operands_use(uses, args),
        Term::Unwind | Term::Unreachable => {}
    }
}

/// The plan for `f`: every tuple that is built locally, read only by `tuple_get`, has scalar
/// fields and does not escape; and, when `records` is set, every single-variant record that is
/// built locally, read only by `field` and tag switches, has scalar fields and does not escape.
fn plan(
    m: &Module,
    f: &Function,
    escaping: &escape::Set,
    uses: &FxHashMap<VarId, Uses>,
    records: bool,
    record_escape: Option<&escape::Set>,
) -> Plan {
    let mut plan = Plan::default();
    for (_, seq) in f.seqs.iter() {
        for &id in &seq.stmts {
            match &f.stmts[id] {
                Stmt::Let {
                    dst: Some(tuple),
                    value: Value::Tuple(args),
                    on_defect: None,
                    on_suspend: None,
                } => plan_tuple(f, escaping, uses, id, *tuple, args, &mut plan),
                Stmt::Let {
                    dst: Some(record),
                    value: Value::Make { ctor, args },
                    on_defect,
                    on_suspend,
                } if records => {
                    if !is_record(m, f, *record, ctor, args) {
                        continue;
                    }
                    if on_defect.is_some() || on_suspend.is_some() {
                        plan.deopts += 1;
                        continue;
                    }
                    let construction = Construction {
                        id,
                        record: *record,
                        ctor,
                        args,
                    };
                    if !plan_record(f, record_escape, uses, &construction, &mut plan) {
                        plan.deopts += 1;
                    }
                }
                _ => {}
            }
        }
    }
    plan
}

/// Adds the plan for one tuple to `plan`.
fn plan_tuple(
    f: &Function,
    escaping: &escape::Set,
    uses: &FxHashMap<VarId, Uses>,
    id: StmtId,
    tuple: VarId,
    args: &[Operand],
    plan: &mut Plan,
) {
    if escaping.contains(&tuple) || !args.iter().all(|a| is_scalar(f, a)) {
        return;
    }
    let Some(used) = uses.get(&tuple) else {
        return;
    };
    if used.other > 0
        || !used.field_reads.is_empty()
        || !used.tag_switches.is_empty()
        || used.tuple_reads.is_empty()
    {
        return;
    }
    // Every variable field must stay used after the read that binds it goes away, or a later
    // rebuild of the function would drop its definition (which may have effects). A field read
    // whose destination is not used does not keep it.
    let mut read_used = vec![false; args.len()];
    for &(read, index) in &used.tuple_reads {
        let Some(dst) = f.stmts[read].dst() else {
            continue;
        };
        if uses.get(&dst).is_some_and(Uses::is_used) {
            read_used[index] = true;
        }
    }
    if args
        .iter()
        .enumerate()
        .any(|(i, field)| matches!(field, Operand::Var(_)) && !read_used[i])
    {
        return;
    }
    let mut reads = Vec::new();
    let mut substitution = Vec::new();
    let mut constants = Vec::new();
    let mut legal = true;
    for &(read, index) in &used.tuple_reads {
        let field = &args[index];
        let read_stmt = &f.stmts[read];
        let Some(dst) = read_stmt.dst() else {
            legal = false;
            break;
        };
        if !matches!(
            read_stmt,
            Stmt::Let {
                on_defect: None,
                on_suspend: None,
                ..
            }
        ) {
            legal = false;
            break;
        }
        match field {
            Operand::Var(v) => {
                substitution.push((dst, *v));
                reads.push(read);
            }
            Operand::Const(c) => constants.push((read, c.clone())),
        }
    }
    if !legal {
        return;
    }
    plan.constructions.insert(id);
    plan.reads.extend(reads);
    plan.substitution.extend(substitution);
    plan.constants.extend(constants);
    plan.tuples += 1;
}

/// Whether `ctor` with `args` makes a record this rule may decompose: a single-variant enum that
/// is neither an error enum nor `@external`, held as a reference, with at least one field.
fn is_record(m: &Module, f: &Function, record: VarId, ctor: &CtorRef, args: &[Operand]) -> bool {
    let def = &m.core.enums[ctor.enum_id];
    !def.error
        && !def.external
        && def.variants.len() == 1
        && ctor.variant == 0
        && !args.is_empty()
        && f.vars[record].repr == Repr::Ref
}

/// A record construction to consider: where it is, its destination, its constructor and its
/// fields.
struct Construction<'a> {
    id: StmtId,
    record: VarId,
    ctor: &'a CtorRef,
    args: &'a [Operand],
}

/// Adds the plan for one record to `plan`, or answers `false` when the record must be left alone.
fn plan_record(
    f: &Function,
    record_escape: Option<&escape::Set>,
    uses: &FxHashMap<VarId, Uses>,
    construction: &Construction<'_>,
    plan: &mut Plan,
) -> bool {
    let Construction {
        id,
        record,
        ctor,
        args,
    } = *construction;
    // A missing answer is `Unknown`: the pass must not read it as "does not escape".
    let Some(escaping) = record_escape else {
        return false;
    };
    if escaping.contains(&record) {
        return false;
    }
    let Some(used) = uses.get(&record) else {
        return false;
    };
    if used.other > 0 || !used.tuple_reads.is_empty() {
        return false;
    }
    if used.field_reads.is_empty() && used.tag_switches.is_empty() {
        return false;
    }
    // A reference field (a nested record included) would move an owned reference, which this rule
    // leaves to reference counting.
    if !args.iter().all(|a| is_scalar(f, a)) {
        return false;
    }
    let mut switches = Vec::new();
    for &switch in &used.tag_switches {
        let Stmt::Switch { cases, default, .. } = &f.stmts[switch] else {
            return false;
        };
        // The arm of the only variant: its case, or the default when the switch has no case.
        let arm = cases
            .iter()
            .find(|case| case.keys.contains(&CaseKey::Variant(ctor.variant)))
            .map(|case| case.body)
            .or(*default);
        let Some(arm) = arm else {
            return false;
        };
        switches.push((switch, arm));
    }
    // Every field read must be a plain `let` of that field, and every variable field must have a
    // read whose destination is used (see `plan_tuple`).
    let mut read_used = vec![false; args.len()];
    let mut reads = Vec::new();
    let mut substitution = Vec::new();
    let mut constants = Vec::new();
    for &(read, index, read_ctor) in &used.field_reads {
        if read_ctor != *ctor || index >= args.len() {
            return false;
        }
        let read_stmt = &f.stmts[read];
        let Some(dst) = read_stmt.dst() else {
            return false;
        };
        if !matches!(
            read_stmt,
            Stmt::Let {
                on_defect: None,
                on_suspend: None,
                ..
            }
        ) {
            return false;
        }
        if uses.get(&dst).is_some_and(Uses::is_used) {
            read_used[index] = true;
        }
        match &args[index] {
            Operand::Var(v) => {
                substitution.push((dst, *v));
                reads.push(read);
            }
            Operand::Const(c) => constants.push((read, c.clone())),
        }
    }
    if args
        .iter()
        .enumerate()
        .any(|(i, field)| matches!(field, Operand::Var(_)) && !read_used[i])
    {
        return false;
    }
    plan.constructions.insert(id);
    plan.reads.extend(reads);
    plan.substitution.extend(substitution);
    plan.constants.extend(constants);
    plan.switches.extend(switches);
    plan.records += 1;
    true
}

/// Whether a tuple field has a representation SRA can copy (`I32`, `I64`, `F64`). A reference
/// field would move an owned reference, which the first version leaves to reference counting.
fn is_scalar(f: &Function, o: &Operand) -> bool {
    let repr = match o {
        Operand::Var(v) => f.vars[*v].repr,
        Operand::Const(c) => c.repr(),
    };
    matches!(repr, Repr::I32 | Repr::I64 | Repr::F64)
}

fn rewrite_stmt(stmt: &mut Stmt, substitution: &FxHashMap<VarId, VarId>) {
    match stmt {
        Stmt::Let {
            value, on_suspend, ..
        } => {
            value.for_each_var_mut(|v| rename(v, substitution));
            if let Some(suspend) = on_suspend {
                for v in &mut suspend.saved {
                    rename(v, substitution);
                }
            }
        }
        Stmt::If { cond, .. } => rename_operand(cond, substitution),
        Stmt::Switch { on, .. } => {
            let (SwitchOn::Tag(v) | SwitchOn::Lit(v)) = on;
            rename(v, substitution);
        }
        Stmt::Loop { inits, .. } => {
            for o in inits {
                rename_operand(o, substitution);
            }
        }
        Stmt::Block { .. } | Stmt::Protect { .. } => {}
        Stmt::Dup(v) | Stmt::Drop(v) => rename(v, substitution),
    }
}

fn rewrite_term(term: &mut Term, substitution: &FxHashMap<VarId, VarId>) {
    match term {
        Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
            if let Some(v) = v {
                rename_operand(v, substitution);
            }
        }
        Term::Fail(v) | Term::Defect(v, _) => rename_operand(v, substitution),
        Term::Recur(args) => {
            for o in args {
                rename_operand(o, substitution);
            }
        }
        Term::Unwind | Term::Unreachable => {}
    }
}

fn rename(v: &mut VarId, substitution: &FxHashMap<VarId, VarId>) {
    if let Some(&to) = substitution.get(v) {
        *v = to;
    }
}

fn rename_operand(o: &mut Operand, substitution: &FxHashMap<VarId, VarId>) {
    if let Operand::Var(v) = o {
        rename(v, substitution);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::FnBuilder;
    use crate::ir::{Callee, FnId, PrimOp, PrimTy, Stage};
    use crate::verify;
    use emela_core::{FnType, TypeId};

    fn int(n: i32) -> Operand {
        Operand::Const(Const::Int(n))
    }

    /// A Core module with the types the tests use, plus a builder that adds a function.
    struct Fixture {
        m: Module,
        pair: TypeId,
        list_pair: TypeId,
        nested: TypeId,
        ref_pair: TypeId,
        thunk: TypeId,
        /// `type Point(x: Int, y: Int)`: a single-variant record.
        point: TypeId,
        point_ctor: CtorRef,
        list_point: TypeId,
        /// A record with a reference field.
        named: TypeId,
        named_ctor: CtorRef,
        /// A record with a record field.
        outer: TypeId,
        outer_ctor: CtorRef,
        inner_ctor: CtorRef,
    }

    impl Fixture {
        fn new() -> Self {
            use emela_core::{EnumDef, VariantDef};

            let mut core = emela_core::Module::new();
            let pair = core.tuple(vec![TypeId::INT, TypeId::INT]);
            let list_pair = core.list(pair);
            let nested = core.tuple(vec![pair, TypeId::INT]);
            let ref_pair = core.tuple(vec![TypeId::STRING, TypeId::INT]);
            let thunk = core.func(FnType {
                params: vec![],
                ret: TypeId::INT,
                fails: false,
                suspends: false,
                ev: false,
                sets: None,
            });
            let point_enum = core.add_enum(EnumDef {
                name: "Point".to_owned(),
                params: 0,
                variants: vec![VariantDef::positional(
                    "Point",
                    vec![TypeId::INT, TypeId::INT],
                )],
                error: false,
                external: false,
            });
            let point = core.enum_type(point_enum, vec![]);
            let list_point = core.list(point);
            let named_enum = core.add_enum(EnumDef {
                name: "Named".to_owned(),
                params: 0,
                variants: vec![VariantDef::positional(
                    "Named",
                    vec![TypeId::STRING, TypeId::INT],
                )],
                error: false,
                external: false,
            });
            let named = core.enum_type(named_enum, vec![]);
            let outer_enum = core.add_enum(EnumDef {
                name: "Outer".to_owned(),
                params: 0,
                variants: vec![VariantDef::positional("Outer", vec![point, TypeId::INT])],
                error: false,
                external: false,
            });
            let outer = core.enum_type(outer_enum, vec![]);
            Fixture {
                m: Module::new(core),
                pair,
                list_pair,
                nested,
                ref_pair,
                thunk,
                point,
                point_ctor: CtorRef {
                    enum_id: point_enum,
                    variant: 0,
                },
                list_point,
                named,
                named_ctor: CtorRef {
                    enum_id: named_enum,
                    variant: 0,
                },
                outer,
                outer_ctor: CtorRef {
                    enum_id: outer_enum,
                    variant: 0,
                },
                inner_ctor: CtorRef {
                    enum_id: point_enum,
                    variant: 0,
                },
            }
        }

        fn build<R>(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> (FnId, R) {
            let mut b = FnBuilder::new(&self.m.core, name);
            let r = f(&mut b);
            let function = b.finish();
            (self.m.add(function), r)
        }

        /// Runs the pass at the stage of its precondition and checks the result. Records are
        /// decomposed too: the tests below cover both rules.
        fn run(&mut self) -> Stats {
            self.m.stage = Stage::ClosuresConverted;
            let stats = super::run(&mut self.m, true);
            if let Err(errors) = verify(&self.m) {
                let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                panic!("SRA produced invalid IR:\n{}", lines.join("\n"));
            }
            stats
        }
    }

    fn dump(fx: &Fixture, id: FnId) -> String {
        crate::dump_function(&fx.m, &fx.m.functions[id])
    }

    #[test]
    fn a_non_escaping_tuple_becomes_its_fields() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("read", |b| {
            b.set_ret(TypeId::INT);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            let y = b.assign("y", TypeId::INT, Value::TupleGet { base: t, index: 1 });
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![x.into(), y.into()],
                },
            );
            b.term(Term::Return(Some(sum.into())));
        });
        assert_eq!(fx.run().sra_tuples, 1);
        let text = dump(&fx, id);
        assert!(!text.contains("tuple("), "{text}");
        assert!(!text.contains("tuple_get"), "{text}");
    }

    #[test]
    fn reading_the_same_field_twice_uses_the_same_scalar() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("twice", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let b_ = b.param("b", TypeId::INT);
            let t = b.assign("t", pair, Value::Tuple(vec![a.into(), b_.into()]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            let y = b.assign("y", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            let z = b.assign("z", TypeId::INT, Value::TupleGet { base: t, index: 1 });
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![x.into(), y.into()],
                },
            );
            let total = b.assign(
                "total",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![sum.into(), z.into()],
                },
            );
            b.term(Term::Return(Some(total.into())));
        });
        assert_eq!(fx.run().sra_tuples, 1);
        let text = dump(&fx, id);
        assert!(!text.contains("tuple_get"), "{text}");
        assert!(text.matches("a").count() >= 2, "{text}");
    }

    #[test]
    fn a_constant_field_becomes_a_constant_binding() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("constant", |b| {
            b.set_ret(TypeId::INT);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.term(Term::Return(Some(x.into())));
        });
        assert_eq!(fx.run().sra_tuples, 1);
        let text = dump(&fx, id);
        assert!(!text.contains("tuple("), "{text}");
        assert!(!text.contains("tuple_get"), "{text}");
        assert!(text.contains("= 1"), "{text}");
    }

    #[test]
    fn field_evaluation_order_is_kept() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (f, ()) = fx.build("f", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(1))));
        });
        let (g, ()) = fx.build("g", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(2))));
        });
        let (id, _) = fx.build("order", |b| {
            b.set_ret(TypeId::INT);
            let a = b.assign(
                "a",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(f),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            let b_ = b.assign(
                "b",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(g),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            let t = b.assign("t", pair, Value::Tuple(vec![a.into(), b_.into()]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            let y = b.assign("y", TypeId::INT, Value::TupleGet { base: t, index: 1 });
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![x.into(), y.into()],
                },
            );
            b.term(Term::Return(Some(sum.into())));
        });
        assert_eq!(fx.run().sra_tuples, 1);
        let text = dump(&fx, id);
        let f_at = text.find("call f").expect("a call of f");
        let g_at = text.find("call g").expect("a call of g");
        assert!(f_at < g_at, "{text}");
    }

    fn no_sra(fx: &mut Fixture, build: impl FnOnce(&mut FnBuilder<'_>)) -> (FnId, String) {
        let (id, ()) = fx.build("deopt", build);
        let stats = fx.run();
        assert_eq!(stats.sra_tuples, 0, "SRA fired where it must not");
        let text = dump(fx, id);
        (id, text)
    }

    #[test]
    fn a_returned_tuple_is_left_alone() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (_, text) = no_sra(&mut fx, |b| {
            b.set_ret(pair);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.term(Term::Return(Some(t.into())));
        });
        assert!(text.contains("tuple("), "{text}");
    }

    #[test]
    fn a_tuple_stored_in_a_list_is_left_alone() {
        let mut fx = Fixture::new();
        let (pair, list) = (fx.pair, fx.list_pair);
        let (_, text) = no_sra(&mut fx, |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.assign(
                "xs",
                list,
                Value::List {
                    elems: vec![t.into()],
                    tail: None,
                },
            );
            b.term(Term::Return(None));
        });
        assert!(text.contains("tuple("), "{text}");
    }

    #[test]
    fn a_tuple_captured_by_a_closure_is_left_alone() {
        let mut fx = Fixture::new();
        let (pair, thunk) = (fx.pair, fx.thunk);
        // A lifted function taking its closure record as the environment, with one capture.
        let mut b = FnBuilder::new(&fx.m.core, "g");
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let _ = env;
        b.term(Term::Return(Some(int(0))));
        let mut g = b.finish();
        g.env = Some(vec![pair]);
        let g = fx.m.add(g);
        let (_, text) = no_sra(&mut fx, |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.assign(
                "closure",
                thunk,
                Value::MakeClosure {
                    function: g,
                    captures: vec![t],
                },
            );
            b.term(Term::Return(None));
        });
        assert!(text.contains("tuple("), "{text}");
    }

    #[test]
    fn a_tuple_boxed_into_any_is_left_alone() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (_, text) = no_sra(&mut fx, |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.assign("boxed", TypeId::ANY, Value::Coerce(t));
            b.term(Term::Return(None));
        });
        assert!(text.contains("tuple("), "{text}");
    }

    #[test]
    fn a_tuple_used_as_a_loop_variable_is_left_alone() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (_, text) = no_sra(&mut fx, |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            let v = b.var("v", pair);
            b.loop_(None, vec![(v, t.into())], |b| {
                b.term(Term::Yield(None));
            });
            b.term(Term::Return(None));
        });
        assert!(text.contains("tuple("), "{text}");
    }

    #[test]
    fn a_tuple_with_a_reference_field_is_left_alone() {
        let mut fx = Fixture::new();
        let ref_pair = fx.ref_pair;
        let (_, text) = no_sra(&mut fx, |b| {
            b.set_ret(TypeId::STRING);
            let s = b.param("s", TypeId::STRING);
            let t = b.assign("t", ref_pair, Value::Tuple(vec![s.into(), int(1)]));
            let x = b.assign("x", TypeId::STRING, Value::TupleGet { base: t, index: 0 });
            b.term(Term::Return(Some(x.into())));
        });
        assert!(text.contains("tuple("), "{text}");
    }

    #[test]
    fn a_tuple_with_an_unread_effectful_field_is_left_alone() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (g, ()) = fx.build("g", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(1))));
        });
        let (h, ()) = fx.build("h", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(2))));
        });
        let (_, text) = no_sra(&mut fx, |b| {
            b.set_ret(TypeId::INT);
            let a = b.assign(
                "a",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(g),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            let other = b.assign(
                "other",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(h),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            let t = b.assign("t", pair, Value::Tuple(vec![a.into(), other.into()]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.term(Term::Return(Some(x.into())));
        });
        // Both calls stay: dropping `other`'s definition would drop a call.
        assert_eq!(text.matches("tuple(").count(), 1, "{text}");
        assert!(text.contains("call g"), "{text}");
        assert!(text.contains("call h"), "{text}");
    }

    #[test]
    fn a_chain_of_tuples_resolves_to_the_first_field() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("chain", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let b_ = b.param("b", TypeId::INT);
            let t1 = b.assign("t1", pair, Value::Tuple(vec![a.into(), b_.into()]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t1, index: 0 });
            let z = b.assign("z", TypeId::INT, Value::TupleGet { base: t1, index: 1 });
            let t2 = b.assign("t2", pair, Value::Tuple(vec![x.into(), int(3)]));
            let y = b.assign("y", TypeId::INT, Value::TupleGet { base: t2, index: 0 });
            let w = b.assign("w", TypeId::INT, Value::TupleGet { base: t2, index: 1 });
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![y.into(), z.into()],
                },
            );
            let total = b.assign(
                "total",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![sum.into(), w.into()],
                },
            );
            b.term(Term::Return(Some(total.into())));
        });
        assert_eq!(fx.run().sra_tuples, 2);
        let text = dump(&fx, id);
        assert!(!text.contains("tuple("), "{text}");
        assert!(!text.contains("tuple_get"), "{text}");
        // `y` is the first field of the second tuple, whose value is the first field of the first.
        assert!(text.matches("a").count() >= 2, "{text}");
    }

    #[test]
    fn a_tuple_whose_field_read_is_unused_is_left_alone() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (_, text) = no_sra(&mut fx, |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let b_ = b.param("b", TypeId::INT);
            let t = b.assign("t", pair, Value::Tuple(vec![a.into(), b_.into()]));
            let _dead = b.assign("dead", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.term(Term::Return(Some(int(0))));
        });
        assert!(text.contains("tuple("), "{text}");
    }

    #[test]
    fn a_nested_tuple_is_left_alone() {
        let mut fx = Fixture::new();
        let (pair, nested) = (fx.pair, fx.nested);
        let (_, text) = no_sra(&mut fx, |b| {
            b.set_ret(pair);
            let inner = b.assign("inner", pair, Value::Tuple(vec![int(1), int(2)]));
            let outer = b.assign("outer", nested, Value::Tuple(vec![inner.into(), int(3)]));
            let x = b.assign(
                "x",
                pair,
                Value::TupleGet {
                    base: outer,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        // Both tuples stay: the outer has a reference field, the inner is used by the outer.
        assert_eq!(text.matches("tuple(").count(), 2, "{text}");
    }

    #[test]
    fn the_pass_is_idempotent() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        fx.build("idem", |b| {
            b.set_ret(TypeId::INT);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.term(Term::Return(Some(x.into())));
        });
        assert_eq!(fx.run().sra_tuples, 1);
        let after_first = crate::dump(&fx.m);
        assert_eq!(fx.run().sra_tuples, 0);
        assert_eq!(crate::dump(&fx.m), after_first);
    }

    #[test]
    fn a_tuple_of_parameters_is_not_escaping_by_itself() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        fx.build("params", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let b_ = b.param("b", TypeId::INT);
            let t = b.assign("t", pair, Value::Tuple(vec![a.into(), b_.into()]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            let y = b.assign("y", TypeId::INT, Value::TupleGet { base: t, index: 1 });
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![x.into(), y.into()],
                },
            );
            b.term(Term::Return(Some(sum.into())));
        });
        assert_eq!(fx.run().sra_tuples, 1);
    }

    // Records (C5/PR-043).

    /// Runs the record pass on a function whose record must not be decomposed.
    fn no_record_sra(fx: &mut Fixture, build: impl FnOnce(&mut FnBuilder<'_>)) -> (FnId, String) {
        let (id, ()) = fx.build("deopt", build);
        let stats = fx.run();
        assert_eq!(stats.sra_records, 0, "record SRA fired where it must not");
        assert!(
            stats.sra_deopts > 0,
            "the candidate was not counted as a deopt"
        );
        let text = dump(fx, id);
        (id, text)
    }

    #[test]
    fn a_non_escaping_record_becomes_its_fields() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (id, _) = fx.build("read", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let c = b.param("c", TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![a.into(), c.into()],
                },
            );
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            let y = b.assign(
                "y",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 1,
                },
            );
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![x.into(), y.into()],
                },
            );
            b.term(Term::Return(Some(sum.into())));
        });
        let stats = fx.run();
        assert_eq!(stats.sra_records, 1);
        assert_eq!(stats.sra_deopts, 0);
        let text = dump(&fx, id);
        assert!(!text.contains("make"), "{text}");
        assert!(!text.contains("field"), "{text}");
        // The fields are the variables that the construction already used.
        assert_eq!(text.matches("a%0").count(), 2, "{text}");
    }

    #[test]
    fn a_constant_record_field_becomes_a_constant_binding() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (id, _) = fx.build("constant", |b| {
            b.set_ret(TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![int(1), int(2)],
                },
            );
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        assert_eq!(fx.run().sra_records, 1);
        let text = dump(&fx, id);
        assert!(!text.contains("make"), "{text}");
        assert!(!text.contains("field"), "{text}");
        assert!(text.contains("= 1"), "{text}");
    }

    #[test]
    fn a_tag_switch_on_a_record_takes_the_known_arm() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (id, _) = fx.build("choose", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![a.into(), int(2)],
                },
            );
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            let s = b.var("s", TypeId::INT);
            // The arm reads a field of the record the switch looks at.
            let arm = b.seq(|b| b.term(Term::Yield(Some(x.into()))));
            b.switch(
                Some(s),
                SwitchOn::Tag(p),
                vec![(CaseKey::Variant(0), arm)],
                None,
            );
            b.term(Term::Return(Some(s.into())));
        });
        assert_eq!(fx.run().sra_records, 1);
        let text = dump(&fx, id);
        assert!(!text.contains("make"), "{text}");
        assert!(!text.contains("field"), "{text}");
        assert!(!text.contains("switch"), "{text}");
        // The single arm is now the body of a block with the switch's destination.
        assert!(text.contains("block"), "{text}");
        assert!(text.contains("yield a%0"), "{text}");
    }

    #[test]
    fn a_tag_switch_with_a_default_takes_the_default() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (id, _) = fx.build("fallback", |b| {
            b.set_ret(TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![int(1), int(2)],
                },
            );
            let s = b.var("s", TypeId::INT);
            let arm = b.seq(|b| b.term(Term::Yield(Some(int(7)))));
            b.switch(Some(s), SwitchOn::Tag(p), Vec::new(), Some(arm));
            b.term(Term::Return(Some(s.into())));
        });
        assert_eq!(fx.run().sra_records, 1);
        let text = dump(&fx, id);
        assert!(!text.contains("make"), "{text}");
        assert!(!text.contains("switch"), "{text}");
        assert!(text.contains("7"), "{text}");
    }

    #[test]
    fn a_returned_record_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (_, text) = no_record_sra(&mut fx, |b| {
            b.set_ret(point);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![int(1), int(2)],
                },
            );
            b.term(Term::Return(Some(p.into())));
        });
        assert!(text.contains("make"), "{text}");
    }

    #[test]
    fn a_record_stored_in_a_list_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, ctor, xs) = (fx.point, fx.point_ctor, fx.list_point);
        let (_, text) = no_record_sra(&mut fx, |b| {
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![int(1), int(2)],
                },
            );
            b.assign(
                "xs",
                xs,
                Value::List {
                    elems: vec![p.into()],
                    tail: None,
                },
            );
            b.term(Term::Return(None));
        });
        assert!(text.contains("make"), "{text}");
    }

    #[test]
    fn a_record_captured_by_a_closure_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, ctor, thunk) = (fx.point, fx.point_ctor, fx.thunk);
        let mut b = FnBuilder::new(&fx.m.core, "g");
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let _ = env;
        b.term(Term::Return(Some(int(0))));
        let mut g = b.finish();
        g.env = Some(vec![point]);
        let g = fx.m.add(g);
        let (_, text) = no_record_sra(&mut fx, |b| {
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![int(1), int(2)],
                },
            );
            b.assign(
                "closure",
                thunk,
                Value::MakeClosure {
                    function: g,
                    captures: vec![p],
                },
            );
            b.term(Term::Return(None));
        });
        assert!(text.contains("make"), "{text}");
    }

    #[test]
    fn a_record_with_a_reference_field_is_left_alone() {
        let mut fx = Fixture::new();
        let (named, ctor) = (fx.named, fx.named_ctor);
        let (_, text) = no_record_sra(&mut fx, |b| {
            b.set_ret(TypeId::INT);
            let s = b.param("s", TypeId::STRING);
            let r = b.assign(
                "r",
                named,
                Value::Make {
                    ctor,
                    args: vec![s.into(), int(1)],
                },
            );
            let n = b.assign(
                "n",
                TypeId::INT,
                Value::Field {
                    base: r,
                    ctor,
                    index: 1,
                },
            );
            let name = b.assign(
                "name",
                TypeId::STRING,
                Value::Field {
                    base: r,
                    ctor,
                    index: 0,
                },
            );
            // Both fields are read, so only the reference field can be the reason to deopt.
            let same = b.assign(
                "same",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Eq,
                    ty: PrimTy::String,
                    args: vec![name.into(), s.into()],
                },
            );
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![same.into(), n.into()],
                },
            );
            b.term(Term::Return(Some(sum.into())));
        });
        assert!(text.contains("make"), "{text}");
    }

    #[test]
    fn a_record_with_an_unread_effectful_field_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (g, ()) = fx.build("g", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(1))));
        });
        let (h, ()) = fx.build("h", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(2))));
        });
        let (_, text) = no_record_sra(&mut fx, |b| {
            b.set_ret(TypeId::INT);
            let first = b.assign(
                "first",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(g),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            let other = b.assign(
                "other",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(h),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![first.into(), other.into()],
                },
            );
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        // Dropping `other` would drop a call, so the record stays.
        assert!(text.contains("call g"), "{text}");
        assert!(text.contains("call h"), "{text}");
    }

    #[test]
    fn a_record_with_a_defect_landing_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (id, _) = fx.build("landing", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let landing = b.seq(|b| b.term(Term::Return(Some(int(0)))));
            let p = b.var("p", point);
            b.stmt(Stmt::Let {
                dst: Some(p),
                value: Value::Make {
                    ctor,
                    args: vec![a.into(), int(1)],
                },
                on_defect: Some(landing),
                on_suspend: None,
            });
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        // A defect landing cannot exist at `ClosuresConverted` in valid IR, so this asks the
        // planning step directly instead of building a module the verifier would reject.
        let f = &fx.m.functions[id];
        let escaping = escape::escaping(&fx.m, f);
        let uses = uses(f);
        let p = plan(&fx.m, f, &escaping, &uses, true, Some(&escaping));
        assert_eq!(p.records, 0);
        assert_eq!(p.deopts, 1);
    }

    #[test]
    fn a_record_with_a_suspension_edge_is_left_alone() {
        use crate::ir::Suspend;

        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (id, _) = fx.build("suspend", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let p = b.var("p", point);
            b.stmt(Stmt::Let {
                dst: Some(p),
                value: Value::Make {
                    ctor,
                    args: vec![a.into(), int(1)],
                },
                on_defect: None,
                on_suspend: Some(Suspend {
                    state: 0,
                    saved: Vec::new(),
                }),
            });
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        let f = &fx.m.functions[id];
        let escaping = escape::escaping(&fx.m, f);
        let uses = uses(f);
        let p = plan(&fx.m, f, &escaping, &uses, true, Some(&escaping));
        assert_eq!(p.records, 0);
        assert_eq!(p.deopts, 1);
    }

    #[test]
    fn an_unknown_escape_answer_is_a_deopt() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (id, _) = fx.build("unknown", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![a.into(), int(1)],
                },
            );
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        let f = &fx.m.functions[id];
        let escaping = escape::escaping(&fx.m, f);
        let uses = uses(f);
        let p = plan(&fx.m, f, &escaping, &uses, true, None);
        assert_eq!(
            p.records, 0,
            "`Unknown` must not be read as `does not escape`"
        );
        assert_eq!(p.deopts, 1);
    }

    #[test]
    fn a_nested_record_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, inner_ctor) = (fx.point, fx.inner_ctor);
        let (outer, outer_ctor) = (fx.outer, fx.outer_ctor);
        let (id, _) = fx.build("nested", |b| {
            b.set_ret(TypeId::INT);
            let inner = b.assign(
                "inner",
                point,
                Value::Make {
                    ctor: inner_ctor,
                    args: vec![int(1), int(2)],
                },
            );
            let o = b.assign(
                "o",
                outer,
                Value::Make {
                    ctor: outer_ctor,
                    args: vec![inner.into(), int(3)],
                },
            );
            let k = b.assign(
                "k",
                TypeId::INT,
                Value::Field {
                    base: o,
                    ctor: outer_ctor,
                    index: 1,
                },
            );
            b.term(Term::Return(Some(k.into())));
        });
        let stats = fx.run();
        assert_eq!(stats.sra_records, 0);
        // The outer has a reference field, the inner is used by the outer construction.
        assert_eq!(stats.sra_deopts, 2);
        let text = dump(&fx, id);
        assert_eq!(text.matches("make").count(), 2, "{text}");
    }

    #[test]
    fn a_record_with_no_use_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (_, text) = no_record_sra(&mut fx, |b| {
            b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![int(1), int(2)],
                },
            );
            b.term(Term::Return(None));
        });
        assert!(text.contains("make"), "{text}");
    }

    #[test]
    fn the_record_pass_is_idempotent() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        fx.build("idem", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![a.into(), int(2)],
                },
            );
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        let stats = fx.run();
        assert_eq!(stats.sra_records, 1);
        let after_first = crate::dump(&fx.m);
        let stats = fx.run();
        assert_eq!(stats.sra_records, 0, "record SRA fired again");
        assert_eq!(crate::dump(&fx.m), after_first);
    }

    #[test]
    fn the_record_flag_off_leaves_records_alone() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        fx.build("off", |b| {
            b.set_ret(TypeId::INT);
            let a = b.param("a", TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![a.into(), int(2)],
                },
            );
            let x = b.assign(
                "x",
                TypeId::INT,
                Value::Field {
                    base: p,
                    ctor,
                    index: 0,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        fx.m.stage = Stage::ClosuresConverted;
        let before = crate::dump(&fx.m);
        let stats = super::run(&mut fx.m, false);
        assert_eq!(stats.sra_records, 0);
        assert_eq!(stats.sra_deopts, 0);
        assert_eq!(crate::dump(&fx.m), before);
    }

    #[test]
    fn a_record_used_by_tag_as_a_value_is_left_alone() {
        let mut fx = Fixture::new();
        let (point, ctor) = (fx.point, fx.point_ctor);
        let (_, text) = no_record_sra(&mut fx, |b| {
            b.set_ret(TypeId::INT);
            let p = b.assign(
                "p",
                point,
                Value::Make {
                    ctor,
                    args: vec![int(1), int(2)],
                },
            );
            let t = b.assign("t", TypeId::INT, Value::Tag(p));
            b.term(Term::Return(Some(t.into())));
        });
        assert!(text.contains("make"), "{text}");
    }
}
