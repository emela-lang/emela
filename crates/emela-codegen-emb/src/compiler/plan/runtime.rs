use super::*;
use crate::contract::runtime::{RuntimeHelper as Helper, RuntimeRequirements as Requirements};
pub(in crate::compiler::plan) trait HelperDependencies: Sized {
    fn dependencies(self) -> &'static [Self];
}
impl HelperDependencies for Helper {
    fn dependencies(self) -> &'static [Self] {
        use Helper::*;
        match self {
            I32Shift => &[I32FromBits],
            I64Shift => &[I64FromBits],
            StringContains | StringStartsWith | StringEndsWith => &[StringMatchesAt],
            Grapheme | StringTrim => &[Utf8Decode],
            _ => &[],
        }
    }
}
pub(in crate::compiler::plan) trait RuntimePreparation: Sized {
    fn require(&mut self, helper: Helper);
    fn collect(flow: &crate::contract::ir::Program, emitted: &HashSet<FnId>) -> Self;
}
impl RuntimePreparation for Requirements {
    fn require(&mut self, helper: Helper) {
        if self.helpers.insert(helper) {
            for &dependency in helper.dependencies() {
                self.require(dependency);
            }
        }
    }

    fn collect(flow: &crate::contract::ir::Program, emitted: &HashSet<FnId>) -> Self {
        let mut requirements = Self::default();
        for (_, function) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
            requirements.defects |= function.exported && function.fails && !function.suspends;
            if function.constant {
                continue;
            }
            let active = runtime_statements(function);
            for (id, statement) in function.stmts.iter() {
                super::operations::statements::require_runtime(
                    statement,
                    active.contains(&id),
                    &mut requirements,
                );
            }
            for (_, sequence) in function.seqs.iter() {
                super::operations::terms::require_runtime(
                    flow,
                    function,
                    &sequence.term,
                    &mut requirements,
                );
            }
        }
        requirements
    }
}

#[test]
fn shared_runtime_dependencies_are_emitted_before_their_consumers() {
    for root in [
        Helper::I32Shift,
        Helper::I64Shift,
        Helper::StringContains,
        Helper::StringStartsWith,
        Helper::StringEndsWith,
        Helper::StringTrim,
        Helper::Grapheme,
    ] {
        let mut requirements = Requirements::default();
        requirements.require(root);
        requirements.require(root);
        let mut emitted = BTreeSet::new();
        for &helper in &requirements.helpers {
            for dependency in helper.dependencies() {
                assert!(
                    emitted.contains(dependency),
                    "{helper:?} needs {dependency:?}"
                );
            }
            assert!(emitted.insert(helper));
        }
        assert!(emitted.contains(&root));
    }
}

#[cfg(test)]
use std::collections::BTreeSet;
