use emela_core::{Builtin, Type, TypeId};
use emela_lir::{FnBuilder, Module, Term, Value};

pub fn add(flow: &mut Module) {
    // A reachable Cell[Any] enables tracing without creating a cycle in the workload.
    let cell_type = flow.core.types.intern(Type::Cell(TypeId::ANY));
    let mut b = FnBuilder::new(&flow.core, "gc_anchor");
    b.set_exported().set_ret(TypeId::ANY);
    let value = b.param("value", TypeId::ANY);
    let cell = b.assign(
        "cell",
        cell_type,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![value.into()],
        },
    );
    let result = b.assign(
        "result",
        TypeId::ANY,
        Value::Builtin {
            op: Builtin::CellGet,
            args: vec![cell.into()],
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
}
