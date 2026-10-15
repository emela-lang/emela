/* The Emela C runtime. See emela_runtime.h for the value representation.
 *
 * Builtins follow the JS runtime (crates/emela-codegen-js/src/runtime.mjs) exactly: the same
 * results, the same defect messages. */
#include "emela_runtime.h"

#include <errno.h>
#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The exit codes of spec appendix B. */
#define EM_EXIT_ERROR 1
#define EM_EXIT_DEFECT 101
/* A leak found by the check at exit (EMELA_LEAK_CHECK). */
#define EM_EXIT_LEAK 99
/* A feature the C target does not support yet was reached at run time. */
#define EM_EXIT_UNSUPPORTED 70

em_ref em_defect = NULL;
em_ref em_failure = NULL;

/* The number of live heap objects, for the leak check. */
static int64_t em_live = 0;
/* The name of the handler retired last: the release that raised a defect while another one was
 * unwinding names it in the note (spec 14.4). */
static em_ref em_last_retired = NULL;
/* Whether the pending defect is a const referring to itself (not remembered by consts). */
static bool em_self_reference = false;

/* ---- Allocation ---- */

static _Noreturn void em_out_of_memory(void) {
  fputs("emela: out of memory\n", stderr);
  abort();
}

em_ref em_alloc(const em_desc *desc, size_t nslots) {
  em_ref o = malloc(sizeof(em_hdr) + nslots * sizeof(em_word));
  if (o == NULL) em_out_of_memory();
  o->hdr.u.h.rc = 1;
  o->hdr.u.h.aux = 0;
  o->hdr.desc = desc;
  em_live++;
  return o;
}

/* Frees `o` (whose count reached 0) and every object that only it kept alive, without
 * recursion: the objects to free form a stack linked through their first 8 bytes, which hold
 * the count while the object is alive. */
void em_free(em_ref o) {
  em_ref stack = o;
  o->hdr.u.next = NULL;
  while (stack != NULL) {
    em_ref x = stack;
    stack = x->hdr.u.next;
    const em_desc *d = x->hdr.desc;
#define EM_RELEASE_CHILD(c)                                           \
  do {                                                               \
    em_ref c_ = (c);                                                 \
    if (c_ != NULL && c_->hdr.u.h.rc != 0 && --c_->hdr.u.h.rc == 0) { \
      c_->hdr.u.next = stack;                                        \
      stack = c_;                                                    \
    }                                                                \
  } while (0)
    switch (d->kind) {
      case EM_KIND_FIELDS:
        for (uint32_t i = 0; i < d->nfields; i++) {
          if (d->refs[i]) EM_RELEASE_CHILD(x->slots[i].p);
        }
        break;
      case EM_KIND_EVIDENCE: {
        int64_t n = x->slots[0].l;
        for (int64_t k = 0; k < n; k++) EM_RELEASE_CHILD(x->slots[2 + 2 * k].p);
        break;
      }
      default:
        break;
    }
#undef EM_RELEASE_CHILD
    free(x);
    em_live--;
  }
}

static const uint8_t em_refs_none[2] = {0, 0};
static const uint8_t em_refs_cons_ref[2] = {1, 1};
static const uint8_t em_refs_cons_val[2] = {0, 1};
static const uint8_t em_refs_one[1] = {1};

const em_desc em_desc_string = {EM_KIND_STRING, 0, 0, -1, "String", NULL, em_refs_none};
const em_desc em_desc_cons_val = {EM_KIND_FIELDS, 2, 0, -1, "cons", NULL, em_refs_cons_val};
const em_desc em_desc_cons_ref = {EM_KIND_FIELDS, 2, 0, -1, "cons", NULL, em_refs_cons_ref};
const em_desc em_desc_box_val = {EM_KIND_FIELDS, 1, 0, -1, "box", NULL, em_refs_none};
const em_desc em_desc_cell_val = {EM_KIND_FIELDS, 1, 0, -1, "cell", NULL, em_refs_none};
const em_desc em_desc_cell_ref = {EM_KIND_FIELDS, 1, 0, -1, "cell", NULL, em_refs_one};
static const em_desc em_desc_evidence = {EM_KIND_EVIDENCE, 0, 0, -1, "evidence", NULL,
                                         em_refs_none};
