//! Byte operations keep unsigned data, checked lengths, and explicit storage ownership.
use super::super::*;

fn is_type(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
    expected: Type,
) -> bool {
    operand_type(function, operand).is_some_and(|ty| layouts.shapes.types[ty] == expected)
}
fn byte(layouts: &TypeLayouts<'_>, function: &Function, args: &[Operand]) -> bool {
    is_type(layouts, function, &args[0], Type::Bytes)
}
fn integer_list(layouts: &TypeLayouts<'_>, function: &Function, args: &[Operand]) -> bool {
    is_type(layouts, function, &args[0], Type::List(TypeId::INT))
}
fn byte_list(layouts: &TypeLayouts<'_>, function: &Function, args: &[Operand]) -> bool {
    is_type(layouts, function, &args[0], Type::List(TypeId::BYTES))
}
fn indexed(layouts: &TypeLayouts<'_>, function: &Function, args: &[Operand]) -> bool {
    byte(layouts, function, args)
        && args[1..]
            .iter()
            .all(|arg| operand_is_int(layouts, function, arg))
}
fn string(layouts: &TypeLayouts<'_>, function: &Function, args: &[Operand]) -> bool {
    operand_is_string(layouts, function, &args[0])
}
fn prepare(
    context: &BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
) -> Result<(VarId, Vec<String>), Vec<Error>> {
    type Check = fn(&TypeLayouts<'_>, &Function, &[Operand]) -> bool;
    let (arity, result, check): (usize, ResultType, Check) = match op {
        Builtin::BytesFromList => (1, ResultType::Option(TypeId::BYTES), integer_list),
        Builtin::BytesToList => (1, ResultType::Exact(Type::List(TypeId::INT)), byte),
        Builtin::BytesSize => (1, ResultType::Exact(Type::Int), byte),
        Builtin::BytesGet => (2, ResultType::Option(TypeId::INT), indexed),
        Builtin::BytesSlice => (3, ResultType::Exact(Type::Bytes), indexed),
        Builtin::BytesConcat => (1, ResultType::Exact(Type::Bytes), byte_list),
        Builtin::BytesEncodeUtf8 => (1, ResultType::Exact(Type::Bytes), string),
        Builtin::BytesDecodeValidUtf8 => (1, ResultType::Exact(Type::String), byte),
        _ => unreachable!("only Byte entries use this signature table"),
    };
    builtin_utilities::prepare(context, dst, op, args, arity, result, check)
}
fn prefix(dst: VarId) -> String {
    format!("emela_bytes_{}", u32::from(dst.into_raw()))
}
fn length_overflow(writer: &mut Writer) {
    writer.line("emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, \"Bytes length exceeds Int\", UINT16_C(24));");
}

pub(in crate::compiler::emit) fn bytes_size(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesSize, args)?;
    context.writer.assign(
        &variable_name(dst),
        &format!("(int32_t)({}).length", expressions[0]),
    );
    Ok(())
}

pub(in crate::compiler::emit) fn bytes_get(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesGet, args)?;
    let p = prefix(dst);
    let ty = context.function.vars[dst].ty;
    let none = option_variant_expression(context.layouts, context.function, ty, "None", None)?;
    let some = option_variant_expression(
        context.layouts,
        context.function,
        ty,
        "Some",
        Some(&format!("(int32_t){p}_input.data[{p}_index]")),
    )?;
    let writer = &mut *context.writer;
    writer.decl("EmelaBytes", &format!("{p}_input"), Some(&expressions[0]));
    writer.decl("int32_t", &format!("{p}_index"), Some(&expressions[1]));
    writer.begin_block(&format!(
        "if ({p}_index < 0 || (uint32_t){p}_index >= {p}_input.length)"
    ));
    writer.assign(&variable_name(dst), &none);
    writer.end_block();
    writer.begin_block("else");
    writer.assign(&variable_name(dst), &some);
    writer.end_block();
    Ok(())
}

