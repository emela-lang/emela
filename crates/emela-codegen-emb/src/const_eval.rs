use std::collections::HashMap;

use emela_core::Builtin;
use emela_lir::{
    Callee, Const, FnId, Function, Module, Operand, PrimOp, PrimTy, Stmt, Term, Value, VarId,
};

use crate::unicode_grapheme::grapheme_count;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Value(Const),
    Defect { kind: &'static str, message: String },
    SelfReference { message: String },
}

pub(crate) fn evaluate(module: &Module, function: FnId) -> Result<Outcome, &'static str> {
    evaluate_function(module, function, Vec::new(), &mut Vec::new(), &mut 4096)
}

fn evaluate_function(
    module: &Module,
    id: FnId,
    arguments: Vec<Const>,
    active: &mut Vec<FnId>,
    fuel: &mut usize,
) -> Result<Outcome, &'static str> {
    if *fuel == 0 || active.len() >= 64 {
        return Err("static const evaluation budget exhausted");
    }
    *fuel -= 1;
    let function = &module.functions[id];
    if active.contains(&id) {
        if function.constant {
            return Ok(Outcome::SelfReference {
                message: format!(
                    "const {} refers to itself while being evaluated",
                    function.name
                ),
            });
        }
        return Err("const evaluation reached a recursive helper function");
    }
    if arguments.len() != function.params.len() {
        return Err("direct call has an unsupported argument count");
    }
    if function.constant && !arguments.is_empty() {
        return Err("const function has parameters");
    }
    if function.fails || function.suspends || function.external.is_some() {
        return Err("called function has a failure, suspension, or external body");
    }
    let body = function.body.ok_or("called function has no body")?;

    active.push(id);
    let result = evaluate_body(module, function, body, arguments, active, fuel);
    active.pop();
    let outcome = result?;

    if function.constant {
        Ok(match outcome {
            Outcome::Defect { kind, message } => Outcome::Defect {
                kind,
                message: format!("while evaluating const {}: {message}", function.name),
            },
            other => other,
        })
    } else {
        Ok(outcome)
    }
}

fn evaluate_body(
    module: &Module,
    function: &Function,
    body: emela_lir::SeqId,
    arguments: Vec<Const>,
    active: &mut Vec<FnId>,
    fuel: &mut usize,
) -> Result<Outcome, &'static str> {
    let sequence = &function.seqs[body];
    let mut values = HashMap::new();
    for (&param, value) in function.params.iter().zip(arguments) {
        values.insert(param, value);
    }

    for &statement_id in &sequence.stmts {
        let Stmt::Let {
            dst,
            value,
            on_defect: None,
            on_suspend: None,
        } = &function.stmts[statement_id]
        else {
            return Err("const body contains unsupported control flow");
        };

        if *fuel == 0 {
            return Err("static const evaluation budget exhausted");
        }
        *fuel -= 1;
        match evaluate_value(module, value, &values, active, fuel)? {
            Outcome::Value(value) => {
                if let Some(dst) = dst {
                    values.insert(*dst, value);
                }
            }
            outcome => return Ok(outcome),
        }
    }

    match &sequence.term {
        Term::Return(None) => Ok(Outcome::Value(Const::Unit)),
        Term::Return(Some(operand)) => Ok(Outcome::Value(operand_value(operand, &values)?)),
        Term::Defect(operand) => {
            let Const::String(message) = operand_value(operand, &values)? else {
                return Err("const defect message is not a String");
            };
            Ok(Outcome::Defect {
                kind: defect_kind(&message),
                message,
            })
        }
        _ => Err("const body has an unsupported terminator"),
    }
}

fn evaluate_value(
    module: &Module,
    value: &Value,
    values: &HashMap<VarId, Const>,
    active: &mut Vec<FnId>,
    fuel: &mut usize,
) -> Result<Outcome, &'static str> {
    match value {
        Value::Const(value) => Ok(Outcome::Value(value.clone())),
        Value::Prim { op, ty, args } => {
            let args = args
                .iter()
                .map(|arg| operand_value(arg, values))
                .collect::<Result<Vec<_>, _>>()?;
            match evaluate_primitive(*op, *ty, args) {
                Ok(value) => Ok(Outcome::Value(value)),
                Err(message) if message == "division by zero" => Ok(Outcome::Defect {
                    kind: defect_kind(&message),
                    message,
                }),
                Err(_) => Err("const body contains an unsupported primitive"),
            }
        }
        Value::Call {
            callee: Callee::Direct(id),
            args,
            suspends: false,
            on_fail: None,
        } => {
            let arguments = args
                .iter()
                .map(|arg| operand_value(arg, values))
                .collect::<Result<Vec<_>, _>>()?;
            evaluate_function(module, *id, arguments, active, fuel)
        }
        Value::Call { .. } => Err("const body contains an indirect, fallible, or suspending call"),
        Value::Builtin { op, args } => {
            let args = args
                .iter()
                .map(|arg| operand_value(arg, values))
                .collect::<Result<Vec<_>, _>>()?;
            match evaluate_builtin(*op, args) {
                Ok(value) => Ok(Outcome::Value(value)),
                Err("float conversion to Int is out of range") => Ok(Outcome::Defect {
                    kind: "FLOAT_TO_INT",
                    message: "cannot convert Float to Int".to_owned(),
                }),
                Err(reason) => Err(reason),
            }
        }
        _ => Err("const body contains a value that is not statically evaluable"),
    }
}

