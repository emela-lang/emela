use super::*;
use crate::contract::services::Operation;
use std::collections::BTreeSet;
fn snake(name: &str) -> String {
    let mut result = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index != 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

fn qualified(module: &str, item: &str) -> String {
    if item.starts_with(module) {
        item.into()
    } else {
        format!("{module}{item}")
    }
}

fn enum_name(core: &crate::contract::ir::Definitions, id: EnumId) -> Option<String> {
    let rest = core.enums[id].name.strip_prefix("Std.Embedded.")?;
    let (module, item) = rest.split_once('.')?;
    Some(format!("Emela{}", qualified(module, item)))
}

fn board_type(core: &crate::contract::ir::Definitions, ty: TypeId) -> Result<String, Vec<Error>> {
    match &core.types[ty] {
        Type::Int | Type::Bool => Ok("int32_t".into()),
        Type::Int64 => Ok("int64_t".into()),
        Type::Unit => Ok("void".into()),
        Type::Enum(id, args) if args.is_empty() => enum_name(core, *id)
            .ok_or_else(|| unsupported("Std.Embedded.Board", "non-peripheral board type")),
        _ => Err(unsupported(
            "Std.Embedded.Board",
            "unsupported board value type",
        )),
    }
}

fn int_list(core: &crate::contract::ir::Definitions, ty: TypeId) -> bool {
    matches!(core.types[ty], Type::List(element) if element == TypeId::INT)
}

fn field_names(variant: &crate::contract::ir::VariantDef) -> Vec<String> {
    match &variant.fields {
        crate::contract::ir::VariantFields::Named(names) => names.clone(),
        _ => (0..variant.tys.len())
            .map(|index| format!("f{index}"))
            .collect(),
    }
}

fn emit_enum(
    core: &crate::contract::ir::Definitions,
    id: EnumId,
    written: &mut HashSet<EnumId>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if !written.insert(id) {
        return Ok(());
    }
    let def = &core.enums[id];
    let name = enum_name(core, id).ok_or_else(|| unsupported(&def.name, "invalid board type"))?;
    if def.variants.iter().all(|variant| variant.tys.is_empty()) {
        writer.line(&format!("typedef int32_t {name};"));
        writer.line("enum {");
        for (index, variant) in def.variants.iter().enumerate() {
            writer.line(&format!(
                "  EMELA_{}_{} = {index},",
                snake(name.strip_prefix("Emela").unwrap()).to_uppercase(),
                snake(&variant.name).to_uppercase()
            ));
        }
        writer.line("};");
    } else if def.variants.len() == 1 {
        let variant = &def.variants[0];
        for &field in &variant.tys {
            if let Type::Enum(child, _) = core.types[field] {
                emit_enum(core, child, written, writer)?;
            }
        }
        writer.line("typedef struct {");
        for (&ty, field) in variant.tys.iter().zip(field_names(variant)) {
            writer.line(&format!("  {} {field};", board_type(core, ty)?));
        }
        writer.line(&format!("}} {name};"));
    } else {
        return Err(unsupported(&def.name, "unsupported board enum payload"));
    }
    writer.blank();
    Ok(())
}

fn prototype(
    core: &crate::contract::ir::Definitions,
    peripheral: &str,
    op: &crate::contract::ir::OpDef,
) -> Result<String, Vec<Error>> {
    let mut params = Vec::new();
    for (index, &ty) in op.params.iter().enumerate() {
        if int_list(core, ty) {
            params.extend([
                format!("const uint16_t *p{index}"),
                format!("uint16_t p{index}_length"),
            ]);
        } else {
            params.push(format!("{} p{index}", board_type(core, ty)?));
        }
    }
    let returns_list = int_list(core, op.ret);
    let ret = if op.fails {
        if returns_list {
            params.extend([
                "uint16_t *out".into(),
                "uint16_t capacity".into(),
                "uint16_t *length".into(),
            ]);
        } else if op.ret != TypeId::UNIT {
            params.push(format!("{} *out", board_type(core, op.ret)?));
        }
        "int32_t".into()
    } else {
        board_type(core, op.ret)?
    };
    Ok(format!(
        "{ret} emela_{peripheral}_{}({});",
        op.name,
        if params.is_empty() {
            "void".into()
        } else {
            params.join(", ")
        }
    ))
}

pub(super) fn headers(
    flow: &crate::contract::ir::Program,
    operations: &HashMap<FnId, Operation>,
) -> Result<Vec<GeneratedFile>, Vec<Error>> {
    let mut used = BTreeSet::new();
    for operation in operations.values() {
        used.insert(operation.peripheral.clone());
    }
    let mut files = Vec::new();
    if used
        .iter()
        .any(|name| matches!(name.as_str(), "gpio" | "uart" | "i2c" | "spi"))
    {
        files.push(GeneratedFile { path: "emela_peripheral.h".into(), contents: "#ifndef EMELA_PERIPHERAL_H\n#define EMELA_PERIPHERAL_H\n#include <stdint.h>\n/* Reserved bridge errors; board implementations must use different nonzero codes. */\n#define EMELA_PERIPHERAL_INVALID_ARGUMENT (-INT32_C(1))\n#define EMELA_PERIPHERAL_CAPACITY_EXCEEDED (-INT32_C(2))\n#endif\n".into() });
    }
    for peripheral in used {
        let mut writer = Writer::new();
        let guard = format!("EMELA_{}_H", peripheral.to_uppercase());
        writer.line(&format!("#ifndef {guard}"));
        writer.line(&format!("#define {guard}"));
        writer.line("#include <stdint.h>");
        match peripheral.as_str() {
            "clock" => {
                writer.line("#include \"emela_embedded.h\"");
                writer
                    .line("/* Milliseconds since the Unix epoch; the board supplies the epoch. */");
                writer.line("int64_t emela_board_clock_now(void);");
                writer.line("#ifdef EMELA_HAS_TASK_RUNTIME");
                writer.line("/* Complete with emela_task_complete_unit from an ISR; dispatch runs in main. */");
                writer.line("void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds);");
                writer.line("#endif");
            }
            "console" => {
                writer.line(
                    "/* Borrowed UTF-8 bytes; append one newline. Do not retain the pointer. */",
                );
                writer.line(
                    "void emela_board_console_print_line(const char *data, uint16_t length);",
                );
                writer.line(
                    "void emela_board_console_print_error_line(const char *data, uint16_t length);",
                );
            }
            _ => {
                writer.line("#include \"emela_peripheral.h\"");
                writer.line("/* Synchronous, main-context operations. Arrays are borrowed only during the call. */");
                writer.line("/* Return 0 on success. On failure output values are ignored. Never exceed capacity. */");
                writer.line("/* Words preserve 0..65535. The board must reject values outside its configured 7/8/9/16-bit width, never truncate. */");
                let module = match peripheral.as_str() {
                    "gpio" => "Gpio",
                    "uart" => "Uart",
                    "i2c" => "I2c",
                    _ => "Spi",
                };
                let prefix = format!("Std.Embedded.{module}.");
                let mut written = HashSet::new();
                for (id, _) in
                    flow.core.enums.iter().filter(|(_, def)| {
                        def.name.starts_with(&prefix) && !def.error && !def.external
                    })
                {
                    emit_enum(&flow.core, id, &mut written, &mut writer)?;
                }
                let effect_name = format!("{prefix}{module}");
                if let Some((_, effect)) = flow
                    .core
                    .effects
                    .iter()
                    .find(|(_, effect)| effect.name == effect_name)
                {
                    for op in &effect.ops {
                        writer.line(&prototype(&flow.core, &peripheral, op)?);
                    }
                }
            }
        }
        writer.line("#endif");
        files.push(GeneratedFile {
            path: format!("emela_{peripheral}.h"),
            contents: writer.finish(),
        });
    }
    Ok(files)
}

fn to_board(layouts: &TypeLayouts<'_>, ty: TypeId, expression: &str) -> Result<String, Vec<Error>> {
    if let Type::Enum(id, _) = layouts.shapes.types[ty] {
        let def = &layouts.shapes.core.enums[id];
        if !layouts.enum_is_i32(id) {
            let variant = &def.variants[0];
            let fields = variant
                .tys
                .iter()
                .zip(field_names(variant))
                .enumerate()
                .map(|(index, (&field, name))| {
                    Ok(format!(
                        ".{name} = {}",
                        to_board(
                            layouts,
                            field,
                            &format!("({expression}).payload.v0.f{index}")
                        )?
                    ))
                })
                .collect::<Result<Vec<_>, Vec<Error>>>()?;
            return Ok(format!(
                "({}){{ {} }}",
                enum_name(layouts.shapes.core, id).unwrap(),
                fields.join(", ")
            ));
        }
    }
    Ok(expression.into())
}

pub(super) fn emit_adapter(
    flow: &crate::contract::ir::Program,
    op: &Operation,
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    capacity: u16,
    writer: &mut Writer,
) -> Result<bool, Vec<Error>> {
    writer.begin_block(&signature(layouts, function, name, true, false, false));
    let variables: Vec<_> = function.params.iter().map(|&p| variable_name(p)).collect();
    if op.peripheral == "clock" {
        writer.ret(Some("emela_board_clock_now()"));
    } else if op.peripheral == "console" {
        writer.line(&format!(
            "emela_board_console_{}({}.data, {}.length);",
            op.name, variables[0], variables[0]
        ));
    } else {
        let mut args = Vec::new();
        if op.fails {
            writer.decl(&layouts.c_type(function.ret), "result", Some("{0}"));
        }
        for (index, (&ty, variable)) in op.params.iter().zip(&variables).enumerate() {
            if int_list(&flow.core, ty) {
                let node = layouts.list_node_name(ty);
                writer.line(&format!("uint16_t input_{index}[{capacity}];"));
                writer.line(&format!("uint16_t length_{index} = 0;"));
                writer.begin_block(&format!("for (const {node} *cursor = (const {node} *){variable}; cursor != NULL; cursor = (const {node} *)cursor->tail)"));
                writer.line(&format!("if (length_{index} == UINT16_C({capacity})) {{ result.f0 = EMELA_PERIPHERAL_CAPACITY_EXCEEDED; return result; }}"));
                writer.line("if (cursor->head < 0 || cursor->head > INT32_C(65535)) { result.f0 = EMELA_PERIPHERAL_INVALID_ARGUMENT; return result; }");
                writer.line(&format!(
                    "input_{index}[length_{index}++] = (uint16_t)cursor->head;"
                ));
                writer.end_block();
                args.extend([format!("input_{index}"), format!("length_{index}")]);
            } else {
                args.push(to_board(layouts, ty, variable)?);
            }
        }
        let returns_list = int_list(&flow.core, op.ret);
        if returns_list {
            if op.name != "transfer" {
                let size = &variables[variables.len() - 2];
                writer.line(&format!("if ({size} < 0) {{ result.f0 = EMELA_PERIPHERAL_INVALID_ARGUMENT; return result; }}"));
                writer.line(&format!("if ((uint32_t){size} > UINT32_C({capacity})) {{ result.f0 = EMELA_PERIPHERAL_CAPACITY_EXCEEDED; return result; }}"));
            }
            writer.line(&format!("uint16_t output[{capacity}] = {{0}};"));
            writer.line("uint16_t output_length = 0;");
            args.extend([
                "output".into(),
                format!("UINT16_C({capacity})"),
                "&output_length".into(),
            ]);
        } else if op.fails && op.ret != TypeId::UNIT {
            args.push("&result.f1".into());
        }
        let call = format!("emela_{}_{}({})", op.peripheral, op.name, args.join(", "));
        if op.fails {
            writer.assign("result.f0", &call);
            if returns_list {
                writer.line("if (result.f0 != 0) return result;");
                if capacity != u16::MAX {
                    writer.line(&format!("if (output_length > UINT16_C({capacity})) {{ result.f0 = EMELA_PERIPHERAL_CAPACITY_EXCEEDED; return result; }}"));
                }
                if op.name != "transfer" {
                    let size = &variables[variables.len() - 2];
                    writer.line(&format!("if ((uint32_t)output_length > (uint32_t){size}) {{ result.f0 = EMELA_PERIPHERAL_CAPACITY_EXCEEDED; return result; }}"));
                } else {
                    let index = op
                        .params
                        .iter()
                        .position(|&ty| int_list(&flow.core, ty))
                        .unwrap();
                    writer.line(&format!("if (output_length > length_{index}) {{ result.f0 = EMELA_PERIPHERAL_CAPACITY_EXCEEDED; return result; }}"));
                }
                let node = layouts.list_node_name(op.ret);
                let kind = layouts.list_index(op.ret);
                writer.begin_block("for (uint16_t index = output_length; index-- > 0;)");
                writer.line(&format!("{node} *node = ({node} *)emela_heap_allocate(sizeof({node}), UINT32_C({kind}));"));
                writer.line("node->head = (int32_t)output[index];");
                writer.line("node->tail = result.f1;");
                writer.line("result.f1 = node;");
                writer.end_block();
            }
            writer.ret(Some("result"));
        } else if op.ret == TypeId::UNIT {
            writer.line(&format!("{call};"));
        } else {
            writer.ret(Some(&call));
        }
    }
    writer.end_block();
    writer.blank();
    Ok(true)
}
