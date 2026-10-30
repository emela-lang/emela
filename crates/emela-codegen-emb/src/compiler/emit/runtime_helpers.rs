use super::*;
use crate::contract::runtime::{RuntimeHelper as Helper, RuntimeRequirements as Requirements};
pub(in crate::compiler::emit) trait HelperEmission: Sized {
    fn emit(self, writer: &mut Writer);
}
impl HelperEmission for Helper {
    fn emit(self, writer: &mut Writer) {
        match self {
            Self::StringComparison => {
                features::primitive_operations::emit_string_comparison_helper(writer)
            }
            Self::I32FromBits => numeric_runtime::emit_i32_from_bits_helper(writer),
            Self::I32Shift => features::builtins::integer::emit_i32_shift_helpers(writer),
            Self::NegativeShift => numeric_runtime::emit_negative_shift_helper(writer),
            Self::I32Division => features::primitive_operations::emit_i32_division_helpers(writer),
            Self::I64FromBits => numeric_runtime::emit_i64_wrap_helper(writer),
            Self::I64Shift => features::builtins::integer64::emit_i64_shift_helpers(writer),
            Self::I64Division => features::primitive_operations::emit_i64_division_helpers(writer),
            Self::FloatToInt => features::builtins::float::emit_float_to_int_helper(writer),
            Self::DynamicDefect => features::terms::defect::emit_dynamic_defect_kind_helper(writer),
            Self::StringMatchesAt => string_runtime::emit_string_matches_at_helper(writer),
            Self::StringContains => {
                features::builtins::string_contains::emit_string_contains_helper(writer)
            }
            Self::StringStartsWith => {
                features::builtins::string_starts_with::emit_string_starts_with_helper(writer)
            }
            Self::StringEndsWith => {
                features::builtins::string_ends_with::emit_string_ends_with_helper(writer)
            }
            Self::Utf8Decode => string_runtime::emit_utf8_decode_helper(writer),
            Self::Grapheme => super::unicode_grapheme::emit_runtime(writer),
            Self::StringTrim => features::builtins::string_trim::emit_string_trim_helper(writer),
            Self::StringJoin => features::builtins::string_join::emit_string_join_helper(writer),
            Self::StringAppend => {
                features::builtins::string_append::emit_string_append_helper(writer)
            }
            Self::StringShow => features::runtime::emit_string_show_helpers(writer),
            Self::IntToString => string_runtime::emit_int_to_string_helper(writer),
            Self::IntParse => features::builtins::int_parse::emit_int_parse_helper(writer),
            Self::FloatParse => crate::compiler::emit::float_parse::emit_runtime(writer),
            Self::FloatToString => crate::compiler::emit::float_to_string::emit_runtime(writer),
            Self::StringOutput => super::string_buffers::emit_string_output_helper(writer),
        }
    }
}
pub(in crate::compiler::emit) trait RuntimeEmission: Sized {
    fn emit_helpers(&self, writer: &mut Writer);
}
impl RuntimeEmission for Requirements {
    fn emit_helpers(&self, writer: &mut Writer) {
        for &helper in &self.helpers {
            helper.emit(writer);
        }
    }
}
