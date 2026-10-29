use crate::writer::Writer;

const HEAP_EXHAUSTED_MESSAGE: &str = "fixed heap exhausted";
const REFERENCE_OVERFLOW_MESSAGE: &str = "heap reference count overflow";
const INVALID_REFERENCE_MESSAGE: &str = "invalid fixed heap reference";

pub(crate) fn emit_runtime(writer: &mut Writer, capacity: u32, cycles: bool) {
    writer.line("typedef struct emela_heap_block emela_heap_block;");
    writer.line("struct emela_heap_block {");
    writer.line("  size_t size;");
    writer.line("  size_t previous_size;");
    writer.line("  emela_heap_block *free_next;");
    writer.line("  emela_heap_block *free_previous;");
    writer.line("  uint32_t references;");
    writer.line("  uint32_t kind;");
    writer.line("  int is_free;");
    if cycles {
        writer.line("  uint32_t gc_references;");
        writer.line("  uint32_t gc_state;");
    }
    writer.line("};");
    writer.line("#define EMELA_HEAP_BIN_COUNT 32");
    writer.line(&format!(
        "_Alignas(max_align_t) static unsigned char emela_heap_storage[{capacity}];"
    ));
    writer.line("static emela_heap_block *emela_heap_bins[EMELA_HEAP_BIN_COUNT];");
    writer.line("static int emela_heap_initialized;");
    writer.blank();
    writer.line(
        "static void emela_heap_destroy(uint32_t kind, void *value, emela_heap_block **pending, emela_heap_block **tail);",
    );
    if cycles {
        writer.line("static int emela_heap_trace_mode;");
        writer.line("static void emela_heap_collect_cycles(void);");
    }
    emit_allocator_functions(writer);
    emit_reference_functions(writer, cycles);
    if cycles {
        emit_cycle_collector(writer);
    }
}

pub(crate) fn emit_public_release(writer: &mut Writer) {
    writer.begin_block("void emela_embedded_release(void *value)");
    writer.line("emela_heap_release(value);");
    writer.end_block();
    writer.blank();
}

pub(crate) fn emit_public_retain(writer: &mut Writer) {
    writer.begin_block("void emela_embedded_retain(void *value)");
    writer.line("emela_heap_retain(value);");
    writer.end_block();
    writer.blank();
}

