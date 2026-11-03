---
title: Closure syntax for anonymous functions
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [fn.decl, fn.block, lex.continuation, grammar.expr, type.fn, type.inference, type.rules, fiber.combinators, fmt.wrap, fmt.closure (new), fmt.io, diag.list]
requires: []
related: []
---

# Closure syntax for anonymous functions

## Summary

An anonymous function is written `|x| e`, `|a, b| e` or `|| e`. The body is one expression and extends as far to the right as possible; a body of several statements is a block, `|x| { ... }`. The form `fn(x) { e }` is removed from expressions, and `fn` is used only for declarations and function types. Existing code is rewritten mechanically by `emela fmt`.

## Motivation

Emela is written in direct style, so a computation to run later is passed as a function, and the wrapping cannot be avoided (`[fiber.combinators]`). Every such wrapping costs `fn() { ... }`: seven characters of ceremony for a thunk, and a closing `}` that has to be matched by eye with the `})` of the call.

In the `.emel` files of the emela-lang organization (196 files, excluding build output and test fixtures) there are 649 anonymous functions: 274 without parameters and 375 with parameters. 346 of them fit on one line, where the braces hold a single expression.

The wrapping piles up when calls that take thunks are nested: `Fiber.scope`, `Fiber.fork`, `with_permit`, `State.run`, `Quince.field`.

```emel
(a, b) = Fiber.both(fn() { fetch(1, 200) }, fn() { fetch(2, 100) })
names = Fiber.map([3, 4, 5], fn(id) { fetch(id, 100 * (6 - id)) })
first = Fiber.race([fn() { fetch(6, 300) }, fn() { fetch(7, 100) }])
failed = Fiber.scope(fn() {
  ok = Fiber.fork(fn() { fetch(9, 1000) })
  bad = Fiber.fork(fn() { fetch(-1, 100) })
  "#{Fiber.join(ok)} #{Fiber.join(bad)}"
}) escape {
  NotFound(id:) -> "not found: #{id}"
}
```

With this proposal:

```emel
(a, b) = Fiber.both(|| fetch(1, 200), || fetch(2, 100))
names = Fiber.map([3, 4, 5], |id| fetch(id, 100 * (6 - id)))
first = Fiber.race([|| fetch(6, 300), || fetch(7, 100)])
failed = Fiber.scope(|| {
  ok = Fiber.fork(|| fetch(9, 1000))
  bad = Fiber.fork(|| fetch(-1, 100))
  "#{Fiber.join(ok)} #{Fiber.join(bad)}"
}) escape {
  NotFound(id:) -> "not found: #{id}"
}
```

The thunks cannot be removed, so this proposal makes them light. The current specification has no workaround: `[fn.decl]` and `[fiber.combinators]` state that there is no shorthand.

## Proposal

### Syntax

```emel
xs |> List.map(|x| x + 1)
List.fold(xs, 0, |acc, x| acc + x)
Fiber.fork(|| fetch(1))
List.map(pairs, |(key, _)| key)
List.map(ids, |id: Int| id * 2)
Fiber.scope(|| {
  a = Fiber.fork(|| fetch(1))
  Fiber.join(a)
})
```

- The parameters are written between `|` and `|`, separated by `,`. Without parameters, the token `||` is written.
- A parameter is a pattern with an optional type annotation, as in the current form (`|(a, b): (Int, Int)| a + b`). There is no annotation of the return type, as in the current form.
- The body is one expression. A body of several statements is written as a block expression, `|x| { ... }`. Records and constructors are written with parentheses (`[data.parens]`), so a `{` after the parameter list always starts a block.
- `fn` is written only in declarations and in function types (`fn(A) -> B`). `fn(...)` in an expression is a compile error.

### Extent of the body

The body extends as far to the right as possible. It is an `expr`, so it includes `|>`, binary operators, postfix `match` and `escape`.

```emel
List.map(results, |r| r match {
  Ok(v) -> v
  Err(_) -> 0
})
```

The body ends where an `expr` ends: at a `,` or a closing bracket of an argument list, list or tuple, at the end of a statement, and at the end of an arm. Inside an argument list, `|a, b| a + b, 0` ends the body at the `,`.

Because the body takes everything to its right, an anonymous function can appear as an operand, but nothing can follow it in the same expression. To call it, pipe it, or apply an operator to it, it is enclosed in parentheses: `(|x| x + 1)(2)`. `a || || b` reads as `a || (|| b)`, and the formatter writes the parentheses.

