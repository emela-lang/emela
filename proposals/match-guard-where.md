---
title: Write the guard of an arm with where
authors: [somnicattus]
status: draft
issue:
touches: [kw, kw.reserved, kw.future, grammar.expr, fn.match, lib.api-rules, fmt.io, fmt.block, lsp.code-action, diag.list]
requires: []
related: []
---

# Write the guard of an arm with where

## Summary

The guard of a match or escape arm is written after `where` instead of `if`: `Circle(radius:) where radius > 0.0 -> ...`. `where` moves from the words reserved for the future (`[kw.future]`) to the reserved words (`[kw.reserved]`). The meaning of a guard does not change. Until 0.30, a guard written with `if` keeps its meaning and is reported with a warning (W1401); from 0.30 it is a compile error (E1406). `emela fmt` rewrites `if` to `where`, and the language server offers the rewrite as a quick fix.

## Motivation

### A guard and an if expression look the same

`if` starts two constructs: the if expression (`[fn.if]`) and the guard of an arm (`arm = pattern [ "if" expr ] "->" expr`, `[grammar.expr]`). A reader tells them apart only by what comes before `if`: a pattern, or the start of an expression. A guard is any expression, so the two constructs can meet in one arm. The program below compiles today, and `emela fmt` keeps it as it is (emela 0.29.0 at 5ad03e79):

```emel
fn sign(o: Option[Int], b: Bool) -> Int {
  o match {
    Some(x) if if b { x > 0 } else { x < 0 } -> 1
    _                                        -> 0
  }
}
```

`if` also suggests the parts of an if expression: a condition, a block after it, and an `else`. A guard has no block and no `else`: when it is true, the body after `->` is evaluated, and when it is false, the next arm is tried.

### The guard is a condition on the matched value

A guard narrows the values the pattern matches: the arm `Circle(radius:) if radius > 0.0` matches the circles whose radius is positive. A word that reads as "such that" states this: `Circle(radius:) where radius > 0.0` reads as "a circle whose radius is positive". Some guards do not use the names the pattern binds (`Some(_) where ready`); how `where` reads there is in Open questions.

`where` has no syntax today. It is in `[kw.future]`, so no program uses it as an identifier, and making it a keyword breaks no program. Only removing `if` as the word of a guard breaks programs (Compatibility). The two places where `where` was considered before did not take it: as the name of a predicate parameter (`[lib.api-rules]`: rejected because reserved words cannot be parameter names), and as a block of helpers after the body of a fn (rejected in `local-fn`: "it adds a second place for declarations with its own scope rules").

## Proposal

### Syntax

In `[grammar.expr]`:

```ebnf
arm        = pattern [ "where" expr ] "->" expr
```

The guard of a match arm and of an escape arm are both `arm`, so both change.

```emel
shape match {
  Circle(radius:) where radius > 0.0 -> 3.14 * radius * radius
  Circle(_)                          -> 0.0
  Rect(w, h)                         -> w * h
  Empty                              -> 0.0
}
```

