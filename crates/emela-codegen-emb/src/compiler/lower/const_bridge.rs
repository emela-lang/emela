use emela_lir::{Callee, Stmt, Value};
pub(super) fn prepare(
    flow: &mut emela_lir::Module,
    groups: &[crate::input::upstream::const_origins::Group],
) {
    for group in groups {
        for &id in &group.specializations {
            let function = &mut flow.functions[id];
            function.constant = false;
            let statement = function.stmts.alloc(Stmt::Let {
                dst: None,
                value: Value::Call {
                    callee: Callee::Direct(group.canonical),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
                on_defect: None,
                on_suspend: None,
            });
            // Keep both callable families reachable until the C initializer is replaced.
            let body = function.body.expect("const definition has a body");
            function.seqs[body].stmts.insert(0, statement);
        }
    }
}
