use super::*;
pub(in crate::compiler::emit) fn i32_expression(
    op: PrimOp,
    args: &[Operand],
) -> Result<String, &'static str> {
    primitive_operations::emit(op, PrimTy::I32, args)
}
pub(in crate::compiler::emit) fn i64_expression(
    op: PrimOp,
    args: &[Operand],
) -> Result<String, &'static str> {
    primitive_operations::emit(op, PrimTy::I64, args)
}
pub(in crate::compiler::emit) fn f64_expression(
    op: PrimOp,
    args: &[Operand],
) -> Result<String, &'static str> {
    primitive_operations::emit(op, PrimTy::F64, args)
}
pub(in crate::compiler::emit) fn string_expression(
    op: PrimOp,
    args: &[Operand],
) -> Result<String, &'static str> {
    primitive_operations::emit(op, PrimTy::String, args)
}
pub(in crate::compiler::emit) fn emit(
    op: PrimOp,
    ty: PrimTy,
    args: &[Operand],
) -> Result<String, &'static str> {
    match ty {
        PrimTy::I32 => i32_expression(op, args),
        PrimTy::I64 => i64_expression(op, args),
        PrimTy::F64 => f64_expression(op, args),
        PrimTy::String => string_expression(op, args),
    }
}
