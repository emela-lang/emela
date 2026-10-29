//! Ownership of error payloads and shared failure metadata.
use super::*;

impl TypeLayouts<'_> {
    pub(super) fn has_error_notes(&self) -> bool {
        self.has_scopes && self.has_error_abi
    }

    pub(super) fn has_error_ownership(&self) -> bool {
        self.has_error_notes() || self.managed_error_payloads
    }

    pub(super) fn error_notes_kind(&self) -> u32 {
        self.cause_kind() + 1
    }

    pub(super) fn error_note_kind(&self) -> u32 {
        self.cause_kind() + 2
    }
}

pub(super) fn emit_types(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if layouts.has_error_notes() {
        writer.line("typedef struct emela_error_note { struct emela_error_note *next; uint16_t length; char bytes[]; } emela_error_note;");
        writer.line(
            "typedef struct { emela_error_note *head; emela_error_note *tail; } emela_error_notes;",
        );
    }
}

pub(super) fn emit_ownership_declarations(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.has_error_abi {
        return;
    }
    writer.line("static inline EMELA_UNUSED void emela_error_retain(EmelaError error);");
    writer.line("static inline EMELA_UNUSED void emela_error_own(EmelaError *error);");
    writer.line("static inline EMELA_UNUSED void emela_error_release(EmelaError error);");
    writer.line("static inline EMELA_UNUSED void emela_error_enqueue(EmelaError error, emela_heap_block **pending, emela_heap_block **tail);");
}