`where` is added to the row Expressions of `[kw.reserved]`, after `match`, and removed from `[kw.future]`. The count of reserved words in `[kw]` goes from 25 to 26 on `main` at the time of writing. The loop expression (`loop-expression`, #909) and the words reserved for the future (`future-reserved-words`, #950) also change `[kw]` and `[kw.future]`; whichever lands later is rebased onto the others, and the count follows the order in which they land.

Until 0.30, `if` MAY be written in place of `where`, with the same meaning, and is reported with W1401. From 0.30, `if` after the pattern of an arm is E1406, and the parser recovers by reading it as `where`. `[grammar.expr]` states this below the grammar:

> - Until 0.30, the guard of an arm MAY be written after `if` instead of `where`, with the same meaning (W1401). From 0.30 it is a compile error (E1406).

### Meaning

Unchanged. A guard is evaluated after its pattern matches (`[dyn.order]`), arms with a guard are not counted for exhaustiveness and do not cover the arms below them (`[fn.match]`), and a tag is removed from E only by arms without guards (`[error.escape]`). In `[fn.match]`, "Arms with a guard (`if condition`)" becomes "Arms with a guard (`where condition`)", and the example at the top of the section uses `where`.

The expression after `where` is any expression, as after `if` today.

### Typing rules

Unchanged.

### Tools

- `[fmt.block]` gains "The formatter rewrites `if` before the guard of a match or escape arm (W1401, E1406) as `where`." The `->` of the arms is aligned after the rewrite.
- `[fmt.io]`: E1406 joins E1404 and E1405 in the sentence that lists the diagnostics that are not syntax errors for the formatter, because the formatter rewrites them. W1401 is a warning, so a file with it is formatted anyway.
- `[lsp.code-action]`: the list of fixes gains "W1401 and E1406 (a guard written after `if`): replaces `if` with `where`".
- `[lsp.completion]` offers the reserved words, so `where` becomes a candidate with them, as `match` and `escape` are; its text does not change.

### Other sections

- `[lib.api-rules]`: "both are reserved words (3.1, 3.3)" becomes "both are reserved words (3.1)", since `where` moves to 3.1.

## Diagnostics

| Code | Meaning | Example message |
|---|---|---|
| E1406 | From 0.30, the guard of an arm written after `if` (6.5). At `if`. The help is ``= help: write `where` instead of `if` ``. The formatter rewrites it, and the language server offers the rewrite as a quick fix | ``write `where` before a guard, not `if` `` |
| W1401 | Until 0.30, the guard of an arm written after `if` (6.5). The arm keeps its meaning. At `if`. The help is ``= help: write `where` instead of `if` ``. The formatter rewrites it, and the language server offers the rewrite as a quick fix | ``a guard written after `if` is deprecated`` |

- [ ] W1401 at `if` after the pattern of a match arm and of an escape arm; the program keeps its meaning
- [ ] `where` as an identifier is still a compile error, with the diagnostics it gets today (E1101 in a declaration; E1102 and E1109 in a binding)
- [ ] fmt rewrites `if` to `where` and keeps the alignment of `->`
- [ ] the quick fix of W1401

E1406 and the switch from W1401 to E1406 are implemented in the pull request that makes 0.30.

## Compatibility

`where` was reserved, so no identifier changes meaning. Programs that use a guard compile with W1401 until 0.30 and fail to compile from 0.30. `emela fmt` rewrites them.

In the repository (upstream/main at de785e14): 42 arms with a guard in 11 files of std and core (`crates/emela-core/lib/`), 12 in `.emel` files of tests and fixtures, programs in Rust tests and examples in doc comments (16 Rust files), and the example of `[fn.match]`.

The new form lands in a patch release with W1401 (`docs/release.md`: a deprecation is announced in a patch, and the form is removed in the next minor). It does not need an experimental feature: it is one word in one position, and the old form keeps its meaning until 0.30.

## Implementation

- `emela-syntax`: the lexer makes `where` a keyword token (it is now rejected as a reserved word in `crates/emela-syntax/src/lexer/name.rs`). The parser of `arm` accepts `where`, and `if` with W1401.
- `emela-fmt`: the rewrite.
- `emela-lsp`: the quick fix.
- std, core, tests and fixtures are rewritten with `emela fmt` in the same pull request, so that the repository has no W1401 outside the tests of W1401 and of the rewrite.
- Editors: `editors/tree-sitter-emela/grammar.js` and `queries/highlights.scm`, `editors/emela.sublime-syntax`, `editors/nvim/syntax/emela.vim`, `editors/vscode/syntaxes/emela.tmLanguage.json` and `editors/zed/languages/emela/highlights.scm` (copied by `editors/zed/sync-queries.sh`) highlight `where` as a keyword in the guard.

## Alternatives

- Keep `if`. No reserved word gains syntax and nothing breaks. The guard and the if expression stay alike.
- `when`, as a contextual keyword (`[kw.contextual]`). A pattern is never followed by a lower-case name, so `when` can be a keyword in this position only and stay an identifier elsewhere. It is the most common word for a guard in other languages (Prior art). It reads as a plain condition of a branch rather than as a narrowing of the pattern, and it adds a keyword while `where` uses a word reserved already (Open questions).
- `and`, as a contextual keyword. It reads as a condition, but logical conjunction is written `&&` (`[fn.operator]`), and `Some(x) and x > 0 && ok` would spell it two ways.
- `&&` itself. A pattern never contains `&&`, so there is no conflict, but its left operand would be a pattern, not a `Bool` expression.
- `of`. It is the contextual keyword of label declarations (`effect Audit of Log`, `[kw.contextual]`), and does not read as a condition.
- `whose`. It reads well when the guard is about the names the pattern binds, but a guard need not use them (`Some(_) whose ready`). Choosing it would raise whether to require that a guard use a bound name.
- Two words (`of which`). No keyword of two words exists now.
- Require parentheses around a guard that is an if expression or a match expression (`Some(x) where (if b { ... } else { ... }) -> 1`). Not taken: once the word of the guard is not `if`, an if expression after it is read as the condition of the guard, and the restriction would add a rule for a form that no guard in the repository uses.

## Prior art

- Swift writes the guard of a `case` with `where` (`case let x where x > 0:`). Swift also uses `where` for constraints of generic declarations and for the filter of a `for` loop.
- OCaml, F#, Erlang, Elixir and C# write the guard with `when`.
- Rust and Scala write it with `if`, after the pattern. Kotlin uses `when` for its branching construct itself (`when (x) { ... }`) and writes its guards with `if`.
- Haskell writes guards with `|`; its `where` holds local bindings after a definition.

## Open questions

- `where` or `when`. `where` uses a word reserved already and reads as "such that", which narrows the values the pattern matches. Read as the "such that" of a statement of logic, it also suggests that the condition is about the value matched, and a guard need not be. 9 of the 42 guards in std and core use no name the pattern binds, but 5 of them, in `std/map.emel`, are still about the value matched, named outside the pattern (`Node(..) if right_heavy`, where `right_heavy = too_heavy(right, left)` and `right` is the value matched). 4 of the 42 are about a value other than the one matched: `_ if index < 0` in `core/list.emel` and `[32, ..rest] if !iso` in `std/duration.emel`, about another value, and `None if is_dir(files, path)` in `std/file_system.emel` and `Some(_) if !host_received_text(received)` in `std/http.emel`, about the value the one matched was made from. Each can be written so that the condition is about the value matched: by adding the other value to the value matched (`(xs, index) match { (_, i) where i < 0 -> None ... }`, and `([32, ..rest], False)` with no guard), or by moving the condition into the body of the arm (`None -> if is_dir(files, path) { ... } else { ... }`). This proposal does not require that a guard be about the value matched. `when` reads as a plain condition of a branch, the role `if` already has, and is the spelling most languages use; it would be a contextual keyword, so no identifier is lost, but the language gains one more keyword and `where` stays reserved without syntax. Giving `where` to the guard also means that a later syntax in another position (a block of helpers, constraints of a declaration) would give the word a second meaning; it does not make that syntax impossible.
