use emela_core::{External, ExternalBinding, FnType, HostRef, Type, TypeId};
use emela_lir::{Callee, FnBuilder, Module, Term, Value};

pub fn flow() -> Module {
    let mut flow = Module::new(emela_core::Module::new());
    let mut targets = Vec::new();
    for (name, ty) in [("integer", TypeId::INT64), ("real", TypeId::FLOAT)] {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_ret(ty);
        b.param("value", ty);
        let target = flow.add(b.finish_external(External {
            bindings: vec![ExternalBinding {
                target: "c".into(),
                host: HostRef::Global(format!("board_{name}")),
            }],
        }));
        targets.push((name, ty, target));
    }
    for indirect in [false, true] {
        for &(name, ty, target) in &targets {
            let ft = flow.core.types.intern(Type::Fn(FnType::pure(vec![ty], ty)));
            let mut b = FnBuilder::new(
                &flow.core,
                &format!("{}_{}", if indirect { "indirect" } else { "direct" }, name),
            );
            b.set_exported().set_ret(ty);
            let input = b.param("value", ty);
            let callee = if indirect {
                Callee::Indirect(b.assign("callback", ft, Value::FnRef(target)))
            } else {
                Callee::Direct(target)
            };
            let result = b.assign(
                "result",
                ty,
                Value::Call {
                    callee,
                    args: vec![input.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(result.into())));
            flow.add(b.finish());
        }
    }
    flow
}
