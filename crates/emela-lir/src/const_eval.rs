//! Compile-time evaluation of consts (`docs/design/c-embedded-architecture.md`, ③ static
//! evaluation): an interpreter of the functions of a module, for consts whose values are scalars
//! and value aggregates.
//!
//! A const is pure (spec 5.6), so evaluating it at compile time gives what the first reference
//! would give at run time. The embedded C target puts the values in read-only data, so no flag
//! records whether a const was evaluated (which an interrupt and the main code could race on).
//! Evaluation stops with [`ConstResult::Defect`] when the const raises a defect (the reference
//! raises it at run time, as spec 5.6 says, so the backend keeps the function), and with
//! [`ConstResult::NotStatic`] when it meets something it does not evaluate (a heap value, an
//! external function): the backend then evaluates the const at run time.

use rustc_hash::FxHashMap;

use crate::ir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, Module, Operand, PrimOp, PrimTy, RuntimeOp,
    SeqId, Stmt, SwitchOn, Term, Value, VarId,
};
use crate::repr::Repr;
use emela_core::Builtin;

/// A value known at compile time.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    /// Int, Bool (0 or 1), and the tag of an enum whose constructors take no arguments.
    I32(i32),
    I64(i64),
    F64(f64),
    /// A value aggregate: the constructor index (0 for a tuple) and every field, in field order
    /// ([`ConstValue::Unit`] for a field without value).
    Agg {
        tag: usize,
        fields: Vec<ConstValue>,
    },
    Unit,
}

/// What evaluating a const gives.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstResult {
    Value(ConstValue),
    /// The const raises a defect with the message.
    Defect(String),
    /// The const uses something the evaluator does not evaluate.
    NotStatic,
    /// The evaluation takes more steps than the budget.
    TooLong,
}

/// Evaluates the consts among `functions` (the functions with [`Function::constant`]), each
/// within `budget` steps.
pub fn eval_consts(m: &Module, functions: &[FnId], budget: u64) -> FxHashMap<FnId, ConstResult> {
    let mut out = FxHashMap::default();
    for &id in functions {
        let f = &m.functions[id];
        if !f.constant || f.body.is_none() || !f.params.is_empty() {
            continue;
        }
        let mut e = Eval {
            m,
            steps: budget,
            depth: 0,
        };
        let result = match e.call(id, Vec::new()) {
            Ok(Some(v)) => ConstResult::Value(v),
            Ok(None) => ConstResult::Value(ConstValue::Unit),
            Err(Stop::Defect(msg)) => ConstResult::Defect(msg),
            Err(Stop::NotStatic) => ConstResult::NotStatic,
            Err(Stop::TooLong) => ConstResult::TooLong,
        };
        out.insert(id, result);
    }
    out
}

enum Stop {
    Defect(String),
    NotStatic,
    TooLong,
}

/// How a sequence ends.
enum Flow {
    Yield(Option<ConstValue>),
    Return(Option<ConstValue>),
    Break(LabelId, Option<ConstValue>),
    Recur(Vec<ConstValue>),
}

type R<T> = Result<T, Stop>;

/// The deepest call nesting evaluated (deeper recursion is [`Stop::TooLong`]).
const MAX_DEPTH: u32 = 2000;

struct Eval<'m> {
    m: &'m Module,
    steps: u64,
    depth: u32,
}

struct Frame<'f> {
    f: &'f Function,
    vars: FxHashMap<VarId, ConstValue>,
}

impl Frame<'_> {
    fn get(&self, o: &Operand) -> R<ConstValue> {
        match o {
            Operand::Var(v) => self.vars.get(v).cloned().ok_or(Stop::NotStatic),
            Operand::Const(c) => constant(c),
        }
    }

    fn var(&self, v: VarId) -> R<ConstValue> {
        self.vars.get(&v).cloned().ok_or(Stop::NotStatic)
    }
}

fn constant(c: &Const) -> R<ConstValue> {
    Ok(match c {
        Const::Int(n) => ConstValue::I32(*n),
        Const::Bool(b) => ConstValue::I32(i32::from(*b)),
        Const::Int64(n) => ConstValue::I64(*n),
        Const::Float(x) => ConstValue::F64(*x),
        Const::Unit => ConstValue::Unit,
        Const::String(_) => return Err(Stop::NotStatic),
    })
}

