// The external scanner: statement-ending newlines ([lex.continuation]) and string bodies
// ([lex.literal]).
//
// A newline is an extra (whitespace) unless the grammar accepts NEWLINE at that point and the
// line does not continue. Inside `(`, `[` and `#{` the grammar never accepts NEWLINE, so the
// innermost-bracket rule (rule 2) follows from the parse state and needs no bracket stack here.

#include "tree_sitter/parser.h"

#include <stdbool.h>

enum TokenType {
  NEWLINE,
  STRING_CONTENT,
  MULTILINE_STRING_CONTENT,
  INTERPOLATION_START,
  SIGNATURE_CONTINUES,
  DERIVE_CONTINUES,
  ERROR_SENTINEL,
};

void *tree_sitter_emela_external_scanner_create(void) { return NULL; }

void tree_sitter_emela_external_scanner_destroy(void *payload) { (void)payload; }

unsigned tree_sitter_emela_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_emela_external_scanner_deserialize(void *payload, const char *buffer,
                                                    unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }

static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

static inline bool is_name_char(int32_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
         c == '_';
}

static inline bool is_blank(int32_t c) { return c == ' ' || c == '\t' || c == '\r'; }

// Skips whitespace, newlines and comments, the trivia between a newline and the next meaningful
// token.
static void skip_trivia(TSLexer *lexer) {
  for (;;) {
    if (is_blank(lexer->lookahead) || lexer->lookahead == '\n') {
      advance(lexer);
    } else if (lexer->lookahead == '#') {
      while (lexer->lookahead != '\n' && !lexer->eof(lexer)) advance(lexer);
    } else {
      return;
    }
  }
}

// Reads a name of at most `size - 1` characters into `buffer`. A longer name is cut off, which
// is enough to compare it with the keywords below.
static void read_name(TSLexer *lexer, char *buffer, size_t size) {
  size_t length = 0;
  while (is_name_char(lexer->lookahead)) {
    if (length + 1 < size) buffer[length++] = (char)lexer->lookahead;
    advance(lexer);
  }
  buffer[length] = '\0';
}

// String equality without libc, which the WebAssembly build of a grammar (Zed) may lack.
static bool equals(const char *a, const char *b) {
  while (*a && *a == *b) {
    a++;
    b++;
  }
  return *a == *b;
}

// Whether the line that starts at the lexer's position continues the previous one. The lexer
// is on the first meaningful character after the newline.
static bool continues_line(TSLexer *lexer, const bool *valid_symbols) {
  int32_t c = lexer->lookahead;
  switch (c) {
    // Rule 1: binary operators (except `-`) and `.`. Rule 3: `|`.
    case '*':
    case '/':
    case '%':
    case '+':
    case '<':
    case '>':
    case '|':
    case '.':
      return true;
    case '=':
    case '!':
      advance(lexer);
      return lexer->lookahead == '=';
    case '&':
      advance(lexer);
      return lexer->lookahead == '&';
    // `-` is a unary minus at the start of a line; `->` continues (rule 3).
    case '-':
      advance(lexer);
      return lexer->lookahead == '>';
    default:
      break;
  }
  if (!(c >= 'a' && c <= 'z')) return false;

  char name[8];
  read_name(lexer, name, sizeof name);
  // Rule 1 and rule 3.
  if (equals(name, "else") || equals(name, "escape")) return true;
  // Rule 1: a postfix `match` is always followed by `{`.
  if (equals(name, "match")) {
    skip_trivia(lexer);
    return lexer->lookahead == '{';
  }
  // Rule 4: inside a signature.
  if (equals(name, "fails") || equals(name, "use")) {
    return valid_symbols[SIGNATURE_CONTINUES];
  }
  // [grammar.decl]: `derive` on the line after a type, enum or error declaration.
  if (equals(name, "derive")) return valid_symbols[DERIVE_CONTINUES];
  return false;
}

