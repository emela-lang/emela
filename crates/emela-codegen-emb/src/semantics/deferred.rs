use crate::Error;
use crate::contract::abi::LayoutBuilder as TypeLayouts;
use crate::contract::ir::*;
use crate::semantics::{
    abi::LayoutRead,
    diagnostics::{invalid_flow_ir, unsupported},
    operands::operand_type,
};
pub(crate) fn value_type(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    handle: &Operand,
) -> Result<TypeId, Vec<Error>> {
    let ty = operand_type(function, handle)
        .ok_or_else(|| invalid_flow_ir(&function.name, "Deferred handle without type"))?;
    match &layouts.shapes.types[ty] {
        Type::Enum(_, args)
            if layouts.is_named_external(ty, "Std.Deferred.Deferred") && args.len() == 1 =>
        {
            Ok(args[0])
        }
        _ => Err(unsupported(
            &function.name,
            "Deferred operation without a concrete Deferred[A] handle",
        )),
    }
}
