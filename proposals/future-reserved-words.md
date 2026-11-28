---
title: More words reserved for the future
authors: [somnicattus]
status: draft
issue:
touches: [kw.future, diag.list]
requires: []
related: []
---

# More words reserved for the future

## Summary

More words are added to the words reserved for the future (`[kw.future]`), so that from 0.30 they can no longer be used as identifiers. No syntax is added. The description of `[kw.future]` states what its words are kept for: syntax in a later version, or a diagnostic that shows how the same thing is written. A word can be kept for the diagnostic alone, when no syntax for it is planned. The words added are `finally`, `break`, `continue`, `for`, `null`, `nil` and `typeof`. A word whose mistaken form cannot be read as anything else, such as `switch x { ... }`, is not reserved; a diagnostic can recognize that form. A word that would belong to a form whose other words are reserved already is added even so.

## Motivation

### Reserving a word after it is used breaks programs

A word reserved after programs have used it as an identifier breaks every program that names a function, a parameter, a field or a local with it. Parameter names are part of the public API (`[fn.args]`: "Parameter names are part of the public API, and renaming one is a breaking change"), so the break reaches the callers of a library, not only its author. There is no edition system that would let a module keep the older set of reserved words (`[open.list]` #12, and `[module.pome]`: "There is no mechanism for switching the meaning of versions per module"). A word reserved now, before any program uses it, can later be given syntax or a diagnostic without breaking anything.

Of the words added in "Proposal", only `for` and `null` are used as identifiers in std, core, the examples, the tests and the editor fixtures of the repository (`upstream/main` at 5ad03e79), and only in tests and fixtures.

```emel
## Compiles today. Fails to compile once `typeof` is reserved.
fn typeof(x: Int) -> Int {
  x
}
```

### Words for syntax that is not planned

`[kw.future]` describes its words only as having no syntax, but it already holds words for syntax that `[intro.premises]` excludes ("Variables are immutable. There is no loop syntax"): `var`, `mut` and `while`. `async`, `try`, `catch` and `throw` are there although no syntax uses them. When `await` was removed, the reasons were that there is no async/await syntax and that std uses `await` as the name of functions (`Fiber.await`, `Deferred.await`; `spec/history.md`); `async` stayed. The words are kept although their syntax is not planned, and the description does not say why. This proposal states the reason in `[kw.future]`, so that words such as `break` and `continue` can be judged by it.

### Diagnostics for the words

A diagnostic at a reserved word can say "there is no syntax for this word; write ... instead". Because no program can use the word as a name, such a diagnostic applies wherever the word is written: in a declaration (`fn break()`) as well as in an expression. A word that stays an identifier can get it only where the name does not resolve: a declaration that uses the word is valid, and the name can become part of the API of a library. A word that is an identifier gets the diagnostics of an identifier: `null` in `x == null` is E2101 (an undefined name), and `finally { ... }` as a statement is E1109 (``expected newline or `}` after statement``) and E2101 (an undefined name).

This is not what happens today. A word of `[kw.future]` written as an identifier is the generic E1101 (`expected lower name` for `fn defer(...)`, and E1102 `expected expression` for `defer(1)` in an expression, as emela 0.29.0 at 5ad03e79 reports them, together with E1109 and E2101 above), with no mention of the word. Reserving a word by itself gives no better diagnostic. This proposal reserves the words and leaves the diagnostics that name them to later proposals ("Diagnostics").

## Proposal

### How the words are chosen

A word is added when either holds:

- it would belong to a form whose other words are reserved already: `finally` with `try` and `catch`, and `for` with `in` (`for x in xs`);
- a mistaken use of it is also a valid use of a name (`x = null`, `break`, `typeof(x)`), so only a reserved word leads every use to a diagnostic.

A word is not added when its mistaken use cannot be read as anything else (`switch x { ... }`, `trait Ord extends Eq`): a diagnostic can recognize the form without reserving the word, as E1403 recognizes `handler H implements E` (`[effect.handler]`). A word that a later syntax would put where no name can stand now is not added either: right after an expression (`x is Some(v)`), right before one (`become f(x)`), or where a type is written (`x: dyn Show`). That syntax can make the word a contextual keyword (`[kw.contextual]`) without breaking programs, as `fails` and `uses` are.

`finally` and `for` would also come where no name can stand (`finally { ... }`, `for x in xs`), and are added for the first reason; `typeof` is added although `typeof x` would be such a form, because `typeof(x)` is also a call of a function.

A word that std or core uses as a name is not added. A common name, such as a usual key of JSON objects, is not added unless its mistaken use is also a valid use of a name (`null`).

The uses below are in `.emel` files of `upstream/main` at 5ad03e79, outside comments and string literals.

### Words added

None of them has planned syntax.

| Word | Reason | Current use |
|---|---|---|
| `finally` | cleanup, which the release of a handler does (`[dyn.with]`); would go with `try` and `catch`, which are reserved | none |
| `break`, `continue` | leaving a repetition, and going on to the next one. `break` alone is also a use of a name. `do` and `while` are already in `[kw.future]` | none |
| `for` | repetition; would go with `in` (`for x in xs`), which is reserved | a local in an editor fixture (below) |
| `null`, `nil` | absence of a value, which `None` writes. `x = null` is also a use of a name | `null`: locals in two tests (below) |
| `typeof` | the type of an expression where a type is written, as in `x: typeof(y)`. `typeof(x)` is also a call of a function | none |

`for`: the old impl head `impl Show for User` is no longer written; it is E1402 (`[trait.impl]`), whose message shows the form `impl User: Show`. To report it, the parser still recognizes `for` in that position; from 0.30 it recognizes the reserved token, and until 0.30 W1001 is not reported at `for` in that position, which E1402 already reports. The only use of `for` as an identifier is in `editors/nvim/test/fixture.emel` (`for = 2`, `fails = for`), with which `editors/nvim/test/check.vim` checks that nvim does not highlight it as a keyword.

`null`: a local that holds the variant `Null` of `Presence`, in `crates/emela-codegen-c/tests/programs/field_presence.emel` and `tests/laws/test/dynamic_presence_test.emel` (`null: Presence[Int] = Null`). `nil` is not used.

### Words not added

The words below are grouped by the reason they are not added. Where a reason covers many words of other languages, a few are given as examples of their kind.

A diagnostic can recognize the mistaken form, shown in parentheses, without reserving the word:

- Words that start other forms of declarations (`class`, `struct`, `interface`): types are written with `type` and `enum`, and interfaces with `trait` (`class User {`).
- Modifiers of declarations (`private`, `override`, `volatile`): there is no inheritance, a declaration is either `pub` or not, every value is immutable, and a function of the host is written with `@external` (`[ext.decl]`). Each modifies a declaration (`override fn show(...)`). Many are common names, and some are usual keys of JSON objects (`private`, `abstract`); the name of a field is its key in a derived Encode and Decode, so a field for such a key would need `@key(...)` (`[trait.derive]`).
- Words of branching and jumping (`switch`, `unless`, `goto`): `match` and `if` write the branching, and there are no labels to jump to (`switch x { ... }`, as E1401 recognizes the prefix form `match x { ... }`).
- Other forms of signatures and of the heads of declarations (`throws`, `extends`): `fails` and `trait Ord: Eq` write them (`fn f() -> A throws E`), as E1403 shows `handler H: E` for `implements`.
- Lower-case names of types (`int`, `char`, `boolean`): the names of types start with an upper-case letter (`x: int`).
- Logical words (`not`, `and`, `xor`): logical operations are written `!`, `&&` and `||` (`[fn.operator]`; `if not c { ... }`). `and` without `or` (below) is unbalanced.

A later syntax can make the word a contextual keyword (`[kw.contextual]`), because it would come where no name can stand now:

- Right after an expression, right before one, or where a type is written (`is`, `become`, `dyn`): `x is Some(v)`, `become f(x)`, `x: dyn Show`. Testing a value against a pattern is written with `match`, a tail call to the function itself is guaranteed not to consume stack without a marker (`[fn.loop]`), and there is no run-time dynamic dispatch (`[trait.not]`).

std or core uses the word as a name:

- `or`: the core function `Option.or` (`crates/emela-core/lib/core/option.emel`).
- `new`: the std functions `Request.new` and `Response.new`.
- `exists`: the operation `exists` of the std effect `FileSystem` (`crates/emela-core/lib/std/file_system.emel`).
- `int`: the std function `Dynamic.int` (`crates/emela-core/lib/std/dynamic.emel`).
- `await`: the operation `await` of `Deferred`. It was removed from `[kw.future]` before (`spec/history.md`).

A word reserved already covers the syntax:

- `gen`: generators would use `yield`, which is in `[kw.future]`. `gen` is a common short name, and the parameter name of generators in a library of the tests (`tests/laws/brix/src/pbt/gen.emel`, `gen: Gen[A]`).

The word is in use already:

- `of`: the contextual keyword of label declarations (`effect Audit of Log`, `[kw.contextual]`). It has syntax, so it does not belong in `[kw.future]`.
- `export`: the attribute `@export(...)` (`[kw.annotation]`). `annotation = "@" lower_name` in `[grammar.decl]`, and `lower_name` excludes reserved words, so `@export` could no longer be written.

### Meaning

The list of `[kw.future]` gains the words decided, and its description states what the words are kept for:

> The following words have no syntax, but they cannot be used as identifiers either. A word is kept here so that a later version can give it syntax, or a diagnostic that shows how the same thing is written, without breaking programs. A word can be kept here although no syntax for it is planned.

The words added are reserved from 0.30, the next minor release. Until 0.30 they can still be used as identifiers, and each such use is reported with the warning W1001, so that a program can be renamed before it stops compiling. `[kw.future]` lists them with this note:

> Until 0.30, `finally`, `break`, `continue`, `for`, `null`, `nil` and `typeof` can be used as identifiers, and each such use is reported with the warning W1001. From 0.30 they are reserved as the other words are.

Besides `[kw.future]` and W1001 in `[diag.list]`, nothing changes: no syntax is added, this proposal changes neither `[kw.reserved]` nor the count of reserved words in `[kw]`, and `lower_name` in `[grammar.lex]` already excludes every reserved word; until 0.30 the note in `[kw.future]` lets the added words through. The loop expression (`loop-expression`, #909) also changes the list of `[kw.future]` (it moves `loop` to `[kw.reserved]`); whichever lands second is rebased onto the other.

## Diagnostics

- [ ] From 0.30, a word added to `[kw.future]` written as an identifier gets the diagnostics that the words already there get (E1101 where a name is expected, E1102 where an expression is expected).
- [ ] W1001 (new): until 0.30, a word added to `[kw.future]` written as an identifier, in any position (a declaration, a parameter, a field, a local, a use), except `for` in the old impl head, which E1402 reports. The written form keeps its meaning. At the word. The help is ``= help: rename it; `null` is reserved from 0.30``. The message is `` `null` will be a reserved word``.
- This proposal adds no diagnostic that names the word and shows how the same thing is written (for `null`, `None`). A later proposal can add one for each word without breaking programs, because the word is reserved.

## Compatibility

A program that uses an added word as an identifier stops compiling in 0.30. No such use is in std, core or the examples. In the tests, `for` is a local in the nvim fixture, and `null` is a local in two files. Uses in other Pomes are not known.

The change breaks programs, so the words are reserved in a minor release (0.30). Before that, patch releases warn at each use of an added word as an identifier (W1001). This follows `docs/release.md`: a change that breaks the current way of writing first gets a warning in a patch release and is switched in the next minor release.

## Implementation

- `emela-syntax` (0.30): add the words to `keyword` in `src/lexer/name.rs` (they become `RESERVED_KW`). The recognition of `impl Show for User` (`at_contextual_kw("for")` in `src/parser/grammar/items.rs`, and `src/rewrite.rs`, which looks for a `LOWER_NAME` `for`) reads the reserved token instead.
- W1001: until 0.30, the lexer keeps the added words as names, and the parser or validation warns at each one. In 0.30 the words move to `keyword` and W1001 is removed.
- Editors: the lists of reserved words in `editors/vscode/syntaxes/emela.tmLanguage.json`, `editors/emela.sublime-syntax` and `editors/nvim/syntax/emela.vim` gain the added words. `editors/nvim/test/fixture.emel` and `editors/nvim/test/check.vim` (which expects no highlight on `for`) change.
- Tests: the locals named `null` in `crates/emela-codegen-c/tests/programs/field_presence.emel` and `tests/laws/test/dynamic_presence_test.emel` are renamed.

## Alternatives

- **Not reserving**: reserve a word only when syntax for it is added. Programs that use it then break, with no edition to soften it.
- **Reserving only words for planned syntax**: `[kw.future]` keeps the words for syntax that may be added, and a word for syntax that is not planned (`break`, `continue`, `null`) is not reserved. Its alternative form is then shown only by a diagnostic where the name does not resolve ("Only a diagnostic that names the word" below). This does not match the words already in `[kw.future]` (`var`, `mut`, `while`), and a library can still take the word as the name of a function or a parameter.
- **Contextual keywords for the words added** (`[kw.contextual]`): no program breaks, but a contextual keyword acts as a keyword only where no name can stand. `break`, `continue`, `null`, `nil` and `typeof(x)` stand where a name can, so a contextual keyword cannot tell them apart from names. `finally` and `for` could be contextual keywords; they are reserved because the other words of their forms are. The words not added that would come where no name can stand are left to this ("Words not added").
- **Only a diagnostic that names the word**: the word stays an identifier, and a name that does not resolve gives "`null` does not exist; write `None`". The word is not kept free, and the diagnostic does not reach a declaration that uses the word as a name ("Diagnostics for the words"). `implements` is handled this way now: it is not reserved, and `handler H implements E` is E1403, which shows the form `handler H: E` (`[effect.handler]`). `extends` can be handled the same way, with a diagnostic for `trait Ord extends Eq` that shows the form `trait Ord: Eq`.

## Prior art

- Words reserved without syntax, kept for later syntax: Rust's reserved keywords (`abstract`, `become`, `box`, `do`, `final`, `gen`, `macro`, `override`, `priv`, `try`, `typeof`, `unsized`, `virtual`, `yield`), and the future reserved words of ECMAScript (`enum`, and `implements`, `interface`, `package`, `private`, `protected` and `public` in strict code).
- Words reserved without syntax so that a mistaken use gets a better diagnostic: Java reserves `const` and `goto`, which no syntax uses, so that a compiler can report them when they appear in a program written in the habits of another language (The Java Language Specification, 3.9).
- Removing reserved words that no syntax used and that programs wanted as names: ECMAScript 5 removed most of the future reserved words of ECMAScript 3 (`abstract`, `final`, `goto`, `int`, `native` and others), as `await` was removed from `[kw.future]` here.
- Adding keywords per unit of compilation, with a version chosen in the unit (editions): Rust editions, chosen per crate; the 2018 edition made `async`, `await` and `dyn` keywords and reserved `try`, and the 2024 edition reserved `gen`. A lint warns at the words in code of the earlier edition, and the migration rewrites them as raw identifiers (`r#try`), a form that writes a reserved word as a name. Emela has neither editions (`[open.list]` #12) nor raw identifiers.
- A warning period before a word becomes reserved: Python made `async` and `await` reserved in 3.7, after 3.5 and 3.6, in which they were keywords only inside `async def` and their use as names elsewhere drew a warning, of pending deprecation in 3.5 and of deprecation in 3.6 (PEP 492).
- Contextual (soft) keywords, which act as keywords only in some positions: Python's soft keywords (`match`, `case`, `type`), C#'s contextual keywords, and Kotlin's soft and modifier keywords; `[kw.contextual]` here.
- Diagnostics that recognize a form of another language without reserving its words: rustc reports `x++` with the help that there is no postfix increment operator. Here, E1401, E1402 and E1403 recognize forms that are no longer written in the same way, and show the form that is (`[diag.bands]`).

## Open questions

## Decision
