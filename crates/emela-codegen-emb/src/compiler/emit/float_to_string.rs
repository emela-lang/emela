use crate::writer::Writer;

pub(crate) fn emit_runtime(writer: &mut Writer) {
    for line in RUNTIME.lines() {
        writer.line(line);
    }
    writer.blank();
}

const RUNTIME: &str = r#"
#include <float.h>

#if DBL_MANT_DIG != 53 || DBL_MAX_EXP != 1024 || DBL_MIN_EXP != (-1021)
#error "Float.to_string requires IEEE 754 binary64 double"
#endif

#define EMELA_EMB_FLOAT_FORMAT_LIMBS UINT32_C(48)

typedef struct {
  uint32_t words[EMELA_EMB_FLOAT_FORMAT_LIMBS];
  uint16_t length;
} EmelaEmbFloatFormatBig;

static inline EMELA_UNUSED void emela_emb_float_format_big_set(EmelaEmbFloatFormatBig *value, uint64_t integer) {
  value->words[0] = (uint32_t)integer;
  value->words[1] = (uint32_t)(integer >> 32);
  value->length = value->words[1] == UINT32_C(0) ? UINT16_C(1) : UINT16_C(2);
}

static inline EMELA_UNUSED uint32_t emela_emb_float_format_big_bit_length(const EmelaEmbFloatFormatBig *value) {
  if (value->length == UINT16_C(0)) {
    return UINT32_C(0);
  }
  uint32_t top = value->words[value->length - UINT16_C(1)];
  uint32_t bits = ((uint32_t)value->length - UINT32_C(1)) * UINT32_C(32);
  while (top != UINT32_C(0)) {
    ++bits;
    top >>= 1;
  }
  return bits;
}

static inline EMELA_UNUSED int emela_emb_float_format_big_multiply_small(EmelaEmbFloatFormatBig *value, uint32_t multiplier) {
  uint64_t carry = UINT64_C(0);
  for (uint32_t index = UINT32_C(0); index < (uint32_t)value->length; ++index) {
    const uint64_t product = (uint64_t)value->words[index] * (uint64_t)multiplier + carry;
    value->words[index] = (uint32_t)product;
    carry = product >> 32;
  }
  if (carry != UINT64_C(0)) {
    if ((uint32_t)value->length >= EMELA_EMB_FLOAT_FORMAT_LIMBS) {
      return 0;
    }
    value->words[value->length] = (uint32_t)carry;
    ++value->length;
  }
  return 1;
}

static inline EMELA_UNUSED int emela_emb_float_format_big_shift_left(EmelaEmbFloatFormatBig *value, uint32_t shift) {
  if (shift == UINT32_C(0)) {
    return 1;
  }
  const uint32_t whole_words = shift / UINT32_C(32);
  const uint32_t remaining_bits = shift % UINT32_C(32);
  const uint32_t old_length = (uint32_t)value->length;
  const uint32_t extra_word = remaining_bits == UINT32_C(0) ? UINT32_C(0) : UINT32_C(1);
  if (old_length + whole_words + extra_word > EMELA_EMB_FLOAT_FORMAT_LIMBS) {
    return 0;
  }
  uint32_t top_carry = UINT32_C(0);
  for (uint32_t index = old_length; index > UINT32_C(0); --index) {
    const uint32_t source_index = index - UINT32_C(1);
    uint64_t shifted = (uint64_t)value->words[source_index] << remaining_bits;
    if (remaining_bits != UINT32_C(0) && source_index != UINT32_C(0)) {
      shifted |= value->words[source_index - UINT32_C(1)] >> (UINT32_C(32) - remaining_bits);
    }
    value->words[source_index + whole_words] = (uint32_t)shifted;
    if (source_index == old_length - UINT32_C(1)) {
      top_carry = (uint32_t)(shifted >> 32);
    }
  }
  uint32_t new_length = old_length + whole_words;
  if (top_carry != UINT32_C(0)) {
    value->words[new_length] = top_carry;
    ++new_length;
  }
  volatile uint32_t *words = value->words;
  for (uint32_t index = UINT32_C(0); index < whole_words; ++index) {
    words[index] = UINT32_C(0);
  }
  value->length = (uint16_t)new_length;
  return 1;
}

