use crate::Error;
use crate::contract::abi::LayoutBuilder as TypeLayouts;
use crate::contract::ir::*;
use crate::semantics::{
    abi::LayoutRead,
    diagnostics::{invalid_flow_ir, unsupported},
    operands::operand_type,
};
pub(crate) fn success_type(layouts: &TypeLayouts<'_>, exit: TypeId) -> Option<TypeId> {
    let fields = layouts
        .enum_fields(exit)?
        .get(crate::contract::ir::EXIT_SUCCESS)?;
    if fields.len() == 1 {
        Some(fields[0])
    } else {
        None
    }
}

pub(crate) fn value_type(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    handle: &Operand,
) -> Result<TypeId, Vec<Error>> {
    let ty = operand_type(function, handle)
        .ok_or_else(|| invalid_flow_ir(&function.name, "Fiber handle without type"))?;
    match &layouts.shapes.types[ty] {
        Type::Enum(_, args)
            if layouts.is_named_external(ty, "Std.Fiber.Fiber") && args.len() == 1 =>
        {
            Ok(args[0])
        }
        _ => Err(unsupported(
            &function.name,
            "Fiber operation without a concrete Fiber[A] handle",
        )),
    }
}