### `|` inside the parameter list

At bracket depth zero inside the parameter list, `|` always closes the list. A type annotation whose error set has two or more items is enclosed in parentheses.

```emel
List.map(tasks, |f: (fn() -> Int fails Timeout | Refused)| f())
```

A single item (`|f: fn() -> Int fails Timeout| f()`) and an error set inside type arguments (`|h: Handler[Timeout | Refused, Db]| ...`) need no parentheses.

### Telling `|` and `||` apart

- `|` is written in error sets of types, and `||` is the logical or operator. Neither starts an operand, so `|` or `||` in operand position (the start of an expression, after a binary operator, after `(`, `[`, `,`, `=`, `->`) starts an anonymous function. After an operand, `||` is the operator. There are no or-patterns (`[grammar.pattern]`), so patterns are not affected.
- The lexer splits tokens by longest match (`[grammar.lex]`). `|x||y| x` reads as `|x` `||` `y|`, which is a syntax error; the formatter writes nested anonymous functions with a space, `|x| |y| x`.

### Line breaks

- A line that starts with `|` continues the previous line only inside a signature (rule 4 of `[lex.continuation]`), where a multi-line error set is written. Inside a body, a line that starts with `|` starts a new statement. Error sets written inside parentheses continue by rule 2 as before.
- A line that starts with `||` continues the previous line, as before (rule 1). An anonymous function without parameters cannot start a statement on its own line; it is enclosed in parentheses, `(|| e)`.
- The body starts on the line of the parameter list. Outside brackets, a newline right after the parameter list ends the statement.

### Meaning

No change. `|p1, ..., pn| e` means what `fn(p1, ..., pn) { e }` means today, and `|p1, ..., pn| { s1 ... sk }` means what `fn(p1, ..., pn) { s1 ... sk }` means. Parameter patterns, their irrefutability (E0317), the binding of the same name in two parameters (E0220), the capture of variables, the enclosing of E and R in the function type, and the rules on where an anonymous function counts as inside a handler (`[effect.handler]`, `[ext.call]`) are unchanged.

A binding `f = |x| ...` keeps the restrictions of today: it is not generalized (`[fn.block]`), it is called with positional arguments only (`[fn.args]`), and it cannot refer to itself, because a binding is visible only after it.

### Typing rules

No change. The row of `[type.rules]` is rewritten with the new form.

## Specification changes

### `[fn.decl]`

Replace the item "Anonymous functions have one form only, ..." and the item that follows with:

- An anonymous function is written `|x| x + 1`. The parameters are written between `|` and `|`, separated by `,` (`|acc, x| acc + x`). An anonymous function without parameters is written with the token `||` (`|| fetch(1)`). Parameter types can be omitted. There is no annotation of the return type.
- The body is one expression and extends as far to the right as possible: it includes `|>`, binary operators, and postfix `match` and `escape`. It ends where an expression ends: at a `,` or a closing bracket, at the end of a statement, and at the end of an arm. A body of several statements is a block, `|x| { ... }`. A `{` after the parameter list always starts a block, because records and constructors are written with parentheses (5.1).
- Since the body takes everything to its right, an anonymous function that is called, piped, or used as the left operand of an operator is enclosed in parentheses (`(|x| x + 1)(2)`).
- `fn` is written only in declarations and in function types. `fn(...)` in an expression is a compile error (E01xx).
- A parameter of an anonymous function can be a pattern instead of a name (`|(key, value)| ...`, `|acc, _| acc`, `|Request(url:, ..)| url`). A type, when written, comes after the pattern (`|(a, b): (Int, Int)| a + b`).
  - A parameter pattern is limited, as for bindings, to patterns that always match (6.3, E0317).
  - `|p| body` means the same as `|t| { p = t; body }`. Parameters are matched from left to right. Binding the same name in two parameters is a compile error (E0220).
  - Inside the parameter list, a `|` at bracket depth zero closes the list. A parameter type whose error set has two or more items is enclosed in parentheses (`|f: (fn() -> Int fails Timeout | Refused)| f()`).
  - The parameters of top-level fns, fns in impls and handlers, and operations of traits and effects are limited to names. This is because a parameter name also serves as the label of a named argument (6.2) and as the link to the declaration. Values of anonymous functions have no parameter names and are called by position only (6.2), so making a parameter a pattern loses nothing.

In the first item of the section, "In an anonymous function they can be omitted." stays as is.

### `[fn.block]`

Add:

