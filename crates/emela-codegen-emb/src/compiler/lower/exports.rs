use emela_core::Type;
use emela_lir::{Callee, OnFail, Term, Value};
pub(in crate::compiler::lower) fn normalize_exports(module: &mut emela_lir::Module) {
    let ids: Vec<_> = module
        .functions
        .iter()
        .filter_map(|(id, f)| (f.exported && f.external.is_some()).then_some(id))
        .collect();
    for id in ids {
        let original = module.functions[id].clone();
        let mut target = original.clone();
        target.exported = false;
        target.name = format!("{}$external_{}", original.name, u32::from(id.into_raw()));
        let target = module.functions.alloc(target);
        let mut b = emela_lir::FnBuilder::new(&module.core, &original.name);
        b.set_exported().set_ret(original.ret);
        if original.fails {
            b.set_fails();
        }
        if original.suspends {
            b.set_suspends();
        }
        let args = original
            .params
            .iter()
            .map(|&p| b.param(&original.vars[p].hint, original.vars[p].ty).into())
            .collect();
        let value = Value::Call {
            callee: Callee::Direct(target),
            args,
            suspends: original.suspends,
            on_fail: original.fails.then_some(OnFail::Propagate),
        };
        if matches!(module.core.types[original.ret], Type::Unit | Type::Never) {
            b.let_(None, value);
            b.term(Term::Return(None));
        } else {
            let result = b.assign("result", original.ret, value);
            b.term(Term::Return(Some(result.into())));
        }
        module.functions[id] = b.finish();
    }
}
