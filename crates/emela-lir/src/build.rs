//! Building LIR functions.
//!
//! [`FnBuilder`] appends statements to the innermost open sequence. Compound statements take
//! closures that build their sequences:
//!
//! ```
//! # use emela_core::TypeId;
//! # use emela_lir::{Const, FnBuilder, Operand, PrimOp, PrimTy, Term, Value};
//! let core = emela_core::Module::new();
//! // fn abs(n: Int) -> Int { if n < 0 { -n } else { n } }
//! let mut b = FnBuilder::new(&core, "abs");
//! b.set_ret(TypeId::INT);
//! let n = b.param("n", TypeId::INT);
//! let neg = b.assign("neg", TypeId::BOOL, Value::Prim {
//!     op: PrimOp::Lt,
//!     ty: PrimTy::I32,
//!     args: vec![n.into(), Const::Int(0).into()],
//! });
//! let r = b.var("r", TypeId::INT);
//! b.if_(
//!     Some(r),
//!     neg,
//!     |b| {
//!         let m = b.assign("m", TypeId::INT, Value::Prim {
//!             op: PrimOp::Neg,
//!             ty: PrimTy::I32,
//!             args: vec![n.into()],
//!         });
//!         b.term(Term::Yield(Some(m.into())));
//!     },
//!     |b| b.term(Term::Yield(Some(n.into()))),
//! );
//! b.term(Term::Return(Some(r.into())));
//! let f = b.finish();
//! ```
//!
//! The builder does not check what it builds beyond its own structure (a sequence gets at most one
//! terminator, and no statement after it); [`crate::verify`] does.

use emela_core::{External, TypeId};

use crate::ir::{
    CaseKey, Closure, Function, LabelData, LabelId, Operand, Seq, SeqId, Stmt, SwitchOn, Term,
    Value, VarData, VarId,
};
use crate::repr::{Repr, repr_of};

/// Builds one [`Function`]. Borrows the Core module to give variables their representations.
pub struct FnBuilder<'a> {
    core: &'a emela_core::Module,
    f: Function,
    /// The sequences being built, innermost last. The first is the body.
    open: Vec<OpenSeq>,
}

#[derive(Default)]
struct OpenSeq {
    stmts: Vec<crate::ir::StmtId>,
    term: Option<Term>,
}

impl<'a> FnBuilder<'a> {
    /// A function named `name` (unique in the module), returning `()` until [`Self::set_ret`].
    pub fn new(core: &'a emela_core::Module, name: &str) -> Self {
        FnBuilder {
            core,
            f: Function::new(name),
            open: vec![OpenSeq::default()],
        }
    }

