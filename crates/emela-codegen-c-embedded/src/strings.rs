//! The C of String for the embedded target (`docs/design/c-embedded-architecture.md`, 決定:
//! String の表現), written into `main.c` when the program uses strings.
//!
//! A String variable or field is an `EmString`: its length, and either a pointer to the bytes of
//! a literal (in flash, never copied) or its own buffer of `EMELA_STRING_CAPACITY` bytes. A String
//! passed to a function or a builtin is an `EmStr`, a view (pointer, length, whether it is a
//! literal): strings are immutable and the caller's frame outlives the call, so nothing is copied.
//! Building a string longer than the capacity is the defect `STRING_CAPACITY`; nothing is cut
//! silently. The bytes always end with a NUL (not counted), so C functions can read them as C
//! strings. Semantics (trimming, showing, number formatting, parsing) are the host C target's.

/// The definitions every program with strings gets.
pub(crate) const STRING_C: &str = r##"#include <string.h>

typedef struct {
  const char *ptr;
  uint16_t len;
  /* Whether `ptr` points to a literal, which lives as long as the program. */
  uint8_t lit;
} EmStr;

typedef struct {
  uint16_t len;
  /* The bytes of a literal, or NULL: then `buf`. */
  const char *lit;
  char buf[EMELA_STRING_CAPACITY + 1];
} EmString;

static inline EmStr em_lit(const char *s, uint16_t len) {
  EmStr v = {s, len, 1};
  return v;
}

static inline EmStr em_view(const EmString *s) {
  EmStr v = {s->lit ? s->lit : s->buf, s->len, (uint8_t)(s->lit != NULL)};
  return v;
}

static inline void em_str_capacity(size_t n) {
  if (n > EMELA_STRING_CAPACITY) {
    emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, "string capacity exceeded", 24);
  }
}

/* `d` takes the value `v`: a literal is pointed to, other bytes are copied. */
static inline void em_str_set(EmString *d, EmStr v) {
  d->len = v.len;
  if (v.lit) {
    d->lit = v.ptr;
  } else {
    em_str_capacity(v.len);
    d->lit = NULL;
    if (d->buf != v.ptr) memmove(d->buf, v.ptr, v.len);
    d->buf[v.len] = '\0';
  }
}

static inline EmString em_string_of(EmStr v) {
  EmString s;
  em_str_set(&s, v);
  return s;
}

static inline void em_str_bytes(EmString *d, const char *p, size_t n) {
  em_str_capacity(n);
  d->lit = NULL;
  memcpy(d->buf, p, n);
  d->buf[n] = '\0';
  d->len = (uint16_t)n;
}

static inline void em_str_append(EmString *d, EmStr a, EmStr b) {
  if (b.len == 0) {
    em_str_set(d, a);
    return;
  }
  if (a.len == 0) {
    em_str_set(d, b);
    return;
  }
  size_t n = (size_t)a.len + b.len;
  em_str_capacity(n);
  memmove(d->buf, a.ptr, a.len);
  memmove(d->buf + a.len, b.ptr, b.len);
  d->buf[n] = '\0';
  d->len = (uint16_t)n;
  d->lit = NULL;
}

/* Byte order, then length (spec 16.2). */
static inline int em_str_cmp(EmStr a, EmStr b) {
  size_t n = a.len < b.len ? a.len : b.len;
  int c = memcmp(a.ptr, b.ptr, n);
  if (c != 0) return c;
  return a.len < b.len ? -1 : a.len > b.len ? 1 : 0;
}

static inline bool em_str_starts_with(EmStr s, EmStr p) {
  return p.len <= s.len && memcmp(s.ptr, p.ptr, p.len) == 0;
}

static inline bool em_str_ends_with(EmStr s, EmStr p) {
  return p.len <= s.len && memcmp(s.ptr + s.len - p.len, p.ptr, p.len) == 0;
}