static inline EMELA_UNUSED uint32_t emela_emb_float_format_big_shifted_word(const EmelaEmbFloatFormatBig *value, uint32_t shift, uint32_t index) {
  const uint32_t whole_words = shift / UINT32_C(32);
  const uint32_t remaining_bits = shift % UINT32_C(32);
  if (index < whole_words) {
    return UINT32_C(0);
  }
  const uint32_t source_index = index - whole_words;
  const uint32_t lower = source_index < (uint32_t)value->length
      ? value->words[source_index]
      : UINT32_C(0);
  if (remaining_bits == UINT32_C(0)) {
    return lower;
  }
  const uint32_t upper = source_index > UINT32_C(0)
          && source_index - UINT32_C(1) < (uint32_t)value->length
      ? value->words[source_index - UINT32_C(1)]
      : UINT32_C(0);
  return (lower << remaining_bits) | (upper >> (UINT32_C(32) - remaining_bits));
}

static inline EMELA_UNUSED int emela_emb_float_format_big_compare_shifted(
    const EmelaEmbFloatFormatBig *left,
    uint32_t left_shift,
    const EmelaEmbFloatFormatBig *right,
    uint32_t right_shift) {
  const uint32_t left_bits = emela_emb_float_format_big_bit_length(left) + left_shift;
  const uint32_t right_bits = emela_emb_float_format_big_bit_length(right) + right_shift;
  if (left_bits < right_bits) {
    return -1;
  }
  if (left_bits > right_bits) {
    return 1;
  }
  const uint32_t word_count = (left_bits + UINT32_C(31)) / UINT32_C(32);
  for (uint32_t index = word_count; index > UINT32_C(0); --index) {
    const uint32_t left_word = emela_emb_float_format_big_shifted_word(left, left_shift, index - UINT32_C(1));
    const uint32_t right_word = emela_emb_float_format_big_shifted_word(right, right_shift, index - UINT32_C(1));
    if (left_word < right_word) {
      return -1;
    }
    if (left_word > right_word) {
      return 1;
    }
  }
  return 0;
}

static inline EMELA_UNUSED void emela_emb_float_format_big_subtract_shifted(
    EmelaEmbFloatFormatBig *left,
    const EmelaEmbFloatFormatBig *right,
    uint32_t right_shift) {
  uint64_t borrow = UINT64_C(0);
  for (uint32_t index = UINT32_C(0); index < (uint32_t)left->length; ++index) {
    const uint64_t subtrahend = (uint64_t)emela_emb_float_format_big_shifted_word(right, right_shift, index) + borrow;
    const uint64_t minuend = (uint64_t)left->words[index];
    left->words[index] = (uint32_t)(minuend - subtrahend);
    borrow = minuend < subtrahend ? UINT64_C(1) : UINT64_C(0);
  }
  while (left->length != UINT16_C(0)
      && left->words[left->length - UINT16_C(1)] == UINT32_C(0)) {
    --left->length;
  }
}

static inline EMELA_UNUSED int emela_emb_float_format_big_power5(EmelaEmbFloatFormatBig *value, uint32_t exponent) {
  for (uint32_t index = UINT32_C(0); index < exponent; ++index) {
    if (!emela_emb_float_format_big_multiply_small(value, UINT32_C(5))) {
      return 0;
    }
  }
  return 1;
}

static inline EMELA_UNUSED int emela_emb_float_format_big_compare_binary(
    const EmelaEmbFloatFormatBig *left,
    int32_t left_exponent,
    const EmelaEmbFloatFormatBig *right,
    int32_t right_exponent) {
  if (left_exponent >= right_exponent) {
    return emela_emb_float_format_big_compare_shifted(
        left,
        (uint32_t)(left_exponent - right_exponent),
        right,
        UINT32_C(0));
  }
  return emela_emb_float_format_big_compare_shifted(
      left,
      UINT32_C(0),
      right,
      (uint32_t)(right_exponent - left_exponent));
}

