use super::*;
pub(in crate::compiler::emit) fn variant_label(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    key: &CaseKey,
    on: SwitchOn,
) -> Result<String, Vec<Error>> {
    Ok(match (key, on) {
        (CaseKey::Variant(variant), SwitchOn::Tag(value)) => {
            let ty = function.vars[value].ty;
            let Type::Enum(enum_id, _) = layouts.shapes.types[ty] else {
                unreachable!("tag switch was validated above")
            };
            if *variant >= layouts.shapes.core.enums[enum_id].variants.len() {
                return Err(invalid_flow_ir(
                    &function.name,
                    "tag switch variant is out of range",
                ));
            }
            let tag = i32::try_from(*variant).map_err(|_| {
                invalid_flow_ir(&function.name, "tag switch variant does not fit Int32")
            })?;
            int32_literal(tag)
        }
        _ => {
            return Err(invalid_flow_ir(
                &function.name,
                "switch case key does not match its scrutinee",
            ));
        }
    })
}
pub(in crate::compiler::emit) fn lit_label(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    key: &CaseKey,
    on: SwitchOn,
) -> Result<String, Vec<Error>> {
    Ok(match (key, on) {
        (CaseKey::Lit(Const::Int(value)), SwitchOn::Lit(scrutinee))
            if matches!(layouts.shapes.types[function.vars[scrutinee].ty], Type::Int) =>
        {
            int32_literal(*value)
        }
        (CaseKey::Lit(Const::Int64(value)), SwitchOn::Lit(scrutinee))
            if matches!(
                layouts.shapes.types[function.vars[scrutinee].ty],
                Type::Int64
            ) =>
        {
            int64_literal(*value)
        }
        _ => {
            return Err(invalid_flow_ir(
                &function.name,
                "switch case key does not match its scrutinee",
            ));
        }
    })
}
pub(in crate::compiler::emit) fn label(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    key: &CaseKey,
    on: SwitchOn,
) -> Result<String, Vec<Error>> {
    match key {
        CaseKey::Variant(_) => variant_label(layouts, function, key, on),
        CaseKey::Lit(_) => lit_label(layouts, function, key, on),
    }
}