static const em_desc em_desc_host = {EM_KIND_HOST, 1, 0, -1, "host", NULL, em_refs_none};
/* Prelude's Option as the builtins make it (`Some` is 0, `None` is 1). */
static const em_desc em_desc_some_val = {EM_KIND_FIELDS, 1, 0, -1, "Some", NULL, em_refs_none};
static const em_desc em_desc_some_ref = {EM_KIND_FIELDS, 1, 0, -1, "Some", NULL, em_refs_one};
static const em_desc em_desc_none = {EM_KIND_FIELDS, 0, 1, -1, "None", NULL, em_refs_none};
static em_hdr em_none_obj = EM_STATIC_HDR(&em_desc_none);
#define EM_NONE ((em_ref)&em_none_obj)

static em_ref em_some(em_word w, bool ref) {
  em_ref o = em_alloc(ref ? &em_desc_some_ref : &em_desc_some_val, 1);
  o->slots[0] = w;
  return o;
}

em_ref em_box(em_word w) {
  em_ref o = em_alloc(&em_desc_box_val, 1);
  o->slots[0] = w;
  return o;
}

em_ref em_host_box(void *h) {
  em_ref o = em_alloc(&em_desc_host, 1);
  o->slots[0].h = h;
  return o;
}

/* ---- Strings ---- */

#define EM_STR_LEN(s) ((size_t)(s)->slots[0].l)
#define EM_STR_BYTES(s) ((char *)&(s)->slots[1])

static em_ref em_str_alloc(size_t len) {
  em_ref s = em_alloc(&em_desc_string, 1 + (len + 1 + sizeof(em_word) - 1) / sizeof(em_word));
  s->slots[0].l = (int64_t)len;
  EM_STR_BYTES(s)[len] = '\0';
  return s;
}

static em_ref em_str_from(const char *bytes, size_t len) {
  em_ref s = em_str_alloc(len);
  memcpy(EM_STR_BYTES(s), bytes, len);
  return s;
}

static em_ref em_str_cstr(const char *bytes) { return em_str_from(bytes, strlen(bytes)); }

/* A growable byte buffer. */
typedef struct em_buf {
  char *data;
  size_t len;
  size_t cap;
} em_buf;

static void em_buf_reserve(em_buf *b, size_t extra) {
  if (b->len + extra <= b->cap) return;
  size_t cap = b->cap == 0 ? 64 : b->cap;
  while (cap < b->len + extra) cap *= 2;
  char *data = realloc(b->data, cap);
  if (data == NULL) em_out_of_memory();
  b->data = data;
  b->cap = cap;
}

static void em_buf_put(em_buf *b, const char *bytes, size_t len) {
  em_buf_reserve(b, len);
  memcpy(b->data + b->len, bytes, len);
  b->len += len;
}

static void em_buf_puts(em_buf *b, const char *s) { em_buf_put(b, s, strlen(s)); }

static em_ref em_buf_finish(em_buf *b) {
  em_ref s = em_str_from(b->data == NULL ? "" : b->data, b->len);
  free(b->data);
  return s;
}

/* Decodes the code point at `s[i]` of a valid UTF-8 string; returns its length. */
static size_t em_utf8_next(const unsigned char *s, size_t i, uint32_t *cp) {
  unsigned char b = s[i];
  if (b < 0x80) {
    *cp = b;
    return 1;
  }
  if (b < 0xE0) {
    *cp = ((uint32_t)(b & 0x1F) << 6) | (s[i + 1] & 0x3F);
    return 2;
  }
  if (b < 0xF0) {
    *cp = ((uint32_t)(b & 0x0F) << 12) | ((uint32_t)(s[i + 1] & 0x3F) << 6) | (s[i + 2] & 0x3F);
    return 3;
  }
  *cp = ((uint32_t)(b & 0x07) << 18) | ((uint32_t)(s[i + 1] & 0x3F) << 12) |
        ((uint32_t)(s[i + 2] & 0x3F) << 6) | (s[i + 3] & 0x3F);
  return 4;
}

