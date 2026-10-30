//! Byte storage uses the fixed heap; UTF-8 decoding uses the planned String buffer.
use super::*;

pub(in crate::compiler::plan) fn string_behavior(op: Builtin) -> StringBehavior {
    StringBehavior {
        output: if op == Builtin::BytesDecodeValidUtf8 {
            StringOutput::Buffer
        } else {
            StringOutput::None
        },
        input_use: InputUse::ReadOnly,
        ..StringBehavior::default()
    }
}

pub(in crate::compiler::plan) fn needs_heap(op: Builtin) -> bool {
    matches!(
        op,
        Builtin::BytesFromList
            | Builtin::BytesToList
            | Builtin::BytesSlice
            | Builtin::BytesConcat
            | Builtin::BytesEncodeUtf8
    )
}

pub(in crate::compiler::plan) fn require_runtime(
    op: Builtin,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    if op == Builtin::BytesDecodeValidUtf8 {
        requirements.require(crate::contract::runtime::RuntimeHelper::StringOutput);
    }
    requirements.defects |= needs_heap(op) || op == Builtin::BytesDecodeValidUtf8;
}
