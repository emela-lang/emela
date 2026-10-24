//! The Flow IR verifier.
//!
//! Checks what the data types do not enforce by themselves and reports every violation, not just
//! the first:
//!
//! - every variable is defined once, before its uses, in a sequence enclosing them; a closure body
//!   uses outer variables only through its capture list
//! - representation types agree: variables with their Core types, operands with the operations,
//!   calls with the callees' parameters and results, `yield` / `break` / `return` with their
//!   targets, loop variables with their initial values and `recur`
//! - `break` and failure edges target an enclosing block of the same body, `recur` an enclosing
//!   loop (not across `protect`), and nothing leaves a cleanup but its end
//! - function and closure bodies end with a terminator other than `yield` on every path
//! - failure edges are present exactly on fallible calls, and only fallible bodies fail
//! - the constructs agree with the module's [`Stage`]: no nested closures or function references
//!   after closure conversion, and closure records only refer to lifted functions whose
//!   environment matches the captures; no `protect`, `defect` or failure edges without landing
//!   once defects are explicit, defect landings exactly on the values that may raise a defect,
//!   `unwind` only in functions that may raise one, and landings that do not yield; `dup` and
//!   `drop` only once reference counted, and then every owned reference consumed exactly once on
//!   every path (`heap::rc_check`)
//!
//! ANF is enforced by the data types (arguments are [`Operand`]s), and so is the position of
//! terminators (each sequence has one, at its end).

use emela_core::{FiberOp, Sig, Type, TypeId};
use rustc_hash::FxHashSet;

use crate::ir::{
    Callee, CaseKey, Function, LabelId, Module, OnFail, Operand, RuntimeOp, SeqId, Stage, Stmt,
    SwitchOn, Term, Value, VarId,
};
use crate::repr::{Repr, enum_repr, repr_of, repr_of_subst};

/// A violation found by [`verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// The function's name.
    pub function: String,
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fn {}: {}", self.function, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A variable used where it is not defined: before its definition, or outside the sequence
    /// that defines it.
    UseOutOfScope,
    /// A variable defined twice.
    Redefined,
    /// A closure body uses a variable of an enclosing body that is not in its capture list.
    Uncaptured,
    /// A variable whose representation is `void`.
    VoidVariable,
    /// A variable whose representation is not that of its Core type.
    VarRepr,
    /// A variable whose Core type has a type parameter (LIR comes after monomorphisation).
    Polymorphic,
    /// An operand, result or destination of the wrong representation.
    ReprMismatch,
    /// The wrong number of arguments, loop values or handler operations.
    Arity,
    /// A destination for a value without representation.
    VoidDestination,
    /// A closure without destination (its Core type is the destination's).
    MissingDestination,
    /// A value of the wrong Core type for the operation (a field of a non-enum, a call of a
    /// non-function).
    WrongType,
    /// A constructor, field, element or operation index out of range.
    BadIndex,
    /// A primitive operation not defined for its operand type.
    BadPrim,
    /// A switch case of the wrong kind, or a switch on a value that cannot be switched on.
    BadCase,
    DuplicateCase,
    /// A switch without default that does not cover every constructor, or a literal switch
    /// without default.
    NonExhaustiveSwitch,
    /// A `break` or failure edge to a label that is not an enclosing block of the same body.
    UnknownLabel,
    /// A label used by two blocks.
    LabelRedefined,
    /// A `recur` outside a loop of the same body, or across `protect`.
    RecurOutsideLoop,
    /// A function or closure body that can reach its end (`yield`).
    MissingTerminator,
    /// `fail`, or a failure edge to the body itself, in a body that cannot fail.
    FailInInfallible,
    /// A fallible call without failure edge, or an infallible one with one.
    FailEdge,
    /// `return`, `fail`, `break` or a failure edge leaving a cleanup.
    EscapeFromCleanup,
    /// A call whose `suspends` disagrees with the callee.
    SuspendMismatch,
    /// A function without body, or an external function with one.
    MissingBody,
    /// A sequence or statement that appears in two places.
    SharedNode,
    /// A construct not allowed at the module's stage (a nested closure after closure conversion,
    /// a closure record before it).
    StageViolation,
    /// A lifted function's environment that disagrees with its use: an `env_get` outside a lifted
    /// function or not on its environment parameter, or a closure record of a function that is
    /// not lifted or whose captures differ.
    BadEnvironment,
    /// A value that may raise a defect without a defect landing, a defect landing on one that
    /// cannot, or `unwind` in a function that does not raise defects (Heap IR).
    DefectEdge,
    /// A suspension edge where it does not belong (Heap IR from `Stage::StateMachines`): a value
    /// of a suspending function that may suspend without one, one on a value that cannot suspend
    /// or in a function that does not suspend, two edges with the same state, a saved set that
    /// misses a variable needed after the value, or a fairness yield outside a suspending
    /// function.
    SuspendEdge,
    /// References that do not balance (Heap IR at `Stage::RefCounted`): a reference consumed or
    /// dropped that is not owned, a borrowed use without a reference, references left at the end
    /// of a scope, or join points reached with different references.
    RcImbalance,
}