pub(super) fn emit_ownership(layouts: &TypeLayouts<'_>, heap: bool, writer: &mut Writer) {
    if !layouts.has_error_abi {
        return;
    }
    writer.begin_block("static inline EMELA_UNUSED void emela_error_own(EmelaError *error)");
    writer.line("(void)error;");
    if heap && layouts.managed_error_payloads {
        writer.begin_block("switch (error->tag)");
        for &(id, ty) in &layouts.error_types {
            if !layouts.is_managed_value(ty) {
                continue;
            }
            writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
            writer.begin_block("");
            layouts.emit_own_value_in_place(
                ty,
                &format!("error->payload.e{}", enum_index(id)),
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        writer.line("default: break;");
        writer.end_block();
    }
    if heap && layouts.has_error_notes() {
        writer.line("emela_heap_retain(error->notes);");
    }
    writer.end_block();
    writer.begin_block("static inline EMELA_UNUSED void emela_error_retain(EmelaError error)");
    writer.line("(void)error;");
    if layouts.managed_error_payloads {
        emit_payloads(layouts, true, writer);
    }
    if layouts.has_error_notes() {
        writer.line("emela_heap_retain(error.notes);");
    }
    writer.end_block();
    if heap {
        writer.begin_block("static inline EMELA_UNUSED void emela_error_enqueue(EmelaError error, emela_heap_block **pending, emela_heap_block **tail)");
        writer.line("(void)error; (void)pending; (void)tail;");
        if layouts.managed_error_payloads {
            emit_payloads(layouts, false, writer);
        }
        if layouts.has_error_notes() {
            writer.line("emela_heap_enqueue(error.notes, pending, tail);");
        }
        writer.end_block();
    }
    writer.begin_block("static inline EMELA_UNUSED void emela_error_release(EmelaError error)");
    if layouts.has_error_ownership() {
        writer.line("emela_heap_block *pending = NULL, *tail = NULL;");
        writer.line("emela_error_enqueue(error, &pending, &tail);");
        writer.line("emela_heap_drain(pending, tail);");
    } else {
        writer.line("(void)error;");
    }
    writer.end_block();
    writer.begin_block("static inline EMELA_UNUSED void emela_error_clear(EmelaError *error)");
    writer.line("EmelaError old = *error; *error = (EmelaError){0}; emela_error_release(old);");
    writer.end_block();
    if layouts.has_error_notes() {
        writer.line("static inline EMELA_UNUSED void emela_error_ensure_notes(EmelaError *error);");
        writer
            .line("static inline EMELA_UNUSED void emela_error_add_note(EmelaError *first, EmelaError other);");
    }
    if layouts.has_resumes {
        writer.line("static _Noreturn void emela_operation_failed_after_resume(const char *duplicate_message, uint16_t duplicate_length, EmelaError error);");
    }
}

fn emit_payloads(layouts: &TypeLayouts<'_>, retain: bool, writer: &mut Writer) {
    writer.begin_block("switch (error.tag)");
    for &(id, ty) in &layouts.error_types {
        if !layouts.is_managed_value(ty) {
            continue;
        }
        writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
        writer.begin_block("");
        let value = format!("error.payload.e{}", enum_index(id));
        if retain {
            layouts.emit_retain_value_expression(ty, &value, writer);
        } else {
            layouts.emit_enqueue_value_expression(ty, &value, "pending", "tail", writer);
        }
        writer.line("break;");
        writer.end_block();
    }
    writer.line("default: break;");
    writer.end_block();
}

pub(super) fn needs_display(layouts: &TypeLayouts<'_>) -> bool {
    layouts.has_error_abi && (layouts.has_causes || layouts.has_scopes || layouts.has_resumes)
}

pub(super) fn emit_display(
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    output_calls: &HashSet<FnId>,
    capacity: u16,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if !needs_display(layouts) {
        return Ok(());
    }
    writer.begin_block("static inline EMELA_UNUSED EmelaString emela_error_to_string(EmelaError error, char *output, uint16_t capacity)");
    writer.line("if (capacity < 7) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
    writer.line("EmelaString text = {0};");
    writer.begin_block("switch (error.tag)");
    for &(error, _) in &layouts.error_types {
        let (id, name) = layouts
            .error_shows
            .iter()
            .find(|(e, _)| *e == error)
            .and_then(|(_, id)| names.get(id).map(|name| (*id, name)))
            .ok_or_else(|| unsupported("error display", "error without a show function"))?;
        let output = if output_calls.contains(&id) {
            ", output + 7, (uint16_t)(capacity - 7)"
        } else {
            ""
        };
        writer.line(&format!(
            "case UINT32_C({}): text = {name}(error.payload.e{}{output}); break;",
            enum_index(error) + 1,
            enum_index(error)
        ));
    }
    writer
        .line("default: emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid error\", UINT16_C(13));");
    writer.end_block();
    writer.line("if (text.length > capacity - 7) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
    writer.line("if (text.data != output + 7) for (uint32_t i = 0; i < text.length; i++) output[7 + i] = text.data[i];");
    writer.line("for (uint32_t i = 0; i < 7; i++) output[i] = \"error: \"[i];");
    writer.line("uint32_t length = 7 + text.length; emela_heap_release(text.owner);");
    if layouts.has_error_notes() {
        const PREFIX: &str = "\nnote: another fiber also failed: ";
        writer.line("emela_error_notes *notes = (emela_error_notes *)error.notes;");
        writer.begin_block("for (emela_error_note *note = notes == NULL ? NULL : notes->head; note != NULL; note = note->next)");
        writer.line(&format!("if (length + {} + note->length > capacity) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));", PREFIX.len()));
        writer.line(&format!(
            "for (uint32_t i = 0; i < {}; i++) output[length++] = {}[i];",
            PREFIX.len(),
            c_string_literal(PREFIX.as_bytes())
        ));
        writer
            .line("for (uint32_t i = 0; i < note->length; i++) output[length++] = note->bytes[i];");
        writer.end_block();
    }
    writer.line("return (EmelaString){.data = output, .length = (uint16_t)length};");
    writer.end_block();
    if layouts.has_resumes {
        const DUPLICATE_PREFIX: &str = "resume called twice in ";
        const SUFFIX: &str = " failed after resume: ";
        writer.begin_block("static _Noreturn void emela_operation_failed_after_resume(const char *duplicate_message, uint16_t duplicate_length, EmelaError error)");
        writer.line(&format!(
            "if (duplicate_length < {}) emela_scheduler_invalid();",
            DUPLICATE_PREFIX.len()
        ));
        writer.line(&format!(
            "uint32_t where_length = duplicate_length - {};",
            DUPLICATE_PREFIX.len()
        ));
        writer.line(&format!(
            "uint32_t prefix_length = where_length + {};",
            SUFFIX.len()
        ));
        writer.line(&format!(
            "uint32_t allocation = prefix_length - 7 + UINT32_C({capacity});"
        ));
        writer.line("if (allocation > UINT16_MAX) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
        writer.line(&format!(
            "char *output = (char *)emela_heap_allocate(allocation, UINT32_C({}));",
            layouts.string_owner_kind()
        ));
        // The prefix overwrites "error: " after formatting, without a second scratch buffer.
        writer.line(&format!("EmelaString text = emela_error_to_string(error, output + prefix_length - 7, UINT16_C({capacity}));"));
        writer.line(&format!(
            "for (uint32_t i = 0; i < where_length; i++) output[i] = duplicate_message[{} + i];",
            DUPLICATE_PREFIX.len()
        ));
        writer.line(&format!(
            "for (uint32_t i = 0; i < {}; i++) output[where_length + i] = {}[i];",
            SUFFIX.len(),
            c_string_literal(SUFFIX.as_bytes())
        ));
        writer.line("emela_board_defect(EMELA_DEFECT_PANIC, output, (uint16_t)(prefix_length + text.length - 7));");
        writer.end_block();
    }
    if layouts.has_error_notes() {
        writer.begin_block(
            "static inline EMELA_UNUSED void emela_error_ensure_notes(EmelaError *error)",
        );
        writer.begin_block("if (error->notes == NULL)");
        writer.line(&format!(
            "error->notes = emela_heap_allocate(sizeof(emela_error_notes), UINT32_C({}));",
            layouts.error_notes_kind()
        ));
        writer.line("*(emela_error_notes *)error->notes = (emela_error_notes){0};");
        writer.end_block();
        writer.end_block();
        writer.begin_block(
            "static inline EMELA_UNUSED void emela_error_add_note(EmelaError *first, EmelaError other)",
        );
        writer.line("emela_error_ensure_notes(first);");
        writer.line(&format!(
            "char *scratch = (char *)emela_heap_allocate(UINT16_C({capacity}), UINT32_C({}));",
            layouts.string_owner_kind()
        ));
        writer.line(&format!(
            "EmelaString text = emela_error_to_string(other, scratch, UINT16_C({capacity}));"
        ));
        writer.line(&format!("emela_error_note *note = (emela_error_note *)emela_heap_allocate(sizeof(*note) + text.length, UINT32_C({}));", layouts.error_note_kind()));
        writer.line("note->next = NULL; note->length = text.length;");
        writer.line("for (uint32_t i = 0; i < text.length; i++) note->bytes[i] = scratch[i];");
        writer.line("emela_heap_release(scratch);");
        writer.line("emela_error_notes *notes = (emela_error_notes *)first->notes;");
        writer.line("if (notes->tail == NULL) notes->head = note; else notes->tail->next = note;");
        writer.line("notes->tail = note;");
        writer.end_block();
    }
    Ok(())
}