    pub fn core(&self) -> &'a emela_core::Module {
        self.core
    }

    /// The function so far (to look up variables while building).
    pub fn function(&self) -> &Function {
        &self.f
    }

    pub fn set_ret(&mut self, ty: TypeId) -> &mut Self {
        self.f.ret = ty;
        self
    }

    pub fn set_exported(&mut self) -> &mut Self {
        self.f.exported = true;
        self
    }

    pub fn set_fails(&mut self) -> &mut Self {
        self.f.fails = true;
        self
    }

    pub fn set_suspends(&mut self) -> &mut Self {
        self.f.suspends = true;
        self
    }

    pub fn set_constant(&mut self) -> &mut Self {
        self.f.constant = true;
        self
    }

    /// A new variable of the Core type, with its representation. Panics if the type has no
    /// representation (`()`, `Never`): such values get no variable.
    pub fn var(&mut self, hint: &str, ty: TypeId) -> VarId {
        let repr = repr_of(self.core, ty);
        assert!(
            repr != Repr::Void,
            "variable `{hint}` of a type without representation"
        );
        let hint = if hint.is_empty() { "v" } else { hint };
        self.f.vars.alloc(VarData {
            hint: hint.to_owned(),
            repr,
            ty,
        })
    }

    /// A new variable appended to the parameters.
    pub fn param(&mut self, hint: &str, ty: TypeId) -> VarId {
        let v = self.var(hint, ty);
        self.f.params.push(v);
        v
    }

    pub fn label(&mut self, hint: &str) -> LabelId {
        self.f.labels.alloc(LabelData {
            hint: hint.to_owned(),
        })
    }

    /// Appends a statement to the innermost open sequence.
    pub fn stmt(&mut self, stmt: Stmt) -> crate::ir::StmtId {
        let id = self.f.stmts.alloc(stmt);
        let seq = self.open.last_mut().expect("no open sequence");
        assert!(seq.term.is_none(), "statement after a terminator");
        seq.stmts.push(id);
        id
    }

    /// `dst = value`．
    pub fn let_(&mut self, dst: Option<VarId>, value: Value) {
        self.stmt(Stmt::Let {
            dst,
            value,
            on_defect: None,
            on_suspend: None,
        });
    }

    /// A new variable bound to `value`.
    pub fn assign(&mut self, hint: &str, ty: TypeId, value: Value) -> VarId {
        let v = self.var(hint, ty);
        self.let_(Some(v), value);
        v
    }

    /// Ends the innermost open sequence. Panics if it already ended.
    pub fn term(&mut self, term: Term) {
        let seq = self.open.last_mut().expect("no open sequence");
        assert!(seq.term.is_none(), "a second terminator");
        seq.term = Some(term);
    }

    /// Builds a sequence apart from the current one (for [`Stmt::Switch`] cases and other
    /// statements built by hand). Ends with [`Term::Yield`] without value unless `build` ends it.
    pub fn seq(&mut self, build: impl FnOnce(&mut Self)) -> SeqId {
        self.open.push(OpenSeq::default());
        build(self);
        let open = self.open.pop().expect("unbalanced sequences");
        self.f.seqs.alloc(Seq {
            stmts: open.stmts,
            term: open.term.unwrap_or(Term::Yield(None)),
        })
    }

    pub fn if_(
        &mut self,
        dst: Option<VarId>,
        cond: impl Into<Operand>,
        then: impl FnOnce(&mut Self),
        else_: impl FnOnce(&mut Self),
    ) {
        let cond = cond.into();
        let then = self.seq(then);
        let else_ = self.seq(else_);
        self.stmt(Stmt::If {
            dst,
            cond,
            then,
            else_,
        });
    }

    /// A switch over sequences built with [`Self::seq`].
    pub fn switch(
        &mut self,
        dst: Option<VarId>,
        on: SwitchOn,
        cases: Vec<(CaseKey, SeqId)>,
        default: Option<SeqId>,
    ) {
        let cases = cases
            .into_iter()
            .map(|(key, body)| crate::ir::Case { key, body })
            .collect();
        self.stmt(Stmt::Switch {
            dst,
            on,
            cases,
            default,
        });
    }

    /// A block labeled with a new label, passed to `body`.
    pub fn block(
        &mut self,
        dst: Option<VarId>,
        hint: &str,
        body: impl FnOnce(&mut Self, LabelId),
    ) -> LabelId {
        let label = self.label(hint);
        let body = self.seq(|b| body(b, label));
        self.stmt(Stmt::Block { dst, label, body });
        label
    }

    /// A loop over `vars`, initialized from the paired operands.
    pub fn loop_(
        &mut self,
        dst: Option<VarId>,
        vars: Vec<(VarId, Operand)>,
        body: impl FnOnce(&mut Self),
    ) {
        let (vars, inits) = vars.into_iter().unzip();
        let body = self.seq(body);
        self.stmt(Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        });
    }

    pub fn protect(
        &mut self,
        dst: Option<VarId>,
        body: impl FnOnce(&mut Self),
        cleanup: impl FnOnce(&mut Self),
    ) {
        let body = self.seq(body);
        let cleanup = self.seq(cleanup);
        self.stmt(Stmt::Protect { dst, body, cleanup });
    }

    /// `dst = closure(params) [captures] { body }`. The parameters are created by the caller (with
    /// [`Self::var`]).
    pub fn closure(
        &mut self,
        dst: VarId,
        params: Vec<VarId>,
        captures: Vec<VarId>,
        suspends: bool,
        body: impl FnOnce(&mut Self),
    ) {
        let body = self.seq(body);
        self.let_(
            Some(dst),
            Value::Closure(Box::new(Closure {
                params,
                captures,
                body,
                suspends,
            })),
        );
    }

    /// The finished function. The body ends with [`Term::Yield`] without value unless it was ended
    /// (which the verifier rejects).
    pub fn finish(mut self) -> Function {
        assert_eq!(self.open.len(), 1, "unbalanced sequences");
        let open = self.open.pop().expect("no body");
        self.f.body = Some(self.f.seqs.alloc(Seq {
            stmts: open.stmts,
            term: open.term.unwrap_or(Term::Yield(None)),
        }));
        self.f
    }

    /// The finished function as an external function: the host function instead of a body.
    /// Panics if statements were added.
    pub fn finish_external(mut self, external: External) -> Function {
        assert!(
            self.open.len() == 1 && self.open[0].stmts.is_empty() && self.open[0].term.is_none(),
            "an external function has no body"
        );
        self.f.external = Some(external);
        self.f
    }
}
