---
title: The value of a handler is made only with Self
authors: [somnicattus]
status: draft
issue: 904
touches: [effect.handler, lsp.code-action, diag.list]
requires: []
related: [handler-self-constructor, handler-init-arguments, handler-member, generic-handler]
---

# The value of a handler is made only with Self

## Summary

- Inside the init of a handler, its value is made only with `Self(...)` (`Self` for a handler without parentheses, `Self()` for `handler H()`). The name of a handler no longer stands for its value: it neither makes the value nor matches it in a pattern. The name of a handler is then written only to open it (an item of a with or a layer, the right of `by`) and to refer to its declaration (import, `pub`).
- Until 0.30, writing the name of a handler for its value is deprecated and keeps its meaning: a constructor or a record update in its init (W4001), and a constructor pattern (W4002). From 0.30 they are compile errors (E4014, E4015). A constructor is rewritten by replacing the name with `Self`, a record update by listing the fields in `Self(...)`, and a pattern by reading the fields with `.`.
- For an inner handler, and for a pattern of a handler without parentheses, the implementation already rejects the name, so these are compile errors at once, without a deprecation.

## Motivation

### One spelling, two argument lists

Inside the init of a handler, `H(...)` makes the value of H and takes its fields. As an item of a with, a layer, or on the right of `by`, `H(...)` runs the init of H and takes the parameters of init (`[effect.handler]`).

```emel
handler PostgresUsers(conn: Connection): Users {
  init(url: String) {
    PostgresUsers(conn: pg_connect(url)) # makes the value: takes the fields
  }
}

fn main() {
  with PostgresUsers(url: "...") { () } # runs init: takes the parameters of init
}
```

