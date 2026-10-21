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
#error "Float.parse requires IEEE 754 binary64 double"
#endif

/* A binary64 rounding midpoint has at most 768 significant decimal digits. */
#define EMELA_EMB_FLOAT_PARSE_LIMBS UINT32_C(120)
#define EMELA_EMB_FLOAT_PARSE_DIGITS UINT32_C(800)

typedef struct {
  uint32_t words[120];
  uint16_t length;
} EmelaEmbFloatParseBig;

static inline double emela_emb_float_from_bits(uint64_t bits) {
  union {
    uint64_t bits;
    double value;
  } converted;
  converted.bits = bits;
  return converted.value;
}

static inline int emela_emb_float_text_equals(EmelaString value, const char *expected, uint32_t length) {
  if ((uint32_t)value.length != length) {
    return 0;
  }
  for (uint32_t index = UINT32_C(0); index < length; ++index) {
    if (value.data[index] != expected[index]) {
      return 0;
    }
  }
  return 1;
}

static inline int emela_emb_float_big_multiply_small(EmelaEmbFloatParseBig *value, uint32_t multiplier) {
  uint64_t carry = UINT64_C(0);
  for (uint32_t index = UINT32_C(0); index < (uint32_t)value->length; ++index) {
    const uint64_t product = (uint64_t)value->words[index] * (uint64_t)multiplier + carry;
    value->words[index] = (uint32_t)product;
    carry = product >> 32;
  }
  if (carry != UINT64_C(0)) {
    if ((uint32_t)value->length >= EMELA_EMB_FLOAT_PARSE_LIMBS) {
      return 0;
    }
    value->words[value->length] = (uint32_t)carry;
    ++value->length;
  }
  return 1;
}

static inline int emela_emb_float_big_add_small(EmelaEmbFloatParseBig *value, uint32_t addition) {
  uint64_t carry = (uint64_t)addition;
  uint32_t index = UINT32_C(0);
  while (carry != UINT64_C(0) && index < (uint32_t)value->length) {
    const uint64_t sum = (uint64_t)value->words[index] + carry;
    value->words[index] = (uint32_t)sum;
    carry = sum >> 32;
    ++index;
  }
  if (carry != UINT64_C(0)) {
    if ((uint32_t)value->length >= EMELA_EMB_FLOAT_PARSE_LIMBS) {
      return 0;
    }
    value->words[value->length] = (uint32_t)carry;
    ++value->length;
  } else if (value->length == UINT16_C(0) && addition != UINT32_C(0)) {
    value->words[0] = addition;
    value->length = UINT16_C(1);
  }
  return 1;
}

