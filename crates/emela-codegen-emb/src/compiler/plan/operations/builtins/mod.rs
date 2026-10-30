pub(in crate::compiler::plan) mod bytes;
use super::*;

pub(in crate::compiler::plan) fn string_behavior(op: Builtin, args: &[Operand]) -> StringBehavior {
    match op {
        Builtin::BytesFromList
        | Builtin::BytesToList
        | Builtin::BytesSize
        | Builtin::BytesGet
        | Builtin::BytesSlice
        | Builtin::BytesConcat
        | Builtin::BytesEncodeUtf8
        | Builtin::BytesDecodeValidUtf8 => bytes::string_behavior(op),
        Builtin::StringAppend => string_append::string_behavior(args),
        Builtin::StringByteSize => string_byte_size::string_behavior(args),
        Builtin::StringContains => string_contains::string_behavior(args),
        Builtin::StringStartsWith => string_starts_with::string_behavior(args),
        Builtin::StringEndsWith => string_ends_with::string_behavior(args),
        Builtin::StringJoin => string_join::string_behavior(args),
        Builtin::IntToString => int_to_string::string_behavior(args),
        Builtin::Int64ToString => int64_to_string::string_behavior(args),
        Builtin::FloatToString => float_to_string::string_behavior(args),
        Builtin::StringFromCodePoint => string_from_code_point::string_behavior(args),
        Builtin::StringLength
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringTrim
        | Builtin::StringCodePoints
        | Builtin::ShowNested
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::AssertFail
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => StringBehavior::default(),
    }
}

pub(in crate::compiler::plan) fn string_bound(
    op: Builtin,
    args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    match op {
        Builtin::BytesDecodeValidUtf8 => Some(usize::from(bounds.capacity)),
        Builtin::StringAppend => string_append::string_bound(args, bounds),
        Builtin::StringTrim => string_trim::string_bound(args, bounds),
        Builtin::StringJoin => string_join::string_bound(args, bounds),
        Builtin::IntToString => int_to_string::string_bound(args, bounds),
        Builtin::Int64ToString => int64_to_string::string_bound(args, bounds),
        Builtin::FloatToString => float_to_string::string_bound(args, bounds),
        Builtin::StringFromCodePoint => string_from_code_point::string_bound(args, bounds),
        Builtin::StringLength
        | Builtin::StringByteSize
        | Builtin::StringContains
        | Builtin::StringStartsWith
        | Builtin::StringEndsWith
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringCodePoints
        | Builtin::ShowNested
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::AssertFail
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::BytesFromList
        | Builtin::BytesToList
        | Builtin::BytesSize
        | Builtin::BytesGet
        | Builtin::BytesSlice
        | Builtin::BytesConcat
        | Builtin::BytesEncodeUtf8
        | Builtin::DebugPrint => None,
    }
}

pub(in crate::compiler::plan) fn result_ownership(
    op: Builtin,
    args: &[Operand],
) -> ResultOwnership {
    match op {
        Builtin::StringTrim => string_trim::result_ownership(),
        Builtin::StringAppend => string_append::result_ownership(args),
        Builtin::StringLength
        | Builtin::StringByteSize
        | Builtin::StringContains
        | Builtin::StringStartsWith
        | Builtin::StringEndsWith
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringJoin
        | Builtin::StringCodePoints
        | Builtin::StringFromCodePoint
        | Builtin::ShowNested
        | Builtin::IntToString
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToString
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatToString
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::AssertFail
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::BytesFromList
        | Builtin::BytesToList
        | Builtin::BytesSize
        | Builtin::BytesGet
        | Builtin::BytesSlice
        | Builtin::BytesConcat
        | Builtin::BytesEncodeUtf8
        | Builtin::BytesDecodeValidUtf8
        | Builtin::DebugPrint => ResultOwnership::Owned,
    }
}

