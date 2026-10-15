/* The Emela C runtime. See emela_runtime.h for the value representation.
 *
 * Builtins follow the JS runtime (crates/emela-codegen-js/src/runtime.mjs) exactly: the same
 * results, the same defect messages. */
/* poll(2), pipe(2), clock_gettime and pthreads for the event loop (spec 12.6). */
#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include "emela_runtime.h"

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <math.h>
#include <poll.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* The exit codes of spec appendix B. */
#define EM_EXIT_ERROR 1
#define EM_EXIT_DEFECT 101
/* A leak found by the check at exit (EMELA_LEAK_CHECK). */
#define EM_EXIT_LEAK 99
/* The run was unwound as interrupted (spec 14.5). */
#define EM_EXIT_INTERRUPTED 130

em_ref em_defect = NULL;
em_ref em_failure = NULL;

/* The number of live heap objects, for the leak check. */
static int64_t em_live = 0;
/* The name of the handler retired last: the release that raised a defect while another one was
 * unwinding names it in the note (spec 14.4). */
static em_ref em_last_retired = NULL;
/* Whether the pending defect is a const referring to itself (not remembered by consts). */
static bool em_self_reference = false;

/* Defined with the fibers below. */
static bool em_is_interrupted(em_ref x);
static em_ref em_show_failure(em_ref e);
static void em_notes_clear(void);

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
      case EM_KIND_RUNTIME:
        if (d->fin != NULL) d->fin(x);
        break;
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
        /* A defect, so that the releases run and attempt can catch it (the Heap IR counts split
         * among the builtins that may raise one). */
        em_raise_cstr(
            "`String.split` with an empty separator on non-ASCII text needs the Unicode grapheme "
            "cluster tables, which target `c` does not support yet");
        return NULL;
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

