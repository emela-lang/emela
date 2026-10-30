use super::*;
pub(in crate::compiler::emit) fn emit(
    op: PrimOp,
    ty: PrimTy,
    args: &[Operand],
) -> Result<String, &'static str> {
    let args = args
        .iter()
        .map(operand_expression)
        .collect::<Result<Vec<_>, _>>()?;
    match op {
        PrimOp::Add => prim_add(ty, &args),
        PrimOp::Sub => prim_sub(ty, &args),
        PrimOp::Mul => prim_mul(ty, &args),
        PrimOp::Div => prim_div(ty, &args),
        PrimOp::Rem => prim_rem(ty, &args),
        PrimOp::Neg => prim_neg(ty, &args),
        PrimOp::Not => prim_not(ty, &args),
        PrimOp::Eq => prim_eq(ty, &args),
        PrimOp::Ne => prim_ne(ty, &args),
        PrimOp::Lt => prim_lt(ty, &args),
        PrimOp::Le => prim_le(ty, &args),
        PrimOp::Gt => prim_gt(ty, &args),
        PrimOp::Ge => prim_ge(ty, &args),
    }
}

pub(in crate::compiler::emit) fn emit_string_comparison_helper(writer: &mut Writer) {
    writer.begin_block("static inline EMELA_UNUSED int32_t emela_emb_string_compare(const EmelaString *left, const EmelaString *right)");
    // Valid UTF-8 has the same lexicographic byte order as Unicode scalar values.
    writer.line("uint32_t length = left->length < right->length ? left->length : right->length;");
    writer.begin_block("for (uint32_t i = 0; i < length; i++)");
    writer.line("uint8_t a = (uint8_t)left->data[i], b = (uint8_t)right->data[i];");
    writer.line("if (a < b) return -INT32_C(1);");
    writer.line("if (a > b) return INT32_C(1);");
    writer.end_block();
    writer.line("return left->length < right->length ? -INT32_C(1) : left->length > right->length ? INT32_C(1) : INT32_C(0);");
    writer.end_block();
    writer.blank();
}

pub(in crate::compiler::emit) fn emit_i32_division_helpers(writer: &mut Writer) {
    writer
        .begin_block("static inline EMELA_UNUSED int32_t emela_i32_div(int32_t lhs, int32_t rhs)");
    writer.begin_block("if (rhs == INT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT32_MIN && rhs == -INT32_C(1))");
    writer.line("return INT32_MIN;");
    writer.end_block();
    writer.line("return lhs / rhs;");
    writer.end_block();
    writer.blank();

    writer
        .begin_block("static inline EMELA_UNUSED int32_t emela_i32_rem(int32_t lhs, int32_t rhs)");
    writer.begin_block("if (rhs == INT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT32_MIN && rhs == -INT32_C(1))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return lhs % rhs;");
    writer.end_block();
    writer.blank();
}

pub(in crate::compiler::emit) fn emit_i64_division_helpers(writer: &mut Writer) {
    writer
        .begin_block("static inline EMELA_UNUSED int64_t emela_i64_div(int64_t lhs, int64_t rhs)");
    writer.begin_block("if (rhs == INT64_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT64_MIN && rhs == -INT64_C(1))");
    writer.line("return INT64_MIN;");
    writer.end_block();
    writer.line("return lhs / rhs;");
    writer.end_block();
    writer.blank();

    writer
        .begin_block("static inline EMELA_UNUSED int64_t emela_i64_rem(int64_t lhs, int64_t rhs)");
    writer.begin_block("if (rhs == INT64_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT64_MIN && rhs == -INT64_C(1))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return lhs % rhs;");
    writer.end_block();
    writer.blank();
}

type Expression = Result<String, &'static str>;

pub(in crate::compiler::emit) fn prim_add(ty: PrimTy, args: &[String]) -> Expression {
    arithmetic(ty, args, "+")
}

pub(in crate::compiler::emit) fn prim_sub(ty: PrimTy, args: &[String]) -> Expression {
    arithmetic(ty, args, "-")
}

pub(in crate::compiler::emit) fn prim_mul(ty: PrimTy, args: &[String]) -> Expression {
    arithmetic(ty, args, "*")
}

pub(in crate::compiler::emit) fn prim_div(ty: PrimTy, args: &[String]) -> Expression {
    let [lhs, rhs] = binary_args(ty, args)?;
    match ty {
        PrimTy::I32 => Ok(format!("emela_i32_div({lhs}, {rhs})")),
        PrimTy::I64 => Ok(format!("emela_i64_div({lhs}, {rhs})")),
        PrimTy::F64 => Ok(format!("(({lhs}) / ({rhs}))")),
        PrimTy::String => Err("unsupported String primitive"),
    }
}

