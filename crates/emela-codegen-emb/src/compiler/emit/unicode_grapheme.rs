use crate::semantics::unicode_grapheme::{
    EXTENDED_PICTOGRAPHIC_RANGES, GRAPHEME_BREAK_RANGES, INDIC_CONJUNCT_BREAK_RANGES,
};
use crate::writer::Writer;
pub(crate) fn emit_runtime(writer: &mut Writer) {
    writer.line("typedef struct { uint32_t start; uint32_t end_and_value; } EmelaUnicodeRange;");
    writer.blank();
    emit_table(writer, "emela_gcb_ranges", GRAPHEME_BREAK_RANGES);
    emit_table(
        writer,
        "emela_extended_pictographic_ranges",
        EXTENDED_PICTOGRAPHIC_RANGES,
    );
    emit_table(writer, "emela_incb_ranges", INDIC_CONJUNCT_BREAK_RANGES);
    writer.blank();
    writer.begin_block("static inline EMELA_UNUSED uint32_t emela_emb_unicode_lookup(const EmelaUnicodeRange *ranges, uint32_t count, uint32_t code_point)");
    writer.line("uint32_t low = UINT32_C(0);");
    writer.line("uint32_t high = count;");
    writer.begin_block("while (low < high)");
    writer.line("const uint32_t middle = low + ((high - low) / UINT32_C(2));");
    writer.line("const EmelaUnicodeRange range = ranges[middle];");
    writer.line("const uint32_t range_end = range.end_and_value >> 4;");
    writer.begin_block("if (code_point < range.start)");
    writer.line("high = middle;");
    writer.end_block();
    writer.line("else if (code_point > range_end)");
    writer.begin_block("");
    writer.line("low = middle + UINT32_C(1);");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("return range.end_and_value & UINT32_C(0x0f);");
    writer.end_block();
    writer.end_block();
    writer.line("return UINT32_C(0);");
    writer.end_block();
    writer.blank();
    writer.begin_block("static inline EMELA_UNUSED uint32_t emela_emb_gcb(uint32_t code_point)");
    writer.begin_block("if (code_point < UINT32_C(0x80))");
    writer.begin_block("if (code_point == UINT32_C(0x0d))");
    writer.line("return UINT32_C(1);");
    writer.end_block();
    writer.begin_block("if (code_point == UINT32_C(0x0a))");
    writer.line("return UINT32_C(2);");
    writer.end_block();
    writer.begin_block("if (code_point < UINT32_C(0x20) || code_point == UINT32_C(0x7f))");
    writer.line("return UINT32_C(3);");
    writer.end_block();
    writer.line("return UINT32_C(0);");
    writer.end_block();
    writer.begin_block("if (code_point >= UINT32_C(0xac00) && code_point <= UINT32_C(0xd7a3))");
    writer.line("return ((code_point - UINT32_C(0xac00)) % UINT32_C(28)) == UINT32_C(0) ? UINT32_C(12) : UINT32_C(13);");
    writer.end_block();
    writer.line(&format!(
        "return emela_emb_unicode_lookup(emela_gcb_ranges, UINT32_C({}), code_point);",
        GRAPHEME_BREAK_RANGES.len()
    ));
    writer.end_block();
    writer.blank();
    writer.begin_block("static inline EMELA_UNUSED uint32_t emela_emb_incb(uint32_t code_point)");
    writer.begin_block("if (code_point < UINT32_C(0x80))");
    writer.line("return UINT32_C(0);");
    writer.end_block();
    writer.line(&format!(
        "return emela_emb_unicode_lookup(emela_incb_ranges, UINT32_C({}), code_point);",
        INDIC_CONJUNCT_BREAK_RANGES.len()
    ));
    writer.end_block();
    writer.blank();
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_is_extended_pictographic(uint32_t code_point)",
    );
    writer.begin_block("if (code_point < UINT32_C(0x80))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line(&format!("return emela_emb_unicode_lookup(emela_extended_pictographic_ranges, UINT32_C({}), code_point) != UINT32_C(0);", EXTENDED_PICTOGRAPHIC_RANGES.len()));
    writer.end_block();
    writer.blank();
    emit_grapheme_next(writer);
}

fn emit_table(writer: &mut Writer, name: &str, ranges: &[(u32, u32)]) {
    writer.line(&format!(
        "static const EmelaUnicodeRange {name}[{}] = {{",
        ranges.len()
    ));
    for (start, end_and_value) in ranges {
        writer.line(&format!(
            "  {{ UINT32_C(0x{start:06x}), UINT32_C(0x{end_and_value:08x}) }},"
        ));
    }
    writer.line("};");
    writer.blank();
}