fn emit_allocator_functions(writer: &mut Writer) {
    writer.begin_block("static size_t emela_heap_alignment(void)");
    writer.line("return _Alignof(max_align_t);");
    writer.end_block();

    writer.begin_block("static size_t emela_heap_header_size(void)");
    writer.line("size_t alignment = emela_heap_alignment();");
    writer.line("size_t size = sizeof(emela_heap_block);");
    writer.line("size_t remainder = size % alignment;");
    writer.line("return remainder == 0 ? size : size + alignment - remainder;");
    writer.end_block();

    writer.begin_block("static unsigned emela_heap_bin(size_t size)");
    writer.line("unsigned index = 0;");
    writer.line("size_t bound = 64;");
    writer.begin_block(
        "while (size > bound && index + 1 < EMELA_HEAP_BIN_COUNT && bound <= SIZE_MAX / 2)",
    );
    writer.line("bound *= 2;");
    writer.line("index++;");
    writer.end_block();
    writer.line("return index;");
    writer.end_block();

    writer.begin_block("static void emela_heap_add_free(emela_heap_block *block)");
    writer.line("unsigned index = emela_heap_bin(block->size);");
    writer.line("block->is_free = 1;");
    writer.line("block->free_previous = NULL;");
    writer.line("block->free_next = emela_heap_bins[index];");
    writer.begin_block("if (block->free_next != NULL)");
    writer.line("block->free_next->free_previous = block;");
    writer.end_block();
    writer.line("emela_heap_bins[index] = block;");
    writer.end_block();

    writer.begin_block("static void emela_heap_remove_free(emela_heap_block *block)");
    writer.line("unsigned index = emela_heap_bin(block->size);");
    writer.begin_block("if (block->free_previous != NULL)");
    writer.line("block->free_previous->free_next = block->free_next;");
    writer.end_block();
    writer.begin_block("else");
    writer.line("emela_heap_bins[index] = block->free_next;");
    writer.end_block();
    writer.begin_block("if (block->free_next != NULL)");
    writer.line("block->free_next->free_previous = block->free_previous;");
    writer.end_block();
    writer.line("block->free_next = NULL;");
    writer.line("block->free_previous = NULL;");
    writer.end_block();

    writer.begin_block("static unsigned char *emela_heap_end(void)");
    writer.line("size_t alignment = emela_heap_alignment();");
    writer.line("size_t capacity = sizeof(emela_heap_storage);");
    writer.line("capacity -= capacity % alignment;");
    writer.line("return emela_heap_storage + capacity;");
    writer.end_block();

    writer.begin_block("static void emela_heap_initialize(void)");
    writer.begin_block("if (emela_heap_initialized)");
    writer.line("return;");
    writer.end_block();
    writer.line("emela_heap_initialized = 1;");
    writer.line("size_t header_size = emela_heap_header_size();");
    writer.line("unsigned char *end = emela_heap_end();");
    writer.line("size_t available = (size_t)(end - emela_heap_storage);");
    writer.begin_block("if (available < header_size + emela_heap_alignment())");
    writer.line("return;");
    writer.end_block();
    writer.line("emela_heap_block *first = (emela_heap_block *)(void *)emela_heap_storage;");
    writer.line("first->size = available;");
    writer.line("first->previous_size = 0;");
    writer.line("first->references = 0;");
    writer.line("first->kind = 0;");
    writer.line("first->free_previous = NULL;");
    writer.line("first->free_next = NULL;");
    writer.line("emela_heap_add_free(first);");
    writer.end_block();

    writer.begin_block("static emela_heap_block *emela_heap_free_block(emela_heap_block *block)");
    writer.line("unsigned char *end = emela_heap_end();");
    writer.line("unsigned char *block_end = (unsigned char *)block + block->size;");
    writer.begin_block("if (block_end < end)");
    writer.line("emela_heap_block *next = (emela_heap_block *)(void *)block_end;");
    writer.begin_block("if (next->is_free)");
    writer.line("emela_heap_remove_free(next);");
    writer.line("block->size += next->size;");
    writer.end_block();
    writer.end_block();
    writer.line("block_end = (unsigned char *)block + block->size;");
    writer.begin_block("if (block_end < end)");
    writer.line("((emela_heap_block *)(void *)block_end)->previous_size = block->size;");
    writer.end_block();
    writer.begin_block("if (block->previous_size != 0)");
    writer.line(
        "emela_heap_block *previous = (emela_heap_block *)(void *)((unsigned char *)block - block->previous_size);",
    );
    writer.begin_block("if (previous->is_free)");
    writer.line("emela_heap_remove_free(previous);");
    writer.line("previous->size += block->size;");
    writer.line("block = previous;");
    writer.end_block();
    writer.end_block();
    writer.line("block_end = (unsigned char *)block + block->size;");
    writer.begin_block("if (block_end < end)");
    writer.line("((emela_heap_block *)(void *)block_end)->previous_size = block->size;");
    writer.end_block();
    writer.line("block->references = 0;");
    writer.line("block->kind = 0;");
    writer.line("emela_heap_add_free(block);");
    writer.line("return block;");
    writer.end_block();

    writer.begin_block("static void *emela_heap_allocate(size_t payload_size, uint32_t kind)");
    writer.line("emela_heap_initialize();");
    writer.line("size_t alignment = emela_heap_alignment();");
    writer.line("size_t header_size = emela_heap_header_size();");
    writer.begin_block("if (payload_size > SIZE_MAX - header_size - alignment)");
    emit_heap_defect(writer, HEAP_EXHAUSTED_MESSAGE);
    writer.end_block();
    writer.line("size_t total_size = header_size + payload_size;");
    writer.line("size_t remainder = total_size % alignment;");
    writer.begin_block("if (remainder != 0)");
    writer.line("total_size += alignment - remainder;");
    writer.end_block();
    writer.line("unsigned first_bin = emela_heap_bin(total_size);");
    writer.begin_block("for (unsigned index = first_bin; index < EMELA_HEAP_BIN_COUNT; index++)");
    writer.begin_block("for (emela_heap_block *block = emela_heap_bins[index]; block != NULL; block = block->free_next)");
    writer.begin_block("if (block->size >= total_size)");
    writer.line("emela_heap_remove_free(block);");
    writer.line("size_t leftover = block->size - total_size;");
    writer.begin_block("if (leftover >= header_size + alignment)");
    writer.line("block->size = total_size;");
    writer.line("unsigned char *split_address = (unsigned char *)block + total_size;");
    writer.line("emela_heap_block *split = (emela_heap_block *)(void *)split_address;");
    writer.line("split->size = leftover;");
    writer.line("split->previous_size = total_size;");
    writer.line("split->references = 0;");
    writer.line("split->kind = 0;");
    writer.line("split->free_next = NULL;");
    writer.line("split->free_previous = NULL;");
    writer.line("emela_heap_add_free(split);");
    writer.line("unsigned char *after_split = split_address + leftover;");
    writer.begin_block("if (after_split < emela_heap_end())");
    writer.line("((emela_heap_block *)(void *)after_split)->previous_size = leftover;");
    writer.end_block();
    writer.end_block();
    writer.line("block->is_free = 0;");
    writer.line("block->references = 1;");
    writer.line("block->kind = kind;");
    writer.line("block->free_next = NULL;");
    writer.line("block->free_previous = NULL;");
    writer.line("return (unsigned char *)block + header_size;");
    writer.end_block();
    writer.end_block();
    writer.end_block();
    emit_heap_defect(writer, HEAP_EXHAUSTED_MESSAGE);
    writer.end_block();
    writer.blank();
}

