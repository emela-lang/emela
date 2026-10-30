//! Shared generated runtime helpers.
use super::*;

pub(in crate::compiler::emit) fn emit_negative_shift_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED void emela_emb_negative_shift_defect(int32_t amount)",
    );
    writer.line("char message[48];");
    writer.line("static const char prefix[] = \"negative shift amount: \";");
    writer.line("uint16_t prefix_length = (uint16_t)(sizeof prefix - 1);");
    writer.line("uint32_t magnitude = (uint32_t)(-(amount + INT32_C(1))) + UINT32_C(1);");
    writer.line("char *digits = message + sizeof message;");
    writer.begin_block("do");
    writer.line("*--digits = (char)('0' + magnitude % UINT32_C(10));");
    writer.line("magnitude /= UINT32_C(10);");
    writer.end_block();
    writer.line("while (magnitude != UINT32_C(0));");
    writer.line("*--digits = '-';");
    writer.line("uint16_t number_length = (uint16_t)((message + sizeof message) - digits);");
    writer.begin_block("for (uint16_t i = 0; i < prefix_length; ++i)");
    writer.line("message[i] = prefix[i];");
    writer.end_block();
    writer.begin_block("for (uint16_t i = 0; i < number_length; ++i)");
    writer.line("message[prefix_length + i] = digits[i];");
    writer.end_block();
    writer.line(
        "emela_board_defect(EMELA_DEFECT_NEGATIVE_SHIFT, message, (uint16_t)(prefix_length + number_length));",
    );
    writer.end_block();
    writer.blank();
}

pub(in crate::compiler::emit) fn emit_i64_wrap_helper(writer: &mut Writer) {
    writer.begin_block("static inline EMELA_UNUSED int64_t emela_i64_from_bits(uint64_t bits)");
    writer.begin_block("if (bits <= (uint64_t)INT64_MAX)");
    writer.line("return (int64_t)bits;");
    writer.end_block();
    writer.line("return -INT64_C(1) - (int64_t)(UINT64_MAX - bits);");
    writer.end_block();
    writer.blank();
}

pub(in crate::compiler::emit) fn emit_i32_from_bits_helper(writer: &mut Writer) {
    writer.begin_block("static inline EMELA_UNUSED int32_t emela_i32_from_bits(uint32_t bits)");
    writer.begin_block("if (bits <= (uint32_t)INT32_MAX)");
    writer.line("return (int32_t)bits;");
    writer.end_block();
    writer.line("return -INT32_C(1) - (int32_t)(UINT32_MAX - bits);");
    writer.end_block();
    writer.blank();
}
