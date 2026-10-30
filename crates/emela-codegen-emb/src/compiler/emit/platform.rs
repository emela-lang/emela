//! Shared runtime hook declarations. Legacy C names are kept for output/ABI compatibility.
//! Neither these declarations nor their names select a concrete peripheral or board.
use super::{GeneratedFile, Writer};

pub(super) struct Requirements {
    pub defects: bool,
    pub const_defects: bool,
    pub lifecycle: bool,
    pub debug: bool,
    pub event_lock: bool,
}

pub(super) fn header(requirements: Requirements) -> GeneratedFile {
    let mut writer = Writer::new();
    writer.line("#ifndef EMELA_BOARD_H");
    writer.line("#define EMELA_BOARD_H");
    writer.blank();
    writer.line("#include <stdint.h>");
    writer.blank();
    writer.line("typedef enum {");
    for (index, kind) in [
        "PANIC",
        "TODO",
        "DIVISION_BY_ZERO",
        "NO_MATCH",
        "FLOAT_TO_INT",
        "NEGATIVE_SHIFT",
        "HEAP_EXHAUSTED",
        "STRING_CAPACITY",
        "UNSUPPORTED_UNICODE",
        "UNHANDLED_ERROR",
        "ASSERT",
    ]
    .iter()
    .enumerate()
    {
        let suffix = if index == 10 { "" } else { "," };
        writer.line(&format!("  EMELA_DEFECT_{kind}{suffix}"));
    }
    writer.line("} EmelaDefectKind;");
    writer.blank();
    if requirements.defects {
        writer.line(
            "_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length);",
        );
    }
    if requirements.const_defects {
        writer.line("/* Reports a defect through active const contexts; direct board hook calls bypass them. */");
        writer.line("_Noreturn void emela_embedded_report_defect(EmelaDefectKind kind, const char *message, uint16_t length);");
    }
    if requirements.lifecycle {
        writer.line("/* ABI 2: platform initializes, invokes program exactly once, then returns or idles. */");
        writer.line("void emela_platform_start(void (*program)(void));");
        writer.line("void emela_board_idle(void);");
    }
    if requirements.debug {
        writer.line("void emela_board_debug(const char *bytes, uint16_t length);");
    }
    if requirements.event_lock {
        writer.line("/* Save/restore interrupt state and serialize task/event access; callable from ISR. */");
        writer.line("uint32_t emela_board_event_lock(void);");
        writer.line("void emela_board_event_unlock(uint32_t previous_state);");
    }
    writer.blank();
    writer.line("#endif");
    GeneratedFile {
        path: "emela_board.h".into(),
        contents: writer.finish(),
    }
}