fn emit_reference_functions(writer: &mut Writer, cycles: bool) {
    writer.begin_block("static emela_heap_block *emela_heap_header(void *value)");
    writer.line(
        "return (emela_heap_block *)(void *)((unsigned char *)value - emela_heap_header_size());",
    );
    writer.end_block();

    writer.begin_block("static inline EMELA_UNUSED void emela_heap_retain(void *value)");
    writer.begin_block("if (value == NULL)");
    writer.line("return;");
    writer.end_block();
    writer.line("emela_heap_block *block = emela_heap_header(value);");
    writer.begin_block("if (block->is_free || block->references == 0)");
    emit_heap_defect(writer, INVALID_REFERENCE_MESSAGE);
    writer.end_block();
    writer.begin_block("if (block->references == UINT32_MAX)");
    emit_heap_defect(writer, REFERENCE_OVERFLOW_MESSAGE);
    writer.end_block();
    writer.line("block->references++;");
    writer.end_block();

    writer.begin_block(
        "static void emela_heap_enqueue(void *value, emela_heap_block **pending, emela_heap_block **tail)",
    );
    writer.begin_block("if (value == NULL)");
    writer.line("return;");
    writer.end_block();
    writer.line("emela_heap_block *block = emela_heap_header(value);");
    writer.begin_block("if (block->is_free || block->references == 0)");
    emit_heap_defect(writer, INVALID_REFERENCE_MESSAGE);
    writer.end_block();
    if cycles {
        writer.begin_block("if (emela_heap_trace_mode == 1)");
        writer.line("if (block->gc_references == 0) emela_board_defect(EMELA_DEFECT_PANIC, \"invalid heap ownership graph\", UINT16_C(28));");
        writer.line("block->gc_references--; return;");
        writer.end_block();
        writer.begin_block("if (emela_heap_trace_mode == 2)");
        writer.line("if (block->gc_state == 0) block->gc_state = 1;");
        writer.line("return;");
        writer.end_block();
        writer.line("if (emela_heap_trace_mode == 3 && block->gc_state == 0) return;");
    }
    writer.line("block->references--;");
    writer.begin_block("if (block->references == 0)");
    writer.line("block->free_next = NULL;");
    writer.begin_block("if (*tail == NULL)");
    writer.line("*pending = block;");
    writer.end_block();
    writer.begin_block("else");
    writer.line("(*tail)->free_next = block;");
    writer.end_block();
    writer.line("*tail = block;");
    writer.end_block();
    writer.end_block();

    writer.begin_block(
        "static void emela_heap_drain(emela_heap_block *pending, emela_heap_block *tail)",
    );
    writer.begin_block("while (pending != NULL)");
    writer.line("emela_heap_block *block = pending;");
    writer.line("pending = block->free_next;");
    writer.begin_block("if (pending == NULL)");
    writer.line("tail = NULL;");
    writer.end_block();
    writer.line("void *payload = (unsigned char *)block + emela_heap_header_size();");
    writer.line("emela_heap_destroy(block->kind, payload, &pending, &tail);");
    writer.line("emela_heap_free_block(block);");
    writer.end_block();
    if cycles {
        writer.line("if (emela_heap_trace_mode == 0) emela_heap_collect_cycles();");
    }
    writer.end_block();

    writer.begin_block("static void emela_heap_release(void *value)");
    writer.line("emela_heap_block *pending = NULL;");
    writer.line("emela_heap_block *tail = NULL;");
    writer.line("emela_heap_enqueue(value, &pending, &tail);");
    writer.line("emela_heap_drain(pending, tail);");
    writer.end_block();
    writer.blank();
}