pub(in crate::compiler::emit) fn prim_rem(ty: PrimTy, args: &[String]) -> Expression {
    let [lhs, rhs] = binary_args(ty, args)?;
    match ty {
        PrimTy::I32 => Ok(format!("emela_i32_rem({lhs}, {rhs})")),
        PrimTy::I64 => Ok(format!("emela_i64_rem({lhs}, {rhs})")),
        PrimTy::F64 => Ok(format!("fmod(({lhs}), ({rhs}))")),
        PrimTy::String => Err("unsupported String primitive"),
    }
}

pub(in crate::compiler::emit) fn prim_neg(ty: PrimTy, args: &[String]) -> Expression {
    // The original String emitter checks binary arity before rejecting the operation.
    if ty == PrimTy::String {
        binary_args(ty, args)?;
        return Err("unsupported String primitive");
    }
    let [value] = args else {
        return Err("Value::Prim with an invalid arity");
    };
    match ty {
        PrimTy::I32 => Ok(format!(
            "emela_i32_from_bits((uint32_t)(UINT64_C(0) - (uint64_t)(uint32_t)({value})))"
        )),
        PrimTy::I64 => Ok(format!(
            "emela_i64_from_bits(UINT64_C(0) - (uint64_t)({value}))"
        )),
        PrimTy::F64 => Ok(format!("(-({value}))")),
        PrimTy::String => unreachable!("String was rejected above"),
    }
}

pub(in crate::compiler::emit) fn prim_not(ty: PrimTy, args: &[String]) -> Expression {
    if ty == PrimTy::String {
        binary_args(ty, args)?;
        return Err("unsupported String primitive");
    }
    match (ty, args) {
        (PrimTy::I32, [value]) => Ok(format!(
            "(({value}) == INT32_C(0) ? INT32_C(1) : INT32_C(0))"
        )),
        _ => Err("Value::Prim with an invalid arity"),
    }
}

pub(in crate::compiler::emit) fn prim_eq(ty: PrimTy, args: &[String]) -> Expression {
    comparison(ty, args, "==")
}

pub(in crate::compiler::emit) fn prim_ne(ty: PrimTy, args: &[String]) -> Expression {
    comparison(ty, args, "!=")
}

pub(in crate::compiler::emit) fn prim_lt(ty: PrimTy, args: &[String]) -> Expression {
    comparison(ty, args, "<")
}

pub(in crate::compiler::emit) fn prim_le(ty: PrimTy, args: &[String]) -> Expression {
    comparison(ty, args, "<=")
}

pub(in crate::compiler::emit) fn prim_gt(ty: PrimTy, args: &[String]) -> Expression {
    comparison(ty, args, ">")
}

pub(in crate::compiler::emit) fn prim_ge(ty: PrimTy, args: &[String]) -> Expression {
    comparison(ty, args, ">=")
}

fn binary_args(ty: PrimTy, args: &[String]) -> Result<&[String; 2], &'static str> {
    args.try_into().map_err(|_| match ty {
        PrimTy::String => "String comparison with an invalid arity",
        _ => "Value::Prim with an invalid arity",
    })
}

fn arithmetic(ty: PrimTy, args: &[String], symbol: &str) -> Expression {
    // Deliberately does not call binary_args: utilities stay within one tier.
    let [lhs, rhs] = args else {
        return Err(match ty {
            PrimTy::String => "String comparison with an invalid arity",
            _ => "Value::Prim with an invalid arity",
        });
    };
    match ty {
        PrimTy::I32 => Ok(format!(
            "emela_i32_from_bits((uint32_t)((uint64_t)(uint32_t)({lhs}) {symbol} (uint64_t)(uint32_t)({rhs})))"
        )),
        PrimTy::I64 => Ok(format!(
            "emela_i64_from_bits((uint64_t)({lhs}) {symbol} (uint64_t)({rhs}))"
        )),
        PrimTy::F64 => Ok(format!("(({lhs}) {symbol} ({rhs}))")),
        PrimTy::String => Err("unsupported String primitive"),
    }
}

fn comparison(ty: PrimTy, args: &[String], symbol: &str) -> Expression {
    let [lhs, rhs] = args else {
        return Err(match ty {
            PrimTy::String => "String comparison with an invalid arity",
            _ => "Value::Prim with an invalid arity",
        });
    };
    if ty == PrimTy::String {
        Ok(format!(
            "((emela_emb_string_compare(&({lhs}), &({rhs}))) {symbol} (INT32_C(0)) ? INT32_C(1) : INT32_C(0))"
        ))
    } else {
        Ok(format!(
            "(({lhs}) {symbol} ({rhs}) ? INT32_C(1) : INT32_C(0))"
        ))
    }
}
