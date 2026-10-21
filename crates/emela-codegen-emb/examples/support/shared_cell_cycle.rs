use emela_core::{Builtin, FnType, Type, TypeId};
use emela_lir::{Const, FnBuilder, Module, Term, Value};

pub fn flow() -> Module {
    let mut core = emela_core::Module::new();
    let callback = core
        .types
        .intern(Type::Fn(FnType::pure(vec![], TypeId::INT)));
    let cell_ty = core.types.intern(Type::Cell(callback));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "constant");
    b.set_ret(TypeId::INT);
    b.term(Term::Return(Some(Const::Int(42).into())));
    let constant = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_ret(callback).set_exported();
    let initial = b.assign("initial", callback, Value::FnRef(constant));
    let cell = b.assign(
        "cell",
        cell_ty,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![initial.into()],
        },
    );
    let reader = b.var("reader", callback);
    b.closure(reader, vec![], vec![cell], false, |b| {
        let _read = b.assign(
            "read",
            callback,
            Value::Builtin {
                op: Builtin::CellGet,
                args: vec![cell.into()],
            },
        );
        b.term(Term::Return(Some(Const::Int(42).into())));
    });
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellSet,
            args: vec![cell.into(), reader.into()],
        },
    );
    b.term(Term::Return(Some(reader.into())));
    flow.add(b.finish());
    flow
}