fn evaluate_builtin(op: Builtin, args: Vec<Const>) -> Result<Const, &'static str> {
    match (op, args.as_slice()) {
        (Builtin::StringAppend, [Const::String(left), Const::String(right)]) => {
            // A few repeated doublings must not allocate exponentially in the compiler.
            if left
                .len()
                .checked_add(right.len())
                .is_none_or(|bytes| bytes > usize::from(u16::MAX))
            {
                return Err("static String evaluation byte budget exhausted");
            }
            Ok(Const::String(format!("{left}{right}")))
        }
        (Builtin::IntToString, [Const::Int(value)]) => Ok(Const::String(value.to_string())),
        (Builtin::Int64ToString, [Const::Int64(value)]) => Ok(Const::String(value.to_string())),
        (Builtin::FloatToString, [Const::Float(value)]) => Ok(Const::String(format_float(*value))),
        (Builtin::FloatFromInt, [Const::Int(value)]) => Ok(Const::Float(f64::from(*value))),
        (Builtin::FloatFloor, [Const::Float(value)]) => float_to_int(value.floor()),
        (Builtin::FloatCeil, [Const::Float(value)]) => float_to_int(value.ceil()),
        (Builtin::FloatRound, [Const::Float(value)]) => float_to_int(value.round()),
        (Builtin::FloatTruncate, [Const::Float(value)]) => float_to_int(value.trunc()),
        (Builtin::StringByteSize, [Const::String(value)]) => i32::try_from(value.len())
            .map(Const::Int)
            .map_err(|_| "String byte size exceeds Int range"),
        (Builtin::StringLength, [Const::String(value)]) => i32::try_from(grapheme_count(value))
            .map(Const::Int)
            .map_err(|_| "String grapheme count exceeds Int range"),
        (Builtin::StringContains, [Const::String(value), Const::String(needle)]) => {
            Ok(Const::Bool(value.contains(needle.as_str())))
        }
        (Builtin::StringStartsWith, [Const::String(value), Const::String(prefix)]) => {
            Ok(Const::Bool(value.starts_with(prefix.as_str())))
        }
        (Builtin::StringEndsWith, [Const::String(value), Const::String(suffix)]) => {
            Ok(Const::Bool(value.ends_with(suffix.as_str())))
        }
        (Builtin::StringTrim, [Const::String(value)]) => Ok(Const::String(
            value
                .trim_matches(|character: char| is_white_space(character as u32))
                .to_owned(),
        )),
        _ => Err("const body contains an unsupported builtin or builtin operand"),
    }
}

pub(crate) fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "Infinity".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".to_owned();
    }
    if value == 0.0 {
        return "0.0".to_owned();
    }

    let raw = format!("{value:?}");
    let (negative, unsigned) = raw
        .strip_prefix('-')
        .map_or((false, raw.as_str()), |value| (true, value));
    let (mantissa, exponent) = unsigned
        .split_once('e')
        .or_else(|| unsigned.split_once('E'))
        .map_or((unsigned, 0), |(mantissa, exponent)| {
            (
                mantissa,
                exponent.parse::<i32>().expect("valid Float exponent"),
            )
        });
    let decimal_position = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let mut digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let leading_zeroes = digits.bytes().take_while(|byte| *byte == b'0').count();
    if leading_zeroes > 0 {
        digits.drain(..leading_zeroes);
    }
    let decimal_position = decimal_position - leading_zeroes as i32;
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }

    let digit_count = digits.len() as i32;
    let mut output = String::with_capacity(24);
    if negative {
        output.push('-');
    }
    if digit_count <= decimal_position && decimal_position <= 21 {
        output.push_str(&digits);
        for _ in digit_count..decimal_position {
            output.push('0');
        }
        output.push_str(".0");
    } else if 0 < decimal_position && decimal_position <= 21 {
        let split = decimal_position as usize;
        output.push_str(&digits[..split]);
        output.push('.');
        output.push_str(&digits[split..]);
    } else if -6 < decimal_position && decimal_position <= 0 {
        output.push_str("0.");
        for _ in decimal_position..0 {
            output.push('0');
        }
        output.push_str(&digits);
    } else {
        output.push(digits.as_bytes()[0] as char);
        if digit_count > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        let exponent = decimal_position - 1;
        output.push('e');
        output.push(if exponent < 0 { '-' } else { '+' });
        output.push_str(&exponent.unsigned_abs().to_string());
    }
    output
}

