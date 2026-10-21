# 2. Lexical structure `[lex]`

The kind of a name is determined by case, and the end of a statement is determined by line breaks. Both are fixed during lexical analysis and do not depend on the result of name resolution.

## 2.1 Source files `[lex.source]`

- The file extension is `.emel` and the encoding is UTF-8.
- Semicolons are not used. A statement ends at a line break (2.5).
- Whitespace is the space and the tab. A line break is `\n` or `\r\n`, and either one is treated as a single line break. A lone `\r` and Unicode whitespace (such as the ideographic space) are unrecognized characters.

## 2.2 Comments `[lex.comment]`

- A comment runs from `#` to the end of the line. A comment ends before the line break, and the `\r` of `\r\n` is not part of the comment.
- `##` is a doc comment. It attaches to the declaration, variant, field, effect operation, or handler item that immediately follows it. A doc comment written between a declaration and its `derive` does not attach to the declaration.
- There are no block comments.

## 2.3 Lexical classes of names `[lex.name]`

| Class | Form | Used for | Examples |
|---|---|---|---|
| Lowercase name | snake_case starting with a lowercase letter | Variables, functions, fields, argument names, effect operations | `user_id` `find` |
| Type name | PascalCase starting with an uppercase letter and containing at least one lowercase letter | Types, constructors, errors, effects, handlers, layers, Traits, modules, type parameters | `User` `Http` `Db` `Item` |
| Uppercase name | Only uppercase letters, digits, and underscores | consts, type parameters | `MAX_SIZE` `A` |

- A name that fits no class is a syntax error, which is a compile error (E0103). The whole name counts as one mistake, and only one diagnostic is reported (`Http_client`). `HTTP` and `DB` cannot be used as type names. Write `Http` and `Db`.
- Type names and uppercase names refer to existing definitions. They are never newly bound inside an expression or a pattern. As a result, a typo in a pattern is always a compile error as an undefined name.
- `_` is a pattern that binds no value.
- A type parameter can be written as either an uppercase name (`A`, `T`) or a type name (`Item`, `Key`). Type parameters can be declared only inside `[...]` and can be referenced only in type positions (writing one in an expression is E0217). Whether a type parameter written as a type name is a type or a type parameter is decided by name resolution.
- A type parameter shadows an outer type, effect, error, or Trait with the same name. When a type parameter written as a type name shadows one, the compiler warns (W0201). This keeps existing code from breaking when types are added later (including additions to the Prelude).
- Names can contain only ASCII characters. A non-ASCII character (`é`) is an unrecognized character (E0105).
- Lexical analysis follows the EBNF in 17.2. A lowercase name containing uppercase letters in the middle (`userId`) and a type name with consecutive uppercase letters (`HTTPServer`) are allowed lexically. The linter checks the snake_case and PascalCase conventions.

From these rules, the meaning of `.` is clear from its left-hand side.

- Type name on the left: a static reference (a function of a module, a method of a Trait). Example: `List.map`
- Lowercase name or uppercase name on the left: access to a value (a field, an operation of a capability obtained with `use`). Examples: `user.name`, `log.info`, `DEFAULT_CONFIG.port`

## 2.4 Literals `[lex.literal]`

| Kind | Form | Notes |
|---|---|---|
| Integer | `42` `1_000_000` `0xFF` `0b1010` | Int64 if the context requires Int64, otherwise Int. Never Float. `0x` is hexadecimal and `0b` is binary |
| Floating point | `3.14` `1.0e9` | Float |
| String | `"hello"` `"""..."""` | `#{expression}` interpolates. The type of the expression MUST satisfy Show. The inside of `#{ }` is lexed as ordinary code and can contain comments and line breaks (a line break continues the line by 2.5). Enclosing in `"""` allows multiple lines |
| List | `[1, 2, 3]` `[x, ..rest]` | `List[Int]`. A trailing `..expression` builds a list with the preceding elements prepended to it (17.5) |
| Tuple | `(a, b)` | Two or more elements |
| Unit | `()` | The tuple with zero elements |

Boolean values are not literals. They are `True` and `False`, the constructors of Bool.