static bool scan_newline(TSLexer *lexer, const bool *valid_symbols) {
  while (is_blank(lexer->lookahead)) skip(lexer);
  if (lexer->lookahead != '\n') return false;
  advance(lexer);
  lexer->mark_end(lexer);
  skip_trivia(lexer);
  if (lexer->eof(lexer)) {
    lexer->result_symbol = NEWLINE;
    return true;
  }
  if (continues_line(lexer, valid_symbols)) return false;
  lexer->result_symbol = NEWLINE;
  return true;
}

// The lexer has read `#` and is on `{`, and the end is marked before `#`. Returns the body read
// so far, or `#{` itself when there is none.
static bool finish_before_interpolation(TSLexer *lexer, bool has_content,
                                        enum TokenType content) {
  if (has_content) {
    lexer->result_symbol = content;
    return true;
  }
  advance(lexer);
  lexer->mark_end(lexer);
  lexer->result_symbol = INTERPOLATION_START;
  return true;
}

// The body of a `"` string up to the closing `"`, an escape, an interpolation or the end of the
// line (an unterminated string, E0101, ends there).
static bool scan_string_content(TSLexer *lexer) {
  bool has_content = false;
  for (;;) {
    int32_t c = lexer->lookahead;
    if (lexer->eof(lexer) || c == '"' || c == '\\' || c == '\n') break;
    if (c == '#') {
      lexer->mark_end(lexer);
      advance(lexer);
      if (lexer->lookahead == '{') return finish_before_interpolation(lexer, has_content,
                                                                       STRING_CONTENT);
      has_content = true;
      continue;
    }
    advance(lexer);
    has_content = true;
  }
  lexer->mark_end(lexer);
  lexer->result_symbol = STRING_CONTENT;
  return has_content;
}

// Whether the rest of the current line is whitespace followed by `"""`. Consumes what it reads.
static bool at_closing_line(TSLexer *lexer) {
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') advance(lexer);
  for (int i = 0; i < 3; i++) {
    if (lexer->lookahead != '"') return false;
    advance(lexer);
  }
  return true;
}

// The body of a `"""` string. A line that holds only the closing `"""` ends it before its
// indentation; a `"""` elsewhere also ends it (E0109 in the compiler).
static bool scan_multiline_string_content(TSLexer *lexer) {
  // The indentation of the closing line is whitespace before the `"""` token, not body.
  if (lexer->get_column(lexer) == 0 && at_closing_line(lexer)) return false;
  bool has_content = false;
  for (;;) {
    lexer->mark_end(lexer);
    int32_t c = lexer->lookahead;
    if (lexer->eof(lexer) || c == '\\') break;
    if (c == '\n') {
      advance(lexer);
      has_content = true;
      lexer->mark_end(lexer);
      if (at_closing_line(lexer)) break;
      // `at_closing_line` consumed some of the next line; it belongs to the body.
      continue;
    }
    if (c == '"') {
      advance(lexer);
      if (lexer->lookahead == '"') {
        advance(lexer);
        if (lexer->lookahead == '"') break;
      }
      has_content = true;
      continue;
    }
    if (c == '#') {
      advance(lexer);
      if (lexer->lookahead == '{') return finish_before_interpolation(lexer, has_content,
                                                                       MULTILINE_STRING_CONTENT);
      has_content = true;
      continue;
    }
    advance(lexer);
    has_content = true;
  }
  lexer->result_symbol = MULTILINE_STRING_CONTENT;
  return has_content;
}

bool tree_sitter_emela_external_scanner_scan(void *payload, TSLexer *lexer,
                                             const bool *valid_symbols) {
  (void)payload;
  if (valid_symbols[ERROR_SENTINEL]) return false;

  if (valid_symbols[STRING_CONTENT]) return scan_string_content(lexer);
  if (valid_symbols[MULTILINE_STRING_CONTENT]) return scan_multiline_string_content(lexer);
  if (valid_symbols[NEWLINE]) return scan_newline(lexer, valid_symbols);
  return false;
}