pub(crate) fn parse_float(value: &str) -> Option<f64> {
    match value {
        "NaN" => return Some(f64::NAN),
        "Infinity" | "+Infinity" => return Some(f64::INFINITY),
        "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }

    let bytes = value.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == integer_start {
        return None;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == fraction_start {
            return None;
        }
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return None;
        }
    }
    (index == bytes.len())
        .then(|| value.parse::<f64>().ok())
        .flatten()
}

fn float_to_int(value: f64) -> Result<Const, &'static str> {
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err("float conversion to Int is out of range");
    }
    Ok(Const::Int(value as i32))
}

fn is_white_space(code_point: u32) -> bool {
    (0x0009..=0x000d).contains(&code_point)
        || matches!(
            code_point,
            0x0020 | 0x0085 | 0x00a0 | 0x1680 | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000
        )
        || (0x2000..=0x200a).contains(&code_point)
}

fn operand_value(operand: &Operand, values: &HashMap<VarId, Const>) -> Result<Const, &'static str> {
    match operand {
        Operand::Const(value) => Ok(value.clone()),
        Operand::Var(variable) => values
            .get(variable)
            .cloned()
            .ok_or("const body reads a value that is not statically available"),
    }
}

fn evaluate_primitive(op: PrimOp, ty: PrimTy, args: Vec<Const>) -> Result<Const, String> {
    if args.len() != op.arity() {
        return Err("invalid primitive arity".to_owned());
    }
    let defect = || "division by zero".to_owned();

    match ty {
        PrimTy::I32 => {
            let lhs = args.first().and_then(as_i32).ok_or("invalid Int operand")?;
            if op == PrimOp::Not {
                if args.len() != 1 || !matches!(args[0], Const::Bool(_)) {
                    return Err("invalid Bool operand".to_owned());
                }
                return Ok(Const::Bool(lhs == 0));
            }
            if op == PrimOp::Neg {
                return Ok(Const::Int(lhs.wrapping_neg()));
            }
            let rhs = args.get(1).and_then(as_i32).ok_or("invalid Int operand")?;
            match op {
                PrimOp::Add => Ok(Const::Int(lhs.wrapping_add(rhs))),
                PrimOp::Sub => Ok(Const::Int(lhs.wrapping_sub(rhs))),
                PrimOp::Mul => Ok(Const::Int(lhs.wrapping_mul(rhs))),
                PrimOp::Div if rhs == 0 => Err(defect()),
                PrimOp::Div if lhs == i32::MIN && rhs == -1 => Ok(Const::Int(i32::MIN)),
                PrimOp::Div => Ok(Const::Int(lhs / rhs)),
                PrimOp::Rem if rhs == 0 => Err(defect()),
                PrimOp::Rem if lhs == i32::MIN && rhs == -1 => Ok(Const::Int(0)),
                PrimOp::Rem => Ok(Const::Int(lhs % rhs)),
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                PrimOp::Lt => Ok(Const::Bool(lhs < rhs)),
                PrimOp::Le => Ok(Const::Bool(lhs <= rhs)),
                PrimOp::Gt => Ok(Const::Bool(lhs > rhs)),
                PrimOp::Ge => Ok(Const::Bool(lhs >= rhs)),
                PrimOp::Neg | PrimOp::Not => unreachable!(),
            }
        }
        PrimTy::I64 => {
            let lhs = args
                .first()
                .and_then(as_i64)
                .ok_or("invalid Int64 operand")?;
            if op == PrimOp::Neg {
                return Ok(Const::Int64(lhs.wrapping_neg()));
            }
            let rhs = args
                .get(1)
                .and_then(as_i64)
                .ok_or("invalid Int64 operand")?;
            match op {
                PrimOp::Add => Ok(Const::Int64(lhs.wrapping_add(rhs))),
                PrimOp::Sub => Ok(Const::Int64(lhs.wrapping_sub(rhs))),
                PrimOp::Mul => Ok(Const::Int64(lhs.wrapping_mul(rhs))),
                PrimOp::Div if rhs == 0 => Err(defect()),
                PrimOp::Div if lhs == i64::MIN && rhs == -1 => Ok(Const::Int64(i64::MIN)),
                PrimOp::Div => Ok(Const::Int64(lhs / rhs)),
                PrimOp::Rem if rhs == 0 => Err(defect()),
                PrimOp::Rem if lhs == i64::MIN && rhs == -1 => Ok(Const::Int64(0)),
                PrimOp::Rem => Ok(Const::Int64(lhs % rhs)),
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                PrimOp::Lt => Ok(Const::Bool(lhs < rhs)),
                PrimOp::Le => Ok(Const::Bool(lhs <= rhs)),
                PrimOp::Gt => Ok(Const::Bool(lhs > rhs)),
                PrimOp::Ge => Ok(Const::Bool(lhs >= rhs)),
                PrimOp::Neg | PrimOp::Not => unreachable!(),
            }
        }
        PrimTy::F64 => {
            let lhs = args
                .first()
                .and_then(as_f64)
                .ok_or("invalid Float operand")?;
            if op == PrimOp::Neg {
                return Ok(Const::Float(-lhs));
            }
            let rhs = args
                .get(1)
                .and_then(as_f64)
                .ok_or("invalid Float operand")?;
            match op {
                PrimOp::Add => Ok(Const::Float(lhs + rhs)),
                PrimOp::Sub => Ok(Const::Float(lhs - rhs)),
                PrimOp::Mul => Ok(Const::Float(lhs * rhs)),
                PrimOp::Div => Ok(Const::Float(lhs / rhs)),
                PrimOp::Rem => Ok(Const::Float(lhs % rhs)),
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                PrimOp::Lt => Ok(Const::Bool(lhs < rhs)),
                PrimOp::Le => Ok(Const::Bool(lhs <= rhs)),
                PrimOp::Gt => Ok(Const::Bool(lhs > rhs)),
                PrimOp::Ge => Ok(Const::Bool(lhs >= rhs)),
                PrimOp::Neg | PrimOp::Not => unreachable!(),
            }
        }
        PrimTy::String => {
            let [Const::String(lhs), Const::String(rhs)] = args.as_slice() else {
                return Err("invalid String operand".to_owned());
            };
            match op {
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                PrimOp::Lt => Ok(Const::Bool(lhs < rhs)),
                PrimOp::Le => Ok(Const::Bool(lhs <= rhs)),
                PrimOp::Gt => Ok(Const::Bool(lhs > rhs)),
                PrimOp::Ge => Ok(Const::Bool(lhs >= rhs)),
                _ => Err("unsupported String primitive".to_owned()),
            }
        }
    }
}

