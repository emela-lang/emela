use super::*;
pub(in crate::compiler::emit) fn int_value(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    let crate::contract::ir::Const::Int(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(int32_literal(*value)))
}
pub(in crate::compiler::emit) fn int64_value(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    let crate::contract::ir::Const::Int64(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(int64_literal(*value)))
}
pub(in crate::compiler::emit) fn float_value(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    let crate::contract::ir::Const::Float(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(float_literal(*value)))
}
pub(in crate::compiler::emit) fn bool_value(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    let crate::contract::ir::Const::Bool(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(if *value {
        "INT32_C(1)".to_owned()
    } else {
        "INT32_C(0)".to_owned()
    }))
}
pub(in crate::compiler::emit) fn string_value(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    let crate::contract::ir::Const::String(value) = value else {
        unreachable!("variant dispatch")
    };
    {
        let length = u16::try_from(value.len()).map_err(|_| "String constant is too long")?;
        Ok(Some(format!(
            "(EmelaString){{ .data = {}, .length = UINT16_C({length}) }}",
            c_string_literal(value.as_bytes())
        )))
    }
}
pub(in crate::compiler::emit) fn unit_value(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    let crate::contract::ir::Const::Unit = value else {
        unreachable!("variant dispatch")
    };
    Ok(None)
}
pub(in crate::compiler::emit) fn dispatch(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    match value {
        Const::Int(..) => int_value(value),
        Const::Int64(..) => int64_value(value),
        Const::Float(..) => float_value(value),
        Const::Bool(..) => bool_value(value),
        Const::String(..) => string_value(value),
        Const::Unit => unit_value(value),
    }
}
