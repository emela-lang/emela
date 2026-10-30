// Keep analysis dispatch exhaustive: every new variant needs an explicit planning decision.
use super::*;
pub(in crate::compiler::emit) mod bytes;
pub(in crate::compiler::emit) mod cells;
pub(in crate::compiler::emit) mod float;
pub(in crate::compiler::emit) mod float_parse;
pub(in crate::compiler::emit) mod float_to_string;
pub(in crate::compiler::emit) mod int64_to_string;
pub(in crate::compiler::emit) mod int_checked_add;
pub(in crate::compiler::emit) mod int_parse;
pub(in crate::compiler::emit) mod int_to_string;
pub(in crate::compiler::emit) mod integer;
pub(in crate::compiler::emit) mod integer64;
pub(in crate::compiler::emit) mod rejected;
pub(in crate::compiler::emit) mod string_append;
pub(in crate::compiler::emit) mod string_byte_size;
pub(in crate::compiler::emit) mod string_chars;
pub(in crate::compiler::emit) mod string_code_points;
pub(in crate::compiler::emit) mod string_contains;
pub(in crate::compiler::emit) mod string_ends_with;
pub(in crate::compiler::emit) mod string_from_code_point;
pub(in crate::compiler::emit) mod string_join;
pub(in crate::compiler::emit) mod string_length;
pub(in crate::compiler::emit) mod string_split;
pub(in crate::compiler::emit) mod string_starts_with;
pub(in crate::compiler::emit) mod string_trim;
pub(in crate::compiler::emit) fn emit(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    match op {
        Builtin::StringLength => string_length::string_length(context, dst, args),
        Builtin::StringByteSize => string_byte_size::string_byte_size(context, dst, args),
        Builtin::StringAppend => string_append::string_append(context, dst, args),
        Builtin::StringContains => string_contains::string_contains(context, dst, args),
        Builtin::StringStartsWith => string_starts_with::string_starts_with(context, dst, args),
        Builtin::StringEndsWith => string_ends_with::string_ends_with(context, dst, args),
        Builtin::StringSplit => string_split::string_split(context, dst, args),
        Builtin::StringChars => string_chars::string_chars(context, dst, args),
        Builtin::StringJoin => string_join::string_join(context, dst, args),
        Builtin::StringTrim => string_trim::string_trim(context, dst, args),
        Builtin::StringCodePoints => string_code_points::string_code_points(context, dst, args),
        Builtin::StringFromCodePoint => {
            string_from_code_point::string_from_code_point(context, dst, args)
        }
        Builtin::ShowNested => rejected::show_nested(context, dst, args),
        Builtin::IntToString => int_to_string::int_to_string(context, dst, args),
        Builtin::IntParse => int_parse::int_parse(context, dst, args),
        Builtin::IntCheckedAdd => int_checked_add::int_checked_add(context, dst, args),
        Builtin::IntBitAnd => integer::int_bit_and(context, dst, args),
        Builtin::IntBitOr => integer::int_bit_or(context, dst, args),
        Builtin::IntBitXor => integer::int_bit_xor(context, dst, args),
        Builtin::IntBitNot => integer::int_bit_not(context, dst, args),
        Builtin::IntShiftLeft => integer::int_shift_left(context, dst, args),
        Builtin::IntShiftRight => integer::int_shift_right(context, dst, args),
        Builtin::IntShiftRightUnsigned => integer::int_shift_right_unsigned(context, dst, args),
        Builtin::Int64FromInt => integer64::int64_from_int(context, dst, args),
        Builtin::Int64ToString => int64_to_string::int64_to_string(context, dst, args),
        Builtin::Int64ToInt => integer64::int64_to_int(context, dst, args),
        Builtin::Int64BitAnd => integer64::int64_bit_and(context, dst, args),
        Builtin::Int64BitOr => integer64::int64_bit_or(context, dst, args),
        Builtin::Int64BitXor => integer64::int64_bit_xor(context, dst, args),
        Builtin::Int64BitNot => integer64::int64_bit_not(context, dst, args),
        Builtin::Int64ShiftLeft => integer64::int64_shift_left(context, dst, args),
        Builtin::Int64ShiftRight => integer64::int64_shift_right(context, dst, args),
        Builtin::Int64ShiftRightUnsigned => {
            integer64::int64_shift_right_unsigned(context, dst, args)
        }
        Builtin::FloatToString => float_to_string::float_to_string(context, dst, args),
        Builtin::FloatFromInt => float::float_from_int(context, dst, args),
        Builtin::FloatParse => float_parse::float_parse(context, dst, args),
        Builtin::FloatFloor => float::float_floor(context, dst, args),
        Builtin::FloatCeil => float::float_ceil(context, dst, args),
        Builtin::FloatRound => float::float_round(context, dst, args),
        Builtin::FloatTruncate => float::float_truncate(context, dst, args),
        Builtin::BytesFromList => bytes::bytes_from_list(context, dst, args),
        Builtin::BytesToList => bytes::bytes_to_list(context, dst, args),
        Builtin::BytesSize => bytes::bytes_size(context, dst, args),
        Builtin::BytesGet => bytes::bytes_get(context, dst, args),
        Builtin::BytesSlice => bytes::bytes_slice(context, dst, args),
        Builtin::BytesConcat => bytes::bytes_concat(context, dst, args),
        Builtin::BytesEncodeUtf8 => bytes::bytes_encode_utf8(context, dst, args),
        Builtin::BytesDecodeValidUtf8 => bytes::bytes_decode_valid_utf8(context, dst, args),
        Builtin::AssertFail => rejected::assert_fail(context, dst, args),
        Builtin::Panic => rejected::panic(context, dst, args),
        Builtin::Todo => rejected::todo(context, dst, args),
        Builtin::Dbg => rejected::dbg(context, dst, args),
        Builtin::Inspect => rejected::inspect(context, dst, args),
        Builtin::CellNew => cells::cell_new(context, dst, args),
        Builtin::CellGet => cells::cell_get(context, dst, args),
        Builtin::CellSet => cells::cell_set(context, dst, args),
        Builtin::ShowString => rejected::show_string(context, dst, args),
        Builtin::DebugPrint => rejected::debug_print(context, dst, args),
    }
}
