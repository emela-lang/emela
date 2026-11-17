//! Scalar replacement of tuples: a tuple that stays inside its function becomes its fields.
//!
//! A tuple of scalars that is built and read in the same function, and whose value does not
//! escape (see [`super::escape`]), needs no allocation: each `tuple_get` reads a field that was
//! already evaluated when the tuple was built, and the tuple itself is never observed. Replacing
//! the reads with those field values removes the allocation and the tuple object, which is most of
//! what an `Option`, a `(Int, Int)` or a record pair costs in the benchmarks.
//!
//! # Contract
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

use rustc_hash::{FxHashMap, FxHashSet};

use super::Stats;
use super::escape;
use crate::heap::compact;
use crate::ir::{
    Const, FnId, Function, Module, Operand, Stmt, StmtId, SwitchOn, Term, Value, VarId,
};
use crate::repr::Repr;

/// Replaces every non-escaping scalar tuple of every function.
///
/// The functions are visited in arena order and the tuples in statement order, so the result does
/// not depend on the iteration order of a hash map. A changed function is rebuilt with
/// `heap::compact`, which drops the removed statements and variables and numbers the rest in the
/// order met.
pub(crate) fn run(m: &mut Module) -> Stats {
    let mut stats = Stats::default();
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in functions {
        let f = &m.functions[id];
        if f.body.is_none() {
            continue;
        }
        let escaping = escape::escaping(m, f);
        let uses = uses(f);
        let plan = plan(f, &escaping, &uses);
        if plan.count == 0 {
            continue;
        }
        let mut f = f.clone();
        plan.apply(&mut f);
        m.functions[id] = compact(&f);
        stats.sra_tuples += plan.count;
    }
    stats
}

/// A tuple to replace: remove its construction, and either substitute each read or bind it to the
/// field constant.
#[derive(Default)]
struct Plan {
    /// The `dst = tuple(..)` statements to remove.
    tuples: Vec<StmtId>,
    /// The `dst = tuple_get t#i` statements removed because their destination is substituted.
    reads: Vec<StmtId>,
    /// `tuple_get` destination → the field variable that replaces every use of it.
    substitution: FxHashMap<VarId, VarId>,
    /// `tuple_get` statements kept as `dst = const`, with the field constant.
    constants: Vec<(StmtId, Const)>,
    /// Tuples replaced, the counter's contribution.
    count: u32,
}

impl Plan {
    fn apply(&self, f: &mut Function) {
        for (stmt, value) in &self.constants {
            let Stmt::Let { value: at, .. } = &mut f.stmts[*stmt] else {
                unreachable!("a tuple_get is a let")
            };
            *at = Value::Const(value.clone());
        }

        // Resolve chains such as `y -> x` and `x -> a`, which arise when one tuple's field is a
        // read of another tuple. Every chain ends at a field variable (a definition that is not a
        // replaced read), so this terminates.
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

        for (_, stmt) in f.stmts.iter_mut() {
            rewrite_stmt(stmt, &substitution);
        }
        for (_, seq) in f.seqs.iter_mut() {
            rewrite_term(&mut seq.term, &substitution);
        }

        let remove: FxHashSet<StmtId> = self
            .tuples
            .iter()
            .chain(self.reads.iter())
            .copied()
            .collect();
        for (_, seq) in f.seqs.iter_mut() {
            seq.stmts.retain(|s| !remove.contains(s));
        }
    }
}

/// What the function does with each variable: the `tuple_get` reads of it, and every other use.
#[derive(Default)]
struct Uses {
    /// `(the read statement, the field index)` for every `tuple_get` with this variable as base.
    reads: Vec<(StmtId, usize)>,
    /// Uses other than a `tuple_get` base.
    other: usize,
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
            if let Value::TupleGet { base, index } = value {
                get(uses, *base).reads.push((id, *index));
            } else {
                value.for_each_var(|v| get(uses, v).other += 1);
            }
            if let Some(suspend) = on_suspend {
                for &v in &suspend.saved {
                    get(uses, v).other += 1;
                }
            }
        }
        Stmt::If { cond, .. } => operand_use(uses, cond),
        Stmt::Switch { on, .. } => {
            let (SwitchOn::Tag(v) | SwitchOn::Lit(v)) = on;
            get(uses, *v).other += 1;
        }
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
/// fields and does not escape.
fn plan(f: &Function, escaping: &escape::Set, uses: &FxHashMap<VarId, Uses>) -> Plan {
    let mut plan = Plan::default();
    for (_, seq) in f.seqs.iter() {
        for &id in &seq.stmts {
            let Stmt::Let {
                dst: Some(tuple),
                value: Value::Tuple(args),
                on_defect: None,
                on_suspend: None,
            } = &f.stmts[id]
            else {
                continue;
            };
            if escaping.contains(tuple) || !args.iter().all(|a| is_scalar(f, a)) {
                continue;
            }
            let Some(used) = uses.get(tuple) else {
                continue;
            };
            if used.other > 0 || used.reads.is_empty() {
                continue;
            }
            // Every variable field must stay used after the read that binds it goes away, or a
            // later rebuild of the function would drop its definition (which may have effects).
            // A field read whose destination is not used does not keep it.
            let mut read_used = vec![false; args.len()];
            for &(read, index) in &used.reads {
                let Some(dst) = f.stmts[read].dst() else {
                    continue;
                };
                if uses.get(&dst).is_some_and(|u| u.other + u.reads.len() > 0) {
                    read_used[index] = true;
                }
            }
            if args
                .iter()
                .enumerate()
                .any(|(i, field)| matches!(field, Operand::Var(_)) && !read_used[i])
            {
                continue;
            }
            let mut reads = Vec::new();
            let mut substitution = Vec::new();
            let mut constants = Vec::new();
            let mut legal = true;
            for &(read, index) in &used.reads {
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
                continue;
            }
            plan.tuples.push(id);
            plan.reads.extend(reads);
            plan.substitution.extend(substitution);
            plan.constants.extend(constants);
            plan.count += 1;
        }
    }
    plan
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
    }

    impl Fixture {
        fn new() -> Self {
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
            Fixture {
                m: Module::new(core),
                pair,
                list_pair,
                nested,
                ref_pair,
                thunk,
            }
        }

        fn build<R>(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> (FnId, R) {
            let mut b = FnBuilder::new(&self.m.core, name);
            let r = f(&mut b);
            let function = b.finish();
            (self.m.add(function), r)
        }

        /// Runs the pass at the stage of its precondition and checks the result.
        fn run(&mut self) -> Stats {
            self.m.stage = Stage::ClosuresConverted;
            let stats = super::run(&mut self.m);
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
}