static inline EMELA_UNUSED int emela_emb_float_format_compare_decimal_binary(
    uint64_t decimal,
    int32_t decimal_exponent,
    uint64_t binary,
    int32_t binary_exponent) {
  EmelaEmbFloatFormatBig left;
  EmelaEmbFloatFormatBig right;
  emela_emb_float_format_big_set(&left, decimal);
  emela_emb_float_format_big_set(&right, binary);
  int32_t left_exponent = INT32_C(0);
  int32_t right_exponent = binary_exponent;
  if (decimal_exponent >= INT32_C(0)) {
    if (!emela_emb_float_format_big_power5(&left, (uint32_t)decimal_exponent)) {
      return 0;
    }
    left_exponent = decimal_exponent;
  } else {
    const uint32_t magnitude = (uint32_t)(-decimal_exponent);
    if (!emela_emb_float_format_big_power5(&right, magnitude)) {
      return 0;
    }
    right_exponent += (int32_t)magnitude;
  }
  return emela_emb_float_format_big_compare_binary(&left, left_exponent, &right, right_exponent);
}

static inline EMELA_UNUSED int emela_emb_float_format_round_scaled(
    uint64_t mantissa,
    int32_t binary_exponent,
    int32_t decimal_exponent,
    uint64_t *rounded) {
  EmelaEmbFloatFormatBig numerator;
  EmelaEmbFloatFormatBig denominator;
  emela_emb_float_format_big_set(&numerator, mantissa);
  emela_emb_float_format_big_set(&denominator, UINT64_C(1));
  int32_t two_exponent = binary_exponent;
  if (decimal_exponent >= INT32_C(0)) {
    if (!emela_emb_float_format_big_power5(&denominator, (uint32_t)decimal_exponent)) {
      return 0;
    }
    two_exponent -= decimal_exponent;
  } else {
    const uint32_t magnitude = (uint32_t)(-decimal_exponent);
    if (!emela_emb_float_format_big_power5(&numerator, magnitude)) {
      return 0;
    }
    two_exponent += (int32_t)magnitude;
  }
  if (two_exponent >= INT32_C(0)) {
    if (!emela_emb_float_format_big_shift_left(&numerator, (uint32_t)two_exponent)) {
      return 0;
    }
  } else if (!emela_emb_float_format_big_shift_left(&denominator, (uint32_t)(-two_exponent))) {
    return 0;
  }

  const int32_t shift = (int32_t)emela_emb_float_format_big_bit_length(&numerator)
      - (int32_t)emela_emb_float_format_big_bit_length(&denominator);
  if (shift >= INT32_C(64)) {
    return 0;
  }
  uint64_t quotient = UINT64_C(0);
  for (int32_t bit = shift; bit >= INT32_C(0); --bit) {
    if (emela_emb_float_format_big_compare_shifted(
            &numerator,
            UINT32_C(0),
            &denominator,
            (uint32_t)bit)
        >= 0) {
      emela_emb_float_format_big_subtract_shifted(&numerator, &denominator, (uint32_t)bit);
      quotient |= UINT64_C(1) << (uint32_t)bit;
    }
  }
  const int midpoint = emela_emb_float_format_big_compare_shifted(
      &numerator,
      UINT32_C(1),
      &denominator,
      UINT32_C(0));
  if (midpoint > 0 || (midpoint == 0 && (quotient & UINT64_C(1)) != UINT64_C(0))) {
    ++quotient;
  }
  *rounded = quotient;
  return 1;
}

static inline EMELA_UNUSED void emela_emb_float_format_put(
    char *buffer,
    uint16_t capacity,
    uint16_t *length,
    char value) {
  if (buffer == NULL || *length >= capacity) {
    static const char message[] = "string capacity exceeded";
    emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, message, (uint16_t)(sizeof(message) - 1U));
  }
  buffer[*length] = value;
  ++*length;
}

static inline EMELA_UNUSED void emela_emb_float_format_put_text(
    char *buffer,
    uint16_t capacity,
    uint16_t *length,
    const char *value) {
  for (uint32_t index = UINT32_C(0); value[index] != '\0'; ++index) {
    emela_emb_float_format_put(buffer, capacity, length, value[index]);
  }
}

static inline EMELA_UNUSED void emela_emb_float_format_internal_error(void) {
  static const char message[] = "Float.to_string conversion workspace overflow";
  emela_board_defect(EMELA_DEFECT_PANIC, message, (uint16_t)(sizeof(message) - 1U));
}

