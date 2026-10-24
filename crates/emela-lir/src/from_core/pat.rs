//! `match` and `escape`: patterns to decision trees.
//!
//! The arms become rows of a clause matrix, compiled as a backtracking automaton (Le Fessant and
//! Maranget, "Optimizing pattern matching", ICFP 2001):
//!
//! - The rows are split into maximal blocks whose first row's first test looks at the same value
//!   (an occurrence). Each block is compiled inside `block 'next { .. }`; when no row of the block
//!   matches, control breaks out to the rows after it, and finally to `break` to the enclosing
//!   failure label or, at the top of a `match`, `defect("unreachable: no match arm matched")` (as
//!   in JS).
//! - Within a block, every row tests the occurrence, so each row goes to exactly one case: a tag
//!   `switch` for enums (none for an enum of one constructor), a literal `switch` for Int and
//!   Int64, `if` for Bool, `==` comparisons for String and Float, and "empty or not" for lists.
//!   No arm body is converted twice; the price is that a later block may test a value again.
//! - A row without tests left matches: its bindings take the occurrences' variables, then its
//!   guard (if any) is evaluated and, if false, the rows after it are tried.
//!
//! Fields, tuple elements and list heads and tails are read once per path (`Lower::cache`), just
//! before the first test that needs them. The scrutinee is evaluated once (spec 14.1).
//!
//! An `escape` puts its body in a block that failures break out of with the error value; its arms
//! are tried in order with `is_error`, and an error no arm matches goes on to the enclosing
//! failure target (spec 7.3).

use emela_core::{Arm, CtorRef, EnumId, Expr, Lit, Local, Pat, Type, TypeId};

use super::{Key, Lower, R, TMP, irrefutable, lit_const};
use crate::ir::{
    Case, CaseKey, Const, LabelId, Operand, PrimOp, PrimTy, Stmt, SwitchOn, Term, Value, VarId,
};
use crate::repr::Repr;

/// A value being matched: its variable (`None` without representation) and type.
#[derive(Debug, Clone, Copy)]
struct Occ {
    var: Option<VarId>,
    ty: TypeId,
}

/// A pattern of a row; list patterns are taken apart element by element.
#[derive(Debug, Clone, Copy)]
enum PatRef<'e> {
    P(&'e Pat),
    /// `[elems.., ..rest]` (exactly `elems` without `rest`).
    List(&'e [Pat], Option<&'e Pat>),
}

#[derive(Debug, Clone)]
struct Row<'e> {
    /// Tests left, each on a different occurrence.
    tests: Vec<(Occ, PatRef<'e>)>,
    binds: Vec<(Local, Option<Operand>)>,
    /// The index of the arm.
    arm: usize,
}

impl Row<'_> {
    /// The test on `v`, if the row has one.
    fn test_on(&self, v: VarId) -> Option<PatRef<'_>> {
        self.tests
            .iter()
            .find(|(o, _)| o.var == Some(v))
            .map(|(_, p)| *p)
    }
}

/// Where arm bodies give the value: the match's (or escape's) block.
struct Target {
    label: LabelId,
    /// The type of the first arm body that gives a value.
    ty: Option<TypeId>,
}