static inline bool em_str_contains(EmStr s, EmStr sub) {
  if (sub.len == 0) return true;
  for (size_t i = 0; i + sub.len <= s.len; i++) {
    if (s.ptr[i] == sub.ptr[0] && memcmp(s.ptr + i, sub.ptr, sub.len) == 0) return true;
  }
  return false;
}

/* The code point at `i` and its length in bytes (the text is valid UTF-8). */
static inline size_t em_utf8_next(const unsigned char *p, size_t i, uint32_t *cp) {
  unsigned char c = p[i];
  if (c < 0x80) { *cp = c; return 1; }
  if (c < 0xE0) { *cp = ((uint32_t)(c & 0x1F) << 6) | (p[i + 1] & 0x3F); return 2; }
  if (c < 0xF0) {
    *cp = ((uint32_t)(c & 0x0F) << 12) | ((uint32_t)(p[i + 1] & 0x3F) << 6) | (p[i + 2] & 0x3F);
    return 3;
  }
  *cp = ((uint32_t)(c & 0x07) << 18) | ((uint32_t)(p[i + 1] & 0x3F) << 12) |
        ((uint32_t)(p[i + 2] & 0x3F) << 6) | (p[i + 3] & 0x3F);
  return 4;
}

/* The 25 characters with the Unicode White_Space property (spec 16.2). */
static inline bool em_is_white_space(uint32_t c) {
  return (c >= 0x09 && c <= 0x0D) || c == 0x20 || c == 0x85 || c == 0xA0 || c == 0x1680 ||
         (c >= 0x2000 && c <= 0x200A) || c == 0x2028 || c == 0x2029 || c == 0x202F ||
         c == 0x205F || c == 0x3000;
}

static inline void em_str_trim(EmString *d, EmStr s) {
  const unsigned char *bytes = (const unsigned char *)s.ptr;
  size_t start = 0, end = s.len;
  while (start < end) {
    uint32_t cp;
    size_t len = em_utf8_next(bytes, start, &cp);
    if (!em_is_white_space(cp)) break;
    start += len;
  }
  while (end > start) {
    size_t at = end - 1;
    while (at > start && (bytes[at] & 0xC0) == 0x80) at--;
    uint32_t cp;
    em_utf8_next(bytes, at, &cp);
    if (!em_is_white_space(cp)) break;
    end = at;
  }
  if (start == 0 && end == s.len) {
    em_str_set(d, s);
  } else {
    em_str_bytes(d, s.ptr + start, end - start);
  }
}

static inline void em_str_int64(EmString *d, int64_t n) {
  char buf[24];
  int i = (int)sizeof buf;
  uint64_t u = n < 0 ? 0u - (uint64_t)n : (uint64_t)n;
  do {
    buf[--i] = (char)('0' + u % 10);
    u /= 10;
  } while (u != 0);
  if (n < 0) buf[--i] = '-';
  em_str_bytes(d, buf + i, sizeof buf - (size_t)i);
}

static inline void em_str_int(EmString *d, int32_t n) { em_str_int64(d, n); }

/* `Int.parse`: decimal digits with an optional sign, within the Int range. */
static inline bool em_str_parse_int(EmStr s, int32_t *out) {
  size_t i = 0;
  bool negative = false;
  if (i < s.len && (s.ptr[i] == '+' || s.ptr[i] == '-')) negative = s.ptr[i++] == '-';
  if (i == s.len) return false;
  int64_t v = 0;
  for (; i < s.len; i++) {
    if (s.ptr[i] < '0' || s.ptr[i] > '9') return false;
    if (v <= 2147483648LL) v = v * 10 + (s.ptr[i] - '0');
  }
  if (negative) v = -v;
  if (v < -2147483648LL || v > 2147483647LL) return false;
  *out = (int32_t)v;
  return true;
}

