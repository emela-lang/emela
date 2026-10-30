use crate::input::upstream::const_eval;
use emela_lir::{FnId, Module};
use std::collections::HashSet;
pub(crate) fn prepare(
    module: &mut Module,
    reachable: fn(&Module) -> HashSet<FnId>,
) -> HashSet<FnId> {
    let mut runtime = HashSet::new();
    loop {
        let newly_runtime: Vec<_> = reachable(module)
            .into_iter()
            .filter(|&id| {
                module.functions[id].constant && const_eval::evaluate(module, id).is_err()
            })
            .collect();
        if newly_runtime.is_empty() {
            return runtime;
        }
        for id in newly_runtime {
            module.functions[id].constant = false;
            runtime.insert(id);
        }
    }
}