- A binding of an anonymous function (`f = |x| x + 1`) is not generalized, is called with positional arguments only (6.2), and cannot refer to itself, because the name is visible only after the binding.

### `[lex.continuation]`

Replace rule 3 with:

3. The next line starts with `else` or `->`. Neither ever starts a statement, so treating them as continuations creates no ambiguity. The formatter aligns `else` into `} else {`.

Replace rule 4 with:

4. Inside a signature, the next line starts with `fails`, `use`, or `|`. Inside a signature means before the body's `{` of a fn declaration, and in the declaration of an effect operation. No statement starts there, so there is no ambiguity. Inside a body, `fails` can be used as an ordinary identifier, `use` starts an expression, and `|` starts an anonymous function, so they start a new statement. The body's `{` is written on the last line of the signature. The signature of a declaration without a body (an effect operation, an `@external` fn) closes at the end of its line, so `fails`, `use` and `|` at the start of the next declaration's line do not continue it.

Add to the list of tokens that always start a new statement:

- `|`, outside a signature: it starts an anonymous function.

Add to the supplementary notes:

- A line that starts with `||` continues the previous line by rule 1 (`||` is a binary operator). An anonymous function without parameters at the start of a statement is enclosed in parentheses (`(|| e)`).

Replace the example in the note on rule 2 with `f(|x| {⏎a⏎b⏎})`.

### `[grammar.expr]`

Replace `lambda` in `primary` and the rules `lambda` and `lambda_param` with:

```ebnf
unary      = ( "!" | "-" ) unary
           | "use" type_ref [ "[" type { "," type } "]" ]
           | lambda
           | postfix

lambda     = ( "||" | "|" [ lambda_param { "," lambda_param } ] "|" ) expr
lambda_param = pattern [ ":" type ]
```

Add:

- The body of a lambda is an `expr`, so it extends as far to the right as possible. A lambda is never followed by anything in the same expression; to call it or apply an operator to it, it is enclosed in parentheses.
- Inside the parameter list of a lambda, a `|` at bracket depth zero closes the list, and the `error_set` of a function type there has one item. Two or more items are written inside parentheses (`"(" type ")"`).
- `||` in operand position is a lambda without parameters; after an operand it is the operator of `or_expr`.

### `[type.fn]`

Replace the body of `compose` in the example with `|x| g(f(x)) # the returned function ...`.

### `[type.inference]`

Replace `fn caller() { fn(g) { g() } }` with `fn caller() { |g| g() }`.

### `[type.rules]`

Replace the row `fn(x) { e }` with `|x| e`. Replace `List.map(users, fn(u) { u.name })` with `List.map(users, |u| u.name)`.

### `[fiber.combinators]`

Replace the item "Something to run later is wrapped in `fn() { ... }` ..." with:

- Something to run later is wrapped in `|| ...` and passed. The style is direct and does not make the computation itself a value, so the wrapping cannot be avoided. The syntax of anonymous functions keeps it short (6.1).

Rewrite the examples of the section with the new form.

### `[fmt.closure]` (new)

Add to `spec/tooling/fmt.md` after `[fmt.wrap]`:

## Anonymous functions `[fmt.closure]`

- No space is placed inside the parameter list (`|x|`, `|a, b|`, `||`). One space is placed after it.
- Nested anonymous functions are separated by a space (`|x| |y| x`).
- An anonymous function that is the right operand of a binary operator or `|>`, or the operand of a unary operator, is enclosed in parentheses (`a || (|| b)`).
- When the whole body is a block that was written on one line with one expression, the braces are removed (`|x| { x + 1 }` becomes `|x| x + 1`).
- The formatter rewrites `fn(params) { body }` in an expression (E01xx) as an anonymous function: `|params| e` when the block has one expression on one line, and `|params| { ... }` otherwise. It adds the parentheses that the extent of the body requires (`fn(x) { x }(1)` becomes `(|x| x)(1)`).

### `[fmt.wrap]`

Replace `f(xs, fn(x) {` with `f(xs, |x| {` and `xs |> List.map(fn(x) {` with `xs |> List.map(|x| {`. "An anonymous function with a block" stays.

### `[fmt.io]`

Replace "A file with a syntax error is not formatted." with:

- A file with a syntax error is not formatted. E01xx (`fn(...)` in an expression) is not a syntax error here: the formatter rewrites it (`[fmt.closure]`).

### `[diag.list]`

