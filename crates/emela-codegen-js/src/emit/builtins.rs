//! Which runtime function implements each builtin.
//!
//! The builtin table (`emela_core::intrinsics`) only knows names and types; the implementations
//! live in `runtime.mjs`. The match is exhaustive, so adding a builtin fails to compile until the
//! JS backend implements it.
//!
//! Identity conversions such as `Int.to_float` are also runtime calls, so every builtin is
//! emitted the same way.

use emela_core::Builtin;

/// The name of the function in `runtime.mjs` that implements `op`.
pub(crate) fn runtime_name(op: Builtin) -> &'static str {
    match op {
        Builtin::StringLength => "$strlen",
        Builtin::StringByteSize => "$strbytes",
        Builtin::StringAppend => "$strcat",
        Builtin::StringContains => "$strhas",
        Builtin::StringStartsWith => "$strstarts",
        Builtin::StringEndsWith => "$strends",
        Builtin::StringSplit => "$strsplit",
        Builtin::StringChars => "$strchars",
        Builtin::StringJoin => "$strjoin",
        Builtin::StringTrim => "$strtrim",
        Builtin::StringCodePoints => "$strcodepoints",
        Builtin::StringFromCodePoint => "$strfromcodepoint",
        Builtin::IntToString => "$showInt",
        Builtin::IntParse => "$iparse",
        Builtin::IntCheckedAdd => "$icheckedAdd",
        Builtin::IntBitAnd => "$iand",
        Builtin::IntBitOr => "$ior",
        Builtin::IntBitXor => "$ixor",
        Builtin::IntBitNot => "$inot",
        Builtin::IntShiftLeft => "$ishl",
        Builtin::IntShiftRight => "$ishr",
        Builtin::IntShiftRightUnsigned => "$iushr",
        Builtin::Int64FromInt => "$itol",
        Builtin::Int64ToString => "$showInt64",
        Builtin::Int64ToInt => "$ltoi",
        Builtin::Int64BitAnd => "$land",
        Builtin::Int64BitOr => "$lor",
        Builtin::Int64BitXor => "$lxor",
        Builtin::Int64BitNot => "$lnot",
        Builtin::Int64ShiftLeft => "$lshl",
        Builtin::Int64ShiftRight => "$lshr",
        Builtin::Int64ShiftRightUnsigned => "$lushr",
        Builtin::FloatToString => "$showFloat",
        Builtin::FloatFromInt => "$itof",
        Builtin::FloatParse => "$fparse",
        Builtin::FloatFloor => "$ffloor",
        Builtin::FloatCeil => "$fceil",
        Builtin::FloatRound => "$fround",
        Builtin::FloatTruncate => "$ftrunc",
        Builtin::BytesFromList => "$bytesFromList",
        Builtin::BytesToList => "$bytesToList",
        Builtin::BytesSize => "$bytesSize",
        Builtin::BytesGet => "$bytesGet",
        Builtin::BytesSlice => "$bytesSlice",
        Builtin::BytesConcat => "$bytesConcat",
        Builtin::BytesEncodeUtf8 => "$bytesEncodeUtf8",
        Builtin::BytesDecodeValidUtf8 => "$bytesDecodeValidUtf8",
        Builtin::Panic => "$panic",
        Builtin::Todo => "$todo",
        Builtin::Dbg => "$dbg",
        Builtin::Inspect => "$inspect",
        Builtin::CellNew => "$cellNew",
        Builtin::CellGet => "$cellGet",
        Builtin::CellSet => "$cellSet",
        Builtin::ShowString => "$showString",
        Builtin::DebugPrint => "$debugPrint",
        Builtin::AssertFail => "$assertDefect",
        Builtin::ShowNested => unreachable!("monomorphization replaces String.show_nested"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RUNTIME;

    #[test]
    fn runtime_implements_every_builtin() {
        let internal = [
            Builtin::CellNew,
            Builtin::CellGet,
            Builtin::CellSet,
            Builtin::ShowString,
            Builtin::DebugPrint,
            Builtin::AssertFail,
        ];
        let replaced = [Builtin::ShowNested];
        for &op in Builtin::ALL
            .iter()
            .chain(&internal)
            .filter(|op| !replaced.contains(op))
        {
            let name = runtime_name(op);
            let decl = format!("export function {name}(");
            assert!(
                RUNTIME.contains(&decl),
                "the runtime has no implementation {name} for {op:?}"
            );
        }
    }
}