impl Eval<'_> {
    fn step(&mut self) -> R<()> {
        if self.steps == 0 {
            return Err(Stop::TooLong);
        }
        self.steps -= 1;
        Ok(())
    }

    fn call(&mut self, id: FnId, args: Vec<ConstValue>) -> R<Option<ConstValue>> {
        let f = &self.m.functions[id];
        let Some(body) = f.body else {
            return Err(Stop::NotStatic);
        };
        if f.params.len() != args.len() {
            return Err(Stop::NotStatic);
        }
        if self.depth >= MAX_DEPTH {
            return Err(Stop::TooLong);
        }
        self.depth += 1;
        let mut frame = Frame {
            f,
            vars: f.params.iter().copied().zip(args).collect(),
        };
        let flow = self.seq(&mut frame, body);
        self.depth -= 1;
        match flow? {
            Flow::Return(v) => Ok(v),
            _ => Err(Stop::NotStatic),
        }
    }

    fn seq(&mut self, fr: &mut Frame<'_>, id: SeqId) -> R<Flow> {
        let seq = &fr.f.seqs[id];
        for &s in &seq.stmts {
            if let Some(flow) = self.stmt(fr, s)? {
                return Ok(flow);
            }
        }
        self.step()?;
        let opt = |fr: &Frame<'_>, o: &Option<Operand>| o.as_ref().map(|o| fr.get(o)).transpose();
        Ok(match &seq.term {
            Term::Yield(v) => Flow::Yield(opt(fr, v)?),
            Term::Return(v) => Flow::Return(opt(fr, v)?),
            Term::Break { label, value } => Flow::Break(*label, opt(fr, value)?),
            Term::Recur(args) => Flow::Recur(args.iter().map(|a| fr.get(a)).collect::<R<_>>()?),
            Term::Fail(_) | Term::Defect(_) | Term::Unwind | Term::Unreachable => {
                return Err(Stop::NotStatic);
            }
        })
    }

    fn define(fr: &mut Frame<'_>, dst: Option<VarId>, v: Option<ConstValue>) {
        if let (Some(d), Some(v)) = (dst, v) {
            fr.vars.insert(d, v);
        }
    }

    /// Runs a statement; `Some` if control leaves the sequence through it.
    fn stmt(&mut self, fr: &mut Frame<'_>, id: crate::ir::StmtId) -> R<Option<Flow>> {
        self.step()?;
        let f = fr.f;
        match &f.stmts[id] {
            Stmt::Let { dst, value, .. } => {
                let v = self.value(fr, *dst, value)?;
                Self::define(fr, *dst, v);
                Ok(None)
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let c = fr.get(cond)?;
                let seq = if c != ConstValue::I32(0) {
                    *then
                } else {
                    *else_
                };
                self.joined(fr, *dst, seq, None)
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let chosen = match on {
                    SwitchOn::Tag(v) => {
                        let tag = match fr.var(*v)? {
                            ConstValue::I32(t) => t as usize,
                            ConstValue::Agg { tag, .. } => tag,
                            _ => return Err(Stop::NotStatic),
                        };
                        cases
                            .iter()
                            .find(|c| c.key == CaseKey::Variant(tag))
                            .map(|c| c.body)
                    }
                    SwitchOn::Lit(v) => {
                        let x = fr.var(*v)?;
                        cases
                            .iter()
                            .find(|c| match &c.key {
                                CaseKey::Lit(k) => constant(k).is_ok_and(|k| k == x),
                                CaseKey::Variant(_) => false,
                            })
                            .map(|c| c.body)
                    }
                };
                match chosen.or(*default) {
                    Some(seq) => self.joined(fr, *dst, seq, None),
                    None => Err(Stop::NotStatic),
                }
            }
            Stmt::Block { dst, label, body } => self.joined(fr, *dst, *body, Some(*label)),
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                let mut values: Vec<ConstValue> =
                    inits.iter().map(|o| fr.get(o)).collect::<R<_>>()?;
                loop {
                    for (&v, x) in vars.iter().zip(values.drain(..)) {
                        fr.vars.insert(v, x);
                    }
                    match self.seq(fr, *body)? {
                        Flow::Recur(next) => values = next,
                        Flow::Yield(v) => {
                            Self::define(fr, *dst, v);
                            return Ok(None);
                        }
                        other => return Ok(Some(other)),
                    }
                }
            }
            Stmt::Protect { .. } => Err(Stop::NotStatic),
            Stmt::Dup(_) | Stmt::Drop(_) => Ok(None),
        }
    }

    /// Runs a branch or block body whose `yield` (or `break` to `label`) gives `dst`.
    fn joined(
        &mut self,
        fr: &mut Frame<'_>,
        dst: Option<VarId>,
        seq: SeqId,
        label: Option<LabelId>,
    ) -> R<Option<Flow>> {
        match self.seq(fr, seq)? {
            Flow::Yield(v) => {
                Self::define(fr, dst, v);
                Ok(None)
            }
            Flow::Break(l, v) if Some(l) == label => {
                Self::define(fr, dst, v);
                Ok(None)
            }
            other => Ok(Some(other)),
        }
    }

    fn value(
        &mut self,
        fr: &mut Frame<'_>,
        dst: Option<VarId>,
        value: &Value,
    ) -> R<Option<ConstValue>> {
        let f = fr.f;
        let args = |fr: &Frame<'_>, os: &[Operand]| -> R<Vec<ConstValue>> {
            os.iter().map(|o| fr.get(o)).collect()
        };
        Ok(Some(match value {
            Value::Const(c) => constant(c)?,
            Value::Prim { op, ty, args: a } => prim(*op, *ty, &args(fr, a)?)?,
            Value::Call {
                callee: Callee::Direct(g),
                args: a,
                on_fail: None,
                ..
            } => {
                let a = args(fr, a)?;
                return self.call(*g, a);
            }
            Value::Builtin { op, args: a } => builtin(*op, &args(fr, a)?)?,
            Value::Make { ctor, args: a } => {
                let a = args(fr, a)?;
                match dst.map(|d| f.vars[d].repr) {
                    Some(Repr::I32) => ConstValue::I32(ctor.variant as i32),
                    Some(Repr::Agg) => ConstValue::Agg {
                        tag: ctor.variant,
                        fields: a,
                    },
                    _ => return Err(Stop::NotStatic),
                }
            }
            Value::Field { base, index, .. } | Value::TupleGet { base, index } => {
                match fr.var(*base)? {
                    ConstValue::Agg { fields, .. } => {
                        fields.get(*index).cloned().ok_or(Stop::NotStatic)?
                    }
                    _ => return Err(Stop::NotStatic),
                }
            }
            Value::Tag(v) => match fr.var(*v)? {
                ConstValue::Agg { tag, .. } => ConstValue::I32(tag as i32),
                t @ ConstValue::I32(_) => t,
                _ => return Err(Stop::NotStatic),
            },
            Value::Tuple(a) => {
                if dst.is_some_and(|d| f.vars[d].repr != Repr::Agg) {
                    return Err(Stop::NotStatic);
                }
                ConstValue::Agg {
                    tag: 0,
                    fields: args(fr, a)?,
                }
            }
            Value::Coerce(v) => fr.var(*v)?,
            Value::Runtime {
                op: RuntimeOp::Defect,
                args: a,
            } => {
                let msg = match a.first() {
                    Some(Operand::Const(Const::String(s))) => s.clone(),
                    _ => String::new(),
                };
                return Err(Stop::Defect(msg));
            }
            _ => return Err(Stop::NotStatic),
        }))
    }
}