/// Checks every function of the module.
pub fn verify(m: &Module) -> Result<(), Vec<VerifyError>> {
    let errors: Vec<VerifyError> = m
        .functions
        .iter()
        .flat_map(|(_, f)| verify_function(m, f))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Checks one function. `f` need not be in the module yet; direct calls refer to the module's
/// functions.
pub fn verify_function(m: &Module, f: &Function) -> Vec<VerifyError> {
    let stage = m.stage;
    let mut c = Checker {
        m,
        core: &m.core,
        f,
        stage,
        errors: Vec::new(),
        defined: vec![false; f.vars.len()],
        in_scope: vec![false; f.vars.len()],
        frame_of: vec![0; f.vars.len()],
        scope: Vec::new(),
        seq_used: vec![false; f.seqs.len()],
        stmt_used: vec![false; f.stmts.len()],
        label_used: vec![false; f.labels.len()],
        frames: Vec::new(),
        enclosing: Vec::new(),
        states: FxHashSet::default(),
    };
    c.function();
    let mut errors = c.errors;
    if stage >= Stage::StateMachines && f.suspends && errors.is_empty() {
        errors.extend(saved_errors(m, f));
    }
    if stage == Stage::RefCounted && errors.is_empty() {
        errors.extend(
            crate::heap::rc_check::check(m, f)
                .into_iter()
                .map(|message| VerifyError {
                    function: f.name.clone(),
                    kind: ErrorKind::RcImbalance,
                    message,
                }),
        );
    }
    errors
}

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

/// A function or closure body being checked.
struct Frame {
    /// The outer variables the body may use (`None` for the function).
    captures: Option<FxHashSet<VarId>>,
    ret: Repr,
    fails: bool,
}

/// What encloses the statement being checked, innermost last.
enum Encl {
    /// The start of a function or closure body.
    Frame,
    /// A block, with its value's representation.
    Label(LabelId, Repr),
    /// A loop, with its variables' representations.
    Loop(Vec<Repr>),
    Protect,
    Cleanup,
}

/// Where a sequence's `yield` goes.
#[derive(Clone, Copy)]
enum YieldTo {
    /// To the enclosing statement, whose value has the representation (`void` without
    /// destination).
    Stmt(Repr),
    /// Nowhere: the sequence is a function or closure body.
    Body,
    /// Nowhere: the sequence is a failure or defect landing.
    Landing,
}

/// What a value's representation is expected to be.
enum Expect {
    Exactly(Repr),
    /// Not known from LIR alone (an operation call, a builtin with a type parameter).
    Unknown,
}

struct Checker<'a> {
    m: &'a Module,
    core: &'a emela_core::Module,
    f: &'a Function,
    /// The stage the function is checked against.
    stage: Stage,
    errors: Vec<VerifyError>,
    defined: Vec<bool>,
    in_scope: Vec<bool>,
    /// The index in `frames` of the body that defines the variable.
    frame_of: Vec<usize>,
    /// Variables in scope, in definition order (popped when their sequence ends).
    scope: Vec<VarId>,
    seq_used: Vec<bool>,
    stmt_used: Vec<bool>,
    label_used: Vec<bool>,
    frames: Vec<Frame>,
    enclosing: Vec<Encl>,
    /// The states of the suspension edges seen.
    states: FxHashSet<u32>,
}

/// Suspension edges whose saved variables miss one that is needed after the value.
fn saved_errors(m: &Module, f: &Function) -> Vec<VerifyError> {
    let live = crate::heap::live::live_on_lets(f);
    let mut errors = Vec::new();
    for (s, stmt) in f.stmts.iter() {
        let Stmt::Let {
            value,
            on_suspend: Some(edge),
            ..
        } = stmt
        else {
            continue;
        };
        let Some(live_on) = live.get(&s) else {
            continue;
        };
        for v in crate::heap::suspend::saved_vars(m, f, value, live_on) {
            if !edge.saved.contains(&v) {
                errors.push(VerifyError {
                    function: f.name.clone(),
                    kind: ErrorKind::SuspendEdge,
                    message: format!(
                        "the suspension edge of state {} does not save {}%{}, which is needed after it",
                        edge.state,
                        f.vars[v].hint,
                        ix(v)
                    ),
                });
            }
        }
    }
    errors
}