fn emit_grapheme_next(writer: &mut Writer) {
    writer.begin_block("static inline EMELA_UNUSED int32_t emela_emb_grapheme_next(EmelaString value, uint32_t start, uint32_t *next)");
    writer.decl("uint32_t", "previous_code_point", None);
    writer.decl("uint32_t", "previous_width", None);
    writer.begin_block(
        "if (!emela_emb_utf8_decode(value, start, &previous_code_point, &previous_width))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.decl(
        "uint32_t",
        "previous_property",
        Some("emela_emb_gcb(previous_code_point)"),
    );
    writer.decl("uint32_t", "offset", Some("start + previous_width"));
    writer.decl(
        "uint32_t",
        "incb_has_linker",
        Some("emela_emb_incb(previous_code_point) == UINT32_C(3)"),
    );
    writer.decl(
        "uint32_t",
        "ep_extend_run",
        Some("emela_emb_is_extended_pictographic(previous_code_point)"),
    );
    writer.decl("uint32_t", "zwj_after_ep", Some("UINT32_C(0)"));
    writer.decl(
        "uint32_t",
        "regional_indicator_run",
        Some("previous_property == UINT32_C(11) ? UINT32_C(1) : UINT32_C(0)"),
    );
    writer.begin_block("while (offset < (uint32_t)value.length)");
    writer.decl("uint32_t", "current_code_point", None);
    writer.decl("uint32_t", "current_width", None);
    writer.begin_block(
        "if (!emela_emb_utf8_decode(value, offset, &current_code_point, &current_width))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.decl(
        "uint32_t",
        "current_property",
        Some("emela_emb_gcb(current_code_point)"),
    );
    writer.decl(
        "uint32_t",
        "current_incb",
        Some("emela_emb_incb(current_code_point)"),
    );
    writer.decl(
        "uint32_t",
        "current_is_ep",
        Some("(uint32_t)emela_emb_is_extended_pictographic(current_code_point)"),
    );
    writer.decl("uint32_t", "should_break", Some("UINT32_C(1)"));
    writer.begin_block("if (previous_property == UINT32_C(1) && current_property == UINT32_C(2))");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if (previous_property == UINT32_C(1) || previous_property == UINT32_C(2) || previous_property == UINT32_C(3))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(1);");
    writer.end_block();
    writer.line("else if (current_property == UINT32_C(1) || current_property == UINT32_C(2) || current_property == UINT32_C(3))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(1);");
    writer.end_block();
    writer.line("else if (previous_property == UINT32_C(7) && (current_property == UINT32_C(7) || current_property == UINT32_C(8) || current_property == UINT32_C(12) || current_property == UINT32_C(13)))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if ((previous_property == UINT32_C(12) || previous_property == UINT32_C(8)) && (current_property == UINT32_C(8) || current_property == UINT32_C(9)))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if ((previous_property == UINT32_C(13) || previous_property == UINT32_C(9)) && current_property == UINT32_C(9))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if (current_property == UINT32_C(4) || current_property == UINT32_C(10))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if (current_property == UINT32_C(6))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if (previous_property == UINT32_C(5))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("/* Unicode 18 UAX #29 revision 49 GB9c. */");
    writer.line("else if (current_incb == UINT32_C(1) && incb_has_linker != UINT32_C(0))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if (previous_property == UINT32_C(10) && zwj_after_ep != UINT32_C(0) && current_is_ep != UINT32_C(0))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.line("else if (previous_property == UINT32_C(11) && current_property == UINT32_C(11) && (regional_indicator_run % UINT32_C(2)) == UINT32_C(1))");
    writer.begin_block("");
    writer.line("should_break = UINT32_C(0);");
    writer.end_block();
    writer.begin_block("if (should_break != UINT32_C(0))");
    writer.assign("(*next)", "offset");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.decl(
        "uint32_t",
        "current_zwj_after_ep",
        Some("current_property == UINT32_C(10) && ep_extend_run != UINT32_C(0)"),
    );
    writer.begin_block("if (current_is_ep != UINT32_C(0))");
    writer.line("ep_extend_run = UINT32_C(1);");
    writer.end_block();
    writer.line("else if (current_property != UINT32_C(4))");
    writer.begin_block("");
    writer.line("ep_extend_run = UINT32_C(0);");
    writer.end_block();
    writer.assign("zwj_after_ep", "current_zwj_after_ep");
    writer.begin_block("if (current_incb == UINT32_C(3))");
    writer.line("incb_has_linker = UINT32_C(1);");
    writer.end_block();
    writer.line("else if (current_incb != UINT32_C(2))");
    writer.begin_block("");
    writer.line("incb_has_linker = UINT32_C(0);");
    writer.end_block();
    writer.begin_block("if (current_property == UINT32_C(11))");
    writer.begin_block("if (previous_property == UINT32_C(11))");
    writer.line("++regional_indicator_run;");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("regional_indicator_run = UINT32_C(1);");
    writer.end_block();
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("regional_indicator_run = UINT32_C(0);");
    writer.end_block();
    writer.assign("previous_code_point", "current_code_point");
    writer.assign("previous_property", "current_property");
    writer.line("offset += current_width;");
    writer.end_block();
    writer.assign("(*next)", "(uint32_t)value.length");
    writer.line("return INT32_C(1);");
    writer.end_block();
}
