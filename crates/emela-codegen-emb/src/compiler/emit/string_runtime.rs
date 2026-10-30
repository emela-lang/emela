//! Shared generated runtime helpers.
use super::*;

pub(in crate::compiler::emit) fn emit_string_matches_at_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_string_matches_at(EmelaString value, EmelaString needle, uint32_t offset)",
    );
    writer.begin_block(
        "if (offset > (uint32_t)value.length || (uint32_t)needle.length > (uint32_t)value.length - offset)",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("for (uint32_t i = 0; i < (uint32_t)needle.length; ++i)");
    writer.begin_block("if (value.data[offset + i] != needle.data[i])");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.end_block();
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.blank();
}

pub(in crate::compiler::emit) fn emit_utf8_decode_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_utf8_decode(EmelaString value, uint32_t offset, uint32_t *code_point, uint32_t *width)",
    );
    writer.begin_block("if (offset >= (uint32_t)value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("const uint8_t first = (uint8_t)value.data[offset];");
    writer.begin_block("if (first <= UINT8_C(0x7f))");
    writer.line("*code_point = first;");
    writer.line("*width = UINT32_C(1);");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.line("uint32_t count;");
    writer.line("uint32_t decoded;");
    writer.begin_block("if (first >= UINT8_C(0xc2) && first <= UINT8_C(0xdf))");
    writer.line("count = UINT32_C(2);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x1f));");
    writer.end_block();
    writer.line("else if (first >= UINT8_C(0xe0) && first <= UINT8_C(0xef))");
    writer.begin_block("");
    writer.line("count = UINT32_C(3);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x0f));");
    writer.end_block();
    writer.line("else if (first >= UINT8_C(0xf0) && first <= UINT8_C(0xf4))");
    writer.begin_block("");
    writer.line("count = UINT32_C(4);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x07));");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("if (offset + count > (uint32_t)value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("for (uint32_t i = UINT32_C(1); i < count; ++i)");
    writer.line("const uint8_t next = (uint8_t)value.data[offset + i];");
    writer.begin_block("if ((next & UINT8_C(0xc0)) != UINT8_C(0x80))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("decoded = (decoded << 6) | (uint32_t)(next & UINT8_C(0x3f));");
    writer.end_block();
    writer.begin_block(
        "if (count == UINT32_C(3) && (decoded < UINT32_C(0x800) || (decoded >= UINT32_C(0xd800) && decoded <= UINT32_C(0xdfff))))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block(
        "if (count == UINT32_C(4) && (decoded < UINT32_C(0x10000) || decoded > UINT32_C(0x10ffff)))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("*code_point = decoded;");
    writer.line("*width = count;");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.blank();
}

pub(in crate::compiler::emit) fn emit_int_to_string_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_int_to_string(int64_t value, char *buffer, uint16_t capacity)",
    );
    writer.line("uint64_t magnitude = value < INT64_C(0) ? (uint64_t)(-(value + INT64_C(1))) + UINT64_C(1) : (uint64_t)value;");
    writer.line("uint32_t digit_count = UINT32_C(1);");
    writer.begin_block("for (uint64_t remaining = magnitude; remaining >= UINT64_C(10); remaining /= UINT64_C(10))");
    writer.line("++digit_count;");
    writer.end_block();
    writer
        .line("uint32_t length = digit_count + (value < INT64_C(0) ? UINT32_C(1) : UINT32_C(0));");
    writer.begin_block("if (length > (uint32_t)capacity)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("uint32_t cursor = length;");
    writer.begin_block("do");
    writer.line("buffer[--cursor] = (char)('0' + magnitude % UINT64_C(10));");
    writer.line("magnitude /= UINT64_C(10);");
    writer.end_block();
    writer.line("while (magnitude != UINT64_C(0));");
    writer.begin_block("if (value < INT64_C(0))");
    writer.line("buffer[0] = '-';");
    writer.end_block();
    writer.line("return (EmelaString){ .data = buffer, .length = (uint16_t)length };");
    writer.end_block();
    writer.blank();
}
