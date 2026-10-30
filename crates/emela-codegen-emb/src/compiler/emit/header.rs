//! Public C types, runtime interfaces, and exported function declarations.
use super::functions::signature;
use super::{HEADER_FILE, TypeLayouts, external, suspend, task_values};
use crate::contract::ir::{FnId, Program as Module};
use crate::contract::ir::{Type, TypeId};
use crate::{GeneratedFile, writer::Writer};
use std::collections::{HashMap, HashSet};

/// Inputs resolved by the existing module assembly pass. No input is mutated.
pub(super) struct Context<'a, 'ir> {
    pub flow: &'ir Module,
    pub layouts: &'a TypeLayouts<'ir>,
    pub emitted_functions: &'a HashSet<FnId>,
    pub external_bindings: &'a crate::contract::bindings::ExternalBindings,
    pub names: &'a HashMap<FnId, String>,
    pub public_names: &'a HashMap<FnId, String>,
    pub string_output_functions: &'a HashSet<FnId>,
    pub task_value_types: &'a [TypeId],
    pub has_scheduler: bool,
    pub has_tasks: bool,
    pub has_failing_tasks: bool,
    pub heap_required: bool,
    pub entry_declaration: Option<&'static str>,
}

pub(super) fn emit(context: Context<'_, '_>) -> GeneratedFile {
    let Context {
        flow,
        layouts,
        emitted_functions,
        external_bindings,
        names,
        public_names,
        string_output_functions,
        task_value_types,
        has_scheduler,
        has_tasks,
        has_failing_tasks,
        heap_required,
        entry_declaration,
    } = context;
    let mut header = Writer::new();
    header.line("#ifndef EMELA_EMBEDDED_H");
    header.line("#define EMELA_EMBEDDED_H");
    header.blank();
    header.line("#include <stdint.h>");
    header.blank();
    layouts.emit_string_declaration(&mut header);
    header.line(
        "/* Immutable bytes; length is at most INT32_MAX. owner is NULL for borrowed views. */",
    );
    header
        .line("typedef struct { const uint8_t *data; uint32_t length; void *owner; } EmelaBytes;");
    header.blank();
    if has_scheduler
        || layouts
            .shapes
            .function_types
            .iter()
            .any(|&ty| matches!(&layouts.shapes.types[ty], Type::Fn(ft) if ft.suspends))
    {
        header.line("typedef struct emela_fiber emela_fiber;");
    }
    if layouts.runtime.has_error_abi || layouts.runtime.any_abi {
        header.line("typedef struct EmelaError EmelaError;");
        if layouts.runtime.any_abi {
            header.line("typedef struct EmelaError EmelaAny;");
        }
        header.blank();
    }
    layouts.emit_declarations(&mut header);
    external::emit_header(flow, external_bindings, layouts, heap_required, &mut header);
    if has_scheduler {
        header.line(include_str!("runtime/scheduler.h"));
    }
    if has_tasks {
        header.line("#define EMELA_HAS_TASK_RUNTIME 1");
        header.line(include_str!("runtime/task.h"));
        if !task_value_types.is_empty() || has_failing_tasks {
            header.line(include_str!("runtime/task_context.h"));
        }
        if has_failing_tasks {
            header.line(include_str!("runtime/task_failure.h"));
        }
        task_values::emit_header(layouts, task_value_types, &mut header);
        for (id, function) in flow
            .functions
            .iter()
            .filter(|(id, f)| emitted_functions.contains(id) && f.suspends && f.external.is_some())
        {
            if !suspend::scalar(&layouts.shapes.types, function.ret) {
                header.line(&format!("/* Success: emela_task_complete_value_{}. Arguments are borrowed only during start. */", function.ret.index()));
            }
            header.line(&format!(
                "{};",
                suspend::task_signature(layouts, function, &names[&id])
            ));
        }
    }
    if !layouts.shapes.aggregates.is_empty() {
        header.blank();
    }

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported)
    {
        if function.suspends {
            header.line(&format!(
                "{};",
                suspend::start_signature(layouts, function, &public_names[&id])
            ));
            continue;
        }
        let name = &public_names[&id];
        let takes_string_output = string_output_functions.contains(&id);
        if takes_string_output {
            header.line("/* Caller provides this output buffer; the returned EmelaString points into it. */");
        }
        header.line(&format!(
            "{};",
            signature(layouts, function, name, false, false, takes_string_output)
        ));
    }
    if let Some(declaration) = entry_declaration {
        header.line(declaration);
    }
    if heap_required {
        header.line("/* Retains a value allocated from the fixed heap. */");
        header.line("void emela_embedded_retain(void *value);");
        header.line("/* Releases a value allocated from the fixed heap. */");
        header.line("void emela_embedded_release(void *value);");
        header.line("/* Retains the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_retain_string(const EmelaString *value);");
        header.line("/* Releases the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_release_string(EmelaString *value);");
        header.line(
            "/* Retain/release the backing owner of a Bytes value returned by the program. */",
        );
        header.line("void emela_embedded_retain_bytes(const EmelaBytes *value);");
        header.line("void emela_embedded_release_bytes(EmelaBytes *value);");
        layouts.emit_public_release_declarations(&mut header);
        layouts.emit_public_function_ownership_declarations(&mut header);
    }

    header.blank();
    header.line("#endif");

    GeneratedFile {
        path: HEADER_FILE.into(),
        contents: header.finish(),
    }
}

use super::layouts::LayoutEmission;
