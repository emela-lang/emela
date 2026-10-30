use super::super::*;

pub(in crate::compiler::plan) fn require_runtime(
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    if !matches ! (args . get (1) , Some (Operand :: Const (Const :: String (separator))) if ! separator . is_empty ())
    {
        requirements.require(crate::contract::runtime::RuntimeHelper::Grapheme);
    }
}
