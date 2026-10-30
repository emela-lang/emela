//! crate::contract::ir::Value::FnRef
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let names = context.names;
    match value {
        Value::FnRef(target) => {
            let Some(dst) = dst else {
                return Ok(());
            };
            let Type::Fn(_) = layouts.shapes.types[function.vars[*dst].ty] else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "FnRef destination is not a function value",
                ));
            };
            let name = names.get(target).ok_or_else(|| {
                invalid_flow_ir(
                    &function.name,
                    "FnRef target is not reachable for C emission",
                )
            })?;
            let destination = variable_name(*dst);
            if let Some(&environment) = layouts.functions.closure_functions.get(target) {
                let closure_environment = &layouts.functions.closure_environments[environment];
                if closure_environment.function_type != function.vars[*dst].ty {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "closure function type differs from its closure value type",
                    ));
                }
                let Type::Fn(function_type) = &layouts.shapes.types[function.vars[*dst].ty] else {
                    unreachable!()
                };
                let target_name = if function_type.suspends {
                    format!("{name}_push")
                } else {
                    name.clone()
                };
                writer.assign(&format!("{destination}.call"), &target_name);
                let kind = layouts.closure_environment_kind(environment);
                writer.assign(
                            &format!("{destination}.env"),
                            &format!(
                                "emela_heap_allocate(sizeof(emela_closure_env_{environment}), UINT32_C({kind}))"
                            ),
                        );
                for (capture_index, capture) in closure_environment.captures.iter().enumerate() {
                    if layouts.type_repr(capture.ty) == Repr::Void {
                        continue;
                    }
                    layouts.emit_owned_value_store(
                                capture.ty,
                                &variable_name(capture.source),
                                &format!(
                                    "((emela_closure_env_{environment} *){destination}.env)->c{capture_index}"
                                ),
                                writer,
                            );
                }
            } else {
                let function_type = function.vars[*dst].ty;
                writer.assign(
                    &format!("{destination}.call"),
                    &function_ref_adapter_name(*target, function_type),
                );
                writer.assign(&format!("{destination}.env"), "NULL");
            }
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
