//! Shared decimal formatting and parsing; no IR types or C emission.
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

pub(crate) const MAX_FLOAT_OUTPUT_BYTES: usize = 25;