impl Lower<'_> {
    pub(super) fn match_(&mut self, scrutinee: &Expr, arms: &[Arm]) -> R {
        let s = self.expr(scrutinee)?;
        let occ = Occ {
            var: self.as_var(&s),
            ty: s.ty,
        };
        let label = self.label("match");
        let mut target = Target { label, ty: None };
        self.open();
        let rows = arms
            .iter()
            .enumerate()
            .map(|(arm, a)| Row {
                tests: vec![(occ, PatRef::P(&a.pat))],
                binds: Vec::new(),
                arm,
            })
            .collect();
        self.compile(rows, None, &mut target, arms);
        let body = self.close();
        self.end_block(label, body, target.ty)
    }

    /// Pushes the block and gives its value.
    fn end_block(&mut self, label: LabelId, body: crate::SeqId, ty: Option<TypeId>) -> R {
        let dst = ty.and_then(|t| self.new_var(TMP, t));
        self.push(Stmt::Block { dst, label, body });
        self.joined(dst, ty)
    }

    pub(super) fn escape(&mut self, body: &Expr, arms: &[Arm]) -> R {
        let done = self.label("escape");
        let caught = self.label("caught");
        let mut target = Target {
            label: done,
            ty: None,
        };
        self.open();
        self.open();
        self.fail.push(caught);
        let r = self.expr(body);
        self.fail.pop();
        if let Ok(v) = r {
            target.ty = Some(v.ty);
            self.term(Term::Break {
                label: done,
                value: v.op,
            });
        }
        let caught_body = self.close();
        let err = self.var("err", TypeId::ANY);
        self.push(Stmt::Block {
            dst: Some(err),
            label: caught,
            body: caught_body,
        });
        // Errors fully caught by an earlier arm; later arms for them are never tried (as in JS).
        let mut total: Vec<EnumId> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            let Pat::Ctor { ctor, fields } = &arm.pat else {
                self.error("an escape arm's pattern is not an error constructor".into());
                continue;
            };
            let enum_id = ctor.enum_id;
            if total.contains(&enum_id) {
                continue;
            }
            if arm.guard.is_none() && fields.iter().all(|p| irrefutable(&self.sh.core, p)) {
                total.push(enum_id);
            }
            let next = self.label("next");
            self.open();
            let is = self.assign(
                "is",
                TypeId::BOOL,
                Value::IsError {
                    error: err,
                    enum_id,
                },
            );
            self.open();
            let mut row = Row {
                tests: Vec::new(),
                binds: Vec::new(),
                arm: i,
            };
            // The error's fields are read only if the pattern looks at them.
            if !fields.iter().all(|p| matches!(p, Pat::Wild)) {
                let ety = self.intern(Type::Enum(enum_id, Vec::new()));
                let e = self.assign(
                    "e",
                    ety,
                    Value::AsError {
                        error: err,
                        enum_id,
                    },
                );
                let occ = Occ {
                    var: Some(e),
                    ty: ety,
                };
                row.tests.push((occ, PatRef::P(&arm.pat)));
            }
            self.compile(vec![row], Some(next), &mut target, arms);
            let then = self.close();
            let else_ = self.empty();
            self.push(Stmt::If {
                dst: None,
                cond: Operand::Var(is),
                then,
                else_,
            });
            self.term(Term::Break {
                label: next,
                value: None,
            });
            let body = self.close();
            self.push(Stmt::Block {
                dst: None,
                label: next,
                body,
            });
        }
        // An error no arm matched goes on outward. In a body that cannot fail, outside other
        // escapes, the arms catch every error that can arrive (the type checker says so).
        let t = if self.fail.is_empty() && !self.fails {
            Term::Unreachable
        } else {
            self.fail_term(Operand::Var(err))
        };
        self.term(t);
        let body = self.close();
        self.end_block(done, body, target.ty)
    }

    /// Ends the current path when no row matches.
    fn fail_exit(&mut self, fail: Option<LabelId>) {
        match fail {
            Some(label) => self.term(Term::Break { label, value: None }),
            None => self.term(Term::Defect(
                Operand::Const(Const::String("unreachable: no match arm matched".into())),
                None,
            )),
        }
    }

    fn arm_body(&mut self, body: &Expr, target: &mut Target) {
        if let Ok(v) = self.expr(body) {
            target.ty.get_or_insert(v.ty);
            self.term(Term::Break {
                label: target.label,
                value: v.op,
            });
        }
    }

    /// Emits the decision tree for `rows`; every path ends with a terminator.
    fn compile<'e>(
        &mut self,
        mut rows: Vec<Row<'e>>,
        fail: Option<LabelId>,
        target: &mut Target,
        arms: &'e [Arm],
    ) {
        for row in &mut rows {
            let tests = std::mem::take(&mut row.tests);
            for (occ, p) in tests {
                self.normalize(occ, p, row);
            }
        }
        let Some(first) = rows.first() else {
            self.fail_exit(fail);
            return;
        };
        if first.tests.is_empty() {
            let row = rows.remove(0);
            for (l, op) in row.binds {
                self.locals.insert(l, op);
            }
            let arm = &arms[row.arm];
            match &arm.guard {
                None => self.arm_body(&arm.body, target),
                Some(g) => {
                    let Ok(c) = self.expr(g) else {
                        return;
                    };
                    let cond = c.op.unwrap_or(Operand::Const(Const::Bool(false)));
                    self.open();
                    self.arm_body(&arm.body, target);
                    let then = self.close();
                    let else_ = self.empty();
                    self.push(Stmt::If {
                        dst: None,
                        cond,
                        then,
                        else_,
                    });
                    self.compile(rows, fail, target, arms);
                }
            }
            return;
        }
        let occ = first.tests[0].0;
        let Some(v) = occ.var else {
            self.error("a pattern test on a value without representation".into());
            self.fail_exit(fail);
            return;
        };
        let split = rows
            .iter()
            .position(|r| r.test_on(v).is_none())
            .unwrap_or(rows.len());
        let rest = rows.split_off(split);
        if rest.is_empty() {
            self.switch_on(v, occ.ty, rows, fail, target, arms);
            return;
        }
        let next = self.label("next");
        self.open();
        self.switch_on(v, occ.ty, rows, Some(next), target, arms);
        let body = self.close();
        self.push(Stmt::Block {
            dst: None,
            label: next,
            body,
        });
        self.compile(rest, fail, target, arms);
    }

    /// Adds the tests and bindings of `p` on `occ` to the row: wildcards and `()` test nothing,
    /// bindings bind, tuples are taken apart (reading their elements), and what is left are tests
    /// of constructors, literals and lists.
    fn normalize<'e>(&mut self, occ: Occ, p: PatRef<'e>, row: &mut Row<'e>) {
        match p {
            PatRef::P(Pat::Wild | Pat::Lit(Lit::Unit)) => {}
            PatRef::P(Pat::Bind(l)) => row.binds.push((*l, occ.var.map(Operand::Var))),
            PatRef::P(Pat::Tuple(ps)) => {
                let Type::Tuple(ts) = self.ty(occ.ty).clone() else {
                    self.error("a tuple pattern on a value that is not a tuple".into());
                    return;
                };
                for (i, sub) in ps.iter().enumerate() {
                    if let Pat::Wild = sub {
                        continue;
                    }
                    let var = occ.var.and_then(|base| {
                        self.read(
                            Key::Tuple(base, i),
                            ts[i],
                            Value::TupleGet { base, index: i },
                        )
                    });
                    self.normalize(Occ { var, ty: ts[i] }, PatRef::P(sub), row);
                }
            }
            PatRef::P(Pat::List { elems, rest }) => {
                self.normalize(occ, PatRef::List(elems, rest.as_deref()), row);
            }
            PatRef::List([], Some(r)) => self.normalize(occ, PatRef::P(r), row),
            _ => row.tests.push((occ, p)),
        }
    }

    /// A value read out of another, once per path. `None` without representation.
    fn read(&mut self, key: Key, ty: TypeId, value: Value) -> Option<VarId> {
        if self.repr(ty) == Repr::Void {
            return None;
        }
        if let Some(&(_, v)) = self.cache.iter().find(|(k, _)| *k == key) {
            return Some(v);
        }
        let hint = match key {
            Key::Head(_) => "head",
            Key::Tail(_) => "tail",
            Key::Tuple(..) | Key::Field(..) => "x",
        };
        let v = self.assign(hint, ty, value);
        self.cache.push((key, v));
        Some(v)
    }

    /// Takes the test on `v` out of the row.
    fn take_test<'e>(row: &mut Row<'e>, v: VarId) -> Option<PatRef<'e>> {
        let i = row.tests.iter().position(|(o, _)| o.var == Some(v))?;
        Some(row.tests.remove(i).1)
    }

    /// The row with its test of constructor `ctor` on `v` replaced by tests on the fields.
    fn specialize_ctor<'e>(
        &mut self,
        mut row: Row<'e>,
        v: VarId,
        ctor: CtorRef,
        args: &[TypeId],
    ) -> Row<'e> {
        let Some(PatRef::P(Pat::Ctor { fields, .. })) = Self::take_test(&mut row, v) else {
            return row;
        };
        let mut sub = Vec::new();
        for (i, p) in fields.iter().enumerate() {
            if let Pat::Wild = p {
                continue;
            }
            let field = self.sh.core.variant(ctor).tys[i];
            let ty = self.subst(field, args);
            let var = self.read(
                Key::Field(v, ctor.variant, i),
                ty,
                Value::Field {
                    base: v,
                    ctor,
                    index: i,
                },
            );
            sub.push((Occ { var, ty }, PatRef::P(p)));
        }
        sub.append(&mut row.tests);
        row.tests = sub;
        row
    }

    /// The cases of the rows of a block, which all test `v` (of type `ty`).
    fn switch_on<'e>(
        &mut self,
        v: VarId,
        ty: TypeId,
        rows: Vec<Row<'e>>,
        fail: Option<LabelId>,
        target: &mut Target,
        arms: &'e [Arm],
    ) {
        match self.ty(ty).clone() {
            Type::Enum(id, args) => {
                let count = self.sh.core.enums[id].variants.len();
                let ctor = |k| CtorRef {
                    enum_id: id,
                    variant: k,
                };
                if count == 1 {
                    let rows = rows
                        .into_iter()
                        .map(|r| self.specialize_ctor(r, v, ctor(0), &args))
                        .collect();
                    self.compile(rows, fail, target, arms);
                    return;
                }
                let variant_of = |r: &Row| match r.test_on(v) {
                    Some(PatRef::P(Pat::Ctor { ctor, .. })) => Some(ctor.variant),
                    _ => None,
                };
                let mut present: Vec<usize> = rows.iter().filter_map(variant_of).collect();
                present.sort_unstable();
                present.dedup();
                let mut cases = Vec::with_capacity(present.len());
                for &k in &present {
                    self.open();
                    let sub: Vec<Row> = rows
                        .iter()
                        .filter(|r| variant_of(r) == Some(k))
                        .cloned()
                        .map(|r| self.specialize_ctor(r, v, ctor(k), &args))
                        .collect();
                    self.compile(sub, fail, target, arms);
                    cases.push(Case {
                        key: CaseKey::Variant(k),
                        body: self.close(),
                    });
                }
                let default = (present.len() < count).then(|| {
                    self.open();
                    self.fail_exit(fail);
                    self.close()
                });
                self.push(Stmt::Switch {
                    dst: None,
                    on: SwitchOn::Tag(v),
                    cases,
                    default,
                });
                self.term(Term::Unreachable);
            }
            Type::Bool => {
                let mut branches = Vec::with_capacity(2);
                for b in [true, false] {
                    self.open();
                    let sub: Vec<Row> = rows
                        .iter()
                        .filter(|r| {
                            matches!(r.test_on(v), Some(PatRef::P(Pat::Lit(Lit::Bool(x)))) if *x == b)
                        })
                        .cloned()
                        .map(|mut r| {
                            Self::take_test(&mut r, v);
                            r
                        })
                        .collect();
                    self.compile(sub, fail, target, arms);
                    branches.push(self.close());
                }
                self.push(Stmt::If {
                    dst: None,
                    cond: Operand::Var(v),
                    then: branches[0],
                    else_: branches[1],
                });
                self.term(Term::Unreachable);
            }
            Type::Int | Type::Int64 | Type::String | Type::Float => {
                let lit_of = |r: &Row| match r.test_on(v) {
                    Some(PatRef::P(Pat::Lit(lit))) => lit_const(lit),
                    _ => None,
                };
                let mut lits: Vec<Const> = Vec::new();
                for r in &rows {
                    if let Some(c) = lit_of(r)
                        && !lits.contains(&c)
                    {
                        lits.push(c);
                    }
                }
                let switch = matches!(self.ty(ty), Type::Int | Type::Int64);
                let mut cases = Vec::new();
                for lit in lits {
                    let sub: Vec<Row> = rows
                        .iter()
                        .filter(|r| lit_of(r).as_ref() == Some(&lit))
                        .cloned()
                        .map(|mut r| {
                            Self::take_test(&mut r, v);
                            r
                        })
                        .collect();
                    if switch {
                        self.open();
                        self.compile(sub, fail, target, arms);
                        cases.push(Case {
                            key: CaseKey::Lit(lit),
                            body: self.close(),
                        });
                        continue;
                    }
                    let pty = if ty == TypeId::STRING {
                        PrimTy::String
                    } else {
                        PrimTy::F64
                    };
                    let c = self.assign(
                        TMP,
                        TypeId::BOOL,
                        Value::Prim {
                            op: PrimOp::Eq,
                            ty: pty,
                            args: vec![Operand::Var(v), Operand::Const(lit)],
                        },
                    );
                    self.open();
                    self.compile(sub, fail, target, arms);
                    let then = self.close();
                    let else_ = self.empty();
                    self.push(Stmt::If {
                        dst: None,
                        cond: Operand::Var(c),
                        then,
                        else_,
                    });
                }
                if switch {
                    self.open();
                    self.fail_exit(fail);
                    let default = self.close();
                    self.push(Stmt::Switch {
                        dst: None,
                        on: SwitchOn::Lit(v),
                        cases,
                        default: Some(default),
                    });
                    self.term(Term::Unreachable);
                } else {
                    self.fail_exit(fail);
                }
            }
            Type::List(elem) => {
                let empty = self.assign(TMP, TypeId::BOOL, Value::ListIsEmpty(v));
                self.open();
                let sub: Vec<Row> = rows
                    .iter()
                    .filter(|r| matches!(r.test_on(v), Some(PatRef::List([], None))))
                    .cloned()
                    .map(|mut r| {
                        Self::take_test(&mut r, v);
                        r
                    })
                    .collect();
                self.compile(sub, fail, target, arms);
                let then = self.close();
                self.open();
                let mut sub = Vec::new();
                for r in &rows {
                    let Some(PatRef::List([first, more @ ..], rest)) = r.test_on(v) else {
                        continue;
                    };
                    let head = self.read(Key::Head(v), elem, Value::ListHead(v));
                    let tail = self.read(Key::Tail(v), ty, Value::ListTail(v));
                    let mut r = r.clone();
                    Self::take_test(&mut r, v);
                    let mut tests = vec![
                        (
                            Occ {
                                var: head,
                                ty: elem,
                            },
                            PatRef::P(first),
                        ),
                        (Occ { var: tail, ty }, PatRef::List(more, rest)),
                    ];
                    tests.append(&mut r.tests);
                    r.tests = tests;
                    sub.push(r);
                }
                self.compile(sub, fail, target, arms);
                let else_ = self.close();
                self.push(Stmt::If {
                    dst: None,
                    cond: Operand::Var(empty),
                    then,
                    else_,
                });
                self.term(Term::Unreachable);
            }
            _ => {
                self.error(format!(
                    "a pattern test on a value of type {}",
                    crate::core_ty(&self.sh.core, ty)
                ));
                self.fail_exit(fail);
            }
        }
    }
}