/* A string as written in source (spec 10.6), as the host targets show it. */
static inline void em_str_show(EmString *d, EmStr s) {
  char *o = d->buf;
  size_t n = 0;
#define EM_SHOW_PUT(text, count)                       \
  do {                                                 \
    size_t c_ = (count);                               \
    em_str_capacity(n + c_);                           \
    memcpy(o + n, (text), c_);                         \
    n += c_;                                           \
  } while (0)
  EM_SHOW_PUT("\"", 1);
  for (size_t i = 0; i < s.len; i++) {
    unsigned char c = (unsigned char)s.ptr[i];
    switch (c) {
      case '"': EM_SHOW_PUT("\\\"", 2); break;
      case '\\': EM_SHOW_PUT("\\\\", 2); break;
      case '#':
        if (i + 1 < s.len && s.ptr[i + 1] == '{') EM_SHOW_PUT("\\#", 2); else EM_SHOW_PUT("#", 1);
        break;
      case '\n': EM_SHOW_PUT("\\n", 2); break;
      case '\t': EM_SHOW_PUT("\\t", 2); break;
      case '\r': EM_SHOW_PUT("\\r", 2); break;
      default:
        if (c < 0x20 || c == 0x7F) {
          static const char hex[] = "0123456789abcdef";
          char esc[8] = {'\\', 'u', '{', 0, 0, 0, 0, 0};
          size_t k = 3;
          if (c >= 0x10) esc[k++] = hex[c >> 4];
          esc[k++] = hex[c & 0xF];
          esc[k++] = '}';
          EM_SHOW_PUT(esc, k);
        } else {
          EM_SHOW_PUT(&s.ptr[i], 1);
        }
    }
  }
  EM_SHOW_PUT("\"", 1);
#undef EM_SHOW_PUT
  d->buf[n] = '\0';
  d->len = (uint16_t)n;
  d->lit = NULL;
}

/* Building a string piece by piece (`String.join`). */
static inline void em_str_start(EmString *d) {
  d->lit = NULL;
  d->len = 0;
  d->buf[0] = '\0';
}

static inline void em_str_push(EmString *d, EmStr piece) {
  size_t n = (size_t)d->len + piece.len;
  em_str_capacity(n);
  memmove(d->buf + d->len, piece.ptr, piece.len);
  d->buf[n] = '\0';
  d->len = (uint16_t)n;
}

/* The UTF-8 bytes of a code point (a valid one: not a surrogate, at most 0x10FFFF). */
static inline size_t em_utf8_encode(uint32_t c, char *out) {
  if (c < 0x80) { out[0] = (char)c; return 1; }
  if (c < 0x800) { out[0] = (char)(0xC0 | (c >> 6)); out[1] = (char)(0x80 | (c & 0x3F)); return 2; }
  if (c < 0x10000) {
    out[0] = (char)(0xE0 | (c >> 12)); out[1] = (char)(0x80 | ((c >> 6) & 0x3F));
    out[2] = (char)(0x80 | (c & 0x3F)); return 3;
  }
  out[0] = (char)(0xF0 | (c >> 18)); out[1] = (char)(0x80 | ((c >> 12) & 0x3F));
  out[2] = (char)(0x80 | ((c >> 6) & 0x3F)); out[3] = (char)(0x80 | (c & 0x3F)); return 4;
}

/* A number's digits, written to a Console hook (a piece of a line). */
static inline void em_write_int64(void (*write)(const char *, uint16_t), int64_t n) {
  char buf[24];
  int i = (int)sizeof buf;
  uint64_t u = n < 0 ? 0u - (uint64_t)n : (uint64_t)n;
  do {
    buf[--i] = (char)('0' + u % 10);
    u /= 10;
  } while (u != 0);
  if (n < 0) buf[--i] = '-';
  write(buf + i, (uint16_t)(sizeof buf - (size_t)i));
}

/* `dbg`: the board's debug output. */
static inline void em_debug(EmStr s) { emela_board_debug(s.ptr, s.len); }
"##;

