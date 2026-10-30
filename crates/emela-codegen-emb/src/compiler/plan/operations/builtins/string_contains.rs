use super::super::*;

pub(in crate::compiler::plan) fn string_behavior(args: &[Operand]) -> StringBehavior {
    let empty = args.iter().take(2).any(
        |arg| matches ! (arg , Operand :: Const (Const :: String (value)) if value . is_empty ()),
    );
    StringBehavior {
        input_use: if empty {
            InputUse::Escapes
        } else {
            InputUse::ReadOnly
        },
        ..StringBehavior::default()
    }
}

pub(in crate::compiler::plan) fn require_runtime(
    _args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.require(crate::contract::runtime::RuntimeHelper::StringContains);
}