The two have different meanings and different argument lists, but the same spelling. `handler-self-constructor` (#623) added `Self(...)` for this reason ("The two have different meanings, but they look the same"), but kept `H(...)` and preferred neither, so both spellings stay in use: std writes `SystemTcp(registry: host_open_registry())` in the init of `SystemTcp`, the example `resources` writes `MemoryStore(prefix:)` in the init of `MemoryStore`, and the tests write both.

The form that makes the value inside the init cannot be the form that opens the handler: if `H(...)` in the init of H opened H, the init of the opened H would open H again, without end. So a form that does not run init is needed, and `Self(...)` already is that form. For a handler, `Self` is written nowhere else: in an operation body or a release it is E4005, as a type it is E2108, and it is not a pattern. Keeping one spelling for each meaning leaves:

| Written | Where | Means | Takes |
|---|---|---|---|
| `H(...)`, `H` | an item of a with or a layer, the right of `by` | runs the init of H | the parameters of init |
| `Self(...)`, `Self` | the init of H | makes the value of H | the fields of H |

The head of the declaration (`handler PostgresUsers(conn: Connection)`) then matches `Self(...)`, and `init(url: String)` matches the item `PostgresUsers(url: ...)`, each in one position.

### A pattern with the name of a handler only reads fields

A constructor pattern can name a handler (`self match { Counter(n:) -> n }`, `Counter(n:) = self`). It does not make a value (`[effect.handler]`), but it is the one place besides the constructor where the name of a handler stands for its value, and it adds nothing:

- A handler has one constructor, so the pattern always matches. It only binds the fields.
- The fields are read with `.` wherever the pattern can match: `self.n` in an operation body and in release, `a.n` for a value `a` made in the init.
- A value of a handler's type is found only inside the handler. Outside, the capability of an item has the type `use E` (`c match { Counter(n:) -> n }` for `with c = Counter` is E3001), and the type of a handler cannot be written (E2106).

A pattern of a handler without parentheses (`self match { Plain -> 0 }`) is already rejected by the implementation (E2106, "expected constructor, found handler `Plain`").

### Inner handlers already accept only Self

An inner handler that implements a handler member has the name of the member (`[effect.handler]`), and the init of the inner handler often opens the member of the same name on the outer effect:

```emel
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) { Self(inner: Files.Open(path)) }

    fn read() { self.inner.read() }
  }
}
```

The specification lets the inner handler write `Open(inner: Files.Open(path))` there, which puts `Open(...)` that makes the value next to `Files.Open(...)` that opens a handler. The implementation rejects the name of an inner handler as a constructor, in an expression and in a pattern (E2101, "cannot find constructor `Open` in this scope"), so no program written that way compiles now.

## Proposal

### Syntax

No change to the grammar. `Self` as an expression stays as `[grammar.expr]` has it.

```emel
handler PostgresUsers(conn: Connection): Users {
  init(url: String) { Self(conn: pg_connect(url)) }

  release { pg_close(self.conn) }
}

handler ConsoleLogger: Logger {
  init { Self }

  fn info(msg) { console_log(msg) }
}
```

### Meaning

- The value of a handler is made with `Self(fields)` inside the init of that handler, including the anonymous functions inside it. For a handler without parentheses it is written `Self`, and for `handler H()` it is written `Self()`. Its meaning is unchanged: it does not run init, and the value becomes the capability only when init returns it (`[effect.handler]`).
- The name of a handler does not stand for its value. The following are uses of the name for the value:
  - a constructor: `PostgresUsers(conn: ...)`, `ConsoleLogger`, `H()`
  - a record update: `Counter(n: 2, ..a)`
  - a constructor pattern, wherever a pattern is written: `Counter(n:)`, `Counter(..)`, `H()`
- Outside the init of the handler, a constructor or a record update with the name stays E4005, as now.
- For a handler that is not an inner handler, until 0.30 a constructor or a record update with its name inside its init (W4001) and a constructor pattern with its name (W4002) are deprecated and keep their current meaning. From 0.30 they are compile errors (E4014, E4015).
- For an inner handler, a constructor, a record update or a constructor pattern with its name is a compile error (E4014, E4015) from the version that adopts this proposal. So is a constructor pattern that names a handler without parentheses (`Plain`, E4015).
- No form is added in place of the record update and the pattern. `Self(...)` takes no `..` (E2111, as now), and `Self` is not a pattern. A record update is rewritten by listing the fields (`Self(n: 2, m: a.m)`), and a pattern by reading the fields (`self.n`).
- The declaration of a handler without fields keeps both forms, `handler H` and `handler H()`, and the value is `Self` or `Self()` as now.

### Typing rules

- Unchanged. `Self(fields)` has the type of the handler, its fields are checked as for the constructor of a record, and for a handler with type parameters its type arguments are determined from the fields (`Self(value: 1)` in the init of `Memory[S]` has the type `Memory[Int]`, a mismatch, E3001).

## Diagnostics

- [ ] E4014 (new): the name of a handler written for its value as a constructor or a record update inside its own init. From 0.30; for an inner handler, from the adoption. At the name. Message ``the value of `PostgresUsers` is made with `Self` ``, with ``= help: write `Self` in place of `PostgresUsers` ``, or for a record update ``= help: write `Self` with every field, as in `Self(n: 2, m: a.m)` ``, listing the fields of the update.
- [ ] E4015 (new): a constructor pattern that names a handler. From 0.30; for an inner handler or a handler without parentheses, from the adoption. At the name. Message ``handler `Counter` cannot be matched by its name``, with ``= help: read the fields with `.`, as in `self.n` ``.
- [ ] W4001 (new): until 0.30, what E4014 reports from 0.30. Message ``constructing `PostgresUsers` by its name is deprecated``, with the help of E4014.
- [ ] W4002 (new): until 0.30, what E4015 reports from 0.30. Message ``matching `Counter` by its name is deprecated``, with the help of E4015.
- [ ] The language server offers a quick fix for W4001 and E4014 (`[lsp.code-action]`): the name becomes `Self`, and for a record update, `..x` becomes the fields not written, read from x. It is not offered for a record update whose expression after `..` is not a name, since the rewrite would evaluate it more than once. No fix is offered for W4002 and E4015, whose rewrite changes the body that uses the bindings. That the two kinds have different fixes is why they have different codes (`[diag]`).
- E4005 is unchanged: it stays the code for a handler value made outside the init of the handler.
- [ ] E4006 (changed help): ``= help: add `init { ... }` that returns `Self(...)` `` in place of `NoInit(...)`.

## Compatibility

- A program that uses the name of a handler for its value compiles with W4001 or W4002 until 0.30, and stops compiling in 0.30.
  - A constructor is rewritten by replacing the name with `Self`. The types, including the inferred type arguments, and the behavior do not change.
  - A record update is rewritten by listing the fields. The value is the same.
  - A pattern is rewritten by reading the fields with `.`. A pattern of a handler always matches, so the behavior is the same.
- To rewrite in this repository: `SystemTcp` in `crates/emela-core/lib/std/tcp.emel`, `MemoryStore` in `examples/resources`, `editors/nvim/test/fixture.emel`, and the tests (about 54 constructors in `.emel` files under `crates/`, programs inside Rust tests, and the patterns the implementation finds). This pull request rewrites the example of `[effect.handler]`.
- The name of an inner handler as a constructor or a pattern, and the pattern of a handler without parentheses, do not compile now (E2101, E2106), so making them E4014 and E4015 breaks no program.
- `docs/release.md`: 0.29.0 is out without the warnings. W4001 and W4002 land in a patch release of 0.29, and the change to E4014 and E4015 lands in 0.30 as `feat!:`.

## Implementation

- `emela-resolve` / `emela-types`: where the name of a handler resolves to its constructor (an expression, a record update, a pattern), report W4001 or W4002 and keep the current meaning; in 0.30, report E4014 or E4015. For an inner handler and for a pattern of a handler without parentheses, report E4014 or E4015 instead of E2101 and E2106.
- `emela-driver` (quick fixes): the fix for W4001 and E4014.
- The help of E4006.
- std, examples, the nvim fixture and the tests: rewrite.

## Alternatives

- Keep both spellings (`handler-self-constructor` as it is). The two meanings of `H(...)` stay.
- Only a lint, never an error. The spelling `H(...)` keeps two meanings in the language; a lint does not decide which form a reader meets.
- Remove `Self(...)` and keep `H(...)`. The problem of the two meanings remains, and inner handlers would need their own name to be accepted.
- Keep the constructor patterns with the name. They are the one place left where the name of a handler stands for its value, and they only read fields, which `.` already does.
- A pattern `Self(...)`, or `..` after `Self(...)` in the init. Each adds a form whose only use is to read or copy fields, which `.` and listing the fields already do. A record update of a handler value can only appear in its init, from a value made earlier in the same init.
- One code for every use of the name. The constructor and the record update have a quick fix and the pattern has none, and `[diag]` gives such mistakes different codes.
- Make the name an error without a deprecation. The rewrite is mechanical, but std, examples and user programs use the name, and `capability-identity` (#732) set the precedent of a deprecation until the next minor version (W4401).

## Prior art

- A constructor of the current type named by a keyword instead of the type's name, inside the definition of the type.

## Open questions

## Decision
