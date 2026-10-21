# formatter `[fmt]`

This section defines the shape of the source the formatter outputs. The formatter is shared by `emela fmt` (`[cmd.fmt]`) and the language server's formatting (`[lsp.formatting]`). Whichever calls it, it returns the same output for the same input.

## Input and output `[fmt.io]`

- The line width is 100. Width is counted as display width, and a full-width character counts as 2. A line that does not fit because a string or name is long can exceed the width.
- A file with a syntax error is not formatted.
- The formatter re-reads the formatted result and confirms that it has no syntax error and that its token sequence and order of comments match the input. If they do not match, it is an internal error, and the input is not rewritten.
- Line endings are normalized to LF, trailing whitespace is removed, and the file ends with a single newline. An empty file is output empty.
- The indentation and width cannot be changed. The formatting options the language server receives (`[lsp.formatting]`) do not change them either.

## Whitespace and line breaks `[fmt.space]`

- Indentation is two spaces.
- Consecutive blank lines are collapsed into one. Blank lines at the start and end of a file, a block, or a sequence of arms are removed. Blank lines inside sequences such as arguments are also removed.
- Blank lines between top-level declarations are kept as in the original source (0 or 1). No blank lines are added.
- One space is placed before and after operators, after `,`, and after `:`. No space is placed before `:`, inside parentheses, or before and after `.` and `..`. Only the braces of an effect set get spaces inside (`use { Io, Clock }`).
- The space between `-` and a number in a pattern is removed (`[fn.match]`).
- The shorthand of named arguments and field patterns (`id:`) is kept as is. `id: id` is not rewritten to the shorthand either.
- The `=` of bindings is not aligned, because adding one line would make the surrounding lines part of the diff.
- A trailing comment is preceded by one space and is not aligned, for the same reason as the `=` of bindings.

## Blocks and control constructs `[fmt.block]`

- A block or a sequence of arms (match, escape) stays on one line as long as it was written on one line in the original source (no line breaks, at most one statement or arm) and fits in the width. If the original spans multiple lines, it is formatted on multiple lines. An empty block is `{}`.
- The blocks of if, else if, and else are broken together. If any of them is multiline, all of them are multiline.
- A `}` followed by `else {` on the next line is normalized to `} else {` (rule 3 of `[lex.continuation]`). However, if the `}` is followed by a trailing comment, `else` is placed on the next line.
- The `->` of match and escape arms is aligned only among the arms of the same sequence that fit on one line. If alignment makes some arm exceed the width, the arm with the longest head (pattern and guard) is excluded from alignment and alignment is redone. Arms that contain comments are not aligned.

## Wrapping `[fmt.wrap]`

- A sequence of arguments, parameters, tuple elements, list elements, type arguments, or pattern arguments is put on one line without a trailing comma if it fits. If it does not fit, it is put one element per line with a trailing comma. A trailing comma in the original source does not affect how it is broken.
- However, if the last element has a trailing comment and the original source has no comma, no comma is added, because the added comma would go inside the comment.
- The `..expr` of a record update (`User(name: "b", ..user)`) is treated as the last element of the argument sequence, and when broken, it also gets a trailing comma.
- If the last argument is an anonymous function with a block (it can also be the value of a named argument), the formatter keeps the parenthesis open and hugs the body, as long as the other arguments fit on one line and the text up to `f(xs, fn(x) {` fits in the width.
- Binary operators are broken by grouping a chain of operators of the same precedence, breaking before each operator and indenting one level. The formatter does not break before `-`, because a `-` at the start of a line starts a new statement as unary minus (`[lex.continuation]`).
- A pipeline is broken before every `|>` if the original source has a line break before at least one `|>`. Otherwise, the width decides. If there is a single pipe and its right side can be broken inside a hugged anonymous function, the pipe is not broken as long as the first line fits (`xs |> List.map(fn(x) {`).
- The formatter does not break after the `=` of a binding or after the `->` of an arm, because an expression at the start of a line starts a new statement.
- For a postfix match or escape, when its left side is a pipeline and that pipeline is broken (the `|>` lines are lined up indented one level), `match {` or `escape {` is placed on the next line indented one level, and the arms are indented one more level. When the left-side pipeline fits on one line, when it breaks only inside a hugged anonymous function, or when the left side is not a pipeline, the construct continues on the same line, like `} match {`. When the pipeline fits on one line but adding `match {` exceeds the width, only `match {` is moved to the next line. A line starting with `match` continues the previous line by rule 1 of `[lex.continuation]`.
- The inside of a string interpolation `#{ }` is not broken by width. Whitespace inside it is normalized. Only line breaks caused by comments and multiline blocks are kept.
- A sequence or expression that contains a multiline string is always broken there.