- A floating-point exponent can be attached only to the form with a decimal point (17.2). `1e9` cannot be written. Write `1.0e9`. A number immediately followed by a letter (`1e9` `3px`) is a lexical error, which is a compile error (E0104).
- A string cannot span a line break (E0101). Write multiple lines with a `"""` string.
- Hexadecimal and binary prefixes are lowercase only (`0x`, `0b`). Hexadecimal digits can be either uppercase or lowercase. There is no octal. An uppercase prefix (`0XFF`), an unsupported prefix (`0o17`), a prefix with no digits (`0x`), and a hexadecimal or binary fraction (`0x1.5`) are E0104, with the whole chunk counted as one mistake. A value outside the range of the context type (Int or Int64) is a compile error (E0311). A negative number is a unary minus expression, so the range check applies to the literal itself as a positive number. As a result, the minimum Int value `-2147483648` cannot be written as a literal (core provides a constant such as `Int.MIN`). Literals are not reinterpreted as two's complement (`0xFFFF_FFFF` cannot be written as an Int but can be written as an Int64).
- There are seven string escapes, listed below. Any other `\` is a compile error (E0107). The diagnostic points to the `\` and the one character after it.

| Escape | Meaning |
|---|---|
| `\"` | `"` |
| `\\` | `\` |
| `\n` | Line feed (U+000A) |
| `\t` | Tab (U+0009) |
| `\r` | Carriage return (U+000D) |
| `\#` | `#`. Used to write `#{` as characters instead of an interpolation |
| `\u{...}` | A Unicode code point written as 1 to 6 hexadecimal digits. A surrogate (D800 to DFFF) or a value above 10FFFF is a compile error |

- Errors in `\u{...}` are all E0108: a malformed escape (a missing `{` or `}`, or a digit count other than 1 to 6), a surrogate, and a value above 10FFFF. The diagnostic points from the `\` to the last character that was read.

- A multi-line string is enclosed in `"""`. The opening `"""` MUST be immediately followed by a line break, and the closing `"""` is placed alone on its own line. The indentation of the line with the closing `"""` is removed from the start of each line. The line break right after the opening `"""` and the line break right before the closing `"""` are not included. A line (other than an empty line) indented less than the closing `"""` is a compile error (E0109). Indentation is compared as a sequence of characters, and each line other than empty lines MUST start with the same sequence as the indentation of the closing `"""` line. A line that differs only in tabs versus spaces is also considered less indented. When the closing `"""` is not alone on its line, the indentation is not checked. A `\` at the end of a line inside a multi-line string is E0106, and the rest is read as part of the string body. Interpolation and escapes work the same as in ordinary strings.

```emel
query = """
  SELECT *
  FROM users
  WHERE id = #{id}
  """
# same as "SELECT *\nFROM users\nWHERE id = 1"
```

- There is no character type (Char). A single character is represented as a String of length 1 (16.2).
- There are no raw strings that leave escapes uninterpreted.
- A negative number is not a literal. It is an expression with a unary minus. In a pattern, `-` can be written before a numeric literal (6.5, 17.6).

## 2.5 Line continuation rules `[lex.continuation]`

A line break ends a statement. In the following cases, however, the previous line continues.

1. The next line starts with a binary operator, `.`, `escape`, or `match`. `match` continues the line only when the next meaningful token is `{` (a postfix match is always `match {`, so the reading of a correct program does not change. Any other line is read as a new statement with a prefix match, and E0142 is reported).
2. An opened `(` or `[` is not yet closed.
3. The next line starts with `else`, `->`, or `|`. None of these ever starts a statement, so treating them as continuations creates no ambiguity. The formatter aligns them into `} else {`.
4. Inside a signature, the next line starts with `fails` or `use`. Inside a signature means before the body's `{` of a fn declaration, and in the declaration of an effect operation. No statement starts there, so there is no ambiguity. Inside a body, `fails` can be used as an ordinary identifier and `use` starts an expression, so they start a new statement as before. The body's `{` is written on the last line of the signature. The signature of a declaration without a body (an effect operation, an `@external` fn) closes at the end of its line, so `fails` and `use` at the start of the next declaration's line do not continue it.

```emel
pub fn get(url: String) -> Response
  fails InvalidUrl | ConnectionFailed | ResponseNotText
  use Http {
  ...
}
```

A line that starts with one of the following tokens always starts a new statement.

- `(` and `[`: write the opening bracket of a function call or a type argument list on the same line as the name.
- `-`: at the start of a line it is read as a unary minus. A `-` at the start of a line does not continue the previous line and starts a new statement.

Outside brackets, a binary operator at the end of a line does not continue onto the next line. To continue the previous line with a binary operator, place an operator other than `-` at the start of the next line. To write the right-hand side of a subtraction on the next line, enclose the expression in parentheses. A line break inside parentheses continues by rule 2.

Supplementary notes:

- Continuation is decided by the "meaningful token" after the line break. Whitespace, comment-only lines, and empty lines in between are skipped.
- The "opened bracket" in rule 2 is decided by the innermost level of bracket nesting. Even inside `(`, a line break separates statements if a `{` is open further inside (`f(fn(x) {⏎a⏎b⏎})`).
- A line break inside a string interpolation `#{ }` is a continuation, the same as inside `(`.

```emel
user = users.find(id)
  |> Option.or_fail(NotFound(id:)) # continues the previous line

foo
(bar) # a separate statement from foo
```