fn emit_cycle_collector(writer: &mut Writer) {
    // Scratch state lives in fixed heap headers. Refcounts held outside the heap are roots.
    // Edge visits are non-destructive in count/mark modes and never recurse through heap data.
    writer.begin_block("static void emela_heap_collect_cycles(void)");
    writer.line("if (!emela_heap_initialized || emela_heap_trace_mode != 0) return;");
    writer.line("emela_heap_block *pending = NULL, *tail = NULL;");
    writer.line("unsigned char *end = emela_heap_end();");
    writer.begin_block("for (unsigned char *cursor = emela_heap_storage; cursor < end; )");
    writer.line("emela_heap_block *block = (emela_heap_block *)(void *)cursor;");
    writer.line(
        "if (!block->is_free) { block->gc_references = block->references; block->gc_state = 0; }",
    );
    writer.line("cursor += block->size;");
    writer.end_block();
    writer.line("emela_heap_trace_mode = 1;");
    writer.begin_block("for (unsigned char *cursor = emela_heap_storage; cursor < end; )");
    writer.line("emela_heap_block *block = (emela_heap_block *)(void *)cursor;");
    writer.line("if (!block->is_free) emela_heap_destroy(block->kind, cursor + emela_heap_header_size(), &pending, &tail);");
    writer.line("cursor += block->size;");
    writer.end_block();
    writer.begin_block("for (unsigned char *cursor = emela_heap_storage; cursor < end; )");
    writer.line("emela_heap_block *block = (emela_heap_block *)(void *)cursor;");
    writer.line("if (!block->is_free && block->gc_references != 0) block->gc_state = 1;");
    writer.line("cursor += block->size;");
    writer.end_block();
    writer.line("emela_heap_trace_mode = 2;");
    writer.line("int changed;");
    writer.begin_block("do");
    writer.line("changed = 0;");
    writer.begin_block("for (unsigned char *cursor = emela_heap_storage; cursor < end; )");
    writer.line("emela_heap_block *block = (emela_heap_block *)(void *)cursor;");
    writer.begin_block("if (!block->is_free && block->gc_state == 1)");
    writer.line("block->gc_state = 2; changed = 1;");
    writer.line(
        "emela_heap_destroy(block->kind, cursor + emela_heap_header_size(), &pending, &tail);",
    );
    writer.end_block();
    writer.line("cursor += block->size;");
    writer.end_block();
    writer.end_block();
    writer.line("while (changed);");
    writer.line("emela_heap_trace_mode = 3;");
    writer.begin_block("for (unsigned char *cursor = emela_heap_storage; cursor < end; )");
    writer.line("emela_heap_block *block = (emela_heap_block *)(void *)cursor;");
    writer.line("if (!block->is_free && block->gc_state == 0) emela_heap_destroy(block->kind, cursor + emela_heap_header_size(), &pending, &tail);");
    writer.line("cursor += block->size;");
    writer.end_block();
    writer.begin_block("for (unsigned char *cursor = emela_heap_storage; cursor < end; )");
    writer.line("emela_heap_block *block = (emela_heap_block *)(void *)cursor;");
    writer
        .line("if (!block->is_free && block->gc_state == 0) block = emela_heap_free_block(block);");
    writer.line("cursor = (unsigned char *)block + block->size;");
    writer.end_block();
    writer.line("emela_heap_trace_mode = 0;");
    writer.end_block();
    writer.blank();
}

