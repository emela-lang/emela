---
title: Rename the effect clause to uses
authors: [Alliana]
status: implemented
pr: 488
issue:
decided: 2026-11-06
touches: [fmt.io, fmt.space, lex.continuation, kw.contextual, glossary.terms, grammar.decl, grammar.type, type.fn, type.inference, type.rules, type.effect-bound, effect.decl, effect.use, effect.with, effect.handler, effect.async, effect.state, effect.label, error.fails, fn.decl, fn.operator, data.tuple, intro.layers, module.pub-signature, module.resolution, module.pome, trait.effects, trait.not, fiber.scope, fiber.combinators, fiber.interrupt, fiber.cause, fiber.deferred, ext.decl, ext.call, ext.boundary, ext.c-host, ext.export, lib.layers, api.prelude, api.std.state, api.std.fiber, api.std.deferred, api.std.http, target.js, target.c-embedded.entry, fmt.signature, lsp.hover, lsp.completion, lsp.code-action, lsp.inlay-hint, open.list, diag.list]
requires: []
related: []
---

# Rename the effect clause to uses

## Summary

The clause that writes the effect set R of a function signature or a function type is renamed from `use` to `uses` (`fn f() -> A fails E uses { R }`, `fn(A) -> B fails {} uses {}`). `uses` is a contextual keyword, as `fails` is, so no reserved word is added. The expression `use X`, the display of a capability type `use Logger`, and `use` inside init stay as they are. The old clause is a compile error that shows the rewrite (E0147). Because a line starting with `uses` can no longer be mistaken for a `use` expression, rule 4 of `[lex.continuation]` is decided by tokens alone and no longer only inside signatures. The canonical form of a clause always has braces (`uses { Db }`), and `emela fmt` rewrites both the old clause and the form without braces.

## Motivation

`use` has two jobs today.

- As an expression, `use X` returns the capability value of one effect (`[effect.use]`).
- As a clause, `use R` declares the effect set of a function (`[type.fn]`, `[type.inference]`).

Errors already separate the two jobs: `fail e` is the expression, `fails E` is the clause. Effects use one word for both, and the two meet on adjacent lines. From pkgs-api (`src/service/pomes.emel`):

```emel
pub fn search(query: Query, limit: Option[Limit]) -> List[PomeSummary]
  fails IndexUnavailable
  use PomeIndex {
  (use PomeIndex).search(query, limit.or(Pome.default_limit()))
}
```

The third line declares a set; the fourth takes out a capability. They look the same and mean different things.

The clause also does not read as a statement about the function. `fails {}` says "this function fails with nothing"; `use {}` after it reads as an instruction ("use nothing"). `fn(A) -> B fails {} use {}` mixes the two moods in one type.

The overlap also constrains the lexical grammar. Rule 4 of `[lex.continuation]` continues a line starting with `fails` or `use` only inside a signature. `spec/history.md` (2026-10-13, 2.5) gives the reason: adding it to rule 3 "would be ambiguous with `fails` (an identifier) and `use` (an expression) inside bodies". The `use` half of that ambiguity cannot be resolved by looking further: `use Logger` at the start of a body line is a valid statement, with exactly the shape of a clause (the keyword, then a type name). So the lexer tracks whether it is inside a signature, and has a separate exception for declarations without a body.

### Why the clause became `use`