impl Checker<'_> {
    fn error(&mut self, kind: ErrorKind, message: String) {
        self.errors.push(VerifyError {
            function: self.f.name.clone(),
            kind,
            message,
        });
    }

    fn name(&self, v: VarId) -> String {
        format!("{}%{}", self.f.vars[v].hint, ix(v))
    }

    fn label_name(&self, l: LabelId) -> String {
        format!("'{}%{}", self.f.labels[l].hint, ix(l))
    }

    fn function(&mut self) {
        let f = self.f;
        for (v, data) in f.vars.iter() {
            let of_type = repr_of(self.core, data.ty);
            if data.repr == Repr::Void {
                self.error(
                    ErrorKind::VoidVariable,
                    format!("variable {} has no representation", self.name(v)),
                );
            } else if data.repr != of_type {
                self.error(
                    ErrorKind::VarRepr,
                    format!(
                        "variable {} is {} but its Core type is {of_type}",
                        self.name(v),
                        data.repr
                    ),
                );
            }
            if self.core.types.has_param(data.ty) {
                self.error(
                    ErrorKind::Polymorphic,
                    format!("variable {} has a type parameter", self.name(v)),
                );
            }
        }
        if f.env.is_some()
            && f.params
                .first()
                .is_none_or(|&p| f.vars[p].repr != Repr::Ref)
        {
            self.error(
                ErrorKind::BadEnvironment,
                "a lifted function without an environment parameter".into(),
            );
        }
        match (&f.external, f.body) {
            (Some(_), Some(_)) => self.error(
                ErrorKind::MissingBody,
                "an external function has a body".into(),
            ),
            (None, None) => self.error(ErrorKind::MissingBody, "no body".into()),
            _ => {}
        }
        self.frames.push(Frame {
            captures: None,
            ret: repr_of(self.core, f.ret),
            fails: f.fails,
        });
        self.enclosing.push(Encl::Frame);
        for &p in &f.params {
            self.define(p);
        }
        if let Some(body) = f.body {
            self.seq(body, YieldTo::Body);
        }
    }

    // Variables.

    fn define(&mut self, v: VarId) {
        if self.defined[ix(v)] {
            self.error(
                ErrorKind::Redefined,
                format!("variable {} is defined twice", self.name(v)),
            );
            return;
        }
        self.defined[ix(v)] = true;
        self.in_scope[ix(v)] = true;
        self.frame_of[ix(v)] = self.frames.len() - 1;
        self.scope.push(v);
    }

    /// Runs `check` and then takes the variables it defined out of scope.
    fn scoped(&mut self, check: impl FnOnce(&mut Self)) {
        let mark = self.scope.len();
        check(self);
        for v in self.scope.drain(mark..) {
            self.in_scope[ix(v)] = false;
        }
    }

    /// Checks a use and returns the variable's representation.
    fn use_var(&mut self, v: VarId) -> Repr {
        if !self.in_scope[ix(v)] {
            let message = if self.defined[ix(v)] {
                format!("variable {} is used outside its scope", self.name(v))
            } else {
                format!("variable {} is used before its definition", self.name(v))
            };
            self.error(ErrorKind::UseOutOfScope, message);
        } else {
            let current = self.frames.len() - 1;
            let frame = &self.frames[current];
            if self.frame_of[ix(v)] < current
                && !frame.captures.as_ref().is_some_and(|c| c.contains(&v))
            {
                self.error(
                    ErrorKind::Uncaptured,
                    format!(
                        "variable {} is used in a closure without capture",
                        self.name(v)
                    ),
                );
            }
        }
        self.f.vars[v].repr
    }

    fn operand(&mut self, o: &Operand) -> Repr {
        match o {
            Operand::Var(v) => self.use_var(*v),
            Operand::Const(c) => c.repr(),
        }
    }

    /// The representation of an optional value (`void` without value).
    fn opt_operand(&mut self, o: &Option<Operand>) -> Repr {
        o.as_ref().map_or(Repr::Void, |o| self.operand(o))
    }

    fn dst_repr(&self, dst: Option<VarId>) -> Repr {
        dst.map_or(Repr::Void, |d| self.f.vars[d].repr)
    }

    fn expect(&mut self, found: Repr, expected: Repr, what: &str) {
        if found != expected {
            self.error(
                ErrorKind::ReprMismatch,
                format!("{what}: expected {expected}, found {found}"),
            );
        }
    }

    /// Checks operands against expected representations (`None`: not checked).
    fn args(&mut self, args: &[Operand], expected: &[Option<Repr>], what: &str) {
        let found: Vec<Repr> = args.iter().map(|a| self.operand(a)).collect();
        if found.len() != expected.len() {
            self.error(
                ErrorKind::Arity,
                format!(
                    "{what}: expected {} arguments, found {}",
                    expected.len(),
                    found.len()
                ),
            );
            return;
        }
        for (i, (f, e)) in found.iter().zip(expected).enumerate() {
            if let Some(e) = e {
                self.expect(*f, *e, &format!("{what}: argument {i}"));
            }
        }
    }

    // Sequences and statements.

    fn seq(&mut self, id: SeqId, to: YieldTo) {
        if std::mem::replace(&mut self.seq_used[ix(id)], true) {
            self.error(
                ErrorKind::SharedNode,
                format!("sequence {} appears twice", ix(id)),
            );
            return;
        }
        let f = self.f;
        let seq = &f.seqs[id];
        self.scoped(|c| {
            for &s in &seq.stmts {
                if std::mem::replace(&mut c.stmt_used[ix(s)], true) {
                    c.error(
                        ErrorKind::SharedNode,
                        format!("statement {} appears twice", ix(s)),
                    );
                    continue;
                }
                let stmt = &f.stmts[s];
                c.stmt(stmt);
                if let Some(d) = stmt.dst() {
                    c.define(d);
                }
            }
            c.term(&seq.term, to);
        });
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                let expect = self.value(value, *dst);
                self.on_defect(value, *on_defect);
                self.on_suspend(value, on_suspend.as_ref());
                match (dst, expect) {
                    (Some(d), Expect::Exactly(Repr::Void)) => self.error(
                        ErrorKind::VoidDestination,
                        format!(
                            "{} is defined by a value without representation",
                            self.name(*d)
                        ),
                    ),
                    (Some(d), Expect::Exactly(r)) if self.f.vars[*d].repr != r => self.error(
                        ErrorKind::ReprMismatch,
                        format!(
                            "destination {} is {}, but the value is {r}",
                            self.name(*d),
                            self.f.vars[*d].repr
                        ),
                    ),
                    _ => {}
                }
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let r = self.operand(cond);
                self.expect(r, Repr::I32, "if condition");
                let to = YieldTo::Stmt(self.dst_repr(*dst));
                self.seq(*then, to);
                self.seq(*else_, to);
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                self.switch(*on, cases, default.is_some());
                let to = YieldTo::Stmt(self.dst_repr(*dst));
                for case in cases {
                    self.seq(case.body, to);
                }
                if let Some(d) = default {
                    self.seq(*d, to);
                }
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                let expected: Vec<Option<Repr>> =
                    vars.iter().map(|&v| Some(self.f.vars[v].repr)).collect();
                self.args(inits, &expected, "loop initial values");
                let to = YieldTo::Stmt(self.dst_repr(*dst));
                self.scoped(|c| {
                    for &v in vars {
                        c.define(v);
                    }
                    let reprs = vars.iter().map(|&v| c.f.vars[v].repr).collect();
                    c.enclosing.push(Encl::Loop(reprs));
                    c.seq(*body, to);
                    c.enclosing.pop();
                });
            }
            Stmt::Block { dst, label, body } => {
                if std::mem::replace(&mut self.label_used[ix(*label)], true) {
                    self.error(
                        ErrorKind::LabelRedefined,
                        format!("label {} is used by two blocks", self.label_name(*label)),
                    );
                }
                let r = self.dst_repr(*dst);
                self.enclosing.push(Encl::Label(*label, r));
                self.seq(*body, YieldTo::Stmt(r));
                self.enclosing.pop();
            }
            Stmt::Dup(v) | Stmt::Drop(v) => {
                self.stage_at_least(Stage::RefCounted, "dup and drop");
                let r = self.use_var(*v);
                self.expect(r, Repr::Ref, "dup or drop");
            }
            Stmt::Protect { dst, body, cleanup } => {
                self.stage_at_most(Stage::ClosuresConverted, "protect");
                self.enclosing.push(Encl::Protect);
                self.seq(*body, YieldTo::Stmt(self.dst_repr(*dst)));
                self.enclosing.pop();
                self.enclosing.push(Encl::Cleanup);
                self.seq(*cleanup, YieldTo::Stmt(Repr::Void));
                self.enclosing.pop();
            }
        }
    }

    fn switch(&mut self, on: SwitchOn, cases: &[crate::ir::Case], has_default: bool) {
        match on {
            SwitchOn::Tag(v) => {
                self.use_var(v);
                let Type::Enum(id, _) = self.core.ty(self.f.vars[v].ty) else {
                    self.error(
                        ErrorKind::WrongType,
                        format!(
                            "switch on the tag of {}, which is not an enum",
                            self.name(v)
                        ),
                    );
                    return;
                };
                let count = self.core.enums[*id].variants.len();
                let mut seen = vec![false; count];
                for case in cases {
                    match case.key {
                        CaseKey::Variant(i) if i < count => {
                            if std::mem::replace(&mut seen[i], true) {
                                self.error(
                                    ErrorKind::DuplicateCase,
                                    format!("constructor {i} has two cases"),
                                );
                            }
                        }
                        CaseKey::Variant(i) => self.error(
                            ErrorKind::BadCase,
                            format!("constructor {i} out of range ({count} constructors)"),
                        ),
                        CaseKey::Lit(_) => {
                            self.error(ErrorKind::BadCase, "a literal case in a tag switch".into())
                        }
                    }
                }
                if !has_default && seen.iter().any(|s| !s) {
                    self.error(
                        ErrorKind::NonExhaustiveSwitch,
                        "a tag switch without default misses constructors".into(),
                    );
                }
            }
            SwitchOn::Lit(v) => {
                let r = self.use_var(v);
                if !matches!(r, Repr::I32 | Repr::I64) {
                    self.error(
                        ErrorKind::BadCase,
                        format!("switch on {} of representation {r}", self.name(v)),
                    );
                }
                let mut seen = Vec::new();
                for case in cases {
                    match &case.key {
                        CaseKey::Lit(c) if c.repr() == r => {
                            if seen.contains(&c) {
                                self.error(
                                    ErrorKind::DuplicateCase,
                                    format!("literal {c:?} has two cases"),
                                );
                            }
                            seen.push(c);
                        }
                        CaseKey::Lit(c) => self.error(
                            ErrorKind::BadCase,
                            format!("a {} case in a switch on {r}", c.repr()),
                        ),
                        CaseKey::Variant(_) => self.error(
                            ErrorKind::BadCase,
                            "a constructor case in a literal switch".into(),
                        ),
                    }
                }
                if !has_default {
                    self.error(
                        ErrorKind::NonExhaustiveSwitch,
                        "a literal switch without default".into(),
                    );
                }
            }
        }
    }

    // Terminators.

    fn term(&mut self, t: &Term, to: YieldTo) {
        match t {
            Term::Yield(v) => {
                let r = self.opt_operand(v);
                match to {
                    YieldTo::Body => self.error(
                        ErrorKind::MissingTerminator,
                        "a function or closure body reaches its end".into(),
                    ),
                    YieldTo::Landing => self.error(
                        ErrorKind::MissingTerminator,
                        "a landing reaches its end".into(),
                    ),
                    YieldTo::Stmt(expected) => self.expect(r, expected, "yield"),
                }
            }
            Term::Return(v) => {
                let r = self.opt_operand(v);
                if self.leaves_body("return") {
                    let expected = self.frames.last().expect("no frame").ret;
                    self.expect(r, expected, "return");
                }
            }
            Term::Fail(v) => {
                let r = self.operand(v);
                self.expect(r, Repr::Ref, "fail");
                if self.leaves_body("fail") && !self.frames.last().expect("no frame").fails {
                    self.error(
                        ErrorKind::FailInInfallible,
                        "fail in a body that cannot fail".into(),
                    );
                }
            }
            Term::Break { label, value } => {
                let r = self.opt_operand(value);
                if let Some(expected) = self.label_target(*label, "break") {
                    self.expect(r, expected, &format!("break {}", self.label_name(*label)));
                }
            }
            Term::Recur(args) => {
                let target = self.enclosing.iter().rev().find_map(|e| match e {
                    Encl::Label(..) => None,
                    Encl::Loop(reprs) => Some(Some(reprs.clone())),
                    Encl::Frame | Encl::Protect | Encl::Cleanup => Some(None),
                });
                match target.flatten() {
                    Some(reprs) => {
                        let expected: Vec<Option<Repr>> = reprs.into_iter().map(Some).collect();
                        self.args(args, &expected, "recur");
                    }
                    None => {
                        for a in args {
                            self.operand(a);
                        }
                        self.error(
                            ErrorKind::RecurOutsideLoop,
                            "recur outside a loop of the same body, or across protect".into(),
                        );
                    }
                }
            }
            Term::Defect(msg, _) => {
                self.stage_at_most(Stage::ClosuresConverted, "defect");
                let r = self.operand(msg);
                self.expect(r, Repr::Ref, "defect message");
            }
            Term::Unwind => {
                self.stage_at_least(Stage::DefectsExplicit, "unwind");
                if !self.f.defects {
                    self.error(
                        ErrorKind::DefectEdge,
                        "unwind in a function that does not raise defects".into(),
                    );
                }
            }
            Term::Unreachable => {}
        }
    }

    /// Whether leaving the body from here is allowed (not from a cleanup).
    fn leaves_body(&mut self, what: &str) -> bool {
        for e in self.enclosing.iter().rev() {
            match e {
                Encl::Frame => return true,
                Encl::Cleanup => {
                    self.error(
                        ErrorKind::EscapeFromCleanup,
                        format!("{what} leaves a cleanup"),
                    );
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    /// The representation of the value of the enclosing block with the label, if there is one.
    fn label_target(&mut self, label: LabelId, what: &str) -> Option<Repr> {
        for e in self.enclosing.iter().rev() {
            match e {
                Encl::Label(l, r) if *l == label => return Some(*r),
                Encl::Frame => break,
                Encl::Cleanup => {
                    self.error(
                        ErrorKind::EscapeFromCleanup,
                        format!("{what} {} leaves a cleanup", self.label_name(label)),
                    );
                    return None;
                }
                _ => {}
            }
        }
        self.error(
            ErrorKind::UnknownLabel,
            format!(
                "{what} to {}, which is not an enclosing block of the same body",
                self.label_name(label)
            ),
        );
        None
    }

    /// Checks a failure edge against whether the callee may fail.
    fn on_fail(&mut self, on_fail: &Option<OnFail>, fails: bool, callee: &str) {
        match (on_fail, fails) {
            (None, true) => self.error(
                ErrorKind::FailEdge,
                format!("{callee} may fail but has no failure edge"),
            ),
            (Some(_), false) => self.error(
                ErrorKind::FailEdge,
                format!("{callee} cannot fail but has a failure edge"),
            ),
            _ => {}
        }
        if matches!(on_fail, Some(OnFail::Break(_) | OnFail::Propagate)) {
            self.stage_at_most(Stage::ClosuresConverted, "a failure edge without landing");
        }
        match on_fail {
            Some(OnFail::Handle { error, body }) => {
                self.stage_at_least(Stage::DefectsExplicit, "a failure landing");
                if self.f.vars[*error].repr != Repr::Ref {
                    self.error(
                        ErrorKind::ReprMismatch,
                        format!("failure landing: {} is not ref", self.name(*error)),
                    );
                }
                self.scoped(|c| {
                    c.define(*error);
                    c.seq(*body, YieldTo::Landing);
                });
            }
            Some(OnFail::Break(l)) => {
                if let Some(r) = self.label_target(*l, "failure edge") {
                    let what = format!("failure edge to {}", self.label_name(*l));
                    self.expect(r, Repr::Ref, &what);
                }
            }
            Some(OnFail::Propagate) => {
                let allowed = self.leaves_body("failure edge");
                if allowed && !self.frames.last().expect("no frame").fails {
                    self.error(
                        ErrorKind::FailInInfallible,
                        format!("{callee} fails a body that cannot fail"),
                    );
                }
            }
            None => {}
        }
    }

    // Values.

    fn wrong_type(&mut self, message: String) -> Expect {
        self.error(ErrorKind::WrongType, message);
        Expect::Unknown
    }

    fn value(&mut self, v: &Value, dst: Option<VarId>) -> Expect {
        let dst_ty = dst.map(|d| self.f.vars[d].ty);
        match v {
            Value::Const(c) => Expect::Exactly(c.repr()),
            Value::Prim { op, ty, args } => {
                let operand = ty.repr();
                let expected = vec![Some(operand); op.arity()];
                self.args(args, &expected, &format!("{}[{}]", op.symbol(), ty.name()));
                let defined = match op {
                    crate::PrimOp::Not => *ty == crate::PrimTy::I32,
                    _ if op.is_comparison() => true,
                    _ => *ty != crate::PrimTy::String,
                };
                if !defined {
                    self.error(
                        ErrorKind::BadPrim,
                        format!("{} is not defined on {}", op.symbol(), ty.name()),
                    );
                }
                Expect::Exactly(if op.is_comparison() {
                    Repr::I32
                } else {
                    operand
                })
            }
            Value::Call {
                callee,
                args,
                suspends,
                on_fail,
            } => self.call(callee, args, *suspends, on_fail),
            Value::Builtin { op, args } => {
                let info = op.info();
                let params: Vec<Option<Repr>> = info.params.iter().map(|&s| sig_repr(s)).collect();
                let generic = params.iter().filter(|p| p.is_none()).count();
                let concrete: Vec<Option<Repr>> = params
                    .iter()
                    .copied()
                    .filter(|p| *p != Some(Repr::Void))
                    .collect();
                // A parameter of a type parameter has no argument if the type argument has no
                // representation; only the full form is checked argument by argument.
                let what = format!("builtin {}", info.path());
                if args.len() < concrete.len() && args.len() + generic >= concrete.len() {
                    for a in args {
                        self.operand(a);
                    }
                } else {
                    self.args(args, &concrete, &what);
                }
                sig_repr(info.ret).map_or(Expect::Unknown, Expect::Exactly)
            }
            Value::Make { ctor, args } => {
                let def = &self.core.enums[ctor.enum_id];
                let Some(variant) = def.variants.get(ctor.variant) else {
                    for a in args {
                        self.operand(a);
                    }
                    self.error(
                        ErrorKind::BadIndex,
                        format!("constructor {} of {} out of range", ctor.variant, def.name),
                    );
                    return Expect::Unknown;
                };
                let ty_args = match dst_ty.map(|t| self.core.ty(t)) {
                    Some(Type::Enum(id, ts)) if *id == ctor.enum_id => Some(ts.as_slice()),
                    Some(_) => {
                        for a in args {
                            self.operand(a);
                        }
                        return self.wrong_type(format!(
                            "a value of {} defines a variable of another type",
                            def.name
                        ));
                    }
                    None => None,
                };
                let generic = variant
                    .tys
                    .iter()
                    .any(|&t| matches!(self.core.ty(t), Type::Param(_)));
                match ty_args {
                    Some(ts) => {
                        let expected: Vec<Option<Repr>> = variant
                            .tys
                            .iter()
                            .map(|&t| Some(repr_of_subst(self.core, t, ts)))
                            .collect();
                        self.args(args, &expected, &format!("make {}", variant.name));
                    }
                    None if generic => {
                        for a in args {
                            self.operand(a);
                        }
                    }
                    None => {
                        let expected: Vec<Option<Repr>> = variant
                            .tys
                            .iter()
                            .map(|&t| Some(repr_of(self.core, t)))
                            .collect();
                        self.args(args, &expected, &format!("make {}", variant.name));
                    }
                }
                Expect::Exactly(enum_repr(self.core, ctor.enum_id))
            }
            Value::Field { base, ctor, index } => {
                self.use_var(*base);
                let base_ty = self.f.vars[*base].ty;
                let Type::Enum(id, ts) = self.core.ty(base_ty) else {
                    return self.wrong_type(format!("field of {}, not an enum", self.name(*base)));
                };
                if *id != ctor.enum_id {
                    return self.wrong_type(format!(
                        "field of {} through a constructor of another enum",
                        self.name(*base)
                    ));
                }
                let ty = self.core.enums[*id]
                    .variants
                    .get(ctor.variant)
                    .and_then(|v| v.tys.get(*index));
                match ty {
                    Some(&t) => Expect::Exactly(repr_of_subst(self.core, t, ts)),
                    None => {
                        self.error(
                            ErrorKind::BadIndex,
                            format!("field {index} of constructor {} out of range", ctor.variant),
                        );
                        Expect::Unknown
                    }
                }
            }
            Value::Tag(v) => {
                self.use_var(*v);
                if !matches!(self.core.ty(self.f.vars[*v].ty), Type::Enum(..)) {
                    return self.wrong_type(format!("tag of {}, not an enum", self.name(*v)));
                }
                Expect::Exactly(Repr::I32)
            }
            Value::Tuple(es) => {
                match dst_ty.map(|t| self.core.ty(t)) {
                    Some(Type::Tuple(ts)) => {
                        let expected: Vec<Option<Repr>> =
                            ts.iter().map(|&t| Some(repr_of(self.core, t))).collect();
                        self.args(es, &expected, "tuple");
                    }
                    _ => {
                        for e in es {
                            self.operand(e);
                        }
                    }
                }
                Expect::Exactly(Repr::Ref)
            }
            Value::TupleGet { base, index } => {
                self.use_var(*base);
                let Type::Tuple(ts) = self.core.ty(self.f.vars[*base].ty) else {
                    return self
                        .wrong_type(format!("element of {}, not a tuple", self.name(*base)));
                };
                match ts.get(*index) {
                    Some(&t) => Expect::Exactly(repr_of(self.core, t)),
                    None => {
                        self.error(
                            ErrorKind::BadIndex,
                            format!("element {index} of a tuple of {}", ts.len()),
                        );
                        Expect::Unknown
                    }
                }
            }
            Value::List { elems, tail } => {
                let elem = match dst_ty.map(|t| self.core.ty(t)) {
                    Some(Type::List(e)) => Some(repr_of(self.core, *e)),
                    _ => None,
                };
                for e in elems {
                    let r = self.operand(e);
                    if let Some(expected) = elem {
                        self.expect(r, expected, "list element");
                    }
                }
                if let Some(t) = tail {
                    let r = self.operand(t);
                    self.expect(r, Repr::Ref, "list tail");
                }
                Expect::Exactly(Repr::Ref)
            }
            Value::ListIsEmpty(l) | Value::ListHead(l) | Value::ListTail(l) => {
                self.use_var(*l);
                let Type::List(e) = self.core.ty(self.f.vars[*l].ty) else {
                    return self.wrong_type(format!("list operation on {}", self.name(*l)));
                };
                Expect::Exactly(match v {
                    Value::ListIsEmpty(_) => Repr::I32,
                    Value::ListHead(_) => repr_of(self.core, *e),
                    _ => Repr::Ref,
                })
            }
            Value::FnRef(_) => {
                self.stage_at_most(Stage::Flow, "a function reference");
                Expect::Exactly(Repr::Ref)
            }
            Value::MakeClosure { function, captures } => {
                self.stage_at_least(Stage::ClosuresConverted, "a closure record");
                self.make_closure(*function, captures, dst)
            }
            Value::EnvGet { env, index } => {
                self.stage_at_least(Stage::ClosuresConverted, "env_get");
                self.use_var(*env);
                let f = self.f;
                let Some(tys) = f.env.as_ref().filter(|_| f.params.first() == Some(env)) else {
                    self.error(
                        ErrorKind::BadEnvironment,
                        format!(
                            "env_get on {}, not the environment of a lifted function",
                            self.name(*env)
                        ),
                    );
                    return Expect::Unknown;
                };
                let Some(&ty) = tys.get(*index) else {
                    self.error(
                        ErrorKind::BadIndex,
                        format!("capture {index} of an environment of {}", tys.len()),
                    );
                    return Expect::Unknown;
                };
                if dst_ty.is_some_and(|d| d != ty) {
                    return self.wrong_type(format!(
                        "capture {index} defines a variable of another type"
                    ));
                }
                Expect::Exactly(repr_of(self.core, ty))
            }
            Value::Closure(c) => {
                self.stage_at_most(Stage::Flow, "a nested closure");
                for &cap in &c.captures {
                    self.use_var(cap);
                }
                let Some(dst) = dst else {
                    self.error(
                        ErrorKind::MissingDestination,
                        "a closure without destination".into(),
                    );
                    return Expect::Unknown;
                };
                let Type::Fn(ft) = self.core.ty(self.f.vars[dst].ty) else {
                    return self.wrong_type(format!(
                        "closure defines {}, not of a function type",
                        self.name(dst)
                    ));
                };
                let expected = self.fn_params(&ft.params, ft.ev);
                let found: Vec<Option<Repr>> = c
                    .params
                    .iter()
                    .map(|&p| Some(self.f.vars[p].repr))
                    .collect();
                if found.len() != expected.len() {
                    self.error(
                        ErrorKind::Arity,
                        format!(
                            "closure: expected {} parameters, found {}",
                            expected.len(),
                            found.len()
                        ),
                    );
                } else {
                    for (i, (f, e)) in found.iter().zip(&expected).enumerate() {
                        self.expect(
                            f.unwrap_or(Repr::Void),
                            e.unwrap_or(Repr::Void),
                            &format!("closure parameter {i}"),
                        );
                    }
                }
                if c.suspends != ft.suspends {
                    self.error(
                        ErrorKind::SuspendMismatch,
                        "a closure's suspends disagrees with its type".into(),
                    );
                }
                self.frames.push(Frame {
                    captures: Some(c.captures.iter().copied().collect()),
                    ret: repr_of(self.core, ft.ret),
                    fails: ft.fails,
                });
                self.enclosing.push(Encl::Frame);
                self.scoped(|ch| {
                    for &p in &c.params {
                        ch.define(p);
                    }
                    ch.seq(c.body, YieldTo::Body);
                });
                self.enclosing.pop();
                self.frames.pop();
                Expect::Exactly(Repr::Ref)
            }
            Value::EvLookup { ev, .. } => {
                self.use_var(*ev);
                if self.f.vars[*ev].ty != TypeId::EVIDENCE {
                    return self.wrong_type(format!(
                        "evidence lookup in {}, not an evidence table",
                        self.name(*ev)
                    ));
                }
                Expect::Exactly(Repr::Ref)
            }
            Value::EvExtend { base, bindings } => {
                if let Some(b) = base {
                    self.use_var(*b);
                    if self.f.vars[*b].ty != TypeId::EVIDENCE {
                        return self.wrong_type(format!(
                            "evidence extension of {}, not an evidence table",
                            self.name(*b)
                        ));
                    }
                }
                for (_, h) in bindings {
                    let r = self.use_var(*h);
                    self.expect(r, Repr::Ref, "evidence binding");
                }
                Expect::Exactly(Repr::Ref)
            }
            Value::Handler { effect, ops, .. } => {
                for &op in ops {
                    let r = self.use_var(op);
                    self.expect(r, Repr::Ref, "handler operation");
                }
                let count = self.core.effects[*effect].ops.len();
                if ops.len() != count {
                    self.error(
                        ErrorKind::Arity,
                        format!("handler: expected {count} operations, found {}", ops.len()),
                    );
                }
                Expect::Exactly(Repr::Ref)
            }
            Value::Fiber { op, args, on_fail } => {
                for a in args {
                    self.operand(a);
                }
                if args.len() != op.arity() {
                    self.error(
                        ErrorKind::Arity,
                        format!(
                            "fiber {}: expected {} arguments, found {}",
                            op.name(),
                            op.arity(),
                            args.len()
                        ),
                    );
                }
                self.on_fail(on_fail, op.fails(), &format!("fiber {}", op.name()));
                match *op {
                    FiberOp::Fork | FiberOp::Deferred | FiberOp::Poll | FiberOp::CauseToString => {
                        Expect::Exactly(Repr::Ref)
                    }
                    FiberOp::Await { exit } | FiberOp::Attempt { exit, .. } => {
                        Expect::Exactly(enum_repr(self.core, exit))
                    }
                    FiberOp::CauseKind { kind } => Expect::Exactly(enum_repr(self.core, kind)),
                    FiberOp::Complete => Expect::Exactly(Repr::I32),
                    FiberOp::Interrupt | FiberOp::CauseRaise { .. } => Expect::Exactly(Repr::Void),
                    FiberOp::Scope { .. } | FiberOp::Join | FiberOp::DeferredAwait => {
                        Expect::Unknown
                    }
                }
            }
            Value::IsError { error, enum_id } | Value::AsError { error, enum_id } => {
                let r = self.use_var(*error);
                self.expect(r, Repr::Ref, "caught failure");
                let def = &self.core.enums[*enum_id];
                if !def.error {
                    return self.wrong_type(format!("{} is not an error", def.name));
                }
                if let Value::IsError { .. } = v {
                    return Expect::Exactly(Repr::I32);
                }
                match dst_ty.map(|t| self.core.ty(t)) {
                    Some(Type::Enum(id, _)) if id == enum_id => {}
                    None => {}
                    Some(_) => {
                        return self.wrong_type(format!(
                            "as_error {} defines a variable of another type",
                            def.name
                        ));
                    }
                }
                Expect::Exactly(Repr::Ref)
            }
            Value::Coerce(x) => {
                let r = self.use_var(*x);
                let from = self.f.vars[*x].ty;
                match dst {
                    Some(d) if from == TypeId::ANY || self.f.vars[d].ty == TypeId::ANY => {
                        Expect::Unknown
                    }
                    _ => Expect::Exactly(r),
                }
            }
            Value::Runtime { op, args } => {
                let (params, ret) = op.sig();
                let expected: Vec<Option<Repr>> = params.iter().map(|&r| Some(r)).collect();
                self.args(args, &expected, &format!("runtime {}", op.name()));
                Expect::Exactly(ret)
            }
        }
    }

    /// Checks the defect landing of a let against its value.
    fn on_defect(&mut self, value: &Value, on_defect: Option<SeqId>) {
        if self.stage < Stage::DefectsExplicit {
            if on_defect.is_some() {
                self.error(
                    ErrorKind::StageViolation,
                    format!("a defect landing at stage {:?}", self.stage),
                );
            }
        } else {
            match (crate::heap::may_defect(self.m, value), on_defect) {
                (true, None) => self.error(
                    ErrorKind::DefectEdge,
                    "a value that may raise a defect has no defect landing".into(),
                ),
                (false, Some(_)) => self.error(
                    ErrorKind::DefectEdge,
                    "a value that cannot raise a defect has a defect landing".into(),
                ),
                _ => {}
            }
        }
        if let Some(l) = on_defect {
            self.seq(l, YieldTo::Landing);
        }
    }

    /// Checks the suspension edge of a let against its value and the function.
    fn on_suspend(&mut self, value: &Value, edge: Option<&crate::ir::Suspend>) {
        let may = crate::heap::suspend::may_suspend(value);
        if let Value::Runtime {
            op: RuntimeOp::Yield | RuntimeOp::YieldPerform,
            ..
        } = value
            && !self.f.suspends
        {
            self.error(
                ErrorKind::SuspendEdge,
                "a fairness yield in a function that does not suspend".into(),
            );
        }
        if self.stage < Stage::StateMachines {
            if let Value::Runtime {
                op: RuntimeOp::Yield | RuntimeOp::YieldPerform,
                ..
            } = value
            {
                self.stage_at_least(Stage::StateMachines, "a fairness yield");
            }
            if edge.is_some() {
                self.error(
                    ErrorKind::StageViolation,
                    format!("a suspension edge at stage {:?}", self.stage),
                );
            }
            return;
        }
        match (self.f.suspends, may, edge) {
            (true, true, None) => self.error(
                ErrorKind::SuspendEdge,
                "a value that may suspend has no suspension edge".into(),
            ),
            (true, false, Some(_)) => self.error(
                ErrorKind::SuspendEdge,
                "a value that cannot suspend has a suspension edge".into(),
            ),
            (false, _, Some(_)) => self.error(
                ErrorKind::SuspendEdge,
                "a suspension edge in a function that does not suspend".into(),
            ),
            _ => {}
        }
        if let Some(edge) = edge {
            if !self.states.insert(edge.state) {
                self.error(
                    ErrorKind::SuspendEdge,
                    format!("two suspension edges have state {}", edge.state),
                );
            }
            for &v in &edge.saved {
                self.use_var(v);
            }
        }
    }

    fn stage_at_most(&mut self, stage: Stage, what: &str) {
        if self.stage > stage {
            self.error(
                ErrorKind::StageViolation,
                format!("{what} at stage {:?}", self.stage),
            );
        }
    }

    fn stage_at_least(&mut self, stage: Stage, what: &str) {
        if self.stage < stage {
            self.error(
                ErrorKind::StageViolation,
                format!("{what} at stage {:?}", self.stage),
            );
        }
    }

    /// `make_closure function[captures]` defining `dst`.
    fn make_closure(
        &mut self,
        function: crate::ir::FnId,
        captures: &[VarId],
        dst: Option<VarId>,
    ) -> Expect {
        for &c in captures {
            self.use_var(c);
        }
        let target = &self.m.functions[function];
        let what = format!("make_closure {}", target.name);
        let Some(env) = &target.env else {
            self.error(
                ErrorKind::BadEnvironment,
                format!("{what}: not a lifted function"),
            );
            return Expect::Exactly(Repr::Ref);
        };
        if env.len() != captures.len()
            || env
                .iter()
                .zip(captures)
                .any(|(&t, &c)| self.f.vars[c].ty != t)
        {
            self.error(
                ErrorKind::BadEnvironment,
                format!("{what}: the captures differ from the environment"),
            );
        }
        let Some(dst) = dst else {
            return Expect::Exactly(Repr::Ref);
        };
        let Type::Fn(ft) = self.core.ty(self.f.vars[dst].ty) else {
            return self.wrong_type(format!(
                "{what} defines {}, not of a function type",
                self.name(dst)
            ));
        };
        let expected: Vec<Repr> = self
            .fn_params(&ft.params, ft.ev)
            .into_iter()
            .flatten()
            .collect();
        let found: Vec<Repr> = target
            .params
            .iter()
            .skip(1)
            .map(|&p| target.vars[p].repr)
            .collect();
        if expected != found {
            self.error(
                ErrorKind::ReprMismatch,
                format!("{what}: the parameters disagree with the function type"),
            );
        }
        if repr_of(self.core, target.ret) != repr_of(self.core, ft.ret) {
            self.error(
                ErrorKind::ReprMismatch,
                format!("{what}: the result disagrees with the function type"),
            );
        }
        if target.suspends != ft.suspends {
            self.error(
                ErrorKind::SuspendMismatch,
                format!("{what}: suspends disagrees with the function type"),
            );
        }
        if target.fails && !ft.fails {
            self.error(
                ErrorKind::FailEdge,
                format!("{what}: may fail but its type cannot"),
            );
        }
        Expect::Exactly(Repr::Ref)
    }

    /// The representations of a function type's arguments: its parameters that have one, then the
    /// evidence table if it takes one.
    fn fn_params(&self, params: &[TypeId], ev: bool) -> Vec<Option<Repr>> {
        let mut reprs: Vec<Option<Repr>> = params
            .iter()
            .map(|&t| repr_of(self.core, t))
            .filter(|&r| r != Repr::Void)
            .map(Some)
            .collect();
        if ev {
            reprs.push(Some(Repr::Ref));
        }
        reprs
    }

    fn call(
        &mut self,
        callee: &Callee,
        args: &[Operand],
        suspends: bool,
        on_fail: &Option<OnFail>,
    ) -> Expect {
        match callee {
            Callee::Direct(id) => {
                let target = &self.m.functions[*id];
                let what = format!("call {}", target.name);
                let expected: Vec<Option<Repr>> = target
                    .params
                    .iter()
                    .map(|&p| Some(target.vars[p].repr))
                    .collect();
                self.args(args, &expected, &what);
                if suspends != target.suspends {
                    self.error(
                        ErrorKind::SuspendMismatch,
                        format!("{what}: suspends disagrees with the callee"),
                    );
                }
                self.on_fail(on_fail, target.fails, &what);
                Expect::Exactly(repr_of(self.core, target.ret))
            }
            Callee::Indirect(c) => {
                let r = self.use_var(*c);
                self.expect(r, Repr::Ref, "indirect callee");
                let what = format!("call_indirect {}", self.name(*c));
                let Type::Fn(ft) = self.core.ty(self.f.vars[*c].ty) else {
                    for a in args {
                        self.operand(a);
                    }
                    return self.wrong_type(format!("{what}: not a function"));
                };
                let expected = self.fn_params(&ft.params, ft.ev);
                self.args(args, &expected, &what);
                if suspends != ft.suspends {
                    self.error(
                        ErrorKind::SuspendMismatch,
                        format!("{what}: suspends disagrees with the callee's type"),
                    );
                }
                self.on_fail(on_fail, ft.fails, &what);
                Expect::Exactly(repr_of(self.core, ft.ret))
            }
            Callee::Op {
                handler,
                effect,
                op,
            } => {
                let r = self.use_var(*handler);
                self.expect(r, Repr::Ref, "operation handler");
                let def = &self.core.effects[*effect];
                if self.core.ty(self.f.vars[*handler].ty) != &Type::Handler(*effect) {
                    self.error(
                        ErrorKind::WrongType,
                        format!("{} is not a handler of {}", self.name(*handler), def.name),
                    );
                }
                let Some(o) = def.ops.get(*op) else {
                    for a in args {
                        self.operand(a);
                    }
                    self.error(
                        ErrorKind::BadIndex,
                        format!("operation {op} of {} out of range", def.name),
                    );
                    return Expect::Unknown;
                };
                // The operation's own type parameters (`fork[A]`) are `ref` (boxed): a handler
                // implements a generic operation for every instantiation at once.
                let what = format!("call_op {}#{}", def.name, o.name);
                let expected = self.fn_params(&o.params, false);
                self.args(args, &expected, &what);
                if o.suspend && !suspends {
                    self.error(
                        ErrorKind::SuspendMismatch,
                        format!("operation {} suspends", o.name),
                    );
                }
                self.on_fail(on_fail, o.fails, &what);
                Expect::Exactly(repr_of(self.core, o.ret))
            }
        }
    }
}

/// The representation of a builtin's parameter or result type (`None` for a type parameter).
fn sig_repr(s: Sig) -> Option<Repr> {
    Some(match s {
        Sig::Int | Sig::Bool => Repr::I32,
        Sig::Int64 => Repr::I64,
        Sig::Float => Repr::F64,
        // Option has a constructor with a field.
        Sig::String | Sig::List(_) | Sig::Option(_) => Repr::Ref,
        Sig::Unit | Sig::Never => Repr::Void,
        Sig::Param(_) => return None,
    })
}