pub(in crate::compiler::emit) fn bytes_from_list(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesFromList, args)?;
    let p = prefix(dst);
    let ty = context.function.vars[dst].ty;
    let node = context
        .layouts
        .list_node_name(operand_type(context.function, &args[0]).unwrap());
    let none = option_variant_expression(context.layouts, context.function, ty, "None", None)?;
    let some = option_variant_expression(
        context.layouts,
        context.function,
        ty,
        "Some",
        Some(&format!("{p}_value")),
    )?;
    let writer = &mut *context.writer;
    writer.line(&format!("const {node} *{p}_head = {};", expressions[0]));
    writer.line(&format!("uint32_t {p}_length = 0; int {p}_valid = 1;"));
    writer.begin_block(&format!(
        "for (const {node} *{p}_node={p}_head; {p}_node != NULL; {p}_node={p}_node->tail)"
    ));
    writer.line(&format!(
        "if ({p}_node->head < 0 || {p}_node->head > 255) {{ {p}_valid=0; break; }}"
    ));
    writer.begin_block(&format!("if ({p}_length == (uint32_t)INT32_MAX)"));
    length_overflow(writer);
    writer.end_block();
    writer.line(&format!("++{p}_length;"));
    writer.end_block();
    writer.begin_block(&format!("if (!{p}_valid)"));
    writer.assign(&variable_name(dst), &none);
    writer.end_block();
    writer.begin_block("else");
    writer.line(&format!(
        "EmelaBytes {p}_value=emela_heap_allocate_bytes({p}_length); uint32_t {p}_i=0;"
    ));
    writer.begin_block(&format!(
        "for (const {node} *{p}_node={p}_head; {p}_node != NULL; {p}_node={p}_node->tail)"
    ));
    writer.line(&format!(
        "((uint8_t *){p}_value.owner)[{p}_i++]=(uint8_t){p}_node->head;"
    ));
    writer.end_block();
    writer.assign(&variable_name(dst), &some);
    writer.end_block();
    Ok(())
}

pub(in crate::compiler::emit) fn bytes_to_list(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesToList, args)?;
    let p = prefix(dst);
    let ty = context.function.vars[dst].ty;
    let node = context.layouts.list_node_name(ty);
    let kind = context.layouts.list_index(ty);
    let writer = &mut *context.writer;
    writer.line(&format!(
        "EmelaBytes {p}_input={}; void *{p}_head=NULL; {node} *{p}_last=NULL;",
        expressions[0]
    ));
    writer.begin_block(&format!(
        "for (uint32_t {p}_i=0; {p}_i < {p}_input.length; ++{p}_i)"
    ));
    writer.line(&format!(
        "{node} *{p}_node=({node} *)emela_heap_allocate(sizeof({node}), UINT32_C({kind}));"
    ));
    writer.line(&format!(
        "{p}_node->head=(int32_t){p}_input.data[{p}_i]; {p}_node->tail=NULL;"
    ));
    writer.line(&format!(
        "if ({p}_last == NULL) {p}_head={p}_node; else {p}_last->tail={p}_node;"
    ));
    writer.line(&format!("{p}_last={p}_node;"));
    writer.end_block();
    writer.assign(&variable_name(dst), &format!("{p}_head"));
    Ok(())
}

