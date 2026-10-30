use super::*;
use crate::contract::strings::StringBuffers;
pub(in crate::compiler::emit) trait StringBufferEmission {
    fn buffer(&self, variable: VarId) -> Option<(String, String)>;
    fn array_capacity(&self, variable: VarId) -> Option<u16>;
}
impl StringBufferEmission for StringBuffers {
    fn array_capacity(&self, variable: VarId) -> Option<u16> {
        self.buffers
            .get(&variable)
            .and_then(|(slot, capacity)| slot.map(|_| *capacity))
    }

    fn buffer(&self, variable: VarId) -> Option<(String, String)> {
        self.buffers
            .get(&variable)
            .map(|(slot, capacity)| match slot {
                Some(slot) => (string_buffer_name(*slot), format!("UINT16_C({capacity})")),
                None => (
                    "emela_string_output".to_owned(),
                    "emela_emb_string_output_capacity".to_owned(),
                ),
            })
    }
}
fn string_buffer_name(slot: usize) -> String {
    format!("emela_string_buffer_{slot}")
}
pub(in crate::compiler::emit) fn emit_string_output_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_copy_to_output(EmelaString value, char *output, uint16_t capacity, uint16_t maximum_length)",
    );
    writer.begin_block(
        "if (value.length > capacity || value.length > maximum_length || (value.length != UINT16_C(0) && (output == NULL || value.data == NULL)))",
    );
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.begin_block("if (value.data != output && value.length != UINT16_C(0))");
    writer.line("const uintptr_t source_address = (uintptr_t)value.data;");
    writer.line("const uintptr_t output_address = (uintptr_t)output;");
    writer.begin_block(
        "if (output_address > source_address && output_address - source_address < (uintptr_t)value.length)",
    );
    writer.line(
        "for (uint32_t index = (uint32_t)value.length; index > UINT32_C(0); --index) output[index - UINT32_C(1)] = value.data[index - UINT32_C(1)];",
    );
    writer.end_block();
    writer.begin_block("else");
    writer.line(
        "for (uint32_t index = UINT32_C(0); index < (uint32_t)value.length; ++index) output[index] = value.data[index];",
    );
    writer.end_block();
    writer.end_block();
    writer.line("return (EmelaString){ .data = output, .length = value.length, .owner = NULL };");
    writer.end_block();
    writer.blank();
}