/// `Float.to_string` and `Float.parse`, written only when the program uses them: they need the
/// floating-point `snprintf` and `strtod` of the C library (with newlib-nano, link with
/// `-u _printf_float`).
pub(crate) const FLOAT_STRING_C: &str = r#"#include <stdio.h>
#include <stdlib.h>

/* JS `Number.prototype.toString` (the shortest digits that read back as the same number), with
 * `.0` added to a finite integral value (spec 10.6), as the host C target writes it. */
static inline void em_str_float(EmString *d, double x) {
  char out[40];
  if (isnan(x)) {
    em_str_bytes(d, "NaN", 3);
    return;
  }
  if (isinf(x)) {
    if (x > 0) em_str_bytes(d, "Infinity", 8); else em_str_bytes(d, "-Infinity", 9);
    return;
  }
  if (x == 0) {
    em_str_bytes(d, "0.0", 3);
    return;
  }
  char buf[40];
  for (int p = 1; p <= 17; p++) {
    snprintf(buf, sizeof buf, "%.*e", p - 1, x);
    if (strtod(buf, NULL) == x) break;
  }
  const char *q = buf;
  bool negative = *q == '-';
  if (negative) q++;
  char digits[24];
  int k = 0;
  while (*q != 'e') {
    if (*q != '.') digits[k++] = *q;
    q++;
  }
  while (k > 1 && digits[k - 1] == '0') k--;
  digits[k] = '\0';
  int n = atoi(q + 1) + 1;
  char *o = out;
  size_t left = sizeof out;
#define EM_PUT(...)                                  \
  do {                                               \
    int w_ = snprintf(o, left, __VA_ARGS__);         \
    if (w_ > 0 && (size_t)w_ < left) {               \
      o += w_;                                       \
      left -= (size_t)w_;                            \
    }                                                \
  } while (0)
  if (negative) EM_PUT("-");
  if (k <= n && n <= 21) {
    EM_PUT("%s", digits);
    for (int i = k; i < n; i++) EM_PUT("0");
    EM_PUT(".0");
  } else if (0 < n && n <= 21) {
    EM_PUT("%.*s.%s", n, digits, digits + n);
  } else if (-6 < n && n <= 0) {
    EM_PUT("0.");
    for (int i = n; i < 0; i++) EM_PUT("0");
    EM_PUT("%s", digits);
  } else {
    int e = n - 1;
    if (k == 1) {
      EM_PUT("%c", digits[0]);
    } else {
      EM_PUT("%c.%s", digits[0], digits + 1);
    }
    EM_PUT("e%c%d", e < 0 ? '-' : '+', e < 0 ? -e : e);
  }
#undef EM_PUT
  em_str_bytes(d, out, (size_t)(o - out));
}

static inline bool em_digits(const char *p, size_t n, size_t *i) {
  size_t start = *i;
  while (*i < n && p[*i] >= '0' && p[*i] <= '9') (*i)++;
  return *i > start;
}

/* `Float.parse`: the forms `Float.to_string` writes. */
static inline bool em_str_parse_float(EmStr s, double *out) {
  const char *p = s.ptr;
  size_t n = s.len, i = 0;
  bool ok;
  if (n == 3 && memcmp(p, "NaN", 3) == 0) {
    ok = true;
  } else {
    if (i < n && (p[i] == '+' || p[i] == '-')) i++;
    if (n - i == 8 && memcmp(p + i, "Infinity", 8) == 0) {
      ok = true;
    } else {
      ok = em_digits(p, n, &i);
      if (ok && i < n && p[i] == '.') {
        i++;
        ok = em_digits(p, n, &i);
      }
      if (ok && i < n && (p[i] == 'e' || p[i] == 'E')) {
        i++;
        if (i < n && (p[i] == '+' || p[i] == '-')) i++;
        ok = em_digits(p, n, &i);
      }
      ok = ok && i == n;
    }
  }
  if (!ok) return false;
  *out = strtod(p, NULL);
  return true;
}
"#;