## Signatures `[fmt.signature]`

The signatures of fn declarations and effect operations are formatted as follows.

- In a function declaration, only the parameter sequence is broken by width. `->` is not broken and is placed on the same line as `)`.
- The `fails` and `use` clauses (rule 4 of `[lex.continuation]`) form a single group placed after the parameter sequence. If they do not fit on the line, both are placed on the next line, indented one level (the form in the example of `[lex.continuation]`). If the first line still does not fit, the parameter sequence is broken.
- When the parameter sequence is broken, the clauses stay on the `)` line as long as they fit there.
- The `{` of the body is attached to the last line of the signature.
- The inside of a clause (the `|` sequence of `fails`, the set of `use`) is not broken, except for things that force a line break, such as a comment inside `use { ... }`.
- A signature that fits is put on one line. However, if the original source wrote the clauses on the next line, they stay on the next line even if they fit. If either clause is on the next line, the whole group is broken, so both are lined up on the next line. Line breaks in the parameter sequence of the original source are not kept.

```emel
pub fn get(url: String) -> Response
  fails InvalidUrl | ConnectionFailed | ResponseNotText
  use Http {
  ...
}
```

## Declarations `[fmt.decl]`

- The heads of declarations follow the same rules as fn. Sequences of type parameters, arguments, and fields are written directly after the name, and everything else is separated by one space.
- Attributes are placed one per line. The inside of `@external(js, "x")` is formatted by the same rules as an argument sequence. A field attribute (`@json("n") name: String`) is placed on the same line as the field.
- The bodies of enum variants, layer handlers, effect, handler, trait, and impl are put one item per line with the separating commas removed if they span multiple lines in the original source (the form in the example of `[data.enum]`). If the original is on one line, they keep the `{ Red, Green, Blue }` form as long as they fit, and become multiline otherwise. Up to one blank line between items is kept.
- `derive` is placed on the next line, indented one level, if it was written on the next line in the original source. If it was written on the same line and does not fit, it is moved to the next line.
- Field types of a type and trailing comments of variants are not aligned.
- The name list of an import `.{Json, decode}` has no spaces inside the braces (the form in the example of `[module.import]`).

## Strings and literals `[fmt.literal]`

- The contents of a multiline string `"""` are not changed by a single character. Indentation and trailing whitespace are part of the value, so even if the indentation of the enclosing block changes, the inner lines and the closing `"""` do not move. Only the position of the opening `"""` moves when formatting.
- Hexadecimal and binary integers and escapes are output as written. `0xff` is not normalized to `0xFF`.

## Comments `[fmt.comment]`

- A comment attaches to the preceding token if it is on the same line as that token, and to the following token if it is on its own line.
- A comment attached before a declaration, statement, arm, sequence element, operator, or closing bracket is placed in front of it, at that line's indentation.
- A comment in any other position (before `.` or `escape`, inside type arguments, and so on) is output as is before or after the token it is attached to, with a line break inserted if needed. Those positions are inside brackets or in a continuation position, so inserting a line break does not change how the code is read.
- `##` documentation comments (`[lex.comment]`) are handled the same as `#` comments.