/* em_raise, taking the reference to `d` (a String, or an interruption). */
static void em_raise_take(em_ref d) {
  em_raise(d);
  em_drop(d);
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
  /* Unwinding as interrupted: a defect of the cleanup replaces the interruption, and so does an
   * interruption (a suspend operation of a release dropped). Unwinding a defect: an
   * interruption of the cleanup is no note (spec 14.4). */
  if (em_is_interrupted(d)) {
    em_drop(d);
    em_defect = later;
    return;
  }
  if (em_is_interrupted(later)) {
    em_drop(later);
    return;
  }
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
  if (em_self_reference || em_is_interrupted(em_defect)) {
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

/* ---- Suspension and fibers (spec chapter 11, 14.5) ----
 *
 * This follows the JS runtime's scheduler (crates/emela-codegen-js/src/runtime.mjs: `$Root`,
 * `$Fiber`, `$Scope`, `$performOp`) step by step, so that both targets switch fibers in the same
 * order. Continuations are lists of frames (see the header) instead of generators:
 *
 * - A run (`em_root`: main or one test) is a scheduler of fibers on one thread. Jobs run in the
 *   order they are queued. A fiber woken by another (a child that ended, a completed Deferred, a
 *   resume from another fiber) is queued and runs after the running one suspends.
 * - A suspension starts at a request: a fairness yield, a `suspend fn` operation not resumed in
 *   place, or a fiber operation that waits. The request makes a wait (`em_wait`) and sets
 *   `em_suspending`; the frames bubble up, and the driver loop (`em_advance`) attaches the
 *   captured frames to the wait. Waking the wait continues them, on the spot within the same
 *   fiber (spec 14.5, item 4) or as a queued job.
 * - An interruption is delivered as the pending defect (an `em_desc_interrupted` object), so the
 *   defect landings of the Heap IR unwind it and run the releases. A continuation abandoned while
 *   suspended (the run stalls, the fiber is interrupted) is resumed with it.
 * - Where the host's event loop would wait for timers and I/O (`em_run`), nothing waits yet: when
 *   no job is left, the run has stalled (the JS event loop is empty). The C event loop of spec
 *   12.6 plugs in there (see NOTES.md). */

#define EM_BUDGET 1000
#define EM_HOST_EVERY 16

enum { EM_VALUE = 0, EM_FAIL = 1, EM_THROW = 2 };

/* What a continuation goes on with: a value, a failure (the error) or a thrown defect or
 * interruption. `ref`: the value is a reference the input owns. */
typedef struct em_input {
  int32_t mode;
  bool ref;
  em_word value;
} em_input;

static em_input em_in_value(em_word w, bool ref) {
  em_input in = {EM_VALUE, ref, w};
  return in;
}

static em_input em_in_ref(int32_t mode, em_ref p) {
  em_input in = {mode, true, {.p = p}};
  return in;
}

static em_word em_zero(void) {
  em_word w;
  w.l = 0;
  return w;
}

static void em_input_drop(em_input in) {
  if (in.ref) em_drop(in.value.p);
}

/* The input after a call returned `w`: the pending defect or failure, taken, or the value. */
static em_input em_input_taken(em_word w, bool ref) {
  if (em_defect != NULL) {
    em_ref d = em_defect;
    em_defect = NULL;
    return em_in_ref(EM_THROW, d);
  }
  if (em_failure != NULL) {
    em_ref e = em_failure;
    em_failure = NULL;
    return em_in_ref(EM_FAIL, e);
  }
  return em_in_value(w, ref);
}

/* Makes the input the result of the current call: the value returned, or a pending slot. */
static em_word em_input_put(em_input in) {
  switch (in.mode) {
    case EM_FAIL:
      em_failure = in.value.p;
      return em_zero();
    case EM_THROW:
      em_defect = in.value.p;
      return em_zero();
    default:
      return in.value;
  }
}

/* Runtime structures not reachable as Emela values are counted as live allocations too. */
static void *em_rt_alloc(size_t size) {
  void *p = calloc(1, size);
  if (p == NULL) em_out_of_memory();
  em_live++;
  return p;
}

static void em_rt_free(void *p) {
  free(p);
  em_live--;
}

typedef struct em_vec {
  void **items;
  size_t len;
  size_t cap;
} em_vec;

static void em_vec_push(em_vec *v, void *x) {
  if (v->len == v->cap) {
    size_t cap = v->cap == 0 ? 4 : v->cap * 2;
    void **items = realloc(v->items, cap * sizeof(void *));
    if (items == NULL) em_out_of_memory();
    v->items = items;
    v->cap = cap;
  }
  v->items[v->len++] = x;
}

static bool em_vec_has(const em_vec *v, const void *x) {
  for (size_t i = 0; i < v->len; i++) {
    if (v->items[i] == x) return true;
  }
  return false;
}

/* Removes every occurrence, keeping the order. */
static void em_vec_remove(em_vec *v, const void *x) {
  size_t k = 0;
  for (size_t i = 0; i < v->len; i++) {
    if (v->items[i] != x) v->items[k++] = v->items[i];
  }
  v->len = k;
}

static void em_vec_free(em_vec *v) {
  free(v->items);
  v->items = NULL;
  v->len = v->cap = 0;
}

/* Calls the closure `f` with no arguments, or with `a`, returning a value of `repr`. */
static em_word em_call0(em_ref f, int32_t repr) {
  em_fnptr fn = em_closure_fn(f);
  em_word w = em_zero();
  switch (repr) {
    case EM_REPR_I32:
      w.i = ((int32_t (*)(em_ref))fn)(f);
      break;
    case EM_REPR_I64:
      w.l = ((int64_t (*)(em_ref))fn)(f);
      break;
    case EM_REPR_F64:
      w.f = ((double (*)(em_ref))fn)(f);
      break;
    case EM_REPR_REF:
      w.p = ((em_ref (*)(em_ref))fn)(f);
      break;
    default:
      ((void (*)(em_ref))fn)(f);
      break;
  }
  return w;
}

static em_word em_call1(em_ref f, em_ref a, int32_t repr) {
  em_fnptr fn = em_closure_fn(f);
  em_word w = em_zero();
  switch (repr) {
    case EM_REPR_I32:
      w.i = ((int32_t (*)(em_ref, em_ref))fn)(f, a);
      break;
    case EM_REPR_I64:
      w.l = ((int64_t (*)(em_ref, em_ref))fn)(f, a);
      break;
    case EM_REPR_F64:
      w.f = ((double (*)(em_ref, em_ref))fn)(f, a);
      break;
    case EM_REPR_REF:
      w.p = ((em_ref (*)(em_ref, em_ref))fn)(f, a);
      break;
    default:
      ((void (*)(em_ref, em_ref))fn)(f, a);
      break;
  }
  return w;
}

/* A value of `Any` (owned) as a value of `repr`: a reference is itself, anything else is boxed. */
static em_word em_unbox(em_ref any, int32_t repr) {
  if (repr == EM_REPR_REF) {
    em_word w;
    w.p = any;
    return w;
  }
  em_word w = repr == EM_REPR_VOID ? em_zero() : any->slots[0];
  em_drop(any);
  return w;
}

/* ---- Interruptions, causes and Exit ---- */

static const uint8_t em_refs_frame1[4] = {0, 0, 1, 0};
static const uint8_t em_refs_cause[2] = {0, 1};

/* An interruption (`$Interrupted`), pending as a defect: slot 0 the scope whose body it unwinds,
 * or NULL for the whole fiber. */
static const em_desc em_desc_interrupted = {EM_KIND_FIELDS, 1, 0, -1, "interrupted", NULL,
                                            em_refs_one, NULL};
/* A `Cause[E]`: slot 0 the kind (CauseKind's tag), slot 1 the error (Fail) or the defect
 * message (Die). */
static const em_desc em_desc_cause = {EM_KIND_FIELDS, 2, 0, -1, "Cause", NULL, em_refs_cause,
                                      NULL};
/* `Exit`: `Success(A)` is 0, `Failure(Cause[E])` is 1. */
static const em_desc em_desc_success_val = {EM_KIND_FIELDS, 1, 0, -1, "Success", NULL,
                                            em_refs_none, NULL};
static const em_desc em_desc_success_ref = {EM_KIND_FIELDS, 1, 0, -1, "Success", NULL,
                                            em_refs_one, NULL};
static const em_desc em_desc_failure = {EM_KIND_FIELDS, 1, 1, -1, "Failure", NULL, em_refs_one,
                                        NULL};

enum { EM_CAUSE_FAIL = 0, EM_CAUSE_DIE = 1, EM_CAUSE_INTERRUPT = 2 };

static bool em_is_interrupted(em_ref x) {
  return x != NULL && x->hdr.desc == &em_desc_interrupted;
}

static em_ref em_interrupted_new(em_ref scope) {
  em_ref o = em_alloc(&em_desc_interrupted, 1);
  em_dup(scope);
  o->slots[0].p = scope;
  return o;
}

/* Takes `value` (the error or the defect, or NULL). */
static em_ref em_cause_new(int32_t kind, em_ref value) {
  em_ref c = em_alloc(&em_desc_cause, 2);
  c->slots[0].l = 0;
  c->slots[0].i = kind;
  c->slots[1].p = value;
  return c;
}

/* The cause of how a fiber ended, or NULL for a value. */
static em_ref em_cause_of(em_input in) {
  switch (in.mode) {
    case EM_FAIL:
      em_dup(in.value.p);
      return em_cause_new(EM_CAUSE_FAIL, in.value.p);
    case EM_THROW:
      if (em_is_interrupted(in.value.p)) return em_cause_new(EM_CAUSE_INTERRUPT, NULL);
      em_dup(in.value.p);
      return em_cause_new(EM_CAUSE_DIE, in.value.p);
    default:
      return NULL;
  }
}

static em_ref em_exit_new(int32_t tag, em_word value, bool ref) {
  em_ref o = em_alloc(tag == 0 ? (ref ? &em_desc_success_ref : &em_desc_success_val)
                               : &em_desc_failure,
                      1);
  o->slots[0] = value;
  return o;
}

/* Notes of later failures shown after a failure (spec 11.2): the error values they belong to,
 * and the lines, for the rest of the run. JS keeps them in the `$Fail` wrapper, which C does
 * not have. */
static em_vec em_note_errors;
static em_vec em_note_lines;

static em_ref em_notes_of(em_ref error) {
  for (size_t i = 0; i < em_note_errors.len; i++) {
    if (em_note_errors.items[i] == error) return em_note_lines.items[i];
  }
  return NULL;
}

/* Adds the note to the error in `*slot`. A static error (a constructor without fields) is
 * copied first, so that the notes stay with this failure. */
static void em_notes_add(em_ref *slot, em_ref note) {
  em_ref e = *slot;
  if (e->hdr.u.h.rc == 0) {
    const em_desc *d = e->hdr.desc;
    em_ref copy = em_alloc(d, d->nfields);
    for (uint32_t i = 0; i < d->nfields; i++) {
      copy->slots[i] = e->slots[i];
      if (d->refs[i]) em_dup(copy->slots[i].p);
    }
    *slot = copy;
    e = copy;
  }
  for (size_t i = 0; i < em_note_errors.len; i++) {
    if (em_note_errors.items[i] == e) {
      em_ref old = em_note_lines.items[i];
      em_buf b = {NULL, 0, 0};
      em_buf_put(&b, EM_STR_BYTES(old), EM_STR_LEN(old));
      em_buf_puts(&b, "\n");
      em_buf_put(&b, EM_STR_BYTES(note), EM_STR_LEN(note));
      em_note_lines.items[i] = em_buf_finish(&b);
      em_drop(old);
      return;
    }
  }
  em_dup(e);
  em_dup(note);
  em_vec_push(&em_note_errors, e);
  em_vec_push(&em_note_lines, note);
}

static void em_notes_clear(void) {
  for (size_t i = 0; i < em_note_errors.len; i++) {
    em_drop(em_note_errors.items[i]);
    em_drop(em_note_lines.items[i]);
  }
  em_vec_free(&em_note_errors);
  em_vec_free(&em_note_lines);
}

/* `error: NotFound(id: 1)` without the prefix, with its notes; consumes `e`. NULL when it cannot
 * be shown. */
static em_ref em_show_failure(em_ref e) {
  em_ref notes = em_notes_of(e);
  if (notes != NULL) em_dup(notes);
  em_ref (*show)(em_ref) = e->hdr.desc->show;
  em_ref s = NULL;
  if (show != NULL) {
    s = show(e);
    if (em_defect != NULL) {
      em_drop(s);
      s = NULL;
    }
  } else {
    em_drop(e);
  }
  if (notes == NULL) return s;
  em_buf b = {NULL, 0, 0};
  if (s != NULL) em_buf_put(&b, EM_STR_BYTES(s), EM_STR_LEN(s));
  em_buf_puts(&b, "\n");
  em_buf_put(&b, EM_STR_BYTES(notes), EM_STR_LEN(notes));
  em_drop(s);
  em_drop(notes);
  return em_buf_finish(&b);
}

em_ref em_cause_to_string(em_ref c) {
  switch (c->slots[0].i) {
    case EM_CAUSE_FAIL: {
      em_dup(c->slots[1].p);
      em_ref shown = em_show_failure(c->slots[1].p);
      em_buf b = {NULL, 0, 0};
      em_buf_puts(&b, "error: ");
      if (shown != NULL) em_buf_put(&b, EM_STR_BYTES(shown), EM_STR_LEN(shown));
      em_drop(shown);
      return em_buf_finish(&b);
    }
    case EM_CAUSE_DIE: {
      em_ref d = c->slots[1].p;
      em_buf b = {NULL, 0, 0};
      em_buf_puts(&b, "defect: ");
      em_buf_put(&b, EM_STR_BYTES(d), EM_STR_LEN(d));
      return em_buf_finish(&b);
    }
    default:
      return em_str_cstr("interrupted");
  }
}

int32_t em_cause_kind(em_ref c) { return c->slots[0].i; }

void em_cause_raise(em_ref c) {
  em_ref v = c->slots[1].p;
  switch (c->slots[0].i) {
    case EM_CAUSE_FAIL:
      em_dup(v);
      em_failure = v;
      break;
    case EM_CAUSE_DIE:
      em_dup(v);
      em_raise_take(v);
      break;
    default:
      em_raise_take(em_interrupted_new(NULL));
      break;
  }
}

/* Adds the later failure `other` as a note to the display of `first` (spec 11.2). */
static void em_cause_note(em_ref first, em_ref other) {
  em_ref shown = em_cause_to_string(other);
  em_buf b = {NULL, 0, 0};
  em_buf_puts(&b, "note: another fiber also failed: ");
  em_buf_put(&b, EM_STR_BYTES(shown), EM_STR_LEN(shown));
  em_drop(shown);
  em_ref note = em_buf_finish(&b);
  switch (first->slots[0].i) {
    case EM_CAUSE_FAIL:
      em_notes_add(&first->slots[1].p, note);
      break;
    case EM_CAUSE_DIE: {
      em_ref d = first->slots[1].p;
      em_buf m = {NULL, 0, 0};
      em_buf_put(&m, EM_STR_BYTES(d), EM_STR_LEN(d));
      em_buf_puts(&m, "\n");
      em_buf_put(&m, EM_STR_BYTES(note), EM_STR_LEN(note));
      first->slots[1].p = em_buf_finish(&m);
      em_drop(d);
      break;
    }
    default:
      break;
  }
  em_drop(note);
}

/* ---- The scheduler's structures ---- */

typedef struct em_root em_root;
typedef struct em_fiber em_fiber;
typedef struct em_scope em_scope;
typedef struct em_perform em_perform;
/* A task's record (the host sees a handle, `em_task *`; see the event loop's section). */
typedef struct em_htask em_htask;

/* Who receives a continuation's result: the fiber (its main continuation), the operation call
 * whose implementation it runs, or nobody (an implementation unwound when the run stalls; its
 * defect may become the run's result). */
enum { EM_END_MAIN, EM_END_IMPL, EM_END_ORPHAN };

/* What a wait waits for. */
enum { EM_WAIT_PAUSE, EM_WAIT_PERFORM, EM_WAIT_FIBER, EM_WAIT_HOST };

/* A continuation waiting to be continued (JS: the `wait` of `$advance`). Counted by hand. */
typedef struct em_wait {
  int refs;
  int kind;
  /* In the run's list of waits: suspended and not yet taken or dropped. */
  bool live;
  /* A scope waiting for its children: not cut short when the run is unwound. */
  bool closing;
  em_ref chain;
  /* Counted through the fiber's handle. */
  em_fiber *fiber;
  int end;
  /* EM_END_IMPL: the operation call (counted). */
  em_perform *impl;
  /* A caller waiting for a resume: the operation call whose implementation is unwound when the
   * caller is dropped (counted). */
  em_perform *on_drop;
  /* EM_WAIT_HOST: the task of the `suspend fn` external function (not counted; the task counts
   * the wait). Cancelled when the wait is dropped. */
  em_htask *task;
  struct em_wait *prev, *next;
} em_wait;

static void em_task_cancel(em_htask *t);

/* A fiber operation's waiter (join, await, interrupt) or a Deferred's. */
enum { EM_VERB_JOIN, EM_VERB_AWAIT, EM_VERB_INTERRUPT, EM_VERB_DEFERRED };

typedef struct em_watch {
  em_wait *w;
  int verb;
  int32_t repr;
} em_watch;

struct em_fiber {
  /* The handle (`Fiber[A]`), which owns this structure. */
  em_ref self;
  em_root *root;
  /* The scope it was forked into (its object counted), or NULL for the root fiber. */
  em_scope *scope;
  /* The main continuation's wait while it is suspended (not counted). */
  em_wait *wait;
  /* Interruptions not yet delivered: NULL (the whole fiber) or a scope whose body runs on it. */
  em_vec pending;
  bool unwinding;
  /* Scopes whose body unwinds by an interruption delivered to this fiber. */
  em_vec unwinds;
  /* Above 0 while it runs a release that may suspend or a scope waits for its children. */
  int holds;
  int scopes;
  int budget;
  bool ended;
  /* In the run's list of fibers, which counts it. */
  bool listed;
  /* Once ended: how (a child's value is `Any`, a reference). */
  em_input result;
  em_ref cause;
  em_vec watchers;
};

struct em_scope {
  /* The capability value (`fiber_scope`), which owns this structure. */
  em_ref self;
  /* The fiber its body runs on (not counted). */
  em_fiber *fiber;
  int depth;
  /* Children that have not ended (not counted: the run's list counts them). */
  em_vec children;
  bool closed;
  bool body_done;
  bool ended;
  /* The first failure (a Fail or Die cause) of the body or a child. */
  em_ref first;
  /* The scope's wait for its last child (counted). */
  em_wait *closer;
};

typedef struct em_deferred {
  em_ref self;
  /* The scope's object, counted. */
  em_ref scope;
  bool full;
  bool ref;
  em_word value;
  em_vec waiters;
} em_deferred;

/* A call of a `suspend fn` operation (JS: the closures of `$performOp`). Counted by hand. */
struct em_perform {
  int refs;
  /* The run it belongs to (`em_run_id`): a resume after the run is ignored. */
  uint64_t run;
  char *where;
  /* Made inside a non-suspending function (spec 8.7). */
  bool sync;
  /* The implementation is still being called in place. */
  bool sync_phase;
  bool used;
  bool prev_main;
  /* What to continue with, passed during the call. */
  bool has_now;
  em_input now;
  /* The caller's wait once it suspended (counted). */
  em_wait *caller;
  /* The implementation's wait while it is suspended (counted). */
  em_wait *impl_wait;
};

enum {
  EM_JOB_ENTRY,
  EM_JOB_BEGIN,
  EM_JOB_RESUME,
  EM_JOB_GO,
  EM_JOB_INTERRUPT,
  EM_JOB_ROOT_THROW,
  EM_JOB_DROP_IMPL,
};

typedef struct em_job {
  int kind;
  em_wait *w;
  em_fiber *fiber;
  em_ref fn;
  em_input in;
  int end;
} em_job;

typedef struct em_queue {
  em_job *items;
  size_t head;
  size_t len;
  size_t cap;
} em_queue;

struct em_root {
  bool over;
  bool running;
  bool has_result;
  em_input result;
  int32_t result_repr;
  em_word (*entry)(void);
  em_queue queue;
  /* Jobs that go through the host's event loop first (`setImmediate` in JS). */
  em_queue later;
  em_wait *first_wait, *last_wait;
  /* Fibers that have not ended (counted). */
  em_vec fibers;
  /* The first defect of a continuation unwound when the run stalled that was not a fiber's. */
  em_ref orphan;
  int64_t pauses;
};

bool em_suspending = false;
static em_ref em_susp_head = NULL, em_susp_tail = NULL;
static em_wait *em_susp_wait = NULL;
/* Calls of suspending code from non-suspending functions under way (spec 8.7). */
static int em_sync_depth = 0;
/* A defect that ended the run of such a call (a failure after resume): its result. */
static em_ref em_sync_defect = NULL;
/* The fiber being stepped, and whether its main continuation runs (not an operation's). */
static em_fiber *em_cur = NULL;
static bool em_cur_main = false;
static em_root *em_root_cur = NULL;
static uint64_t em_run_id = 0;

static em_scope *em_scope_of(em_ref o) { return (em_scope *)o->slots[0].h; }
static em_fiber *em_fiber_of(em_ref o) { return (em_fiber *)o->slots[0].h; }

/* ---- Frames and the captured continuation ---- */

em_ref em_frame_new(const em_desc *desc, em_resume_fn resume, int32_t state, size_t nsaved) {
  em_ref fr = em_alloc(desc, 3 + nsaved);
  fr->slots[0].fn = (em_fnptr)resume;
  fr->slots[1].l = 0;
  fr->slots[1].i = state;
  fr->slots[2].p = NULL;
  return fr;
}

void em_frame_push(em_ref fr) {
  fr->slots[2].p = NULL;
  if (em_susp_tail == NULL) {
    em_susp_head = fr;
  } else {
    em_susp_tail->slots[2].p = fr;
  }
  em_susp_tail = fr;
}

void em_frame_free(em_ref fr) {
  free(fr);
  em_live--;
}

/* Starts a suspension at the request `w` (NULL: the run is over, and what bubbles up is dropped
 * without being resumed). */
static void em_suspend_on(em_wait *w) {
  em_suspending = true;
  em_susp_wait = w;
  em_susp_head = em_susp_tail = NULL;
}

/* Takes the bubbled suspension: the captured frames (innermost first, the outermost's next is
 * `rest`) and its request. */
static em_wait *em_take_suspension(em_ref rest, em_ref *chain) {
  em_wait *w = em_susp_wait;
  if (em_susp_tail != NULL) {
    em_susp_tail->slots[2].p = rest;
    *chain = em_susp_head;
  } else {
    *chain = rest;
  }
  em_suspending = false;
  em_susp_wait = NULL;
  em_susp_head = em_susp_tail = NULL;
  return w;
}

/* ---- Waits ---- */

static void em_perform_unref(em_perform *p);

static em_wait *em_wait_new(int kind, bool closing) {
  em_wait *w = em_rt_alloc(sizeof(em_wait));
  w->refs = 1;
  w->kind = kind;
  w->closing = closing;
  return w;
}

static void em_wait_ref(em_wait *w) { w->refs++; }

static void em_wait_unref(em_wait *w) {
  if (w == NULL || --w->refs > 0) return;
  em_drop(w->chain);
  if (w->fiber != NULL) em_drop(w->fiber->self);
  if (w->impl != NULL) em_perform_unref(w->impl);
  if (w->on_drop != NULL) em_perform_unref(w->on_drop);
  em_rt_free(w);
}

static void em_root_unlist(em_root *r, em_wait *w) {
  if (w->prev != NULL) w->prev->next = w->next;
  else r->first_wait = w->next;
  if (w->next != NULL) w->next->prev = w->prev;
  else r->last_wait = w->prev;
  w->prev = w->next = NULL;
}

/* Takes the suspended continuation back to continue it, unless it was unwound meanwhile. The
 * run's reference to the wait passes to the caller. */
static bool em_take(em_root *r, em_wait *w) {
  if (!w->live) return false;
  w->live = false;
  em_root_unlist(r, w);
  if (w->fiber->wait == w) w->fiber->wait = NULL;
  if (w->on_drop != NULL) {
    em_perform_unref(w->on_drop);
    w->on_drop = NULL;
  }
  return true;
}

static void em_queue_push(em_queue *q, em_job job) {
  if (q->len == q->cap) {
    size_t cap = q->cap == 0 ? 16 : q->cap * 2;
    em_job *items = malloc(cap * sizeof(em_job));
    if (items == NULL) em_out_of_memory();
    for (size_t i = 0; i < q->len; i++) items[i] = q->items[(q->head + i) % q->cap];
    free(q->items);
    q->items = items;
    q->head = 0;
    q->cap = cap;
  }
  q->items[(q->head + q->len) % q->cap] = job;
  q->len++;
}

static em_job em_queue_pop(em_queue *q) {
  em_job job = q->items[q->head];
  q->head = (q->head + 1) % q->cap;
  q->len--;
  return job;
}

static void em_job_drop(em_job job) {
  em_wait_unref(job.w);
  if (job.fiber != NULL) em_drop(job.fiber->self);
  em_drop(job.fn);
  em_input_drop(job.in);
}

static void em_run_job(em_root *r, em_job job);

static void em_drain(em_root *r) {
  r->running = true;
  while (!r->over && r->queue.len > 0) em_run_job(r, em_queue_pop(&r->queue));
  r->running = false;
}

static void em_schedule(em_root *r, em_job job) {
  em_queue_push(&r->queue, job);
  if (!r->running) em_drain(r);
}

static em_job em_job_of(int kind, em_wait *w) {
  em_job job;
  memset(&job, 0, sizeof job);
  job.kind = kind;
  job.w = w;
  return job;
}

/* Drops a waiting continuation that is unwound or abandoned (`$drop`): it will not be continued,
 * and the implementation it waits for is unwound. The run's reference passes to the caller. */
static void em_drop_wait(em_root *r, em_wait *w) {
  w->live = false;
  em_root_unlist(r, w);
  if (w->fiber->wait == w) w->fiber->wait = NULL;
  /* `wait.controller.abort()`: the host function is told (`em_on_cancel`). */
  if (w->task != NULL) em_task_cancel(w->task);
  em_perform *p = w->on_drop;
  w->on_drop = NULL;
  if (p != NULL) {
    /* `wait.onDrop`: the implementation that has not resumed is unwound too. */
    em_wait *iw = p->impl_wait;
    if (!p->used && iw != NULL && iw->live) {
      em_drop_wait(r, iw);
      em_queue_push(&r->queue, em_job_of(EM_JOB_DROP_IMPL, iw));
    }
    em_perform_unref(p);
  }
}

/* ---- Fibers ---- */

static void em_fiber_fin(em_ref o);
static void em_scope_fin(em_ref o);
static void em_deferred_fin(em_ref o);
static void em_resume_fin(em_ref o);

static const em_desc em_desc_fiber = {EM_KIND_RUNTIME, 1, 0, -1, "Fiber", NULL, em_refs_none,
                                      em_fiber_fin};
static const em_desc em_desc_scope = {EM_KIND_RUNTIME, 1, 0, -1, "Scope", NULL, em_refs_none,
                                      em_scope_fin};
static const em_desc em_desc_deferred = {EM_KIND_RUNTIME, 1, 0, -1, "Deferred", NULL,
                                         em_refs_none, em_deferred_fin};
static const em_desc em_desc_resume = {EM_KIND_RUNTIME, 2, 0, -1, "resume", NULL, em_refs_none,
                                       em_resume_fin};

static em_fiber *em_fiber_new(em_root *r, em_scope *scope) {
  em_fiber *f = em_rt_alloc(sizeof(em_fiber));
  f->self = em_alloc(&em_desc_fiber, 1);
  f->self->slots[0].h = f;
  f->root = r;
  f->scope = scope;
  if (scope != NULL) em_dup(scope->self);
  f->budget = EM_BUDGET;
  f->listed = true;
  em_vec_push(&r->fibers, f);
  return f;
}

static void em_fiber_fin(em_ref o) {
  em_fiber *f = em_fiber_of(o);
  if (f->scope != NULL) {
    if (!f->ended) em_vec_remove(&f->scope->children, f);
    em_drop(f->scope->self);
  }
  if (f->ended) em_input_drop(f->result);
  em_drop(f->cause);
  for (size_t i = 0; i < f->watchers.len; i++) {
    em_watch *watch = f->watchers.items[i];
    em_wait_unref(watch->w);
    free(watch);
  }
  em_vec_free(&f->watchers);
  em_vec_free(&f->pending);
  em_vec_free(&f->unwinds);
  em_rt_free(f);
}

static bool em_interrupting(em_fiber *f) {
  return f->holds == 0 && (f->pending.len > 0 || f->unwinding || f->unwinds.len > 0);
}

static bool em_is_unwinding(em_fiber *f) { return f->unwinding || f->unwinds.len > 0; }

/* Takes the outermost interruption (pending or unwinding) and returns what unwinds it. */
static em_ref em_deliver(em_fiber *f) {
  em_scope *target = NULL;
  if (!f->unwinding && !em_vec_has(&f->pending, NULL)) {
    for (size_t i = 0; i < f->pending.len + f->unwinds.len; i++) {
      em_scope *s = i < f->pending.len ? f->pending.items[i] : f->unwinds.items[i - f->pending.len];
      if (target == NULL || s->depth < target->depth) target = s;
    }
  }
  if (target == NULL) {
    f->unwinding = true;
    f->pending.len = 0;
    f->unwinds.len = 0;
    return em_interrupted_new(NULL);
  }
  em_vec_remove(&f->pending, target);
  if (!em_vec_has(&f->unwinds, target)) em_vec_push(&f->unwinds, target);
  return em_interrupted_new(target->self);
}

/* Delivers a pending interruption at a request of the current fiber's main continuation
 * (spec 11.4): true if it did (the interruption is the pending defect). */
static bool em_interrupt_here(void) {
  if (em_cur == NULL || !em_cur_main || em_sync_depth > 0 || !em_interrupting(em_cur)) {
    return false;
  }
  em_defect = em_deliver(em_cur);
  return true;
}

static void em_step(em_root *r, em_fiber *f, em_ref chain, int end, em_perform *impl,
                    em_input in);

/* Asks the fiber to unwind as interrupted (11.4). `target` is NULL for the whole fiber, or a
 * scope whose body runs on it. It takes effect at the next suspension point, not during a
 * release. */
static void em_interrupt_fiber(em_fiber *f, em_scope *target) {
  if (f->ended || f->unwinding) return;
  if (target == NULL ? em_vec_has(&f->pending, NULL)
                     : em_vec_has(&f->pending, target) || em_vec_has(&f->unwinds, target)) {
    return;
  }
  em_vec_push(&f->pending, target);
  if (f->holds > 0) return;
  em_wait *w = f->wait;
  if (w == NULL || !w->live) return;
  em_drop_wait(f->root, w);
  em_schedule(f->root, em_job_of(EM_JOB_INTERRUPT, w));
}

static em_input em_watch_settle(em_fiber *c, int verb, int32_t repr) {
  em_ref cause = c->cause;
  switch (verb) {
    case EM_VERB_JOIN:
      if (cause == NULL) {
        em_dup(c->result.value.p);
        return em_in_value(em_unbox(c->result.value.p, repr), repr == EM_REPR_REF);
      }
      if (cause->slots[0].i == EM_CAUSE_INTERRUPT) {
        return em_in_ref(EM_THROW, em_str_cstr("fiber was interrupted"));
      }
      return em_in_ref(EM_THROW, em_interrupted_new(NULL));
    case EM_VERB_AWAIT: {
      em_word w;
      if (cause == NULL) {
        em_dup(c->result.value.p);
        em_word v = em_unbox(c->result.value.p, repr);
        w.p = em_exit_new(0, v, repr == EM_REPR_REF);
      } else {
        em_dup(cause);
        em_word v;
        v.p = cause;
        w.p = em_exit_new(1, v, true);
      }
      return em_in_value(w, true);
    }
    default:
      return em_in_value(em_zero(), false);
  }
}

static void em_cont(em_wait *w, em_input in);
static void em_scope_child_ended(em_scope *s, em_fiber *child);
static void em_root_ended(em_root *r, em_input in);

static void em_fiber_finish(em_fiber *f, em_input in) {
  if (f->ended) {
    em_input_drop(in);
    return;
  }
  f->ended = true;
  f->cause = em_cause_of(in);
  f->wait = NULL;
  if (f->scope == NULL) {
    f->result = em_in_value(em_zero(), false);
    em_root_ended(f->root, in);
  } else {
    f->result = in;
    em_scope_child_ended(f->scope, f);
    em_vec watchers = f->watchers;
    memset(&f->watchers, 0, sizeof f->watchers);
    for (size_t i = 0; i < watchers.len; i++) {
      em_watch *watch = watchers.items[i];
      em_cont(watch->w, em_watch_settle(f, watch->verb, watch->repr));
      em_wait_unref(watch->w);
      free(watch);
    }
    em_vec_free(&watchers);
  }
  if (f->listed) {
    f->listed = false;
    em_vec_remove(&f->root->fibers, f);
    em_drop(f->self);
  }
}

static void em_watch_add(em_vec *v, em_wait *w, int verb, int32_t repr) {
  em_watch *watch = malloc(sizeof(em_watch));
  if (watch == NULL) em_out_of_memory();
  watch->w = w;
  watch->verb = verb;
  watch->repr = repr;
  em_wait_ref(w);
  em_vec_push(v, watch);
}

/* ---- The driver loop ---- */

static void em_settled(em_perform *p, em_input in);

/* A continuation finished with `in`. */
static void em_strand_end(em_root *r, em_fiber *f, int end, em_perform *impl, em_input in) {
  switch (end) {
    case EM_END_MAIN:
      if (in.mode == EM_VALUE) {
        in.ref = f->scope != NULL || r->result_repr == EM_REPR_REF;
      }
      em_fiber_finish(f, in);
      break;
    case EM_END_IMPL:
      em_settled(impl, in);
      em_perform_unref(impl);
      break;
    default:
      if (in.mode == EM_THROW && !em_is_interrupted(in.value.p) && r->orphan == NULL) {
        r->orphan = in.value.p;
      } else {
        em_input_drop(in);
      }
      break;
  }
}

/* Attaches the captured continuation to the request it suspended at (`suspend()` of
 * `$advance`); a fairness yield queues the fiber again. Takes the references to `w`, `chain` and
 * `impl`. */
static void em_suspended(em_root *r, em_fiber *f, em_wait *w, em_ref chain, int end,
                         em_perform *impl) {
  w->chain = chain;
  em_dup(f->self);
  w->fiber = f;
  w->end = end;
  w->impl = impl;
  w->live = true;
  w->prev = r->last_wait;
  w->next = NULL;
  if (r->last_wait != NULL) r->last_wait->next = w;
  else r->first_wait = w;
  r->last_wait = w;
  if (end == EM_END_MAIN) {
    f->wait = w;
  } else if (end == EM_END_IMPL) {
    em_wait_ref(w);
    em_wait_unref(impl->impl_wait);
    impl->impl_wait = w;
  }
  if (w->kind == EM_WAIT_PAUSE) {
    em_wait_ref(w);
    em_job go = em_job_of(EM_JOB_GO, w);
    if (++r->pauses % EM_HOST_EVERY == 0) em_queue_push(&r->later, go);
    else em_queue_push(&r->queue, go);
  }
}

/* Advances the continuation with the input until it suspends again or finishes (`$advance`). */
static void em_advance(em_root *r, em_fiber *f, em_ref chain, int end, em_perform *impl,
                       em_input in) {
  for (;;) {
    if (r->over) {
      em_drop(chain);
      em_input_drop(in);
      if (impl != NULL) em_perform_unref(impl);
      return;
    }
    if (chain == NULL) {
      em_strand_end(r, f, end, impl, in);
      return;
    }
    em_ref fr = chain;
    chain = fr->slots[2].p;
    fr->slots[2].p = NULL;
    em_resume_fn resume = (em_resume_fn)fr->slots[0].fn;
    em_word out = resume(fr, em_input_put(in));
    if (em_suspending) {
      em_wait *w = em_take_suspension(chain, &chain);
      if (w == NULL) {
        em_drop(chain);
        if (impl != NULL) em_perform_unref(impl);
        return;
      }
      em_suspended(r, f, w, chain, end, impl);
      return;
    }
    in = em_input_taken(out, false);
  }
}

static void em_step(em_root *r, em_fiber *f, em_ref chain, int end, em_perform *impl,
                    em_input in) {
  em_fiber *prev = em_cur;
  bool prev_main = em_cur_main;
  em_dup(f->self);
  em_cur = f;
  em_cur_main = end == EM_END_MAIN;
  em_advance(r, f, chain, end, impl, in);
  em_cur = prev;
  em_cur_main = prev_main;
  em_drop(f->self);
}

/* Continues the fiber's continuation queued with `in`; an interruption that arrived meanwhile is
 * delivered instead (`$Fiber#resume`). */
static void em_fiber_resume(em_fiber *f, em_ref chain, int end, em_perform *impl, em_input in) {
  if (end == EM_END_MAIN && in.mode != EM_THROW && em_interrupting(f)) {
    em_input_drop(in);
    in = em_in_ref(EM_THROW, em_deliver(f));
  }
  em_step(f->root, f, chain, end, impl, in);
}

/* Wakes the wait with `in` (`cont` of `$advance`): on the spot within the same fiber,
 * otherwise as a queued job. */
static void em_cont(em_wait *w, em_input in) {
  em_root *r = w->fiber == NULL ? em_root_cur : w->fiber->root;
  if (w->fiber == NULL || !em_take(r, w)) {
    em_input_drop(in);
    return;
  }
  if (em_cur == w->fiber) {
    em_ref chain = w->chain;
    em_perform *impl = w->impl;
    w->chain = NULL;
    w->impl = NULL;
    em_fiber *f = w->fiber;
    int end = w->end;
    em_dup(f->self);
    em_wait_unref(w);
    em_step(r, f, chain, end, impl, in);
    em_drop(f->self);
    return;
  }
  em_job job = em_job_of(EM_JOB_RESUME, w);
  job.in = in;
  em_schedule(r, job);
}

static void em_fiber_begin(em_root *r, em_fiber *f, em_ref fn);

static void em_run_job(em_root *r, em_job job) {
  em_wait *w = job.w;
  switch (job.kind) {
    case EM_JOB_ENTRY:
    case EM_JOB_BEGIN:
      em_fiber_begin(r, job.fiber, job.fn);
      job.fn = NULL;
      break;
    case EM_JOB_RESUME:
    case EM_JOB_GO:
    case EM_JOB_INTERRUPT:
    case EM_JOB_ROOT_THROW:
    case EM_JOB_DROP_IMPL: {
      if (job.kind == EM_JOB_GO) {
        if (!em_take(r, w)) break;
        em_wait_unref(w);
      }
      em_fiber *f = w->fiber;
      em_ref chain = w->chain;
      em_perform *impl = w->impl;
      w->chain = NULL;
      w->impl = NULL;
      int end = w->end;
      em_input in = job.in;
      job.in = em_in_value(em_zero(), false);
      em_dup(f->self);
      switch (job.kind) {
        case EM_JOB_RESUME:
          em_fiber_resume(f, chain, end, impl, in);
          break;
        case EM_JOB_GO:
          em_fiber_resume(f, chain, end, impl, em_in_value(em_zero(), false));
          break;
        case EM_JOB_INTERRUPT:
          em_step(r, f, chain, end, impl, em_in_ref(EM_THROW, em_deliver(f)));
          break;
        case EM_JOB_ROOT_THROW:
          if (job.end == EM_END_MAIN) {
            em_step(r, f, chain, end, impl, em_in_ref(EM_THROW, em_deliver(f)));
          } else {
            if (impl != NULL) em_perform_unref(impl);
            em_step(r, f, chain, EM_END_ORPHAN, NULL,
                    em_in_ref(EM_THROW, em_interrupted_new(NULL)));
          }
          break;
        default:
          em_step(r, f, chain, end, impl, em_in_ref(EM_THROW, em_interrupted_new(NULL)));
          break;
      }
      em_drop(f->self);
      break;
    }
    default:
      break;
  }
  em_job_drop(job);
}

/* Calls the forked function (or the entry) and steps what comes back suspended. */
static void em_fiber_begin(em_root *r, em_fiber *f, em_ref fn) {
  if (f->pending.len > 0) {
    em_drop(fn);
    em_fiber_finish(f, em_in_ref(EM_THROW, em_interrupted_new(NULL)));
    return;
  }
  em_fiber *prev = em_cur;
  bool prev_main = em_cur_main;
  em_cur = f;
  em_cur_main = true;
  em_word out;
  if (fn == NULL) {
    out = r->entry();
  } else {
    out.p = ((em_ref (*)(em_ref))em_closure_fn(fn))(fn);
    em_drop(fn);
  }
  em_cur = prev;
  em_cur_main = prev_main;
  if (em_suspending) {
    em_ref chain;
    em_wait *w = em_take_suspension(NULL, &chain);
    if (w == NULL) {
      em_drop(chain);
      return;
    }
    em_suspended(r, f, w, chain, EM_END_MAIN, NULL);
    return;
  }
  em_strand_end(r, f, EM_END_MAIN, NULL, em_input_taken(out, false));
}

static void em_root_finish(em_root *r, em_input in);

/* ---- `suspend fn` external functions and the event loop (spec 12.6) ----
 *
 * A task (`em_task`) stands for a call of a `suspend fn` external function. The wrapper makes it,
 * calls the host function and then `em_task_end`: a task completed in place gives its value at
 * once (JS: an external function that returns a value that is not a Promise); otherwise the
 * fiber suspends on a wait of its own (EM_WAIT_HOST), which the task counts, and the completion
 * wakes the wait with `em_cont` (JS: the `$Await` request and the settled promise).
 *
 * The event loop runs where the JS host's event loop would: when no job is left (`em_run`). It
 * waits with poll(2) for the file descriptors registered with `em_on_readable` and
 * `em_on_writable`, for the earliest timer of `em_after` (a min-heap ordered by time, then by the
 * order of registration, like Node's timers) and for completions from other threads, which go
 * into a locked inbox and wake the loop through a self-pipe. Callbacks run on the runtime's
 * thread. When nothing is registered and the inbox is empty, the run has stalled: TestClock's
 * earliest sleeper wakes (`em_when_stalled`), and without one the run is unwound as interrupted
 * (`em_root_interrupt`), which cancels the tasks still waiting (`em_on_cancel`; JS: the
 * AbortSignal).
 *
 * The host never sees a task's record (`em_htask`): it gets a handle (`em_task *`, never
 * dereferenced) that names a slot of a table and the slot's generation. A completed task is freed
 * once the runtime is back in its event loop (a second completion inside the same external
 * function call or callback still finds the record: a defect); freeing empties its slot, and the
 * next task in the slot gets the next generation. A completion through a handle whose slot is
 * empty or holds a later generation finds no record, so a late second completion is a defect too
 * and never touches freed memory. Free slots are reused oldest first, so a slot only comes back to
 * the same generation after 2^32 reuses (2^12 where pointers have 32 bits). The table only grows
 * to the most tasks alive at once. */

enum { EM_HOST_DEFECT = -1 };

typedef struct em_reg em_reg;

struct em_htask {
  /* The external function's name (a static string), for messages. */
  const char *function;
  /* What the host gets (`em_slot_take`). */
  em_task *handle;
  uint32_t slot;
  /* The kind of host value the external function returns (EM_HOST_*). */
  int32_t kind;
  /* Only on the runtime's thread: */
  /* The host function is being called: a completion now is picked up by `em_task_end`. */
  bool in_place;
  bool cancelled;
  /* The run's wait while the fiber waits (counted), else NULL. */
  em_wait *wait;
  em_callback on_cancel;
  void *cancel_ctx;
  /* The registrations with the event loop (`em_after`, ...). */
  em_reg *regs;
  /* Retired (delivered or discarded): freed when the runtime is back in its event loop. */
  struct em_htask *retired_next;
  /* Guarded by `em_inbox_lock`: */
  bool completed;
  /* Completed by another thread: in the inbox until the runtime's thread takes it. */
  bool queued;
  /* Completed a second time while the host function was being called (in place). */
  bool twice;
  /* What the completion gave: EM_HOST_* or EM_HOST_DEFECT, and the value. */
  int32_t got;
  em_word value;
  /* The string of em_complete_string or the message of em_complete_defect (malloc'ed). */
  char *str;
  struct em_htask *inbox_next;
};

enum { EM_REG_TIMER, EM_REG_READ, EM_REG_WRITE };

struct em_reg {
  int kind;
  int64_t at;
  uint64_t seq;
  int fd;
  em_htask *task;
  em_callback callback;
  void *ctx;
  /* The position in the timer heap or in the list of file descriptors. */
  size_t index;
  em_reg *task_next;
};

/* A second completion that happened on another thread (the task may be freed by now). */
typedef struct em_twice {
  /* NULL when not known any more (the task's slot was reused). */
  const char *function;
  struct em_twice *next;
} em_twice;

/* A TestClock sleeper waiting for the run to stall (spec 8.8). */
typedef struct em_sleeper {
  int64_t at;
  uint64_t seq;
  em_htask *task;
} em_sleeper;

typedef struct em_regs {
  em_reg **items;
  size_t len;
  size_t cap;
} em_regs;

static em_regs em_timers = {NULL, 0, 0};
static em_regs em_fds = {NULL, 0, 0};
static uint64_t em_reg_seq = 0;
/* The loop's time in milliseconds (a monotonic clock), updated once per turn like libuv's. */
static int64_t em_loop_now = 0;
static em_htask *em_retired = NULL;
static em_vec em_sleepers = {NULL, 0, 0};
static uint64_t em_sleeper_seq = 0;

static pthread_mutex_t em_inbox_lock = PTHREAD_MUTEX_INITIALIZER;
static em_htask *em_inbox_head = NULL, *em_inbox_tail = NULL;
static em_twice *em_inbox_twice = NULL;

/* The task table (the handles of spec 12.6). Guarded by `em_inbox_lock`: completions look handles
 * up from any thread. */
typedef struct em_slot {
  /* The task, or NULL when the slot is free. */
  em_htask *task;
  /* The generation of the task in the slot, or of the last one. */
  uint32_t gen;
  /* The external function of the task in the slot, or of the last one (a static string). */
  const char *function;
  /* The next free slot (oldest first), or EM_NO_SLOT. */
  uint32_t next_free;
} em_slot;

#define EM_NO_SLOT UINT32_MAX
#if UINTPTR_MAX > 0xFFFFFFFFu
#define EM_GEN_BITS 32
#else
#define EM_GEN_BITS 12
#endif
#define EM_GEN_MASK ((uint32_t)((((uint64_t)1) << EM_GEN_BITS) - 1))

static em_slot *em_slots = NULL;
static uint32_t em_slots_len = 0, em_slots_cap = 0;
static uint32_t em_free_head = EM_NO_SLOT, em_free_tail = EM_NO_SLOT;
/* The self-pipe: another thread's completion writes a byte to [1] to wake the poll. */
static int em_wake_fds[2] = {-1, -1};
static pthread_t em_runtime_thread;

static int64_t em_now_ms(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (int64_t)ts.tv_sec * 1000 + (int64_t)(ts.tv_nsec / 1000000);
}

static void em_regs_push(em_regs *v, em_reg *g) {
  if (v->len == v->cap) {
    v->cap = v->cap == 0 ? 8 : v->cap * 2;
    v->items = realloc(v->items, v->cap * sizeof(em_reg *));
    if (v->items == NULL) em_out_of_memory();
  }
  g->index = v->len;
  v->items[v->len++] = g;
}

static bool em_timer_before(const em_reg *a, const em_reg *b) {
  return a->at < b->at || (a->at == b->at && a->seq < b->seq);
}

static void em_heap_set(size_t i, em_reg *g) {
  em_timers.items[i] = g;
  g->index = i;
}

static void em_heap_up(size_t i) {
  em_reg *g = em_timers.items[i];
  while (i > 0) {
    size_t parent = (i - 1) / 2;
    if (!em_timer_before(g, em_timers.items[parent])) break;
    em_heap_set(i, em_timers.items[parent]);
    i = parent;
  }
  em_heap_set(i, g);
}

static void em_heap_down(size_t i) {
  em_reg *g = em_timers.items[i];
  for (;;) {
    size_t l = 2 * i + 1;
    if (l >= em_timers.len) break;
    size_t c = l;
    if (l + 1 < em_timers.len && em_timer_before(em_timers.items[l + 1], em_timers.items[l])) {
      c = l + 1;
    }
    if (!em_timer_before(em_timers.items[c], g)) break;
    em_heap_set(i, em_timers.items[c]);
    i = c;
  }
  em_heap_set(i, g);
}

/* Takes the registration out of the timer heap or the list of file descriptors and frees it
 * (the caller unlinks it from its task). */
static void em_reg_remove(em_reg *g) {
  if (g->kind == EM_REG_TIMER) {
    size_t i = g->index;
    em_reg *last = em_timers.items[--em_timers.len];
    if (i < em_timers.len) {
      em_heap_set(i, last);
      em_heap_down(i);
      em_heap_up(last->index);
    }
  } else {
    /* Keeps the order of registration. */
    for (size_t i = g->index + 1; i < em_fds.len; i++) {
      em_fds.items[i - 1] = em_fds.items[i];
      em_fds.items[i - 1]->index = i - 1;
    }
    em_fds.len--;
  }
  free(g);
}

static void em_reg_unlink(em_reg *g) {
  em_reg **p = &g->task->regs;
  while (*p != g) p = &(*p)->task_next;
  *p = g->task_next;
}

/* Removes every registration of the task. */
static void em_task_unregister(em_htask *t) {
  while (t->regs != NULL) {
    em_reg *g = t->regs;
    t->regs = g->task_next;
    em_reg_remove(g);
  }
}

static bool em_on_runtime_thread(void) {
  return pthread_equal(pthread_self(), em_runtime_thread) != 0;
}

static void em_require_runtime_thread(const char *function) {
  if (!em_on_runtime_thread()) {
    fprintf(stderr, "emela: %s was called outside the runtime's thread\n", function);
    abort();
  }
}

static void em_loop_init(void) {
  em_runtime_thread = pthread_self();
  em_loop_now = em_now_ms();
  if (pipe(em_wake_fds) != 0) {
    perror("emela: pipe");
    abort();
  }
  for (int i = 0; i < 2; i++) {
    fcntl(em_wake_fds[i], F_SETFL, fcntl(em_wake_fds[i], F_GETFL) | O_NONBLOCK);
    fcntl(em_wake_fds[i], F_SETFD, FD_CLOEXEC);
  }
}

static void em_task_retire(em_htask *t) {
  t->retired_next = em_retired;
  em_retired = t;
}

/* Gives the task a slot and its handle: the slot's index + 1 above the generation (never NULL). */
static void em_slot_take(em_htask *t) {
  pthread_mutex_lock(&em_inbox_lock);
  uint32_t i = em_free_head;
  if (i != EM_NO_SLOT) {
    em_free_head = em_slots[i].next_free;
    if (em_free_head == EM_NO_SLOT) em_free_tail = EM_NO_SLOT;
  } else {
    /* The index + 1 must fit above the generation (and in 32 bits). */
    uint64_t limit = sizeof(uintptr_t) * 8 - EM_GEN_BITS >= 32
                         ? (uint64_t)UINT32_MAX
                         : ((uint64_t)1 << (sizeof(uintptr_t) * 8 - EM_GEN_BITS)) - 1;
    if ((uint64_t)em_slots_len + 1 >= limit) em_out_of_memory();
    if (em_slots_len == em_slots_cap) {
      em_slots_cap = em_slots_cap == 0 ? 16 : em_slots_cap * 2;
      em_slots = realloc(em_slots, em_slots_cap * sizeof(em_slot));
      if (em_slots == NULL) em_out_of_memory();
    }
    i = em_slots_len++;
    em_slots[i].gen = 0;
  }
  em_slot *s = &em_slots[i];
  s->gen = (s->gen + 1) & EM_GEN_MASK;
  s->task = t;
  s->function = t->function;
  s->next_free = EM_NO_SLOT;
  t->slot = i;
  t->handle = (em_task *)((((uintptr_t)i + 1) << EM_GEN_BITS) | (uintptr_t)s->gen);
  pthread_mutex_unlock(&em_inbox_lock);
}

/* The task a handle names, or NULL if it has been freed (`*function` is then its external
 * function, or NULL if that is not known any more). A handle that never named a task is a bug of
 * the host: the program aborts. Under `em_inbox_lock`. */
static em_htask *em_slot_find(em_task *h, const char **function, const char *caller) {
  uintptr_t bits = (uintptr_t)h;
  uintptr_t index = bits >> EM_GEN_BITS;
  uint32_t gen = (uint32_t)(bits & EM_GEN_MASK);
  if (index == 0 || index > em_slots_len) {
    fprintf(stderr, "emela: %s was given %p, which is not a task\n", caller, (void *)h);
    abort();
  }
  em_slot *s = &em_slots[index - 1];
  if (s->gen == gen && s->task != NULL) return s->task;
  *function = s->gen == gen ? s->function : NULL;
  return NULL;
}

/* Frees the retired tasks (back in the event loop). Their slots become free, after the others. */
static void em_loop_collect(void) {
  if (em_retired == NULL) return;
  pthread_mutex_lock(&em_inbox_lock);
  while (em_retired != NULL) {
    em_htask *t = em_retired;
    em_retired = t->retired_next;
    em_slots[t->slot].task = NULL;
    if (em_free_tail == EM_NO_SLOT) em_free_head = t->slot;
    else em_slots[em_free_tail].next_free = t->slot;
    em_free_tail = t->slot;
    free(t);
  }
  pthread_mutex_unlock(&em_inbox_lock);
}

static const char *em_complete_name(int32_t got) {
  switch (got) {
    case EM_HOST_INT:
      return "em_complete_int";
    case EM_HOST_INT64:
      return "em_complete_int64";
    case EM_HOST_FLOAT:
      return "em_complete_float";
    case EM_HOST_BOOL:
      return "em_complete_bool";
    case EM_HOST_STRING:
      return "em_complete_string";
    case EM_HOST_HANDLE:
      return "em_complete_handle";
    default:
      return "em_complete_unit";
  }
}

/* `function` is NULL when not known any more (a late completion after the slot was reused). */
static em_ref em_twice_message(const char *function) {
  em_buf b = {NULL, 0, 0};
  if (function == NULL) {
    em_buf_puts(&b, "the task of an external function was completed twice");
    return em_buf_finish(&b);
  }
  em_buf_puts(&b, "the task of external function `");
  em_buf_puts(&b, function);
  em_buf_puts(&b, "` was completed twice");
  return em_buf_finish(&b);
}

/* What the completion passes to the waiting continuation (on the runtime's thread). */
static em_input em_task_input(em_htask *t) {
  em_word w = em_zero();
  if (t->got == EM_HOST_DEFECT) {
    em_ref msg = em_str_cstr(t->str);
    free(t->str);
    t->str = NULL;
    return em_in_ref(EM_THROW, msg);
  }
  if (t->got != t->kind) {
    free(t->str);
    t->str = NULL;
    em_buf b = {NULL, 0, 0};
    em_buf_puts(&b, "external function `");
    em_buf_puts(&b, t->function);
    em_buf_puts(&b, "` was completed with `");
    em_buf_puts(&b, em_complete_name(t->got));
    em_buf_puts(&b, "`, which does not match its return type");
    return em_in_ref(EM_THROW, em_buf_finish(&b));
  }
  switch (t->kind) {
    case EM_HOST_STRING: {
      char *s = t->str;
      t->str = NULL;
      em_ref v = em_from_host_string(s, t->function);
      if (v == NULL) {
        em_ref d = em_defect;
        em_defect = NULL;
        return em_in_ref(EM_THROW, d);
      }
      w.p = v;
      return em_in_value(w, true);
    }
    case EM_HOST_HANDLE:
      w.p = em_host_box(t->value.h);
      return em_in_value(w, true);
    case EM_HOST_UNIT:
      return em_in_value(w, false);
    default:
      return em_in_value(t->value, false);
  }
}

/* Ends the run with a defect: a second completion after the first one was delivered (JS throws
 * `resume called twice` into the host's callback, and the run ends). */
static void em_task_fail_run(em_ref msg) {
  if (em_root_cur != NULL && !em_root_cur->over) {
    em_root_finish(em_root_cur, em_in_ref(EM_THROW, msg));
  } else {
    em_drop(msg);
  }
}

/* The completion reached the runtime's thread: wakes the waiting fiber, or leaves the value to
 * `em_task_end`, or discards it (the fiber was interrupted). */
static void em_task_deliver(em_htask *t) {
  em_task_unregister(t);
  if (t->in_place) return;
  em_wait *w = t->wait;
  t->wait = NULL;
  em_task_retire(t);
  if (w == NULL) {
    free(t->str);
    t->str = NULL;
    return;
  }
  w->task = NULL;
  em_input in = em_task_input(t);
  em_cont(w, in);
  em_wait_unref(w);
}

static void em_wake(void) {
  char byte = 1;
  ssize_t n = write(em_wake_fds[1], &byte, 1);
  (void)n;
}

static void em_complete(em_task *h, const char *caller, int32_t got, em_word value, char *str) {
  bool here = em_on_runtime_thread();
  pthread_mutex_lock(&em_inbox_lock);
  const char *function = NULL;
  em_htask *t = em_slot_find(h, &function, caller);
  /* A second completion: of a task completed before, or freed after its completion was
   * delivered. */
  if (t == NULL || t->completed) {
    free(str);
    if (t != NULL) function = t->function;
    if (here) {
      bool in_place = t != NULL && t->in_place;
      if (in_place) t->twice = true;
      pthread_mutex_unlock(&em_inbox_lock);
      if (!in_place) em_task_fail_run(em_twice_message(function));
      return;
    }
    em_twice *tw = malloc(sizeof(em_twice));
    if (tw == NULL) em_out_of_memory();
    tw->function = function;
    tw->next = em_inbox_twice;
    em_inbox_twice = tw;
    pthread_mutex_unlock(&em_inbox_lock);
    em_wake();
    return;
  }
  t->completed = true;
  t->got = got;
  t->value = value;
  t->str = str;
  if (!here) {
    t->queued = true;
    t->inbox_next = NULL;
    if (em_inbox_tail == NULL) em_inbox_head = t;
    else em_inbox_tail->inbox_next = t;
    em_inbox_tail = t;
    pthread_mutex_unlock(&em_inbox_lock);
    em_wake();
    return;
  }
  pthread_mutex_unlock(&em_inbox_lock);
  em_task_deliver(t);
}

void em_complete_int(em_task *t, int32_t v) {
  em_word w = em_zero();
  w.i = v;
  em_complete(t, "em_complete_int", EM_HOST_INT, w, NULL);
}

void em_complete_int64(em_task *t, int64_t v) {
  em_word w;
  w.l = v;
  em_complete(t, "em_complete_int64", EM_HOST_INT64, w, NULL);
}

void em_complete_float(em_task *t, double v) {
  em_word w;
  w.f = v;
  em_complete(t, "em_complete_float", EM_HOST_FLOAT, w, NULL);
}

void em_complete_bool(em_task *t, bool v) {
  em_word w = em_zero();
  w.i = v ? 1 : 0;
  em_complete(t, "em_complete_bool", EM_HOST_BOOL, w, NULL);
}

void em_complete_string(em_task *t, char *v) {
  em_complete(t, "em_complete_string", EM_HOST_STRING, em_zero(), v);
}

void em_complete_unit(em_task *t) {
  em_complete(t, "em_complete_unit", EM_HOST_UNIT, em_zero(), NULL);
}

void em_complete_handle(em_task *t, void *v) {
  em_word w;
  w.h = v;
  em_complete(t, "em_complete_handle", EM_HOST_HANDLE, w, NULL);
}

void em_complete_defect(em_task *t, const char *message) {
  const char *m = message != NULL ? message : "";
  size_t n = strlen(m);
  char *copy = malloc(n + 1);
  if (copy == NULL) em_out_of_memory();
  memcpy(copy, m, n + 1);
  em_complete(t, "em_complete_defect", EM_HOST_DEFECT, em_zero(), copy);
}

static bool em_task_done(em_htask *t) {
  pthread_mutex_lock(&em_inbox_lock);
  bool done = t->completed;
  pthread_mutex_unlock(&em_inbox_lock);
  return done;
}

/* The task of a handle given to a function that runs on the runtime's thread, or NULL if it has
 * been freed. */
static em_htask *em_task_of(em_task *h, const char *caller) {
  em_require_runtime_thread(caller);
  const char *function = NULL;
  pthread_mutex_lock(&em_inbox_lock);
  em_htask *t = em_slot_find(h, &function, caller);
  pthread_mutex_unlock(&em_inbox_lock);
  return t;
}

static void em_register(int kind, int64_t at, int fd, em_htask *t, em_callback cb, void *ctx) {
  /* A completed (or freed) task has nothing left to wait for. */
  if (t == NULL || em_task_done(t)) return;
  em_reg *g = malloc(sizeof(em_reg));
  if (g == NULL) em_out_of_memory();
  g->kind = kind;
  g->at = at;
  g->seq = em_reg_seq++;
  g->fd = fd;
  g->task = t;
  g->callback = cb;
  g->ctx = ctx;
  g->task_next = t->regs;
  t->regs = g;
  if (kind == EM_REG_TIMER) {
    em_regs_push(&em_timers, g);
    em_heap_up(g->index);
  } else {
    em_regs_push(&em_fds, g);
  }
}

void em_after(int64_t ms, em_task *h, em_callback cb, void *ctx) {
  em_htask *t = em_task_of(h, "em_after");
  em_register(EM_REG_TIMER, em_loop_now + (ms < 0 ? 0 : ms), -1, t, cb, ctx);
}

void em_on_readable(int fd, em_task *h, em_callback cb, void *ctx) {
  em_register(EM_REG_READ, 0, fd, em_task_of(h, "em_on_readable"), cb, ctx);
}

void em_on_writable(int fd, em_task *h, em_callback cb, void *ctx) {
  em_register(EM_REG_WRITE, 0, fd, em_task_of(h, "em_on_writable"), cb, ctx);
}

void em_on_cancel(em_task *h, em_callback cb, void *ctx) {
  em_htask *t = em_task_of(h, "em_on_cancel");
  if (t == NULL) return;
  if (t->cancelled) {
    if (cb != NULL && !em_task_done(t)) cb(h, ctx);
    return;
  }
  t->on_cancel = cb;
  t->cancel_ctx = ctx;
}

/* The waiting fiber was interrupted, or the run ended (`AbortController#abort`). */
static void em_task_cancel(em_htask *t) {
  if (t->cancelled) return;
  t->cancelled = true;
  em_wait *w = t->wait;
  t->wait = NULL;
  if (w != NULL) {
    w->task = NULL;
    em_wait_unref(w);
  }
  em_callback cb = t->on_cancel;
  t->on_cancel = NULL;
  if (cb != NULL) cb(t->handle, t->cancel_ctx);
}

em_task *em_task_new(int32_t kind, const char *function) {
  em_htask *t = calloc(1, sizeof(em_htask));
  if (t == NULL) em_out_of_memory();
  t->function = function;
  t->kind = kind;
  t->in_place = true;
  em_slot_take(t);
  return t->handle;
}

em_word em_task_end(em_task *h) {
  /* Not freed: a task is only retired from here on (a completion in place leaves it alone). */
  em_htask *t = em_task_of(h, "em_task_end");
  t->in_place = false;
  pthread_mutex_lock(&em_inbox_lock);
  /* Completed in place, on this thread (another thread's completion goes through the inbox). */
  bool done = t->completed && !t->queued;
  bool twice = t->twice;
  pthread_mutex_unlock(&em_inbox_lock);
  if (done) {
    em_task_retire(t);
    if (twice) {
      free(t->str);
      t->str = NULL;
      em_raise_take(em_twice_message(t->function));
      return em_zero();
    }
    em_input in = em_task_input(t);
    if (in.mode == EM_THROW) {
      em_defect = in.value.p;
      return em_zero();
    }
    return in.value;
  }
  bool dead = em_sync_depth > 0 ? em_sync_defect != NULL
                                : em_root_cur == NULL || em_root_cur->over;
  if (dead) {
    em_task_cancel(t);
    em_suspend_on(NULL);
    return em_zero();
  }
  em_wait *w = em_wait_new(EM_WAIT_HOST, false);
  w->task = t;
  em_wait_ref(w);
  t->wait = w;
  em_suspend_on(w);
  return em_zero();
}

/* ---- TestClock (spec 8.8) ---- */

static void em_sleeper_cancel(em_task *t, void *ctx) {
  em_sleeper *s = ctx;
  em_vec_remove(&em_sleepers, s);
  free(s);
  em_complete_unit(t);
}

/* The host function behind TestClock's `sleep` (std/clock.c): completes the task when the run
 * stalls and `at` is the earliest time waited for (JS `$whenStalled`). */
void em_when_stalled(int64_t at, em_task *h) {
  if (em_root_cur == NULL) {
    em_complete_defect(h, "TestClock was used outside a run");
    return;
  }
  em_htask *t = em_task_of(h, "em_when_stalled");
  em_sleeper *s = malloc(sizeof(em_sleeper));
  if (s == NULL) em_out_of_memory();
  s->at = at;
  s->seq = em_sleeper_seq++;
  s->task = t;
  em_vec_push(&em_sleepers, s);
  em_on_cancel(h, em_sleeper_cancel, s);
}

/* The run stalled: wakes the sleeper with the earliest time (the earliest call among equal
 * times). False without one. */
static bool em_wake_sleeper(void) {
  if (em_sleepers.len == 0) return false;
  size_t first = 0;
  for (size_t i = 1; i < em_sleepers.len; i++) {
    em_sleeper *a = em_sleepers.items[i];
    em_sleeper *b = em_sleepers.items[first];
    if (a->at < b->at || (a->at == b->at && a->seq < b->seq)) first = i;
  }
  em_sleeper *s = em_sleepers.items[first];
  em_vec_remove(&em_sleepers, s);
  em_htask *t = s->task;
  free(s);
  t->on_cancel = NULL;
  em_complete_unit(t->handle);
  return true;
}

/* ---- The event loop ---- */

/* Whether the loop has something to wait for: a registration, or a completion from another
 * thread not taken yet. */
static bool em_loop_busy(void) {
  if (em_timers.len > 0 || em_fds.len > 0) return true;
  pthread_mutex_lock(&em_inbox_lock);
  bool any = em_inbox_head != NULL || em_inbox_twice != NULL;
  pthread_mutex_unlock(&em_inbox_lock);
  return any;
}

/* Delivers the completions from other threads, in the order they came. */
static void em_loop_inbox(void) {
  char buf[64];
  while (read(em_wake_fds[0], buf, sizeof buf) > 0) {
  }
  pthread_mutex_lock(&em_inbox_lock);
  em_htask *head = em_inbox_head;
  em_twice *tw = em_inbox_twice;
  em_inbox_head = em_inbox_tail = NULL;
  em_inbox_twice = NULL;
  for (em_htask *t = head; t != NULL; t = t->inbox_next) t->queued = false;
  pthread_mutex_unlock(&em_inbox_lock);
  while (tw != NULL) {
    em_twice *next = tw->next;
    em_task_fail_run(em_twice_message(tw->function));
    free(tw);
    tw = next;
  }
  while (head != NULL) {
    em_htask *t = head;
    head = t->inbox_next;
    t->inbox_next = NULL;
    em_task_deliver(t);
  }
}

/* Calls the registration's callback once: it is removed first. */
static void em_reg_fire(em_reg *g) {
  em_task *h = g->task->handle;
  em_callback cb = g->callback;
  void *ctx = g->ctx;
  em_reg_unlink(g);
  em_reg_remove(g);
  cb(h, ctx);
}

/* One turn of the event loop: waits (unless `block` is false) for the first timer, a file
 * descriptor or a completion from another thread, and runs what is ready. */
static void em_loop_turn(bool block) {
  em_loop_collect();
  size_t n = em_fds.len + 1;
  struct pollfd *pfds = malloc(n * sizeof(struct pollfd));
  uint64_t *seqs = malloc(n * sizeof(uint64_t));
  if (pfds == NULL || seqs == NULL) em_out_of_memory();
  pfds[0].fd = em_wake_fds[0];
  pfds[0].events = POLLIN;
  pfds[0].revents = 0;
  for (size_t i = 0; i < em_fds.len; i++) {
    em_reg *g = em_fds.items[i];
    pfds[i + 1].fd = g->fd;
    pfds[i + 1].events = g->kind == EM_REG_READ ? POLLIN : POLLOUT;
    pfds[i + 1].revents = 0;
    seqs[i + 1] = g->seq;
  }
  int timeout = 0;
  if (block) {
    timeout = -1;
    if (em_timers.len > 0) {
      int64_t d = em_timers.items[0]->at - em_now_ms();
      timeout = d < 0 ? 0 : d > 1000000000 ? 1000000000 : (int)d;
    }
  }
  int ready;
  do {
    ready = poll(pfds, (nfds_t)n, timeout);
  } while (ready < 0 && errno == EINTR);
  if (ready < 0) {
    perror("emela: poll");
    abort();
  }
  em_loop_now = em_now_ms();
  em_loop_inbox();
  for (size_t i = 1; ready > 0 && i < n; i++) {
    if (pfds[i].revents == 0) continue;
    /* Still registered? An earlier callback may have completed its task. */
    for (size_t k = 0; k < em_fds.len; k++) {
      if (em_fds.items[k]->seq == seqs[i]) {
        em_reg_fire(em_fds.items[k]);
        break;
      }
    }
  }
  free(pfds);
  free(seqs);
  /* Timers registered by these callbacks wait for the next turn. */
  uint64_t limit = em_reg_seq;
  while (em_timers.len > 0) {
    em_reg *g = em_timers.items[0];
    if (g->at > em_loop_now || g->seq >= limit) break;
    em_reg_fire(g);
  }
}

/* The run ended: what is still registered is dropped. Tasks left uncompleted are not freed (the
 * host may still complete them from another thread; that completion is discarded). */
static void em_loop_reset(void) {
  while (em_timers.len > 0) {
    em_reg *g = em_timers.items[--em_timers.len];
    em_reg_unlink(g);
    free(g);
  }
  while (em_fds.len > 0) {
    em_reg *g = em_fds.items[--em_fds.len];
    em_reg_unlink(g);
    free(g);
  }
  em_loop_collect();
}

/* ---- The run ---- */

static void em_root_finish(em_root *r, em_input in) {
  if (r->over) {
    em_input_drop(in);
    return;
  }
  r->over = true;
  while (r->queue.len > 0) em_job_drop(em_queue_pop(&r->queue));
  while (r->later.len > 0) em_job_drop(em_queue_pop(&r->later));
  /* What the continuations left behind is not resumed: the withs in them are not released
   * (spec 14.5). */
  while (r->first_wait != NULL) {
    em_wait *w = r->first_wait;
    w->live = false;
    em_root_unlist(r, w);
    if (w->fiber->wait == w) w->fiber->wait = NULL;
    em_ref chain = w->chain;
    w->chain = NULL;
    if (w->impl != NULL) {
      em_perform_unref(w->impl);
      w->impl = NULL;
    }
    if (w->on_drop != NULL) {
      em_perform_unref(w->on_drop);
      w->on_drop = NULL;
    }
    if (w->task != NULL) em_task_cancel(w->task);
    em_drop(chain);
    em_wait_unref(w);
  }
  r->result = in;
  r->has_result = true;
}

/* The root fiber ended. */
static void em_root_ended(em_root *r, em_input in) {
  if (in.mode == EM_THROW && em_is_interrupted(in.value.p) && r->orphan != NULL) {
    em_input_drop(in);
    in = em_in_ref(EM_THROW, r->orphan);
    r->orphan = NULL;
  }
  em_root_finish(r, in);
}

/* The run stalled (nothing can go on): unwinds it as interrupted (`$Root#interrupt`). Every fiber
 * is interrupted, and every waiting continuation is resumed with an interruption, which runs the
 * releases of its open withs. This also cuts short releases left suspended. Scopes waiting for
 * their children keep waiting, since the children are unwound too. */
static void em_root_interrupt(em_root *r) {
  if (r->over) return;
  for (size_t i = 0; i < r->fibers.len; i++) {
    em_fiber *f = r->fibers.items[i];
    if (!f->unwinding && !em_vec_has(&f->pending, NULL)) em_vec_push(&f->pending, NULL);
  }
  em_vec waits = {NULL, 0, 0};
  for (em_wait *w = r->last_wait; w != NULL; w = w->prev) {
    if (!w->closing) {
      em_wait_ref(w);
      em_vec_push(&waits, w);
    }
  }
  for (size_t i = 0; i < waits.len; i++) {
    em_wait *w = waits.items[i];
    if (w->live) {
      /* Dropping a caller also unwinds the implementation it waits for (`onDrop`). */
      em_drop_wait(r, w);
      em_job job = em_job_of(EM_JOB_ROOT_THROW, w);
      job.end = w->end == EM_END_MAIN ? EM_END_MAIN : EM_END_ORPHAN;
      em_queue_push(&r->queue, job);
    }
    em_wait_unref(w);
  }
  em_vec_free(&waits);
  if (!r->running) em_drain(r);
}

void em_run(em_word (*entry)(void), int32_t repr) {
  em_root *r = em_rt_alloc(sizeof(em_root));
  r->entry = entry;
  r->result_repr = repr;
  em_root *prev = em_root_cur;
  em_root_cur = r;
  em_run_id++;
  em_fiber *main = em_fiber_new(r, NULL);
  em_dup(main->self);
  em_job job = em_job_of(EM_JOB_ENTRY, NULL);
  job.fiber = main;
  em_schedule(r, job);
  for (;;) {
    if (r->over) break;
    /* No job is left: the event loop runs (JS: the host's event loop). A job that goes through
     * the host (`later`) runs after one turn that does not wait. */
    bool later = r->later.len > 0;
    if (later || em_loop_busy()) {
      em_loop_turn(!later);
      if (!r->over && later && r->later.len > 0) em_schedule(r, em_queue_pop(&r->later));
      continue;
    }
    /* The event loop is empty: the run has stalled. TestClock's earliest sleeper wakes. */
    if (em_wake_sleeper()) continue;
    em_root_interrupt(r);
    /* Like the JS host, which only reports the empty loop again after more work went through
     * it, the run ends when unwinding did not queue or register such work. */
    if (r->later.len == 0 && !em_loop_busy()) break;
  }
  if (!r->over) em_root_finish(r, em_in_value(em_zero(), false));
  em_loop_reset();
  em_input in = r->result;
  switch (in.mode) {
    case EM_FAIL:
      em_failure = in.value.p;
      break;
    case EM_THROW:
      em_defect = in.value.p;
      break;
    default:
      if (repr == EM_REPR_REF) em_drop(in.value.p);
      break;
  }
  for (size_t i = 0; i < r->fibers.len; i++) {
    em_fiber *f = r->fibers.items[i];
    f->listed = false;
    em_drop(f->self);
  }
  em_vec_free(&r->fibers);
  em_drop(r->orphan);
  free(r->queue.items);
  free(r->later.items);
  em_root_cur = prev;
  em_rt_free(r);
}

/* ---- Fairness and holds (spec 11.1, 11.4) ---- */

void em_yield(bool perform) {
  em_fiber *f = em_cur;
  if (f == NULL || em_sync_depth > 0) return;
  if (--f->budget <= 0) {
    if (em_cur_main && em_interrupting(f)) {
      em_defect = em_deliver(f);
      return;
    }
    f->budget = EM_BUDGET;
    em_suspend_on(em_wait_new(EM_WAIT_PAUSE, false));
    return;
  }
  if (perform) em_interrupt_here();
}

void em_hold(void) {
  if (em_cur != NULL) em_cur->holds++;
}

void em_unhold(void) {
  if (em_cur != NULL) em_cur->holds--;
}

/* ---- suspend fn operations (spec 8.4, 14.5) ---- */

static void em_perform_unref(em_perform *p) {
  if (p == NULL || --p->refs > 0) return;
  if (p->has_now) em_input_drop(p->now);
  em_wait_unref(p->caller);
  em_wait_unref(p->impl_wait);
  free(p->where);
  em_rt_free(p);
}

static void em_resume_fin(em_ref o) { em_perform_unref((em_perform *)o->slots[1].h); }

/* The run of the operation call ended with the defect `d`. */
static void em_run_fails(em_perform *p, em_ref d) {
  if (p->sync) {
    if (em_sync_defect == NULL) em_sync_defect = d;
    else em_drop(d);
    return;
  }
  if (em_root_cur != NULL && p->run == em_run_id) em_root_finish(em_root_cur, em_in_ref(EM_THROW, d));
  else em_drop(d);
}

/* Passes `in` to the caller: at once if the implementation is still being called, otherwise by
 * waking the caller's wait. */
static void em_pass(em_perform *p, em_input in) {
  p->used = true;
  if (p->sync_phase) {
    p->has_now = true;
    p->now = in;
    return;
  }
  em_wait *w = p->caller;
  p->caller = NULL;
  if (w == NULL) {
    em_input_drop(in);
    return;
  }
  em_cont(w, in);
  em_wait_unref(w);
}

/* How the implementation ended: a defect or failure before resume goes to the caller; after
 * resume it ends the run (spec 14.5). */
static void em_settled(em_perform *p, em_input in) {
  switch (in.mode) {
    case EM_THROW:
      if (p->used) em_run_fails(p, in.value.p);
      else em_pass(p, in);
      break;
    case EM_FAIL:
      if (p->used) {
        em_ref shown = em_show_failure(in.value.p);
        em_buf b = {NULL, 0, 0};
        em_buf_puts(&b, p->where);
        em_buf_puts(&b, " failed after resume: ");
        if (shown != NULL) em_buf_put(&b, EM_STR_BYTES(shown), EM_STR_LEN(shown));
        em_drop(shown);
        em_run_fails(p, em_buf_finish(&b));
      } else {
        em_pass(p, in);
      }
      break;
    default:
      em_input_drop(in);
      break;
  }
}

static void em_resume_with(em_ref env, em_input in) {
  em_perform *p = (em_perform *)env->slots[1].h;
  if (p->run != em_run_id || em_root_cur == NULL || em_root_cur->over) {
    em_input_drop(in);
    return;
  }
  if (p->used) {
    em_input_drop(in);
    em_buf b = {NULL, 0, 0};
    em_buf_puts(&b, "resume called twice in ");
    em_buf_puts(&b, p->where);
    em_ref msg = em_buf_finish(&b);
    em_raise(msg);
    em_drop(msg);
    return;
  }
  em_pass(p, in);
}

static void em_resume_void(em_ref env) { em_resume_with(env, em_in_value(em_zero(), false)); }

static void em_resume_i32(em_ref env, int32_t v) {
  em_word w = em_zero();
  w.i = v;
  em_resume_with(env, em_in_value(w, false));
}

static void em_resume_i64(em_ref env, int64_t v) {
  em_word w;
  w.l = v;
  em_resume_with(env, em_in_value(w, false));
}

static void em_resume_f64(em_ref env, double v) {
  em_word w;
  w.f = v;
  em_resume_with(env, em_in_value(w, false));
}

static void em_resume_ref(em_ref env, em_ref v) {
  em_word w;
  w.p = v;
  em_resume_with(env, em_in_value(w, true));
}

em_ref em_resume_new(em_ref handler, const char *op, int32_t repr) {
  em_perform *p = em_rt_alloc(sizeof(em_perform));
  p->refs = 1;
  p->run = em_run_id;
  const char *name = handler->hdr.desc->name;
  size_t n = strlen(op) + strlen(name) + 32;
  p->where = malloc(n);
  if (p->where == NULL) em_out_of_memory();
  snprintf(p->where, n, "operation `%s` of handler `%s`", op, name);
  p->sync = em_sync_depth > 0;
  p->sync_phase = true;
  p->prev_main = em_cur_main;
  em_cur_main = false;
  em_ref rs = em_alloc(&em_desc_resume, 2);
  switch (repr) {
    case EM_REPR_I32:
      rs->slots[0].fn = (em_fnptr)em_resume_i32;
      break;
    case EM_REPR_I64:
      rs->slots[0].fn = (em_fnptr)em_resume_i64;
      break;
    case EM_REPR_F64:
      rs->slots[0].fn = (em_fnptr)em_resume_f64;
      break;
    case EM_REPR_REF:
      rs->slots[0].fn = (em_fnptr)em_resume_ref;
      break;
    default:
      rs->slots[0].fn = (em_fnptr)em_resume_void;
      break;
  }
  rs->slots[1].h = p;
  em_dup(rs);
  return rs;
}

em_word em_perform_end(em_ref rs) {
  em_perform *p = (em_perform *)rs->slots[1].h;
  p->refs++;
  em_cur_main = p->prev_main;
  if (em_suspending) {
    /* The implementation suspended: it goes on as another continuation of the same fiber. */
    em_ref chain;
    em_wait *w = em_take_suspension(NULL, &chain);
    if (w == NULL) {
      em_drop(chain);
    } else {
      p->refs++;
      em_suspended(em_root_cur, em_cur, w, chain, EM_END_IMPL, p);
    }
  } else if (em_defect != NULL) {
    em_ref d = em_defect;
    em_defect = NULL;
    em_settled(p, em_in_ref(EM_THROW, d));
  } else if (em_failure != NULL) {
    em_ref e = em_failure;
    em_failure = NULL;
    em_settled(p, em_in_ref(EM_FAIL, e));
  }
  p->sync_phase = false;
  em_drop(rs);
  em_word out = em_zero();
  bool dead = p->sync ? em_sync_defect != NULL : em_root_cur == NULL || em_root_cur->over;
  if (dead) {
    if (p->has_now) em_input_drop(p->now);
    p->has_now = false;
    em_suspend_on(NULL);
  } else if (p->has_now) {
    p->has_now = false;
    out = em_input_put(p->now);
  } else {
    em_wait *w = em_wait_new(EM_WAIT_PERFORM, false);
    p->refs++;
    w->on_drop = p;
    em_wait_ref(w);
    p->caller = w;
    em_suspend_on(w);
  }
  em_perform_unref(p);
  return out;
}

/* ---- Calls of suspending code from non-suspending functions (spec 8.7) ---- */

void em_sync_enter(void) { em_sync_depth++; }

/* Drops the request of a continuation unwound in place. */
static void em_sync_forget(em_wait *w) {
  if (w->task != NULL) em_task_cancel(w->task);
  em_perform *p = w->on_drop;
  if (p != NULL) {
    w->on_drop = NULL;
    if (p->caller == w) {
      p->caller = NULL;
      em_wait_unref(w);
    }
    em_perform_unref(p);
  }
  em_wait_unref(w);
}

void em_sync_leave(void) {
  em_sync_depth--;
  if (!em_suspending) return;
  em_ref chain;
  em_wait *w = em_take_suspension(NULL, &chain);
  em_input in;
  if (w == NULL) {
    em_drop(chain);
    chain = NULL;
    in = em_in_ref(EM_THROW, em_sync_defect != NULL ? em_sync_defect : em_interrupted_new(NULL));
    em_sync_defect = NULL;
  } else {
    /* Not resumed in place, so never resumed: unwound as interrupted at once (spec 14.5). A
     * release that suspends again is cut short the same way. */
    em_sync_forget(w);
    in = em_in_ref(EM_THROW, em_interrupted_new(NULL));
  }
  em_sync_depth++;
  while (chain != NULL) {
    em_ref fr = chain;
    chain = fr->slots[2].p;
    fr->slots[2].p = NULL;
    em_word out = ((em_resume_fn)fr->slots[0].fn)(fr, em_input_put(in));
    if (em_suspending) {
      em_wait *w2 = em_take_suspension(chain, &chain);
      if (w2 == NULL) {
        em_drop(chain);
        chain = NULL;
        in = em_in_ref(EM_THROW,
                       em_sync_defect != NULL ? em_sync_defect : em_interrupted_new(NULL));
        em_sync_defect = NULL;
        break;
      }
      em_sync_forget(w2);
      in = em_in_ref(EM_THROW, em_interrupted_new(NULL));
      continue;
    }
    in = em_input_taken(out, false);
  }
  em_sync_depth--;
  if (in.mode == EM_THROW) {
    em_defect = in.value.p;
  } else {
    em_input_drop(in);
    em_defect = em_interrupted_new(NULL);
  }
}

/* ---- Scopes (spec 11.2) ---- */

static em_word em_scope_body_resume(em_ref fr, em_word in);
static em_word em_scope_close_resume(em_ref fr, em_word in);
static em_word em_attempt_resume(em_ref fr, em_word in);

static const uint8_t em_refs_scope_body[5] = {0, 0, 1, 1, 0};
static const uint8_t em_refs_scope_close_ref[7] = {0, 0, 1, 1, 0, 0, 1};
static const uint8_t em_refs_scope_close_val[7] = {0, 0, 1, 1, 0, 0, 0};
static const em_desc em_desc_scope_body = {EM_KIND_FIELDS, 5, 0, -1, "Fiber.scope", NULL,
                                           em_refs_scope_body, NULL};
static const em_desc em_desc_scope_close_ref = {EM_KIND_FIELDS, 7, 0, -1, "Fiber.scope", NULL,
                                                em_refs_scope_close_ref, NULL};
static const em_desc em_desc_scope_close_val = {EM_KIND_FIELDS, 7, 0, -1, "Fiber.scope", NULL,
                                                em_refs_scope_close_val, NULL};
static const em_desc em_desc_attempt = {EM_KIND_FIELDS, 4, 0, -1, "Fiber.attempt", NULL,
                                        em_refs_frame1, NULL};

static void em_scope_fin(em_ref o) {
  em_scope *s = em_scope_of(o);
  em_drop(s->first);
  em_wait_unref(s->closer);
  em_vec_free(&s->children);
  em_rt_free(s);
}

/* A failure of the body or a child: the first becomes the result and interrupts the others;
 * later ones are notes (spec 11.2). Takes the cause. */
static void em_scope_failed(em_scope *s, em_ref cause) {
  if (s->first != NULL) {
    em_cause_note(s->first, cause);
    em_drop(cause);
    return;
  }
  s->first = cause;
  s->closed = true;
  for (size_t i = 0; i < s->children.len; i++) em_interrupt_fiber(s->children.items[i], NULL);
  if (!s->body_done) em_interrupt_fiber(s->fiber, s);
}

static void em_scope_child_ended(em_scope *s, em_fiber *child) {
  em_vec_remove(&s->children, child);
  em_ref cause = child->cause;
  if (cause != NULL && cause->slots[0].i != EM_CAUSE_INTERRUPT) {
    em_dup(cause);
    em_scope_failed(s, cause);
  }
  if (s->children.len == 0 && s->closer != NULL) {
    em_wait *w = s->closer;
    s->closer = NULL;
    em_cont(w, em_in_value(em_zero(), false));
    em_wait_unref(w);
  }
}

/* After every child ended: the first failure, or how the body ended. Takes `so` and `in`. */
static em_word em_scope_finish(em_ref so, em_input in) {
  em_scope *s = em_scope_of(so);
  s->ended = true;
  em_ref first = s->first;
  em_word out = em_zero();
  if (first != NULL) {
    em_input_drop(in);
    em_ref v = first->slots[1].p;
    em_dup(v);
    if (first->slots[0].i == EM_CAUSE_FAIL) em_failure = v;
    else em_defect = v;
  } else {
    out = em_input_put(in);
  }
  em_drop(so);
  return out;
}

/* The body ended with `in`: the scope closes, interrupts the children left and waits for them
 * (`$scope` after the body). Takes `so` and `in`. */
static em_word em_scope_body_done(em_ref so, int32_t repr, em_input in) {
  em_scope *s = em_scope_of(so);
  em_fiber *f = s->fiber;
  f->scopes--;
  em_vec_remove(&f->pending, s);
  em_vec_remove(&f->unwinds, s);
  s->body_done = true;
  s->closed = true;
  if (in.mode == EM_FAIL) {
    em_dup(in.value.p);
    em_scope_failed(s, em_cause_new(EM_CAUSE_FAIL, in.value.p));
  } else if (in.mode == EM_THROW && !em_is_interrupted(in.value.p)) {
    em_dup(in.value.p);
    em_scope_failed(s, em_cause_new(EM_CAUSE_DIE, in.value.p));
  }
  for (size_t i = 0; i < s->children.len; i++) em_interrupt_fiber(s->children.items[i], NULL);
  if (s->children.len == 0) return em_scope_finish(so, in);
  f->holds++;
  em_wait *w = em_wait_new(EM_WAIT_FIBER, true);
  em_wait_ref(w);
  em_wait_unref(s->closer);
  s->closer = w;
  em_suspend_on(w);
  em_ref fr = em_frame_new(in.ref ? &em_desc_scope_close_ref : &em_desc_scope_close_val,
                           em_scope_close_resume, 0, 4);
  fr->slots[3].p = so;
  fr->slots[4].l = 0;
  fr->slots[4].i = repr;
  fr->slots[5].l = 0;
  fr->slots[5].i = in.mode;
  fr->slots[6] = in.value;
  em_frame_push(fr);
  return em_zero();
}

static em_word em_scope_body_resume(em_ref fr, em_word in) {
  em_ref so = fr->slots[3].p;
  int32_t repr = fr->slots[4].i;
  em_frame_free(fr);
  return em_scope_body_done(so, repr, em_input_taken(in, repr == EM_REPR_REF));
}

static em_word em_scope_close_resume(em_ref fr, em_word in) {
  (void)in;
  em_ref so = fr->slots[3].p;
  em_input body = {fr->slots[5].i, fr->hdr.desc == &em_desc_scope_close_ref, fr->slots[6]};
  em_frame_free(fr);
  em_scope_of(so)->fiber->holds--;
  return em_scope_finish(so, body);
}

em_word em_fiber_scope(em_ref body, int32_t repr) {
  em_fiber *f = em_cur;
  if (f == NULL) em_unreachable();
  em_scope *s = em_rt_alloc(sizeof(em_scope));
  em_ref so = em_alloc(&em_desc_scope, 1);
  so->slots[0].h = s;
  s->self = so;
  s->fiber = f;
  s->depth = f->scopes++;
  em_dup(so);
  em_word out = em_call1(body, so, repr);
  if (em_suspending) {
    em_ref fr = em_frame_new(&em_desc_scope_body, em_scope_body_resume, 0, 2);
    fr->slots[3].p = so;
    fr->slots[4].l = 0;
    fr->slots[4].i = repr;
    em_frame_push(fr);
    return em_zero();
  }
  return em_scope_body_done(so, repr, em_input_taken(out, repr == EM_REPR_REF));
}

em_ref em_fiber_fork(em_ref so, em_ref fn) {
  em_scope *s = em_scope_of(so);
  if (s->closed) {
    em_raise_cstr("fiber scope was used after it ended");
    return NULL;
  }
  em_root *r = em_root_cur;
  em_fiber *child = em_fiber_new(r, s);
  em_vec_push(&s->children, child);
  em_job job = em_job_of(EM_JOB_BEGIN, NULL);
  em_dup(child->self);
  job.fiber = child;
  em_dup(fn);
  job.fn = fn;
  em_schedule(r, job);
  em_dup(child->self);
  return child->self;
}

em_word em_fiber_join(em_ref h, int32_t repr) {
  if (em_interrupt_here()) return em_zero();
  em_fiber *c = em_fiber_of(h);
  if (c->scope->ended) {
    em_raise_cstr("fiber was joined after its scope ended");
    return em_zero();
  }
  if (c->ended) return em_input_put(em_watch_settle(c, EM_VERB_JOIN, repr));
  em_wait *w = em_wait_new(EM_WAIT_FIBER, false);
  em_watch_add(&c->watchers, w, EM_VERB_JOIN, repr);
  em_suspend_on(w);
  return em_zero();
}

em_ref em_fiber_await(em_ref h, int32_t repr) {
  if (em_interrupt_here()) return NULL;
  em_fiber *c = em_fiber_of(h);
  if (c->scope->ended) {
    em_raise_cstr("fiber was awaited after its scope ended");
    return NULL;
  }
  if (c->ended) return em_input_put(em_watch_settle(c, EM_VERB_AWAIT, repr)).p;
  em_wait *w = em_wait_new(EM_WAIT_FIBER, false);
  em_watch_add(&c->watchers, w, EM_VERB_AWAIT, repr);
  em_suspend_on(w);
  return NULL;
}

void em_fiber_interrupt(em_ref h) {
  if (em_interrupt_here()) return;
  em_fiber *c = em_fiber_of(h);
  if (c->scope->ended) {
    em_raise_cstr("fiber was interrupted after its scope ended");
    return;
  }
  em_interrupt_fiber(c, NULL);
  if (c == em_cur) {
    if (c->holds == 0) em_defect = em_deliver(c);
    return;
  }
  if (c->ended) return;
  em_wait *w = em_wait_new(EM_WAIT_FIBER, false);
  em_watch_add(&c->watchers, w, EM_VERB_INTERRUPT, EM_REPR_VOID);
  em_suspend_on(w);
}

/* ---- Fiber.attempt (spec 11.5) ---- */

static em_ref em_attempt_finish(int32_t repr, em_input in) {
  em_word v;
  switch (in.mode) {
    case EM_THROW:
      if (em_is_interrupted(in.value.p) || (em_cur != NULL && em_is_unwinding(em_cur))) {
        em_defect = in.value.p;
        return NULL;
      }
      v.p = em_cause_new(EM_CAUSE_DIE, in.value.p);
      return em_exit_new(1, v, true);
    case EM_FAIL:
      v.p = em_cause_new(EM_CAUSE_FAIL, in.value.p);
      return em_exit_new(1, v, true);
    default:
      return em_exit_new(0, in.value, repr == EM_REPR_REF);
  }
}

static em_word em_attempt_resume(em_ref fr, em_word in) {
  int32_t repr = fr->slots[3].i;
  em_frame_free(fr);
  em_word out;
  out.p = em_attempt_finish(repr, em_input_taken(in, repr == EM_REPR_REF));
  return out;
}

em_ref em_fiber_attempt(em_ref fn, int32_t repr) {
  em_word out = em_call0(fn, repr);
  if (em_suspending) {
    em_ref fr = em_frame_new(&em_desc_attempt, em_attempt_resume, 0, 1);
    fr->slots[3].l = 0;
    fr->slots[3].i = repr;
    em_frame_push(fr);
    return NULL;
  }
  return em_attempt_finish(repr, em_input_taken(out, repr == EM_REPR_REF));
}

/* ---- Deferred (spec 11.7) ---- */

static void em_deferred_fin(em_ref o) {
  em_deferred *d = (em_deferred *)o->slots[0].h;
  if (d->full && d->ref) em_drop(d->value.p);
  em_drop(d->scope);
  for (size_t i = 0; i < d->waiters.len; i++) {
    em_watch *watch = d->waiters.items[i];
    em_wait_unref(watch->w);
    free(watch);
  }
  em_vec_free(&d->waiters);
  em_rt_free(d);
}

static em_deferred *em_deferred_of(em_ref o) { return (em_deferred *)o->slots[0].h; }

static bool em_deferred_ended(em_deferred *d) { return em_scope_of(d->scope)->ended; }

em_ref em_fiber_deferred(em_ref so) {
  if (em_scope_of(so)->closed) {
    em_raise_cstr("fiber scope was used after it ended");
    return NULL;
  }
  em_deferred *d = em_rt_alloc(sizeof(em_deferred));
  em_ref o = em_alloc(&em_desc_deferred, 1);
  o->slots[0].h = d;
  d->self = o;
  em_dup(so);
  d->scope = so;
  return o;
}

int32_t em_deferred_complete(em_ref o, em_word value, int32_t repr) {
  em_deferred *d = em_deferred_of(o);
  if (em_deferred_ended(d)) {
    em_raise_cstr("deferred was completed after its scope ended");
    return 0;
  }
  if (d->full) return 0;
  d->full = true;
  d->ref = repr == EM_REPR_REF;
  d->value = value;
  if (d->ref) em_dup(value.p);
  em_vec waiters = d->waiters;
  memset(&d->waiters, 0, sizeof d->waiters);
  for (size_t i = 0; i < waiters.len; i++) {
    em_watch *watch = waiters.items[i];
    if (d->ref) em_dup(value.p);
    em_cont(watch->w, em_in_value(value, d->ref));
    em_wait_unref(watch->w);
    free(watch);
  }
  em_vec_free(&waiters);
  return 1;
}

em_word em_deferred_await(em_ref o, int32_t repr) {
  if (em_interrupt_here()) return em_zero();
  em_deferred *d = em_deferred_of(o);
  if (em_deferred_ended(d)) {
    em_raise_cstr("deferred was awaited after its scope ended");
    return em_zero();
  }
  if (d->full) {
    if (d->ref) em_dup(d->value.p);
    return d->value;
  }
  em_wait *w = em_wait_new(EM_WAIT_FIBER, false);
  em_watch_add(&d->waiters, w, EM_VERB_DEFERRED, repr);
  em_suspend_on(w);
  return em_zero();
}

em_ref em_deferred_poll(em_ref o, int32_t repr) {
  em_deferred *d = em_deferred_of(o);
  if (em_deferred_ended(d)) {
    em_raise_cstr("deferred was polled after its scope ended");
    return NULL;
  }
  if (!d->full) return EM_NONE;
  if (d->ref) em_dup(d->value.p);
  return em_some(d->value, repr == EM_REPR_REF);
}

/* ---- The program ---- */

void em_start(int argc, char **argv) {
  (void)argc;
  (void)argv;
  em_loop_init();
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
  if (em_is_interrupted(em_defect)) {
    outcome = "interrupted";
    em_drop(em_defect);
    em_defect = NULL;
  } else if (em_defect != NULL) {
    outcome = "defect";
    message = em_defect;
    em_defect = NULL;
  } else if (em_failure != NULL) {
    outcome = "error";
    em_ref e = em_failure;
    em_failure = NULL;
    message = em_show_failure(e);
    if (em_defect != NULL) {
      em_drop(message);
      message = em_defect;
      em_defect = NULL;
    }
  }
  em_notes_clear();
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
  if (em_is_interrupted(em_defect)) {
    em_write_line(stderr, "interrupted", NULL);
    code = EM_EXIT_INTERRUPTED;
  } else if (em_defect != NULL) {
    em_write_line(stderr, "defect: ", em_defect);
    code = EM_EXIT_DEFECT;
  } else if (em_failure != NULL) {
    em_ref e = em_failure;
    em_failure = NULL;
    em_ref s = em_show_failure(e);
    em_write_line(stderr, "error: ", s);
    em_drop(s);
    code = EM_EXIT_ERROR;
  }
  em_drop(em_defect);
  em_defect = NULL;
  em_notes_clear();
  if (cleanup != NULL) cleanup();
  em_drop(em_last_retired);
  em_last_retired = NULL;
  fflush(stdout);
#ifdef EMELA_LEAK_CHECK
  bool check = true;
#else
  const char *env = getenv("EMELA_LEAK_CHECK");
  bool check = env != NULL && *env != '\0';
#endif
  if (check && em_live != 0) {
    fprintf(stderr, "emela: leak check: %" PRId64 " allocations are still live\n", em_live);
    code = EM_EXIT_LEAK;
  }
  fflush(stderr);
  return code;
}