static inline EMELA_UNUSED void emela_emb_float_format_no_candidate(void) {
  static const char message[] = "Float.to_string found no shortest decimal";
  emela_board_defect(EMELA_DEFECT_PANIC, message, (uint16_t)(sizeof(message) - 1U));
}

static inline EMELA_UNUSED EmelaString emela_emb_float_to_string(double value, char *buffer, uint16_t capacity) {
  uint16_t length = UINT16_C(0);
  union {
    double value;
    uint64_t bits;
  } converted;
  converted.value = value;
  const uint64_t bits = converted.bits;
  const uint64_t sign = bits >> UINT64_C(63);
  const uint32_t exponent_bits = (uint32_t)((bits >> UINT64_C(52)) & UINT64_C(0x7ff));
  const uint64_t fraction = bits & UINT64_C(0x000fffffffffffff);
  if (exponent_bits == UINT32_C(0x7ff)) {
    if (fraction != UINT64_C(0)) {
      emela_emb_float_format_put_text(buffer, capacity, &length, "NaN");
    } else if (sign != UINT64_C(0)) {
      emela_emb_float_format_put_text(buffer, capacity, &length, "-Infinity");
    } else {
      emela_emb_float_format_put_text(buffer, capacity, &length, "Infinity");
    }
    return (EmelaString){ .data = buffer, .length = length, .owner = NULL };
  }
  const uint64_t mantissa = exponent_bits == UINT32_C(0)
      ? fraction
      : fraction | UINT64_C(0x0010000000000000);
  if (mantissa == UINT64_C(0)) {
    emela_emb_float_format_put_text(buffer, capacity, &length, "0.0");
    return (EmelaString){ .data = buffer, .length = length, .owner = NULL };
  }
  if (sign != UINT64_C(0)) {
    emela_emb_float_format_put(buffer, capacity, &length, '-');
  }

  const int32_t binary_exponent = exponent_bits == UINT32_C(0)
      ? -INT32_C(1074)
      : (int32_t)exponent_bits - INT32_C(1023) - INT32_C(52);
  int32_t binary_order = binary_exponent;
  uint64_t order_mantissa = mantissa;
  while (order_mantissa != UINT64_C(0)) {
    ++binary_order;
    order_mantissa >>= 1;
  }
  --binary_order;
  const int32_t product = binary_order * INT32_C(30103);
  int32_t decimal_order = product / INT32_C(100000);
  if (product < INT32_C(0) && product % INT32_C(100000) != INT32_C(0)) {
    --decimal_order;
  }
  while (emela_emb_float_format_compare_decimal_binary(
             UINT64_C(1), decimal_order, mantissa, binary_exponent)
      > 0) {
    --decimal_order;
  }
  while (emela_emb_float_format_compare_decimal_binary(
             UINT64_C(1), decimal_order + INT32_C(1), mantissa, binary_exponent)
      <= 0) {
    ++decimal_order;
  }

  const uint64_t lower_mantissa = exponent_bits != UINT32_C(0)
          && exponent_bits != UINT32_C(1)
          && fraction == UINT64_C(0)
      ? mantissa * UINT64_C(4) - UINT64_C(1)
      : mantissa * UINT64_C(2) - UINT64_C(1);
  const int32_t lower_exponent = exponent_bits != UINT32_C(0)
          && exponent_bits != UINT32_C(1)
          && fraction == UINT64_C(0)
      ? binary_exponent - INT32_C(2)
      : binary_exponent - INT32_C(1);
  const uint64_t upper_mantissa = mantissa * UINT64_C(2) + UINT64_C(1);
  const int32_t upper_exponent = binary_exponent - INT32_C(1);
  const int inclusive = (mantissa & UINT64_C(1)) == UINT64_C(0);
  uint64_t decimal = UINT64_C(0);
  int32_t decimal_exponent = INT32_C(0);
  int found = 0;
  for (int32_t digits = INT32_C(1); digits <= INT32_C(17); ++digits) {
    const int32_t candidate_exponent = decimal_order - digits + INT32_C(1);
    uint64_t candidate;
    if (!emela_emb_float_format_round_scaled(
            mantissa, binary_exponent, candidate_exponent, &candidate)) {
      emela_emb_float_format_internal_error();
    }
    const int lower = emela_emb_float_format_compare_decimal_binary(
        candidate, candidate_exponent, lower_mantissa, lower_exponent);
    const int upper = emela_emb_float_format_compare_decimal_binary(
        candidate, candidate_exponent, upper_mantissa, upper_exponent);
    if ((lower > 0 || (inclusive && lower == 0))
        && (upper < 0 || (inclusive && upper == 0))) {
      decimal = candidate;
      decimal_exponent = candidate_exponent;
      found = 1;
      break;
    }
  }
  if (!found) {
    emela_emb_float_format_no_candidate();
  }

  while (decimal > UINT64_C(0)
      && decimal % UINT64_C(10) == UINT64_C(0)) {
    decimal /= UINT64_C(10);
    ++decimal_exponent;
  }
  char digits[20];
  uint32_t digit_count = UINT32_C(0);
  do {
    digits[digit_count] = (char)('0' + (char)(decimal % UINT64_C(10)));
    ++digit_count;
    decimal /= UINT64_C(10);
  } while (decimal != UINT64_C(0));
  for (uint32_t index = UINT32_C(0); index < digit_count / UINT32_C(2); ++index) {
    const char temporary = digits[index];
    digits[index] = digits[digit_count - UINT32_C(1) - index];
    digits[digit_count - UINT32_C(1) - index] = temporary;
  }

  const int32_t decimal_position = (int32_t)digit_count + decimal_exponent;
  if ((int32_t)digit_count <= decimal_position && decimal_position <= INT32_C(21)) {
    for (uint32_t index = UINT32_C(0); index < digit_count; ++index) {
      emela_emb_float_format_put(buffer, capacity, &length, digits[index]);
    }
    for (int32_t index = (int32_t)digit_count; index < decimal_position; ++index) {
      emela_emb_float_format_put(buffer, capacity, &length, '0');
    }
    emela_emb_float_format_put_text(buffer, capacity, &length, ".0");
  } else if (INT32_C(0) < decimal_position && decimal_position <= INT32_C(21)) {
    for (int32_t index = INT32_C(0); index < decimal_position; ++index) {
      emela_emb_float_format_put(buffer, capacity, &length, digits[index]);
    }
    emela_emb_float_format_put(buffer, capacity, &length, '.');
    for (uint32_t index = (uint32_t)decimal_position; index < digit_count; ++index) {
      emela_emb_float_format_put(buffer, capacity, &length, digits[index]);
    }
  } else if (-INT32_C(6) < decimal_position && decimal_position <= INT32_C(0)) {
    emela_emb_float_format_put_text(buffer, capacity, &length, "0.");
    for (int32_t index = decimal_position; index < INT32_C(0); ++index) {
      emela_emb_float_format_put(buffer, capacity, &length, '0');
    }
    for (uint32_t index = UINT32_C(0); index < digit_count; ++index) {
      emela_emb_float_format_put(buffer, capacity, &length, digits[index]);
    }
  } else {
    emela_emb_float_format_put(buffer, capacity, &length, digits[0]);
    if (digit_count > UINT32_C(1)) {
      emela_emb_float_format_put(buffer, capacity, &length, '.');
      for (uint32_t index = UINT32_C(1); index < digit_count; ++index) {
        emela_emb_float_format_put(buffer, capacity, &length, digits[index]);
      }
    }
    const int32_t exponent = decimal_position - INT32_C(1);
    emela_emb_float_format_put(buffer, capacity, &length, 'e');
    emela_emb_float_format_put(buffer, capacity, &length, exponent < INT32_C(0) ? '-' : '+');
    uint32_t magnitude = (uint32_t)(exponent < INT32_C(0) ? -exponent : exponent);
    char exponent_digits[4];
    uint32_t exponent_count = UINT32_C(0);
    do {
      exponent_digits[exponent_count] = (char)('0' + (char)(magnitude % UINT32_C(10)));
      ++exponent_count;
      magnitude /= UINT32_C(10);
    } while (magnitude != UINT32_C(0));
    while (exponent_count > UINT32_C(0)) {
      --exponent_count;
      emela_emb_float_format_put(buffer, capacity, &length, exponent_digits[exponent_count]);
    }
  }
  return (EmelaString){ .data = buffer, .length = length, .owner = NULL };
}
"#;