pub(in crate::compiler::emit) fn bytes_slice(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesSlice, args)?;
    let p = prefix(dst);
    let writer = &mut *context.writer;
    writer.line(&format!(
        "EmelaBytes {p}_input={}; int32_t {p}_from={}, {p}_to={};",
        expressions[0], expressions[1], expressions[2]
    ));
    writer.line(&format!("uint32_t {p}_start={p}_from < 0 ? 0 : (uint32_t){p}_from > {p}_input.length ? {p}_input.length : (uint32_t){p}_from;"));
    writer.line(&format!("uint32_t {p}_end={p}_to < 0 ? 0 : (uint32_t){p}_to > {p}_input.length ? {p}_input.length : (uint32_t){p}_to;"));
    writer.begin_block(&format!("if ({p}_start >= {p}_end)"));
    writer.assign(&variable_name(dst), "(EmelaBytes){0}");
    writer.end_block();
    writer.begin_block("else");
    writer.assign(&variable_name(dst),&format!("emela_heap_copy_bytes((EmelaBytes){{ .data={p}_input.data+{p}_start, .length={p}_end-{p}_start }})"));
    writer.end_block();
    Ok(())
}

pub(in crate::compiler::emit) fn bytes_concat(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesConcat, args)?;
    let p = prefix(dst);
    let node = context
        .layouts
        .list_node_name(operand_type(context.function, &args[0]).unwrap());
    let writer = &mut *context.writer;
    writer.line(&format!(
        "const {node} *{p}_head={}; uint32_t {p}_length=0;",
        expressions[0]
    ));
    writer.begin_block(&format!(
        "for (const {node} *{p}_node={p}_head; {p}_node != NULL; {p}_node={p}_node->tail)"
    ));
    writer.begin_block(&format!(
        "if ({p}_node->head.length > (uint32_t)INT32_MAX-{p}_length)"
    ));
    length_overflow(writer);
    writer.end_block();
    writer.line(&format!("{p}_length+={p}_node->head.length;"));
    writer.end_block();
    writer.line(&format!(
        "EmelaBytes {p}_value=emela_heap_allocate_bytes({p}_length); uint32_t {p}_offset=0;"
    ));
    writer.begin_block(&format!(
        "for (const {node} *{p}_node={p}_head; {p}_node != NULL; {p}_node={p}_node->tail)"
    ));
    writer.begin_block(&format!(
        "for (uint32_t {p}_i=0; {p}_i < {p}_node->head.length; ++{p}_i)"
    ));
    writer.line(&format!(
        "((uint8_t *){p}_value.owner)[{p}_offset++]={p}_node->head.data[{p}_i];"
    ));
    writer.end_block();
    writer.end_block();
    writer.assign(&variable_name(dst), &format!("{p}_value"));
    Ok(())
}

pub(in crate::compiler::emit) fn bytes_encode_utf8(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesEncodeUtf8, args)?;
    let p = prefix(dst);
    let writer = &mut *context.writer;
    writer.decl("EmelaString", &format!("{p}_input"), Some(&expressions[0]));
    writer.assign(&variable_name(dst),&format!("emela_heap_copy_bytes((EmelaBytes){{ .data=(const uint8_t *){p}_input.data, .length={p}_input.length }})"));
    Ok(())
}

pub(in crate::compiler::emit) fn bytes_decode_valid_utf8(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let (dst, expressions) = prepare(context, dst, Builtin::BytesDecodeValidUtf8, args)?;
    let Some((buffer, capacity)) = context.string_buffers.buffer(dst) else {
        return Err(invalid_flow_ir(
            &context.function.name,
            "UTF-8 decoding has no planned String buffer",
        ));
    };
    let p = prefix(dst);
    let writer = &mut *context.writer;
    writer.line(&format!(
        "EmelaBytes {p}_input={}; char *{p}_output={buffer};",
        expressions[0]
    ));
    writer.begin_block(&format!("if ({p}_input.length > (uint32_t)({capacity}) || ({p}_input.length != 0 && {p}_output == NULL))"));
    writer.line("emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
    writer.end_block();
    writer.line(&format!("for (uint32_t {p}_i=0; {p}_i < {p}_input.length; ++{p}_i) {p}_output[{p}_i]=(char){p}_input.data[{p}_i];"));
    writer.assign(
        &variable_name(dst),
        &format!("(EmelaString){{ .data={p}_output, .length=(uint16_t){p}_input.length }}"),
    );
    Ok(())
}
