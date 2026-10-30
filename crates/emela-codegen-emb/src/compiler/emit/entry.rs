use super::*;
pub(super) fn emit(plan: &PlannedModule<'_>, id: FnId, writer: &mut Writer) {
    let layouts = &plan.layouts;
    let function = &plan.input.lowered.functions[id];
    let name = &plan.public_names[&id];
    let options = &plan.options;
    writer.begin_block("static void emela_program_run(void)");
    if function.suspends {
        let args = if layouts.type_repr(function.ret) == Repr::Void {
            String::new()
        } else {
            writer.decl(&layouts.c_type(function.ret), "result", Some("{0}"));
            "&result".into()
        };
        writer.line(&format!("EmelaFiber main_fiber = {name}_start({args});"));
        writer.begin_block("while (!emela_fiber_done(main_fiber))");
        writer.line("(void)emela_scheduler_run(UINT32_C(64));");
        writer.line("if (!emela_fiber_done(main_fiber)) emela_board_idle();");
        writer.end_block();
        writer.line("emela_fiber_release(main_fiber);");
        if layouts.type_repr(function.ret) != Repr::Void {
            layouts.emit_release_value_expression(function.ret, "result", writer);
        }
    } else {
        let args = if plan.string_output_functions.contains(&id) {
            let capacity = options.string_capacity();
            writer.line(&format!("char output[{capacity}];"));
            format!("output, UINT16_C({capacity})")
        } else {
            String::new()
        };
        if layouts.is_managed_value(function.ret) {
            writer.decl(
                &layouts.c_type(function.ret),
                "result",
                Some(&format!("{name}({args})")),
            );
            layouts.emit_release_value_expression(function.ret, "result", writer);
        } else {
            writer.line(&format!("(void){name}({args});"));
        }
    }
    writer.end_block();
    writer.blank();
    writer.begin_block("void emela_embedded_start(void)");
    writer.line("emela_platform_start(emela_program_run);");
    writer.end_block();
    writer.blank();
}
