//! Trusted adapters for the shared Std.Embedded.Board handlers.
use super::*;

const MODULE: &str = "Std.Embedded.Board.";
const SYMBOL: &str = "__emela_peripheral_";

use crate::contract::services::Operation;
pub(super) fn is_binding(function: &Function) -> bool {
    function.name.starts_with(MODULE)
        && function.external.as_ref().is_some_and(|external| {
            external.binding("c").is_some_and(|binding| {
                matches!(&binding.host, crate::contract::ir::HostRef::Global(symbol) if symbol.starts_with(SYMBOL))
            })
        })
}

pub(super) fn resolve(
    flow: &crate::contract::ir::Program,
    function: &Function,
) -> Result<Option<Operation>, Vec<Error>> {
    if !is_binding(function) {
        return Ok(None);
    }
    let item = function.name.strip_prefix(MODULE).unwrap();
    let binding = function.external.as_ref().unwrap().binding("c").unwrap();
    let crate::contract::ir::HostRef::Global(symbol) = &binding.host else {
        unreachable!()
    };
    if symbol != &format!("{SYMBOL}{item}") || function.fails {
        return Err(unsupported(
            &function.name,
            "invalid standard board binding identity",
        ));
    }
    let operation = match item {
        "clock_sleep" => Operation {
            peripheral: "clock".into(),
            name: "sleep".into(),
            params: vec![TypeId::INT],
            ret: TypeId::UNIT,
            fails: false,
        },
        "clock_now" => Operation {
            peripheral: "clock".into(),
            name: "now".into(),
            params: vec![],
            ret: TypeId::INT64,
            fails: false,
        },
        "console_print_line" | "console_print_error_line" => Operation {
            peripheral: "console".into(),
            name: item.strip_prefix("console_").unwrap().into(),
            params: vec![TypeId::STRING],
            ret: TypeId::UNIT,
            fails: false,
        },
        _ => {
            let Some((peripheral, name)) = item.split_once('_') else {
                return Err(unsupported(
                    &function.name,
                    "unknown standard board binding",
                ));
            };
            let module = match peripheral {
                "gpio" => "Gpio",
                "uart" => "Uart",
                "i2c" => "I2c",
                "spi" => "Spi",
                _ => return Err(unsupported(&function.name, "unknown standard peripheral")),
            };
            let effect_name = format!("Std.Embedded.{module}.{module}");
            let Some((_, effect)) = flow
                .core
                .effects
                .iter()
                .find(|(_, effect)| effect.name == effect_name)
            else {
                return Err(unsupported(
                    &function.name,
                    "missing standard peripheral effect",
                ));
            };
            let Some(op) = effect.ops.iter().find(|op| op.name == name) else {
                return Err(unsupported(
                    &function.name,
                    "unknown standard peripheral operation",
                ));
            };
            if op.suspend || op.type_params != 0 {
                return Err(unsupported(
                    &function.name,
                    "unsupported standard peripheral operation signature",
                ));
            }
            Operation {
                peripheral: peripheral.into(),
                name: name.into(),
                params: op.params.clone(),
                ret: op.ret,
                fails: op.fails,
            }
        }
    };
    let params: Vec<_> = function
        .params
        .iter()
        .map(|&p| function.vars[p].ty)
        .collect();
    let return_valid = if operation.fails {
        matches!(&flow.core.types[function.ret], Type::Tuple(fields) if fields == &[TypeId::INT, operation.ret])
    } else {
        function.ret == operation.ret
    };
    if params != operation.params || !return_valid || function.suspends != (item == "clock_sleep") {
        return Err(unsupported(
            &function.name,
            "invalid standard board binding signature",
        ));
    }
    Ok(Some(operation))
}

pub(super) fn task_symbol(function: &Function) -> Option<&'static str> {
    if function.name == "Std.Embedded.Board.clock_sleep" && is_binding(function) {
        let binding = function.external.as_ref()?.binding("c")?;
        if matches!(&binding.host, crate::contract::ir::HostRef::Global(symbol) if symbol == "__emela_peripheral_clock_sleep")
            && !function.fails
            && function.suspends
            && function.ret == TypeId::UNIT
            && function.params.len() == 1
            && function.vars[function.params[0]].ty == TypeId::INT
        {
            return Some("emela_board_clock_sleep");
        }
    }
    None
}