static inline uint32_t emela_emb_float_big_bit_length(const EmelaEmbFloatParseBig *value) {
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

static inline uint32_t emela_emb_float_big_shifted_word(const EmelaEmbFloatParseBig *value, uint32_t shift, uint32_t index) {
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

static inline int emela_emb_float_big_compare_shifted(
    const EmelaEmbFloatParseBig *left,
    uint32_t left_shift,
    const EmelaEmbFloatParseBig *right,
    uint32_t right_shift) {
  const uint32_t left_bits = emela_emb_float_big_bit_length(left) + left_shift;
  const uint32_t right_bits = emela_emb_float_big_bit_length(right) + right_shift;
  if (left_bits < right_bits) {
    return -1;
  }
  if (left_bits > right_bits) {
    return 1;
  }
  const uint32_t word_count = (left_bits + UINT32_C(31)) / UINT32_C(32);
  for (uint32_t index = word_count; index > UINT32_C(0); --index) {
    const uint32_t left_word = emela_emb_float_big_shifted_word(left, left_shift, index - UINT32_C(1));
    const uint32_t right_word = emela_emb_float_big_shifted_word(right, right_shift, index - UINT32_C(1));
    if (left_word < right_word) {
      return -1;
    }
    if (left_word > right_word) {
      return 1;
    }
  }
  return 0;
}

static inline int emela_emb_float_big_shift_left(EmelaEmbFloatParseBig *value, uint32_t shift) {
  if (value->length == UINT16_C(0) || shift == UINT32_C(0)) {
    return 1;
  }
  const uint32_t whole_words = shift / UINT32_C(32);
  const uint32_t remaining_bits = shift % UINT32_C(32);
  const uint32_t old_length = (uint32_t)value->length;
  const uint32_t extra_word = remaining_bits == UINT32_C(0) ? UINT32_C(0) : UINT32_C(1);
  if (old_length + whole_words + extra_word > EMELA_EMB_FLOAT_PARSE_LIMBS) {
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

static inline void emela_emb_float_big_normalize(EmelaEmbFloatParseBig *value) {
  while (value->length != UINT16_C(0)
      && value->words[value->length - UINT16_C(1)] == UINT32_C(0)) {
    --value->length;
  }
}

static inline void emela_emb_float_big_subtract_shifted(
    EmelaEmbFloatParseBig *left,
    const EmelaEmbFloatParseBig *right,
    uint32_t right_shift) {
  uint64_t borrow = UINT64_C(0);
  for (uint32_t index = UINT32_C(0); index < (uint32_t)left->length; ++index) {
    const uint64_t subtrahend = (uint64_t)emela_emb_float_big_shifted_word(right, right_shift, index) + borrow;
    const uint64_t minuend = (uint64_t)left->words[index];
    left->words[index] = (uint32_t)(minuend - subtrahend);
    borrow = minuend < subtrahend ? UINT64_C(1) : UINT64_C(0);
  }
  emela_emb_float_big_normalize(left);
}

static inline int emela_emb_float_big_power_of_ten(EmelaEmbFloatParseBig *value, uint32_t exponent) {
  for (uint32_t index = UINT32_C(0); index < exponent; ++index) {
    if (!emela_emb_float_big_multiply_small(value, UINT32_C(10))) {
      return 0;
    }
  }
  return 1;
}

static inline int emela_emb_float_parse(EmelaString value, double *result) {
  if (value.length != UINT16_C(0) && value.data == NULL) {
    return 0;
  }
  if (emela_emb_float_text_equals(value, "NaN", UINT32_C(3))) {
    *result = emela_emb_float_from_bits(UINT64_C(0x7ff8000000000000));
    return 1;
  }
  if (emela_emb_float_text_equals(value, "Infinity", UINT32_C(8))
      || emela_emb_float_text_equals(value, "+Infinity", UINT32_C(9))) {
    *result = emela_emb_float_from_bits(UINT64_C(0x7ff0000000000000));
    return 1;
  }
  if (emela_emb_float_text_equals(value, "-Infinity", UINT32_C(9))) {
    *result = emela_emb_float_from_bits(UINT64_C(0xfff0000000000000));
    return 1;
  }

  EmelaEmbFloatParseBig numerator;
  EmelaEmbFloatParseBig denominator;
  numerator.length = UINT16_C(0);
  denominator.words[0] = UINT32_C(1);
  denominator.length = UINT16_C(1);
  uint32_t index = UINT32_C(0);
  int negative = 0;
  if (value.length != UINT16_C(0)) {
    const uint8_t first = (uint8_t)value.data[0];
    if (first == UINT8_C(0x2b) || first == UINT8_C(0x2d)) {
      negative = first == UINT8_C(0x2d);
      ++index;
    }
  }

  uint32_t significant_digits = UINT32_C(0);
  uint32_t kept_digits = UINT32_C(0);
  uint32_t fraction_digits = UINT32_C(0);
  int seen_nonzero = 0;
  int sticky = 0;
  int saw_integer_digit = 0;
  int saw_fraction_digit = 0;
  int after_decimal_point = 0;
  for (; index < (uint32_t)value.length;) {
    const uint8_t byte = (uint8_t)value.data[index];
    if (byte >= UINT8_C(0x30) && byte <= UINT8_C(0x39)) {
      if (after_decimal_point) {
        saw_fraction_digit = 1;
        ++fraction_digits;
      } else {
        saw_integer_digit = 1;
      }
      const uint32_t digit = (uint32_t)(byte - UINT8_C(0x30));
      if (seen_nonzero || digit != UINT32_C(0)) {
        seen_nonzero = 1;
        ++significant_digits;
        if (kept_digits < EMELA_EMB_FLOAT_PARSE_DIGITS) {
          if (!emela_emb_float_big_multiply_small(&numerator, UINT32_C(10))
              || !emela_emb_float_big_add_small(&numerator, digit)) {
            return 0;
          }
          ++kept_digits;
        } else if (digit != UINT32_C(0)) {
          sticky = 1;
        }
      }
      ++index;
    } else if (byte == UINT8_C(0x2e) && !after_decimal_point) {
      after_decimal_point = 1;
      ++index;
    } else {
      break;
    }
  }
  if (!saw_integer_digit || (after_decimal_point && !saw_fraction_digit)) {
    return 0;
  }

  int exponent_negative = 0;
  uint32_t exponent_magnitude = UINT32_C(0);
  if (index < (uint32_t)value.length
      && ((uint8_t)value.data[index] == UINT8_C(0x65)
          || (uint8_t)value.data[index] == UINT8_C(0x45))) {
    ++index;
    if (index < (uint32_t)value.length
        && ((uint8_t)value.data[index] == UINT8_C(0x2b)
            || (uint8_t)value.data[index] == UINT8_C(0x2d))) {
      exponent_negative = (uint8_t)value.data[index] == UINT8_C(0x2d);
      ++index;
    }
    const uint32_t exponent_start = index;
    while (index < (uint32_t)value.length) {
      const uint8_t byte = (uint8_t)value.data[index];
      if (byte < UINT8_C(0x30) || byte > UINT8_C(0x39)) {
        break;
      }
      if (exponent_magnitude < UINT32_C(1000000)) {
        exponent_magnitude = exponent_magnitude * UINT32_C(10)
            + (uint32_t)(byte - UINT8_C(0x30));
        if (exponent_magnitude > UINT32_C(1000000)) {
          exponent_magnitude = UINT32_C(1000000);
        }
      }
      ++index;
    }
    if (index == exponent_start) {
      return 0;
    }
  }
  if (index != (uint32_t)value.length) {
    return 0;
  }
  if (!seen_nonzero) {
    *result = emela_emb_float_from_bits(negative ? UINT64_C(0x8000000000000000) : UINT64_C(0));
    return 1;
  }

  const int32_t explicit_exponent = exponent_negative
      ? -(int32_t)exponent_magnitude
      : (int32_t)exponent_magnitude;
  const int32_t decimal_exponent = explicit_exponent - (int32_t)fraction_digits;
  const int32_t decimal_order = (int32_t)significant_digits + decimal_exponent - INT32_C(1);
  const uint64_t sign_bit = negative ? UINT64_C(0x8000000000000000) : UINT64_C(0);
  if (decimal_order > INT32_C(308)) {
    *result = emela_emb_float_from_bits(sign_bit | UINT64_C(0x7ff0000000000000));
    return 1;
  }
  if (decimal_order < -INT32_C(325)) {
    *result = emela_emb_float_from_bits(sign_bit);
    return 1;
  }

  const int32_t scale_exponent = decimal_order - (int32_t)kept_digits + INT32_C(1);
  if (scale_exponent >= INT32_C(0)) {
    if (!emela_emb_float_big_power_of_ten(&numerator, (uint32_t)scale_exponent)) {
      return 0;
    }
  } else if (!emela_emb_float_big_power_of_ten(&denominator, (uint32_t)(-scale_exponent))) {
    return 0;
  }

  int32_t binary_exponent = (int32_t)emela_emb_float_big_bit_length(&numerator)
      - (int32_t)emela_emb_float_big_bit_length(&denominator);
  if (binary_exponent >= INT32_C(0)) {
    if (emela_emb_float_big_compare_shifted(
            &numerator,
            UINT32_C(0),
            &denominator,
            (uint32_t)binary_exponent)
        < 0) {
      --binary_exponent;
    }
  } else if (emela_emb_float_big_compare_shifted(
                 &numerator,
                 (uint32_t)(-binary_exponent),
                 &denominator,
                 UINT32_C(0))
      < 0) {
    --binary_exponent;
  }
  if (binary_exponent > INT32_C(1023)) {
    *result = emela_emb_float_from_bits(sign_bit | UINT64_C(0x7ff0000000000000));
    return 1;
  }
  if (binary_exponent < -INT32_C(1075)) {
    *result = emela_emb_float_from_bits(sign_bit);
    return 1;
  }

  if (binary_exponent >= -INT32_C(1022)) {
    const int32_t shift = INT32_C(52) - binary_exponent;
    if (shift >= INT32_C(0)) {
      if (!emela_emb_float_big_shift_left(&numerator, (uint32_t)shift)) {
        return 0;
      }
    } else if (!emela_emb_float_big_shift_left(&denominator, (uint32_t)(-shift))) {
      return 0;
    }
  } else if (!emela_emb_float_big_shift_left(&numerator, UINT32_C(1074))) {
    return 0;
  }

  uint64_t significand = UINT64_C(0);
  for (int32_t bit = INT32_C(53); bit >= INT32_C(0); --bit) {
    if (emela_emb_float_big_compare_shifted(
            &denominator,
            (uint32_t)bit,
            &numerator,
            UINT32_C(0))
        <= 0) {
      emela_emb_float_big_subtract_shifted(&numerator, &denominator, (uint32_t)bit);
      significand |= UINT64_C(1) << (uint32_t)bit;
    }
  }
  const int half_comparison = emela_emb_float_big_compare_shifted(
      &numerator,
      UINT32_C(1),
      &denominator,
      UINT32_C(0));
  if (half_comparison > 0
      || (half_comparison == 0 && (sticky || (significand & UINT64_C(1)) != UINT64_C(0)))) {
    ++significand;
  }

  uint64_t magnitude_bits = significand;
  if (binary_exponent >= -INT32_C(1022)) {
    if (significand == (UINT64_C(1) << UINT32_C(53))) {
      significand >>= 1;
      ++binary_exponent;
    }
    if (binary_exponent > INT32_C(1023)) {
      magnitude_bits = UINT64_C(0x7ff0000000000000);
    } else {
      magnitude_bits = ((uint64_t)(binary_exponent + INT32_C(1023)) << UINT32_C(52))
          | (significand - (UINT64_C(1) << UINT32_C(52)));
    }
  }
  *result = emela_emb_float_from_bits(sign_bit | magnitude_bits);
  return 1;
}
"#;
