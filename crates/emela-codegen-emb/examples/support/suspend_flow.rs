use emela_core::{External, ExternalBinding, HostRef, TypeId};
use emela_lir::{Callee, Const, FnBuilder, Module, PrimOp, PrimTy, Term, Value};

pub fn recursive_flow() -> Module {
    let mut flow = Module::new(emela_core::Module::new());
    let descend = flow.declare("descend");
    let mut b = FnBuilder::new(&flow.core, "descend");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let empty = b.assign(
        "empty",
        TypeId::BOOL,
        Value::Prim {
            op: PrimOp::Eq,
            ty: PrimTy::I32,
            args: vec![n.into(), Const::Int(0).into()],
        },
    );
    let result = b.var("result", TypeId::INT);
    b.if_(
        Some(result),
        empty,
        |b| b.term(Term::Yield(Some(Const::Int(0).into()))),
        |b| {
            let less = b.assign(
                "less",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Sub,
                    ty: PrimTy::I32,
                    args: vec![n.into(), Const::Int(1).into()],
                },
            );
            let child = b.assign(
                "child",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(descend),
                    args: vec![less.into()],
                    suspends: true,
                    on_fail: None,
                },
            );
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![n.into(), child.into()],
                },
            );
            b.term(Term::Yield(Some(sum.into())));
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.functions[descend] = b.finish();
    flow
}

pub fn task_flow() -> Module {
    let mut flow = Module::new(emela_core::Module::new());
    let mut external = FnBuilder::new(&flow.core, "wait");
    external.set_suspends().set_ret(TypeId::INT);
    external.param("input", TypeId::INT);
    let external = external.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_wait".into()),
        }],
    });
    let wait = flow.add(external);
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let first = b.assign(
        "first",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(wait),
            args: vec![n.into()],
            suspends: true,
            on_fail: None,
        },
    );
    let plus = b.assign(
        "plus",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![first.into(), Const::Int(1).into()],
        },
    );
    let second = b.assign(
        "second",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(wait),
            args: vec![plus.into()],
            suspends: true,
            on_fail: None,
        },
    );
    let result = b.assign(
        "sum",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![first.into(), second.into()],
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    flow
}