static size_t em_utf8_encode(uint32_t cp, char *out) {
  if (cp < 0x80) {
    out[0] = (char)cp;
    return 1;
  }
  if (cp < 0x800) {
    out[0] = (char)(0xC0 | (cp >> 6));
    out[1] = (char)(0x80 | (cp & 0x3F));
    return 2;
  }
  if (cp < 0x10000) {
    out[0] = (char)(0xE0 | (cp >> 12));
    out[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
    out[2] = (char)(0x80 | (cp & 0x3F));
    return 3;
  }
  out[0] = (char)(0xF0 | (cp >> 18));
  out[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
  out[2] = (char)(0x80 | ((cp >> 6) & 0x3F));
  out[3] = (char)(0x80 | (cp & 0x3F));
  return 4;
}

/* The length of the well-formed UTF-8 sequence at `s[i]` (of `n` bytes), or 0 if it is not one;
 * then `*bad` is the length of the maximal subpart to replace with U+FFFD (at least 1). */
static size_t em_utf8_valid(const unsigned char *s, size_t n, size_t i, size_t *bad) {
  unsigned char b = s[i];
  size_t need;
  unsigned char lo = 0x80, hi = 0xBF;
  if (b < 0x80) return 1;
  if (b >= 0xC2 && b <= 0xDF) {
    need = 1;
  } else if (b >= 0xE0 && b <= 0xEF) {
    need = 2;
    if (b == 0xE0) lo = 0xA0;
    if (b == 0xED) hi = 0x9F;
  } else if (b >= 0xF0 && b <= 0xF4) {
    need = 3;
    if (b == 0xF0) lo = 0x90;
    if (b == 0xF4) hi = 0x8F;
  } else {
    *bad = 1;
    return 0;
  }
  for (size_t k = 1; k <= need; k++) {
    if (i + k >= n) {
      *bad = k;
      return 0;
    }
    unsigned char c = s[i + k];
    unsigned char l = k == 1 ? lo : 0x80, h = k == 1 ? hi : 0xBF;
    if (c < l || c > h) {
      *bad = k;
      return 0;
    }
  }
  return need + 1;
}

int32_t em_str_eq(em_ref a, em_ref b) {
  if (a == b) return 1;
  size_t n = EM_STR_LEN(a);
  return n == EM_STR_LEN(b) && memcmp(EM_STR_BYTES(a), EM_STR_BYTES(b), n) == 0;
}

int32_t em_str_cmp(em_ref a, em_ref b) {
  size_t na = EM_STR_LEN(a), nb = EM_STR_LEN(b);
  int c = memcmp(EM_STR_BYTES(a), EM_STR_BYTES(b), na < nb ? na : nb);
  if (c != 0) return c < 0 ? -1 : 1;
  return na < nb ? -1 : na > nb ? 1 : 0;
}

int32_t em_string_byte_size(em_ref s) { return (int32_t)EM_STR_LEN(s); }

em_ref em_string_append(em_ref a, em_ref b) {
  size_t na = EM_STR_LEN(a), nb = EM_STR_LEN(b);
  if (nb == 0) {
    em_dup(a);
    return a;
  }
  if (na == 0) {
    em_dup(b);
    return b;
  }
  em_ref s = em_str_alloc(na + nb);
  memcpy(EM_STR_BYTES(s), EM_STR_BYTES(a), na);
  memcpy(EM_STR_BYTES(s) + na, EM_STR_BYTES(b), nb);
  return s;
}

/* The byte offset of the first `sub` in `s` at or after `from`, or -1. */
static int64_t em_find(em_ref s, em_ref sub, size_t from) {
  size_t n = EM_STR_LEN(s), m = EM_STR_LEN(sub);
  const char *hay = EM_STR_BYTES(s), *needle = EM_STR_BYTES(sub);
  if (m == 0) return (int64_t)from;
  if (m > n) return -1;
  for (size_t i = from; i + m <= n; i++) {
    if (hay[i] == needle[0] && memcmp(hay + i, needle, m) == 0) return (int64_t)i;
  }
  return -1;
}

int32_t em_string_contains(em_ref s, em_ref sub) { return em_find(s, sub, 0) >= 0; }

int32_t em_string_starts_with(em_ref s, em_ref prefix) {
  size_t n = EM_STR_LEN(s), m = EM_STR_LEN(prefix);
  return m <= n && memcmp(EM_STR_BYTES(s), EM_STR_BYTES(prefix), m) == 0;
}

int32_t em_string_ends_with(em_ref s, em_ref suffix) {
  size_t n = EM_STR_LEN(s), m = EM_STR_LEN(suffix);
  return m <= n && memcmp(EM_STR_BYTES(s) + n - m, EM_STR_BYTES(suffix), m) == 0;
}

/* Builds a list front to back. */
typedef struct em_list_builder {
  em_ref head;
  em_ref last;
} em_list_builder;

static void em_list_push(em_list_builder *b, em_word w, bool ref) {
  em_ref c = em_cons(w, ref, NULL);
  if (b->last == NULL) {
    b->head = c;
  } else {
    b->last->slots[1].p = c;
  }
  b->last = c;
}

static void em_push_str(em_list_builder *b, const char *bytes, size_t len) {
  em_word w;
  w.p = em_str_from(bytes, len);
  em_list_push(b, w, true);
}

em_ref em_string_split(em_ref s, em_ref sep) {
  em_list_builder b = {NULL, NULL};
  size_t n = EM_STR_LEN(s), m = EM_STR_LEN(sep);
  const char *bytes = EM_STR_BYTES(s);
  if (m == 0) {
    /* Grapheme clusters (spec 16.2). Without the Unicode tables only ASCII text can be split
     * exactly: every character is a cluster except CR LF, which is one. */
    for (size_t i = 0; i < n; i++) {
      if ((unsigned char)bytes[i] >= 0x80) {
        em_unsupported("String.split with an empty separator on non-ASCII text");
      }
    }
    for (size_t i = 0; i < n;) {
      size_t len = bytes[i] == '\r' && i + 1 < n && bytes[i + 1] == '\n' ? 2 : 1;
      em_push_str(&b, bytes + i, len);
      i += len;
    }
    return b.head;
  }
  size_t start = 0;
  for (;;) {
    int64_t at = em_find(s, sep, start);
    if (at < 0) break;
    em_push_str(&b, bytes + start, (size_t)at - start);
    start = (size_t)at + m;
  }
  em_push_str(&b, bytes + start, n - start);
  return b.head;
}

em_ref em_string_join(em_ref parts, em_ref sep) {
  em_buf b = {NULL, 0, 0};
  for (em_ref p = parts; p != NULL; p = p->slots[1].p) {
    if (p != parts) em_buf_put(&b, EM_STR_BYTES(sep), EM_STR_LEN(sep));
    em_ref part = p->slots[0].p;
    em_buf_put(&b, EM_STR_BYTES(part), EM_STR_LEN(part));
  }
  return em_buf_finish(&b);
}

/* The 25 characters with the Unicode White_Space property (spec 16.2). */
static bool em_is_white_space(uint32_t c) {
  return (c >= 0x09 && c <= 0x0D) || c == 0x20 || c == 0x85 || c == 0xA0 || c == 0x1680 ||
         (c >= 0x2000 && c <= 0x200A) || c == 0x2028 || c == 0x2029 || c == 0x202F ||
         c == 0x205F || c == 0x3000;
}

em_ref em_string_trim(em_ref s) {
  const unsigned char *bytes = (const unsigned char *)EM_STR_BYTES(s);
  size_t n = EM_STR_LEN(s), start = 0, end = n;
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
  if (start == 0 && end == n) {
    em_dup(s);
    return s;
  }
  return em_str_from((const char *)bytes + start, end - start);
}

em_ref em_string_code_points(em_ref s) {
  em_list_builder b = {NULL, NULL};
  const unsigned char *bytes = (const unsigned char *)EM_STR_BYTES(s);
  size_t n = EM_STR_LEN(s);
  for (size_t i = 0; i < n;) {
    uint32_t cp;
    i += em_utf8_next(bytes, i, &cp);
    em_word w;
    w.i = (int32_t)cp;
    em_list_push(&b, w, false);
  }
  return b.head;
}

em_ref em_string_from_code_point(int32_t n) {
  if (n < 0 || n > 0x10FFFF || (n >= 0xD800 && n <= 0xDFFF)) return EM_NONE;
  char buf[4];
  size_t len = em_utf8_encode((uint32_t)n, buf);
  em_word w;
  w.p = em_str_from(buf, len);
  return em_some(w, true);
}

/* ---- Numbers ---- */

em_ref em_int_to_string(int32_t n) {
  char buf[16];
  snprintf(buf, sizeof buf, "%" PRId32, n);
  return em_str_cstr(buf);
}

em_ref em_int64_to_string(int64_t n) {
  char buf[24];
  snprintf(buf, sizeof buf, "%" PRId64, n);
  return em_str_cstr(buf);
}

/* `Int.parse`: decimal digits with an optional sign, within the Int range. */
em_ref em_int_parse(em_ref s) {
  const char *p = EM_STR_BYTES(s);
  size_t n = EM_STR_LEN(s), i = 0;
  bool negative = false;
  if (i < n && (p[i] == '+' || p[i] == '-')) negative = p[i++] == '-';
  if (i == n) return EM_NONE;
  int64_t v = 0;
  for (; i < n; i++) {
    if (p[i] < '0' || p[i] > '9') return EM_NONE;
    if (v <= 2147483648LL) v = v * 10 + (p[i] - '0');
  }
  if (negative) v = -v;
  if (v < -2147483648LL || v > 2147483647LL) return EM_NONE;
  em_word w;
  w.i = (int32_t)v;
  return em_some(w, false);
}

em_ref em_int_checked_add(int32_t a, int32_t b) {
  int64_t r = (int64_t)a + (int64_t)b;
  if (r < -2147483648LL || r > 2147483647LL) return EM_NONE;
  em_word w;
  w.i = (int32_t)r;
  return em_some(w, false);
}

/* JS `Number.prototype.toString` (the shortest digits that read back as the same number), with
 * `.0` added to a finite integral value (spec 10.6). */
static void em_format_float(double x, char *out, size_t size) {
  if (isnan(x)) {
    snprintf(out, size, "NaN");
    return;
  }
  if (isinf(x)) {
    snprintf(out, size, x > 0 ? "Infinity" : "-Infinity");
    return;
  }
  if (x == 0) {
    snprintf(out, size, "0.0");
    return;
  }
  char buf[40];
  for (int p = 1; p <= 17; p++) {
    snprintf(buf, sizeof buf, "%.*e", p - 1, x);
    if (strtod(buf, NULL) == x) break;
  }
  /* buf is `[-]d[.ddd]e(+|-)xx`. */
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
  int n = atoi(q + 1) + 1; /* The position of the decimal point. */
  char *o = out;
  size_t left = size;
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
}

em_ref em_float_to_string(double x) {
  char buf[64];
  em_format_float(x, buf, sizeof buf);
  return em_str_cstr(buf);
}

static bool em_digits(const char *p, size_t n, size_t *i) {
  size_t start = *i;
  while (*i < n && p[*i] >= '0' && p[*i] <= '9') (*i)++;
  return *i > start;
}

/* `Float.parse`: the forms `Float.to_string` writes. */
em_ref em_float_parse(em_ref s) {
  const char *p = EM_STR_BYTES(s);
  size_t n = EM_STR_LEN(s), i = 0;
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
  if (!ok) return EM_NONE;
  em_word w;
  w.f = strtod(p, NULL);
  return em_some(w, false);
}

static int32_t em_float_to_int(double x, double r) {
  if (!(r >= -2147483648.0 && r <= 2147483647.0)) {
    char buf[64], msg[96];
    em_format_float(x, buf, sizeof buf);
    snprintf(msg, sizeof msg, "cannot convert %s to Int", buf);
    em_raise_cstr(msg);
    return 0;
  }
  return (int32_t)r;
}

int32_t em_float_floor(double x) { return em_float_to_int(x, floor(x)); }
int32_t em_float_ceil(double x) { return em_float_to_int(x, ceil(x)); }
/* 0.5 rounds away from zero (spec 16.1). */
int32_t em_float_round(double x) { return em_float_to_int(x, round(x)); }
int32_t em_float_truncate(double x) { return em_float_to_int(x, trunc(x)); }

double em_fmod(double a, double b) { return fmod(a, b); }

static bool em_shift_ok(int32_t by) {
  if (by >= 0) return true;
  char msg[48];
  snprintf(msg, sizeof msg, "negative shift amount: %" PRId32, by);
  em_raise_cstr(msg);
  return false;
}

int32_t em_ishl(int32_t a, int32_t by) {
  if (!em_shift_ok(by)) return 0;
  return by >= 32 ? 0 : (int32_t)((uint32_t)a << by);
}

int32_t em_ishr(int32_t a, int32_t by) {
  if (!em_shift_ok(by)) return 0;
  return a >> (by > 31 ? 31 : by);
}

int32_t em_iushr(int32_t a, int32_t by) {
  if (!em_shift_ok(by)) return 0;
  return by >= 32 ? 0 : (int32_t)((uint32_t)a >> by);
}

int64_t em_lshl(int64_t a, int32_t by) {
  if (!em_shift_ok(by)) return 0;
  return by >= 64 ? 0 : (int64_t)((uint64_t)a << by);
}

int64_t em_lshr(int64_t a, int32_t by) {
  if (!em_shift_ok(by)) return 0;
  return a >> (by > 63 ? 63 : by);
}

int64_t em_lushr(int64_t a, int32_t by) {
  if (!em_shift_ok(by)) return 0;
  return by >= 64 ? 0 : (int64_t)((uint64_t)a >> by);
}

/* ---- Showing ---- */

/* A string as written in source (spec 10.6): `"`, `\`, `#` before `{`, and control characters
 * are escaped as the JS runtime's `$showString` does. */
em_ref em_show_string(em_ref s) {
  em_buf b = {NULL, 0, 0};
  const char *p = EM_STR_BYTES(s);
  size_t n = EM_STR_LEN(s);
  em_buf_put(&b, "\"", 1);
  for (size_t i = 0; i < n; i++) {
    unsigned char c = (unsigned char)p[i];
    switch (c) {
      case '"':
        em_buf_puts(&b, "\\\"");
        break;
      case '\\':
        em_buf_puts(&b, "\\\\");
        break;
      case '#':
        em_buf_puts(&b, i + 1 < n && p[i + 1] == '{' ? "\\#" : "#");
        break;
      case '\n':
        em_buf_puts(&b, "\\n");
        break;
      case '\t':
        em_buf_puts(&b, "\\t");
        break;
      case '\r':
        em_buf_puts(&b, "\\r");
        break;
      default:
        if (c < 0x20 || c == 0x7F) {
          char esc[16];
          snprintf(esc, sizeof esc, "\\u{%x}", c);
          em_buf_puts(&b, esc);
        } else {
          em_buf_put(&b, (const char *)&p[i], 1);
        }
    }
  }
  em_buf_put(&b, "\"", 1);
  return em_buf_finish(&b);
}

void em_debug_print(em_ref s) {
  fflush(stdout);
  fwrite(EM_STR_BYTES(s), 1, EM_STR_LEN(s), stderr);
  fputc('\n', stderr);
}

/* ---- Defects ---- */

void em_raise(em_ref msg) {
  em_self_reference = false;
  if (em_defect == NULL) {
    em_dup(msg);
    em_defect = msg;
    return;
  }
  /* A release failing while a defect unwinds (spec 14.4): a note of the first one. */
  em_ref first = em_defect;
  em_dup(msg);
  em_defect = msg;
  em_defect_resume(first);
}

void em_raise_cstr(const char *msg) {
  em_ref s = em_str_cstr(msg);
  em_raise(s);
  em_drop(s);
}

em_ref em_defect_take(void) {
  em_ref d = em_defect;
  em_defect = NULL;
  if (d == NULL) d = em_str_cstr("");
  return d;
}

void em_defect_resume(em_ref d) {
  em_ref later = em_defect;
  em_defect = d;
  if (later == NULL) return;
  em_buf b = {NULL, 0, 0};
  em_buf_put(&b, EM_STR_BYTES(d), EM_STR_LEN(d));
  em_buf_puts(&b, "\nnote: the release of handler ");
  if (em_last_retired != NULL) {
    em_buf_put(&b, EM_STR_BYTES(em_last_retired), EM_STR_LEN(em_last_retired));
  }
  em_buf_puts(&b, " also failed: ");
  em_buf_put(&b, EM_STR_BYTES(later), EM_STR_LEN(later));
  em_defect = em_buf_finish(&b);
  em_drop(d);
  em_drop(later);
}

void em_retire(em_ref handler, em_ref name) {
  handler->slots[0].i = 1;
  em_dup(name);
  em_drop(em_last_retired);
  em_last_retired = name;
}

void em_handler_used_after_release(em_ref h) {
  char msg[256];
  snprintf(msg, sizeof msg, "handler %s was used after release", h->hdr.desc->name);
  em_raise_cstr(msg);
}

_Noreturn void em_unreachable(void) {
  fflush(stdout);
  fputs("emela: internal error: reached unreachable code\n", stderr);
  abort();
}

_Noreturn void em_unsupported(const char *what) {
  fflush(stdout);
  fprintf(stderr, "emela: %s is not supported on target c yet\n", what);
  exit(EM_EXIT_UNSUPPORTED);
}

/* ---- Constants ---- */

bool em_const_enter(em_const *c, em_word *out) {
  out->l = 0;
  switch (c->state) {
    case 2:
      *out = c->value;
      if (c->ref) em_dup(c->value.p);
      return false;
    case 3:
      em_raise(c->defect);
      return false;
    case 1: {
      char msg[256];
      snprintf(msg, sizeof msg, "const %s refers to itself while being evaluated", c->name);
      em_raise_cstr(msg);
      em_self_reference = true;
      return false;
    }
    default:
      c->state = 1;
      return true;
  }
}

void em_const_leave(em_const *c, em_word value) {
  if (em_defect == NULL) {
    c->value = value;
    if (c->ref) em_dup(value.p);
    c->state = 2;
    return;
  }
  if (em_self_reference) {
    c->state = 0;
    return;
  }
  em_buf b = {NULL, 0, 0};
  em_buf_puts(&b, "while evaluating const ");
  em_buf_puts(&b, c->name);
  em_buf_puts(&b, ": ");
  em_buf_put(&b, EM_STR_BYTES(em_defect), EM_STR_LEN(em_defect));
  em_drop(em_defect);
  em_defect = em_buf_finish(&b);
  em_dup(em_defect);
  c->defect = em_defect;
  c->state = 3;
}

void em_const_release(em_const *c) {
  if (c->state == 2 && c->ref) em_drop(c->value.p);
  em_drop(c->defect);
  c->defect = NULL;
  c->state = 0;
}

/* ---- Evidence tables ---- */

em_ref em_ev_new(em_ref base, int64_t n) {
  int64_t len = base == NULL ? 0 : base->slots[0].l;
  em_ref ev = em_alloc(&em_desc_evidence, (size_t)(1 + 2 * (len + n)));
  ev->slots[0].l = len + n;
  for (int64_t k = 0; k < len; k++) {
    ev->slots[1 + 2 * k] = base->slots[1 + 2 * k];
    ev->slots[2 + 2 * k] = base->slots[2 + 2 * k];
    em_dup(ev->slots[2 + 2 * k].p);
  }
  for (int64_t k = len; k < len + n; k++) {
    ev->slots[1 + 2 * k].i = -1;
    ev->slots[2 + 2 * k].p = NULL;
  }
  em_drop(base);
  return ev;
}

em_ref em_ev_lookup(em_ref ev, int32_t effect) {
  if (ev != NULL) {
    for (int64_t k = ev->slots[0].l - 1; k >= 0; k--) {
      if (ev->slots[1 + 2 * k].i == effect) return ev->slots[2 + 2 * k].p;
    }
  }
  em_unreachable();
}

/* ---- The host boundary ---- */

const char *em_cstr(em_ref s) {
  const char *bytes = EM_STR_BYTES(s);
  if (memchr(bytes, 0, EM_STR_LEN(s)) != NULL) {
    em_raise_cstr("a string passed to C contains U+0000");
    return NULL;
  }
  return bytes;
}

em_ref em_from_host_string(char *s, const char *function) {
  if (s == NULL) {
    char msg[256];
    snprintf(msg, sizeof msg, "external function `%s` returned NULL", function);
    em_raise_cstr(msg);
    return NULL;
  }
  const unsigned char *p = (const unsigned char *)s;
  size_t n = strlen(s);
  em_buf b = {NULL, 0, 0};
  for (size_t i = 0; i < n;) {
    size_t bad = 0, len = em_utf8_valid(p, n, i, &bad);
    if (len > 0) {
      em_buf_put(&b, s + i, len);
      i += len;
    } else {
      em_buf_puts(&b, "\xEF\xBF\xBD");
      i += bad;
    }
  }
  free(s);
  return em_buf_finish(&b);
}

/* ---- The program ---- */

void em_start(int argc, char **argv) {
  (void)argc;
  (void)argv;
}

/* ---- Tests (spec 13.4) ---- */

#define EM_TEST_EVENT "\x1e" "emela-test "

static void em_json_string(const char *s, size_t n) {
  putchar('"');
  for (size_t i = 0; i < n; i++) {
    unsigned char c = (unsigned char)s[i];
    if (c == '"' || c == '\\') {
      putchar('\\');
      putchar(c);
    } else if (c == '\n') {
      fputs("\\n", stdout);
    } else if (c < 0x20 || c == 0x7F) {
      printf("\\u%04x", c);
    } else {
      putchar(c);
    }
  }
  putchar('"');
}

void em_test_result(const char *name) {
  const char *outcome = "ok";
  em_ref message = NULL;
  if (em_defect != NULL) {
    outcome = "defect";
    message = em_defect;
    em_defect = NULL;
  } else if (em_failure != NULL) {
    outcome = "error";
    em_ref e = em_failure;
    em_failure = NULL;
    em_ref (*show)(em_ref) = e->hdr.desc->show;
    if (show != NULL) {
      message = show(e);
      if (em_defect != NULL) {
        em_drop(message);
        message = em_defect;
        em_defect = NULL;
      }
    } else {
      em_drop(e);
    }
  }
  fputs(EM_TEST_EVENT "{\"event\":\"result\",\"name\":", stdout);
  em_json_string(name, strlen(name));
  printf(",\"outcome\":\"%s\"", outcome);
  if (message != NULL) {
    fputs(",\"message\":", stdout);
    em_json_string(EM_STR_BYTES(message), EM_STR_LEN(message));
    em_drop(message);
  }
  fputs("}\n", stdout);
  fflush(stdout);
}

void em_test_done(void) {
  fputs(EM_TEST_EVENT "{\"event\":\"done\"}\n", stdout);
  fflush(stdout);
}

static void em_write_line(FILE *f, const char *prefix, em_ref s) {
  fputs(prefix, f);
  if (s != NULL) fwrite(EM_STR_BYTES(s), 1, EM_STR_LEN(s), f);
  fputc('\n', f);
}

int em_exit(void (*cleanup)(void)) {
  int code = 0;
  fflush(stdout);
  if (em_defect != NULL) {
    em_write_line(stderr, "defect: ", em_defect);
    code = EM_EXIT_DEFECT;
  } else if (em_failure != NULL) {
    em_ref e = em_failure;
    em_failure = NULL;
    em_ref (*show)(em_ref) = e->hdr.desc->show;
    if (show != NULL) {
      em_ref s = show(e);
      em_write_line(stderr, "error: ", em_defect == NULL ? s : NULL);
      if (em_defect == NULL) em_drop(s);
    } else {
      em_write_line(stderr, "error: ", NULL);
      em_drop(e);
    }
    code = EM_EXIT_ERROR;
  }
  em_drop(em_defect);
  em_defect = NULL;
  if (cleanup != NULL) cleanup();
  em_drop(em_last_retired);
  em_last_retired = NULL;
  fflush(stdout);
  const char *check = getenv("EMELA_LEAK_CHECK");
  if (check != NULL && *check != '\0' && em_live != 0) {
    fprintf(stderr, "emela: leak check: %" PRId64 " allocations are still live\n", em_live);
    code = EM_EXIT_LEAK;
  }
  fflush(stderr);
  return code;
}