Until 0.10, the clause was `uses` (`fn map<T, U, e>(list: List<T>, f: (T) -> U uses e) -> List<U> uses e`, spec 0022 and 0023 in `CHANGELOG.md`), and the error clause was `throws`. 0.1x had no `use` expression. The 0.20 reboot (#114) renamed `throws` to `fails`, added the expression `use X`, and wrote the clause as `use`.

No reason for writing the clause as `use` is recorded. `git log` (including the squashed #114 and its NOTES files), `spec/history.md`, `CHANGELOG.md`, the 0.20 draft specification (`docs/spec/emela-0.20.md`) and the author's decision notes say nothing about it. The reasons that could have been given are answered under "Alternatives".

## Proposal

### Syntax

```emel
pub fn search(query: Query, limit: Option[Limit]) -> List[PomeSummary]
  fails IndexUnavailable
  uses { PomeIndex } {
  (use PomeIndex).search(query, limit.or(Pome.default_limit()))
}

fn apply(f: fn(Int) -> Int fails {} uses {}, x: Int) -> Int {
  f(x)
}
```

- The clause after the fails clause of a fn declaration, a trait function, and a function type is `uses effect_set`. The grammar of the effect set does not change: `uses Db` and `uses R` still compile. Its canonical form always has braces (`uses { Db }`, `uses { Db, Log }`, `uses {}`, `uses { R }`), see "Canonical form".
- `uses` is a contextual keyword. It acts as a keyword only in the position of the clause. Elsewhere it is an ordinary identifier (`uses = 2` is a binding, as `fails = 1` is).
- These keep `use`, because they are about one capability, not a set:
  - the expression `use X` and `(use X).op(...)` (`[effect.use]`), including inside init (`[effect.handler]`)
  - the display of the type of a capability value, `log: use Logger` (`[type.inference]`, `[lsp.hover]`), and the possible future capability type `from: use Db` (`[effect.use]`)
- Set type arguments of named types do not change (`Handler[E, { Db }]`, `Scope[E, R]`). They have no keyword.

### Meaning

None changes. A `uses` clause means what a `use` clause means today.

### Line continuation

Rule 4 of `[lex.continuation]` becomes: a line continues the previous one when it starts with `fails` or `uses` and the next token on that line is a type name, an uppercase name, or `{`.

- A clause always has this shape, because an error set or an effect set follows the keyword.
- No statement has this shape. As identifiers, `fails` and `uses` are followed by an operator, `=`, `:`, `(`, `.`, `,`, a closing bracket, `escape`, `match`, `->`, `if`, or a line break; never by a name or `{`.
- So the rule needs neither the signature tracking nor the exception for declarations without a body. A line starting with `fails` or `uses` after an effect operation or an `@external` fn continues it, which is what the line means.
- It also lets a function type in the type annotation of a binding or a const put its clauses on the next line. The formatter still breaks only signatures (`[fmt.signature]`), so this is never its output.

### The old form

`use` in the position of the clause is a compile error (E0147). The diagnostic shows the rewrite in the canonical form (``write `uses { PomeIndex }` instead of `use PomeIndex` ``) and points at the `use` token. The clause is read as a `uses` clause and checking goes on, so no other syntax error is added. This includes the old clause on a line of its own after a signature, which rule 4 no longer continues. There is no migration period, as for the postfix match (2026-10-13) and the heads of impl and handler (2026-10-19, E0144 and E0145). `emela fmt` rewrites the clause (E0147 is not a syntax error for the formatter), because in this position the rewrite needs no judgment and every repository that writes a clause needs it.

### Canonical form

A uses clause is written with braces, however many items it has.

- `emela fmt` adds the braces to a clause that has a bare name: `uses Db` becomes `uses { Db }`, and `uses R` becomes `uses { R }`. The formatter does not resolve names, so an effect and a set type parameter get the same treatment. `{ R }` already means the same set as `R` (`[grammar.type]`).
- Diagnostics, hover, signature help, and inlay hints display the clause in this form.
- The form without braces stays valid, so a file that has not been formatted yet still compiles.
- Set type arguments of named types are out of scope (`Handler[E, Db]`, `Scope[E, R]`). In that position the formatter cannot tell a set from an ordinary type argument.

## Specification changes

### `[kw.contextual]`

Add a row:

| Word | Position |
|---|---|
| `uses` | Function signatures and function types |

### `[glossary.terms]`

In "Used as they are", add `uses` after `use`.

### `[lex.continuation]`

Replace rule 4 with:

4. The next line starts with `fails` or `uses`, and the next token on that line is a type name, an uppercase name, or `{`. The fails clause and the uses clause always have this form, because a set follows the keyword. No statement has it: as ordinary identifiers, `fails` and `uses` are never followed by a name or `{`. The rule therefore applies anywhere, including after the signature of a declaration without a body (an effect operation, an `@external` fn) and in the type annotation of a binding. The body's `{` is written on the last line of the signature.

Change the example to:

```emel
pub fn get(url: String) -> Response
  fails InvalidUrl | ConnectionFailed | ResponseNotText
  uses { Http } {
  ...
}
```

### `[grammar.decl]`

```ebnf
signature    = [ "->" type [ "fails" error_set ] ] [ "uses" effect_set ]
```

Add to the restrictions outside the grammar:

- `fails` and `uses` are contextual keywords (3.2). Writing `use` in the place of `uses` is a compile error (E0147), and the diagnostic shows the rewrite.

### `[grammar.type]`

```ebnf
           | "fn" "(" [ type { "," type } ] ")" "->" type
             [ "fails" error_set ] [ "uses" effect_set ]
```

### `[type.fn]`

Replace `use` with `uses` in the clause throughout: the form `fn(A, B) -> C fails E uses { R }`, the rows `fn(A) -> B uses {}`, `fn(A) -> B fails {} uses {}`, `fn(A) -> B uses { R }`, the example `type Handler[E, R](run: fn(Request) -> Response fails E uses { R })`, "inferred from the position where it is used (`fails E`, `uses { R }`)", "written in a uses clause (`uses { Scope[E, R] }`)", and "The notation matches the fails clause (`A | B`) and the uses clause (`{ A, B }`)".

### `[type.inference]`

- In the table, "use clause" becomes "uses clause".
- "a fn without a use clause" becomes "a fn without a uses clause" (both places).
- "so `fails {}` and `use {}` are written" becomes "so `fails {}` and `uses {}` are written".
- "The `use` clause is written after the `fails` clause" becomes "The `uses` clause is written after the `fails` clause".
- "The type of a capability value is displayed as `use Logger`" stays. Add: "`use` here names one capability value, and `uses` names a set; the two are not interchangeable."

### `[effect.decl]`

- "An operation cannot have a `use` clause (op_sig in 17.3 ...)" becomes "An operation cannot have a `uses` clause ...".
- "Async can be written in a use clause" (`[effect.async]`) and the other mentions of the clause become "uses clause".

### `[effect.use]`

The example becomes:

```emel
fn copy(read: fn() -> Rows uses { Db }, write: fn(Rows) -> () uses { Db }) {
  write(read())
}
```

The text about `use X` and the capability type `from: use Db` does not change.

### `[diag.list]`

Add:

| Code | Situation | Message |
|---|---|---|
| E0147 | `use` written in the place of the uses clause (a signature, a function type, an effect operation) (9.2) | ``write `uses { Http }` instead of `use Http` `` |

Change:

- E0136: "A return type, fails, or uses on an fn in an impl".
- E0137: "A `uses` clause on an effect operation (8.1)", message ``effect operations cannot have a `uses` clause``.
- E0402: "uses clause" in the situation, the help ``= help: add it to `uses` or provide it with `with` `` (and ``add it to `uses` or provide it with `with` outside `Fiber.scope` ``), and the messages ``effect `Logger` is not declared in `uses` `` and ``effects `A`, `B` are not declared in `uses` ``.
- E0403: ``effect set mismatch: expected `uses {}`, found `uses { Logger }` ``.
- E0407: "An `@external` fn with uses omitted", help ``= help: declare it with `uses {}` if it has no side effects``.
- E0423: "(`use State`, a uses clause, the head of a handler)". `use State` is the expression and stays.

### `[fmt.io]`

Change the rules on syntax errors and on the check of the result to:

- A file with a syntax error is not formatted. E0147 (`use` in the place of the uses clause) and E0148 (`fn(...)` in an expression) are not syntax errors here: the formatter rewrites them (`[fmt.signature]`, `[fmt.closure]`).
- The formatter re-reads the formatted result and confirms that it has no syntax error and that its token sequence and order of comments match the input, apart from these rewrites.

### `[fmt.space]`

The example of the braces of an effect set becomes `uses { Io, Clock }`.

### `[fmt.signature]`

- Add: "A uses clause always has braces. The formatter adds them around a bare name (`uses Db` becomes `uses { Db }`, `uses R` becomes `uses { R }`), and rewrites the old clause (E0147: `use Db` becomes `uses { Db }`). Set type arguments of named types are not changed."

- "The `fails` and `uses` clauses (rule 4 of `[lex.continuation]`) form a single group".
- "the set of `uses`", "a comment inside `uses { ... }`".
- The example uses `uses { Http } {`.

### `[lsp.hover]`

- "a `uses { R }` line also follows the `fails E` line" (expressions, handlers, layers).
- "a name that refers to a label (`uses { AuditLog }`, ...)".
- "The type of a capability value is shown as `use Logger`" stays.

### `[lsp.completion]`

- "Right after `use` and `uses`: effects only."
- Signature help has the form `name(label: type, …) -> return fails E uses { R }`.

### `[lsp.code-action]`

- "If the `uses` clause is omitted, `uses { R }` is added".
- E0402: "no edit that propagates uses is offered".
- Add to the fixes: E0147 (the old clause): replaces the clause with its canonical form (`use Db` becomes `uses { Db }`).

### `[lsp.inlay-hint]`

- `-> T fails E uses { R }`, "If only the uses clause is omitted, `uses { R }`", and "The E and R (`fails E uses { R }`)".

### `[open.list]`

In #28 and #31, `use { R1, R2 }` and `fails X use X` become `uses { R1, R2 }` and `fails X uses X`.

### The other sections in `touches`

In code examples, API signatures and prose, the clause `use` becomes `uses` in its canonical form (`use R` becomes `uses { R }`), and "use clause" becomes "uses clause". The expression `use X` (including `(use Scope).fork(f)` in `[api.std.fiber]` and `config = use Config` in `[effect.handler]`) and the capability type `use Logger` do not change. The sections are `[type.rules]` (`fails Ee uses { Re }`), `[type.effect-bound]`, `[effect.with]`, `[effect.handler]` (the type of resume, `fn(T) -> () fails {} uses {}`), `[effect.async]`, `[effect.state]`, `[effect.label]`, `[error.fails]`, `[fn.decl]`, `[fn.operator]`, `[data.tuple]`, `[intro.layers]`, `[module.pub-signature]`, `[module.resolution]`, `[module.pome]`, `[trait.effects]`, `[trait.not]`, `[fiber.scope]`, `[fiber.combinators]`, `[fiber.interrupt]`, `[fiber.cause]`, `[fiber.deferred]`, `[ext.decl]`, `[ext.call]`, `[ext.boundary]`, `[ext.c-host]`, `[ext.export]`, `[lib.layers]`, `[api.prelude]`, `[api.std.state]`, `[api.std.fiber]`, `[api.std.deferred]`, `[api.std.http]`, `[target.js]` and `[target.c-embedded.entry]`.

## Diagnostics

- [x] E0147 for `use` in the place of the uses clause, with the rewrite; also for the old clause on a line of its own after a signature
- [x] E0147 quickfix in the LSP, and the rewrite in `emela fmt`
- [x] E0136, E0137, E0402, E0403, E0407, E0423: wording changed to `uses`, and sets displayed in the canonical form

## Compatibility

- Every program that writes an effect clause stops compiling (E0147). Programs that omit the clause, which is most code at the application level (R is inferred, `[type.inference]`), do not change.
- `emela fmt` makes the rewrite: the old clause and the form without braces both end in the canonical form. Clauses with a bare name gain two tokens, and a signature can end in two braces (`uses { Db } {`).
- `uses` as an identifier keeps working. It is a contextual keyword.
- Line continuation reads every correct program as before. Lines that start with `fails` or `uses` followed by a name or `{` continue in more places, but no correct program had such a line outside a signature.
- In this repository the clause appears in about 200 lines of `.emel` (core, std, tests, examples) and in about 100 places in `spec/`. Outside it: quince, pkgs-api, stdlib and the documentation of emela.dev.
- No migration period, following the author's policy for the postfix match (`spec/history.md`, 2026-10-13). The implementation pull request rewrites the sources in the repository once, with `emela fmt`.

## Implementation

- emela-syntax: add `uses` to the contextual words (`lexer/name.rs`). Rewrite rule 4 in `lexer/continuation.rs` as a check of the token after `fails` / `uses`, and drop the signature tracking. In `parser/grammar/types.rs`, read `uses effect_set` as the clause (rename `USE_CLAUSE` to `USES_CLAUSE`), and read `USE_KW` in that position as the clause with E0147. For the old clause on its own line, the parser looks past the NEWLINE that ends a signature for `use` followed by a name or `{`. A new `DiagnosticCode::OldUseClause`.
- emela-fmt: rewrites `use` in the position of the clause to `uses`, adds the braces around a bare name, and accepts both rewrites in the check of the result (`[fmt.io]`).
- emela-types, emela-driver: the display of types and the messages of E0402 and the others; hover, signature help, inlay hints, code actions (`actions/quickfix.rs` writes `"use"`), completion after `uses` (`complete.rs`), the contextual keywords in `doc/html.rs`. The type representation already calls the field `uses` (`ty.uses`).
- Editors (`docs/syntax-highlight.md` lists them): `uses` joins `fails` in the contextual keyword patterns of `editors/emela.sublime-syntax`, `editors/vscode/syntaxes/emela.tmLanguage.json` and `editors/nvim/syntax/emela.vim` (which already highlights `fails` only before an uppercase name or `{`, the same test as the new rule 4). In `editors/tree-sitter-emela`, `use_clause` becomes `uses_clause`, and the external scanner for `_signature_continues` follows the new rule 4. `use` in expressions stays a reserved keyword, so editors can give the expression and the clause different scopes. `docs/syntax-highlight.md` itself does not change.
- The bundled `.emel` sources, tests, snapshots, examples and README: rewritten mechanically. `use` followed by a set in the position after a return type or a fails clause is the clause; `use` in an expression is not touched.

No prototype yet.

## Alternatives

- Keep `use` for both. The reasons that could be given, and why they do not hold:
  - "One word for one concept, the effect." The two positions are different concepts: the expression gives the value of one capability, the clause declares a set. Errors already use two words for the same split (`fail`, `fails`). The LSP shows both side by side (`log: use Logger` for a capability value, `uses { Logger }` for a set), and one word makes them hard to tell apart.
  - "No new contextual keyword." `uses` works exactly as `fails` does: one more row in `[kw.contextual]`, no reserved word, and identifiers named `uses` keep working.
  - "Shorter." One character.
- Reserve `uses`. Rejected: it would break identifiers named `uses` for no gain, since the clause position is already unambiguous.
- Rename only, keep rule 4 limited to signatures, and change rule 4 in a later proposal. Rejected: the rename is what makes the token rule possible (it was impossible only because of the `use` expression), and doing both at once changes the lexer and the editor definitions (the tree-sitter scanner among them) once instead of twice.
- A different word (`with`, `needs`, `effects`, `requires`). `with` is the expression that provides effects. `needs` and `requires` say the same as `uses` with no pair to `fails`. `effects` is a noun and breaks the pattern of `fails`.
- Only the E0147 quickfix in the LSP, with no rewrite in the formatter. Rejected: the clause has to be rewritten in every repository, and a position that needs no judgment should not need an editor.
- The canonical form of a one-element set without braces (`uses Db`). Rejected: a set of two items or none has braces in any case, so the bare form is the only one that differs, and it looks like the capability `use Db`. Braces make one shape for every clause.
- Decide the canonical form in a later proposal. Rejected: both `use { Sql }` and `use PomeIndex` appear in pkgs-api, and deciding it here means every clause is touched once.
- Make the form without braces an error. Rejected: the two forms mean the same, so a diagnostic would add nothing that the formatter does not already do.
- Handle in the same proposal the asymmetry that an omitted `uses` is inferred while an omitted `fails` with a return type means "no errors" (`[type.inference]`). Separate: it changes typing and the requirements of `[module.pub-signature]`, while this proposal changes no meaning.

## Prior art

- A verb in the third person singular for a clause that states a property of a function: `throws` in Java's method signatures, and `uses` / `throws` in Emela 0.1x.
- Effect rows written next to the result type of a function type: Koka (`-> <console> int`) and Unison abilities (`{IO} Text`), with no keyword.
- Separate syntax for performing an effect and for declaring it: algebraic effect systems in general, where the operation is a term and the row is part of the type.

## Open questions

## Decision

`use` did two jobs: the expression `use X` gives the value of one capability, and the clause declares the set of effects of a function. Errors already separate the same two jobs with `fail` and `fails`, so the clause becomes `uses`, a contextual keyword like `fails`, and no reserved word is added. The rename also removes the reason rule 4 of line continuation had to track signatures, because a line that starts with `uses` followed by a name or `{` can no longer be a statement; the two changes go together so that the lexer and the editor definitions change once. The canonical form of a clause always has braces (`uses { Db }`), because sets of two items or none have them in any case and the bare form looks like the capability `use Db`. It is decided here so that every clause is rewritten once. `emela fmt` rewrites the old clause and adds the braces, because the position needs no judgment and every repository that writes a clause needs the rewrite. The form without braces stays valid, since a diagnostic would add nothing the formatter does not already do. Set type arguments stay out of scope, because the formatter cannot tell a set from an ordinary type argument there.