pub(in crate::compiler::plan) fn require_runtime(
    op: Builtin,
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    match op {
        Builtin::BytesFromList
        | Builtin::BytesToList
        | Builtin::BytesSize
        | Builtin::BytesGet
        | Builtin::BytesSlice
        | Builtin::BytesConcat
        | Builtin::BytesEncodeUtf8
        | Builtin::BytesDecodeValidUtf8 => bytes::require_runtime(op, requirements),
        Builtin::StringAppend => string_append::require_runtime(args, requirements),
        Builtin::IntToString => int_to_string::require_runtime(args, requirements),
        Builtin::Int64ToString => int64_to_string::require_runtime(args, requirements),
        Builtin::FloatToString => float_to_string::require_runtime(args, requirements),
        Builtin::IntParse => int_parse::require_runtime(args, requirements),
        Builtin::FloatParse => float_parse::require_runtime(args, requirements),
        Builtin::StringContains => string_contains::require_runtime(args, requirements),
        Builtin::StringStartsWith => string_starts_with::require_runtime(args, requirements),
        Builtin::StringEndsWith => string_ends_with::require_runtime(args, requirements),
        Builtin::StringTrim => string_trim::require_runtime(args, requirements),
        Builtin::StringJoin => string_join::require_runtime(args, requirements),
        Builtin::StringCodePoints => string_code_points::require_runtime(args, requirements),
        Builtin::StringLength => string_length::require_runtime(args, requirements),
        Builtin::StringChars => string_chars::require_runtime(args, requirements),
        Builtin::StringSplit => string_split::require_runtime(args, requirements),
        Builtin::StringFromCodePoint => string_from_code_point::require_runtime(args, requirements),
        Builtin::IntShiftLeft | Builtin::IntShiftRight | Builtin::IntShiftRightUnsigned => {
            integer::require_runtime(op, requirements)
        }
        Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::Int64ToInt => integer64::require_runtime(op, requirements),
        Builtin::FloatFloor | Builtin::FloatCeil | Builtin::FloatRound | Builtin::FloatTruncate => {
            float::require_runtime(op, requirements)
        }
        Builtin::StringByteSize
        | Builtin::ShowNested
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::Int64FromInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::FloatFromInt
        | Builtin::Panic
        | Builtin::AssertFail
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => {}
    }
}

pub(in crate::compiler::plan) fn needs_heap(op: Builtin) -> bool {
    match op {
        Builtin::BytesFromList
        | Builtin::BytesToList
        | Builtin::BytesSize
        | Builtin::BytesGet
        | Builtin::BytesSlice
        | Builtin::BytesConcat
        | Builtin::BytesEncodeUtf8
        | Builtin::BytesDecodeValidUtf8 => bytes::needs_heap(op),
        Builtin::CellNew => cells::needs_heap(op),
        Builtin::StringLength
        | Builtin::StringByteSize
        | Builtin::StringAppend
        | Builtin::StringContains
        | Builtin::StringStartsWith
        | Builtin::StringEndsWith
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringJoin
        | Builtin::StringTrim
        | Builtin::StringCodePoints
        | Builtin::StringFromCodePoint
        | Builtin::ShowNested
        | Builtin::IntToString
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToString
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatToString
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::AssertFail
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => false,
    }
}

pub(in crate::compiler::plan) mod cells;
pub(in crate::compiler::plan) mod float;
pub(in crate::compiler::plan) mod float_parse;
pub(in crate::compiler::plan) mod float_to_string;
pub(in crate::compiler::plan) mod int64_to_string;
pub(in crate::compiler::plan) mod int_parse;
pub(in crate::compiler::plan) mod int_to_string;
pub(in crate::compiler::plan) mod integer;
pub(in crate::compiler::plan) mod integer64;
pub(in crate::compiler::plan) mod string_append;
pub(in crate::compiler::plan) mod string_byte_size;
pub(in crate::compiler::plan) mod string_chars;
pub(in crate::compiler::plan) mod string_code_points;
pub(in crate::compiler::plan) mod string_contains;
pub(in crate::compiler::plan) mod string_ends_with;
pub(in crate::compiler::plan) mod string_from_code_point;
pub(in crate::compiler::plan) mod string_join;
pub(in crate::compiler::plan) mod string_length;
pub(in crate::compiler::plan) mod string_split;
pub(in crate::compiler::plan) mod string_starts_with;
pub(in crate::compiler::plan) mod string_trim;
