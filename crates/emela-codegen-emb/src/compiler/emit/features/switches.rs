use super::*;
pub(in crate::compiler::emit) fn tag_scrutinee(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    on: SwitchOn,
) -> Result<(String, bool), Vec<Error>> {
    let SwitchOn::Tag(value) = on else {
        unreachable!("switch dispatch")
    };
    Ok({
        let ty = function.vars[value].ty;
        let Type::Enum(enum_id, _) = layouts.shapes.types[ty] else {
            return Err(invalid_flow_ir(
                &function.name,
                "tag switch scrutinee is not an enum",
            ));
        };
        if layouts.enum_is_i32(enum_id) {
            (variable_name(value), true)
        } else if layouts.enum_fields(ty).is_some() {
            (format!("{}.tag", variable_name(value)), true)
        } else {
            return Err(invalid_flow_ir(
                &function.name,
                "tag switch enum has no supported C representation",
            ));
        }
    })
}
pub(in crate::compiler::emit) fn lit_scrutinee(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    on: SwitchOn,
) -> Result<(String, bool), Vec<Error>> {
    let SwitchOn::Lit(value) = on else {
        unreachable!("switch dispatch")
    };
    Ok({
        let ty = function.vars[value].ty;
        if !matches!(layouts.shapes.types[ty], Type::Int | Type::Int64) {
            return Err(invalid_flow_ir(
                &function.name,
                "literal switch scrutinee is not Int or Int64",
            ));
        }
        (variable_name(value), false)
    })
}
pub(in crate::compiler::emit) fn scrutinee(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    on: SwitchOn,
) -> Result<(String, bool), Vec<Error>> {
    match on {
        SwitchOn::Tag(_) => tag_scrutinee(layouts, function, on),
        SwitchOn::Lit(_) => lit_scrutinee(layouts, function, on),
    }
}