fn prim(op: PrimOp, ty: PrimTy, a: &[ConstValue]) -> R<ConstValue> {
    use ConstValue::{F64, I32, I64};
    let b = |x: bool| I32(i32::from(x));
    Ok(match (ty, a) {
        (PrimTy::I32, [I32(x)]) => match op {
            PrimOp::Neg => I32(x.wrapping_neg()),
            PrimOp::Not => b(*x == 0),
            _ => return Err(Stop::NotStatic),
        },
        (PrimTy::I32, [I32(x), I32(y)]) => {
            let (x, y) = (*x, *y);
            match op {
                PrimOp::Add => I32(x.wrapping_add(y)),
                PrimOp::Sub => I32(x.wrapping_sub(y)),
                PrimOp::Mul => I32(x.wrapping_mul(y)),
                // The divisor is not 0: the Heap IR tests it first.
                PrimOp::Div if y != 0 => I32(x.wrapping_div(y)),
                PrimOp::Rem if y != 0 => I32(x.wrapping_rem(y)),
                PrimOp::Eq => b(x == y),
                PrimOp::Ne => b(x != y),
                PrimOp::Lt => b(x < y),
                PrimOp::Le => b(x <= y),
                PrimOp::Gt => b(x > y),
                PrimOp::Ge => b(x >= y),
                _ => return Err(Stop::NotStatic),
            }
        }
        (PrimTy::I64, [I64(x)]) => match op {
            PrimOp::Neg => I64(x.wrapping_neg()),
            _ => return Err(Stop::NotStatic),
        },
        (PrimTy::I64, [I64(x), I64(y)]) => {
            let (x, y) = (*x, *y);
            match op {
                PrimOp::Add => I64(x.wrapping_add(y)),
                PrimOp::Sub => I64(x.wrapping_sub(y)),
                PrimOp::Mul => I64(x.wrapping_mul(y)),
                PrimOp::Div if y != 0 => I64(x.wrapping_div(y)),
                PrimOp::Rem if y != 0 => I64(x.wrapping_rem(y)),
                PrimOp::Eq => b(x == y),
                PrimOp::Ne => b(x != y),
                PrimOp::Lt => b(x < y),
                PrimOp::Le => b(x <= y),
                PrimOp::Gt => b(x > y),
                PrimOp::Ge => b(x >= y),
                _ => return Err(Stop::NotStatic),
            }
        }
        (PrimTy::F64, [F64(x)]) => match op {
            PrimOp::Neg => F64(-x),
            _ => return Err(Stop::NotStatic),
        },
        (PrimTy::F64, [F64(x), F64(y)]) => {
            let (x, y) = (*x, *y);
            match op {
                PrimOp::Add => F64(x + y),
                PrimOp::Sub => F64(x - y),
                PrimOp::Mul => F64(x * y),
                PrimOp::Div => F64(x / y),
                PrimOp::Rem => F64(x % y),
                PrimOp::Eq => b(x == y),
                PrimOp::Ne => b(x != y),
                PrimOp::Lt => b(x < y),
                PrimOp::Le => b(x <= y),
                PrimOp::Gt => b(x > y),
                PrimOp::Ge => b(x >= y),
                _ => return Err(Stop::NotStatic),
            }
        }
        _ => return Err(Stop::NotStatic),
    })
}