- E01xx: "`fn(...)` written as an expression (6.1)". Message ``anonymous functions are written `|x| body` ``, with ``= help: write `|x| ...` `` showing the rewrite. The language server offers the rewrite as a quick fix.

### Examples in the rest of the specification

Every example in `spec/` that writes an anonymous function is rewritten with `emela fmt`. Text that names "anonymous function" is unchanged.

## Diagnostics

- [ ] E01xx: `fn(...)` written as an expression (code to be assigned), with a help and a quick fix
- [ ] Syntax error for a newline right after the parameter list outside brackets (the existing message for an expression that is missing)

## Compatibility

Every program that writes an anonymous function stops compiling. There is no migration period, following the decision to make match postfix (`spec/history.md`, 2026-10-13): from the release that implements this proposal, only the new form exists. `emela fmt` rewrites the old form, so a project migrates by running it once. The repository (the standard library, examples, tests and the examples of `spec/`) is rewritten in the implementation pull request.

The meaning of a program does not change after the rewrite.

## Implementation

- emela-syntax: lex `|` and `||` as today; parse `lambda` at `unary`, with the parameter list closed by `|` at depth zero and the body as `expr`. Read `fn(...)` in an expression into the same node and report E01xx. Move `PIPE` from `starts_continuation` (rule 3) to the signature rule (rule 4).
- emela-fmt: `[fmt.closure]`, hugging for `|x| {`, and the rewrite of the old form.
- emela-lsp: the quick fix for E01xx. Inlay hints on parameters of anonymous functions keep working on the new node (`|x: Int|`).
- Editors and `docs/syntax-highlight.md`: the TextMate grammar (VS Code), `editors/emela.sublime-syntax`, `editors/nvim/syntax/emela.vim` and `editors/tree-sitter-emela` (grammar, highlight, fold and indent queries) learn the closure form and highlight its parameters as parameters.
- No change in emela-resolve, emela-types or the backends beyond the node they read.

No prototype yet.

## Alternatives

- Arrow functions, `x => e` and `(a, b) => e`. Rejected: `(a, b)` and `()` are the forms of a tuple and of unit, so the parser has to look ahead to `=>` to know what it is reading. Emela already has `->` (function types and arms) and `|>`; a third arrow adds to them. Using `->` instead makes an anonymous function inside a match arm (`p -> x -> e`) hard to read.
- A trailing block, `f(a) { ... }`, passed as the last argument. Rejected: only the thunk in the last position gets shorter, so it does nothing for functions that take two thunks, such as `Fiber.both`. In the condition of an if (`if f(a) { ... }`), the block is ambiguous.
- Making the rest of the block a callback (a `use`-style binding). Rejected: the extent of the callback is not visible. It applies only to APIs that wrap "the rest", and in Emela that role belongs to `with`: deep nesting comes from APIs that take a thunk where a handler could be used. `use` is also already the keyword that takes a capability (`[effect.use]`).
- By-name parameters, evaluated each time the parameter is used. Rejected: a call in Emela carries no mark of effects (`[effect.use]`), so the reader could not tell when an argument is evaluated.
- Keeping `fn(x) { e }` beside the new form. Rejected: two ways of writing the same thing are not offered side by side (the decision on postfix match).

## Prior art

- Closures whose parameters are delimited by vertical bars and whose body is a single expression: Rust, Ruby blocks, Smalltalk blocks (`[:x | ...]`).
- Arrow functions: JavaScript, C#, Scala, Kotlin (`{ x -> e }`).
- A trailing lambda passed as the last argument: Kotlin, Swift, Ruby.
- Turning the rest of a block into a callback: `use` in Gleam, `<-` in Koka's `with` syntax.
- By-name parameters: Scala, Algol 60 call by name.

## Open questions

- A parameterless anonymous function at the start of a statement is written `(|| e)`, because `||` at the start of a line continues the previous line. The alternative is to make `||` at the start of a line inside a body start a new statement, which breaks the formatter's way of breaking a chain of `||` (`[fmt.wrap]`). Is the parenthesis acceptable?
- Should the formatter remove the braces of a one-line block body with one expression (`|x| { x + 1 }` to `|x| x + 1`)? Removing them gives one way to write it; keeping them leaves the choice to the writer.
- Does the formatter keep rewriting the old form indefinitely, or only for a fixed number of releases?
- Requiring parentheses for an error set of two or more items in a parameter type, versus reading `|` there as an error-set separator when the token after it is an upper name. The latter needs no parentheses but makes `|x: fn() -> A fails E| B` depend on the case of the next token.

## Decision