fn as_i32(value: &Const) -> Option<i32> {
    match value {
        Const::Int(value) => Some(*value),
        Const::Bool(value) => Some(i32::from(*value)),
        _ => None,
    }
}

fn as_i64(value: &Const) -> Option<i64> {
    match value {
        Const::Int64(value) => Some(*value),
        _ => None,
    }
}

fn as_f64(value: &Const) -> Option<f64> {
    match value {
        Const::Float(value) => Some(*value),
        _ => None,
    }
}

fn defect_kind(message: &str) -> &'static str {
    if message == "not yet implemented" {
        "TODO"
    } else if message.starts_with("unreachable: no match arm matched") {
        "NO_MATCH"
    } else if message.starts_with("assertion failed") {
        "ASSERT"
    } else if message == "division by zero" {
        "DIVISION_BY_ZERO"
    } else {
        "PANIC"
    }
}

#[cfg(test)]
mod tests {
    use emela_core::Builtin;
    use emela_lir::Const;

    use super::{evaluate_builtin, format_float};

    #[test]
    fn formats_float_edges_with_emela_decimal_notation() {
        for (value, expected) in [
            (0.0, "0.0"),
            (-0.0, "0.0"),
            (0.1, "0.1"),
            (1.0e-6, "0.000001"),
            (1.0e-7, "1e-7"),
            (1.0e20, "100000000000000000000.0"),
            (1.0e21, "1e+21"),
            (f64::from_bits(1), "5e-324"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
        ] {
            assert_eq!(format_float(value), expected);
        }
        assert_eq!(format_float(f64::NAN), "NaN");
    }

    #[test]
    fn evaluates_unicode_string_length_with_the_pinned_grapheme_data() {
        assert_eq!(
            evaluate_builtin(
                Builtin::StringLength,
                vec![Const::String("a\u{0308}\r\n👩‍👩‍👧‍👦".to_owned())],
            ),
            Ok(Const::Int(3))
        );
    }
}