fn builtin(op: Builtin, a: &[ConstValue]) -> R<ConstValue> {
    use ConstValue::{F64, I32, I64};
    let shift = |by: i32| {
        if by < 0 {
            Err(Stop::Defect("negative shift amount".into()))
        } else {
            Ok(by as u32)
        }
    };
    let to_int = |r: f64| {
        if (-2147483648.0..=2147483647.0).contains(&r) {
            Ok(I32(r as i32))
        } else {
            Err(Stop::Defect("cannot convert Float to Int".into()))
        }
    };
    Ok(match (op, a) {
        (Builtin::IntBitAnd, [I32(x), I32(y)]) => I32(x & y),
        (Builtin::IntBitOr, [I32(x), I32(y)]) => I32(x | y),
        (Builtin::IntBitXor, [I32(x), I32(y)]) => I32(x ^ y),
        (Builtin::IntBitNot, [I32(x)]) => I32(!x),
        (Builtin::IntShiftLeft, [I32(x), I32(by)]) => {
            let by = shift(*by)?;
            I32(if by >= 32 {
                0
            } else {
                ((*x as u32) << by) as i32
            })
        }
        (Builtin::IntShiftRight, [I32(x), I32(by)]) => I32(x >> shift(*by)?.min(31)),
        (Builtin::IntShiftRightUnsigned, [I32(x), I32(by)]) => {
            let by = shift(*by)?;
            I32(if by >= 32 {
                0
            } else {
                ((*x as u32) >> by) as i32
            })
        }
        (Builtin::Int64BitAnd, [I64(x), I64(y)]) => I64(x & y),
        (Builtin::Int64BitOr, [I64(x), I64(y)]) => I64(x | y),
        (Builtin::Int64BitXor, [I64(x), I64(y)]) => I64(x ^ y),
        (Builtin::Int64BitNot, [I64(x)]) => I64(!x),
        (Builtin::Int64ShiftLeft, [I64(x), I32(by)]) => {
            let by = shift(*by)?;
            I64(if by >= 64 {
                0
            } else {
                ((*x as u64) << by) as i64
            })
        }
        (Builtin::Int64ShiftRight, [I64(x), I32(by)]) => I64(x >> shift(*by)?.min(63)),
        (Builtin::Int64ShiftRightUnsigned, [I64(x), I32(by)]) => {
            let by = shift(*by)?;
            I64(if by >= 64 {
                0
            } else {
                ((*x as u64) >> by) as i64
            })
        }
        (Builtin::Int64FromInt, [I32(x)]) => I64(i64::from(*x)),
        (Builtin::Int64ToInt, [I64(x)]) => I32(*x as i32),
        (Builtin::FloatFromInt, [I32(x)]) => F64(f64::from(*x)),
        (Builtin::FloatFloor, [F64(x)]) => to_int(x.floor())?,
        (Builtin::FloatCeil, [F64(x)]) => to_int(x.ceil())?,
        // 0.5 rounds away from zero (spec 16.1), as Rust's `round`.
        (Builtin::FloatRound, [F64(x)]) => to_int(x.round())?,
        (Builtin::FloatTruncate, [F64(x)]) => to_int(x.trunc())?,
        _ => return Err(Stop::NotStatic),
    })
}