fn emit_heap_defect(writer: &mut Writer, message: &str) {
    writer.line(&format!(
        "emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, {}, UINT16_C({}));",
        c_string_literal(message),
        message.len()
    ));
}

fn c_string_literal(bytes: &str) -> String {
    let mut output = String::from("\"");
    for byte in bytes.bytes() {
        match byte {
            b'\\' => output.push_str("\\\\"),
            b'\"' => output.push_str("\\\""),
            0x20..=0x7e => output.push(char::from(byte)),
            _ => output.push_str(&format!("\\x{byte:02x}")),
        }
    }
    output.push('"');
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    #[test]
    fn fixed_heap_cycle_collection_preserves_roots_and_reclaims_every_iteration() {
        let mut writer = Writer::new();
        writer.line(
            "#include <stddef.h>\n#include <stdint.h>\n#include <assert.h>\n#include <stdlib.h>",
        );
        crate::emit::emit_unused_attribute(&mut writer);
        writer.line("enum { EMELA_DEFECT_HEAP_EXHAUSTED, EMELA_DEFECT_PANIC };");
        writer.line("_Noreturn void emela_board_defect(int kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; abort(); }");
        emit_runtime(&mut writer, 1024, true);
        writer.line("typedef struct node { struct node *edge; int value; } node;");
        writer.line("static void emela_heap_destroy(uint32_t kind, void *value, emela_heap_block **pending, emela_heap_block **tail) { (void)kind; emela_heap_enqueue(((node *)value)->edge, pending, tail); }");
        writer.line(
            r#"
int main(void) {
  for (unsigned iteration = 0; iteration < 1000; iteration++) {
    node *left = emela_heap_allocate(sizeof(node), 0);
    *left = (node){.value = 41};
    node *right = emela_heap_allocate(sizeof(node), 0);
    *right = (node){.edge = left, .value = 42};
    emela_heap_retain(left);
    left->edge = right; emela_heap_retain(right);
    emela_heap_release(right);
    emela_heap_retain(left); emela_heap_release(left);
    assert(left->value == 41 && left->edge->value == 42);
    emela_heap_release(left);
    emela_heap_block *first = (emela_heap_block *)(void *)emela_heap_storage;
    assert(first->is_free && (unsigned char *)first + first->size == emela_heap_end());
  }
  return 0;
}
"#,
        );
        let dir = std::env::temp_dir().join(format!("emela-heap-cycle-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("cycle.c");
        let executable = dir.join("cycle");
        fs::write(&source, writer.finish()).unwrap();
        let output = Command::new("cc")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(Command::new(executable).status().unwrap().success());
        fs::remove_dir_all(dir).unwrap();
    }
}
