---
title: Self(...) in the init of a handler
authors: [Alliana]
status: implemented
pr: 623
issue:
decided: 2026-11-18
touches: [effect.handler, grammar.expr, diag.list]
requires: []
related: [handler-init-arguments]
---

# Self(...) in the init of a handler

## Summary

Inside the init of a handler, its value can be constructed as `Self(...)`. It means the same as writing the handler's name, so the name is not repeated.

## Motivation

The expression that makes the value of a handler at the end of its init has the same form as the item of a with that calls the init (`[effect.handler]`).

```emel
handler PostgresUsers(conn: Connection): Users {
  init(url: String) {
    PostgresUsers(conn: pg_connect(url)) # the value of this handler
  }
}

with PostgresUsers(url: "...") { ... } # a call of init
```

- The two have different meanings, but they look the same.
- Renaming a handler means rewriting its init too.

The draft `handler-member` makes this sharper. There, an inner handler and a member of the outer effect have the same name, and both appear in the same init.

```emel
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) {
      Open(inner: Files.Open(path)) # Open(...) makes this value; Files.Open(...) opens the outer member
    }
  }
}
```

## Proposal

### Syntax

```emel
handler PostgresUsers(conn: Connection): Users {
  init(url: String) {
    Self(conn: pg_connect(url))
  }
}

handler Counter: Tally {
  init {
    Self
  }
}
```

### Meaning

- `Self(fields)` inside the init of a handler makes the value of that handler. It means the same as `H(fields)`.
- For a handler without parentheses it is written `Self`, and for `handler H()` it is written `Self()`, as `ConsoleLogger` and `H()` are written now.
- It can be written only where `H(fields)` can: inside the init, including anonymous functions inside it. Anywhere else inside the handler it is E0418, and outside a handler, trait or impl it is E0219.
- `H(fields)` can still be written. Neither form is preferred: `emela fmt` does not rewrite one into the other, and no lint suggests one.
- `Self` is not a constructor pattern, and is not allowed in a record update (`Self(n: 1, ..self)`). A record update cannot be written in an init, where `self` does not exist yet, nor in an operation body (E0418), so there is no place for it. Patterns use the name of the handler.
- `Self(...)` does not construct types in traits and impls. This proposal covers only the init of a handler.

### Typing rules

- The type of `Self(fields)` is the type of the handler. Its fields are checked as for `H(fields)`.

## Specification changes

### `[effect.handler]`

Add after the item on E0418:

> - Inside the init of a handler, its value can also be constructed as `Self(...)` (`Self` for a handler without parentheses, `Self()` for `handler H()`). It has the same meaning as writing the handler's name, and the same restriction (E0418). `Self` is not a constructor pattern, and does not construct types in traits and impls.

### `[grammar.expr]`

```ebnf
primary    = literal | lower_name | upper_name | type_ref | "self" | "Self"
           | ...
```

Add to the restrictions:

> - `Self` as an expression is the value of a handler, and can be written only inside the init of a handler, called with its fields or alone (8.4).

### `[diag.list]`

Replace the description of E0418 with:

> | E0418 | A handler value is constructed outside the init of that handler, by its name or by `Self` (8.4). The help is ``= help: use `with Counter { ... }`; `init` and `release` run there`` | ``handler `Counter` can only be constructed in its own `init` `` |

## Diagnostics

- [x] `Self(...)` inside a handler, outside its init: E0418
- [x] `Self(...)` outside a handler, trait or impl: E0219

## Compatibility

The change only adds. `Self` can be written only in the position of a type now, so `Self` as an expression is a syntax error now.

## Implementation

- emela-syntax: `Self` as an expression
- emela-resolve: resolve `Self` inside an init to the constructor of that handler
- emela-types: the same checks as `H(fields)`
- emela-fmt: print it as written

## Alternatives

- Not adding it: the init call of a with item and the construction in an init keep looking the same, and in the draft `handler-member` the construction and the opening of a member look alike.
- Lower-case `self(...)`: `self` is a value, and cannot be used in an init (the value does not exist yet). Using it to construct would be confused with `self.conn`.
- A dedicated syntax for construction (`return { conn: ... }`): it departs from `H(fields)` and from the construction of records.
- Allowing `Self` in patterns and record updates: there is no place where a record update of a handler can be written, and the name of the handler is enough for patterns.
- Extending `Self(...)` to traits and impls: a separate question about types, not needed for handlers.

## Prior art

- Referring to the enclosing type by a fixed name inside its own definition, so that a constructor call does not repeat the name of the type.

## Open questions

## Decision

The expression that makes the value of a handler at the end of its init has the same form as the with item that calls the init, though the two mean different things, and renaming a handler means rewriting its init too. `Self(...)` names the value being made without repeating the handler's name. It is limited to where `H(...)` can already be written, so E0418 and the guarantee that init and release run are unchanged. Patterns, record updates, and traits and impls are left out, because there is no place for them in a handler or they are a separate question about types. Neither form is preferred by the formatter or a lint.
