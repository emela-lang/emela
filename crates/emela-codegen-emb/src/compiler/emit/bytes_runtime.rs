//! Immutable byte spans own leaf allocations from the fixed heap (`[api.std.bytes]`).
use super::*;

pub(super) fn emit(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    let kind = layouts.string_owner_kind();
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaBytes emela_heap_allocate_bytes(uint32_t length)",
    );
    writer.line("if (length == 0) return (EmelaBytes){0};");
    writer.line("if (length > (uint32_t)INT32_MAX) emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, \"Bytes length exceeds Int\", UINT16_C(24));");
    writer.line(&format!(
        "void *owner = emela_heap_allocate((size_t)length, UINT32_C({kind}));"
    ));
    writer.line(
        "return (EmelaBytes){ .data = (const uint8_t *)owner, .length = length, .owner = owner };",
    );
    writer.end_block();
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaBytes emela_heap_copy_bytes(EmelaBytes value)",
    );
    writer.line("EmelaBytes copy = emela_heap_allocate_bytes(value.length);");
    writer.line(
        "for (uint32_t i = 0; i < value.length; ++i) ((uint8_t *)copy.owner)[i] = value.data[i];",
    );
    writer.line("return copy;");
    writer.end_block();
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaBytes emela_heap_own_bytes(EmelaBytes value)",
    );
    writer.line("if (value.owner == NULL) return emela_heap_copy_bytes(value);");
    writer.line("emela_heap_retain(value.owner);");
    writer.line("return value;");
    writer.end_block();
    writer.begin_block("void emela_embedded_retain_bytes(const EmelaBytes *value)");
    writer.line("if (value != NULL) emela_heap_retain(value->owner);");
    writer.end_block();
    writer.begin_block("void emela_embedded_release_bytes(EmelaBytes *value)");
    writer
        .line("if (value != NULL) { emela_heap_release(value->owner); *value = (EmelaBytes){0}; }");
    writer.end_block();
    writer.blank();
}
