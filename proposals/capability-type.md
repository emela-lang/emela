---
title: The type of a capability, use E
authors: [Alliana]
status: draft
pr: 622
issue: 616
decided:
touches: [effect.use, effect.escape, effect.handler, fiber.escape, type.rules, grammar.type, diag.list]
requires: [capture-checking]
related: []
---

# The type of a capability, use E

## Summary

The type of a capability is written `use E`, and is an ordinary type. A capability value can be passed as an argument, put into a data structure and returned from a function (E0408, E0409 and E0410 are retired). The one rule that remains is the rule for closures that capture a capability: it does not leave the with that provides it. Capture checking (`capture-checking`) checks it statically.

## Motivation

A capability and a closure that captures a capability are treated differently (`[effect.use]`, `[effect.escape]`).

| | A closure that captures a capability | A capability |
|---|---|---|
| Passing it as an argument | Allowed | E0408 |
| Putting it into a data structure | Allowed | E0409 |
| Returning it from a function | Allowed | E0410 |
| Taking it out of its with | Not allowed (checked at run time now) | Not allowed |

A closure is made just by wrapping, `|| cap.read(10)`, so whatever the left column allows can be done for the right column by wrapping. E0408 to E0410 on capabilities add no safety, while the wrapping and its cost to reading remain. The example of `[effect.use]` passes two closures to tell two Dbs apart.

```emel
# now
fn copy(read: fn() -> Rows uses { Db }, write: fn(Rows) -> () uses { Db }) {
  write(read())
}

copy(|| replica.query("..."), |rows| primary.execute(rows))

# with this proposal
fn copy(from: use Db, to: use Db) {
  to.execute(from.query("..."))
}

copy(replica, primary)
```

`[effect.use]` says "The type of a capability value cannot be written in source", and the restrictions rest on that.

With this type, the resources of handler members (`handler-member`, an open file) can be passed to functions, and a handler can hold the capability of another handler (a provider that wraps a returned handler, a point left open in #616).

## Proposal

### Syntax

```emel
fn copy(src: use ReadFile, dst: use WriteFile) -> () fails FileSystemFailure { ... }

fn pick(primary: use Db, replica: use Db, write: Bool) -> use Db {
  if write { primary } else { replica }
}

handler PostgresUsers(db: use Db): Users {
  init(db: use Db) { PostgresUsers(db:) }
  fn find(id) { self.db.query("...") }
}
```

- `use E` is a type. E is an effect (with its type arguments) or a label. It can be written wherever a type can: parameters, return types, fields, type arguments, and inside function types.

### Meaning

- A capability value can be passed as an argument, put into a data structure and returned, as any other value.
- A capability value, and any value that holds one, cannot leave the with that provides the capability (`[effect.escape]`). The rule is the same as for closures.
  - Capture checking checks it statically. A capture set follows, together with the type of a value, which capabilities the value holds, so `List[use Db]` and a function that returns `use Db` are followed by the same mechanism.
- Every lifetime is decided by the lexical nesting of withs. Parameters and fields are safe by the nesting alone.
  - Parameters: the with of the caller does not end before the called function returns.
  - Fields of a handler: the arguments of init are evaluated outside the with (`[effect.with]`), so the with of a capability obtained there encloses the with of the handler.
- R follows capture checking. Calling an operation of a capability received as a parameter, a binding or a captured variable adds that capability to R (`fn sync_users(from: use Db, to: use Db) -> () fails DbError uses { from, to }`). It does not have to be provided by name.
- The capability elements of R are inferred and shown by the language server; they are not written in source, as capture checking decided. A function type whose `uses` names another parameter is therefore never written.
- Types are displayed (by the language server and in diagnostics) as they are written: `use E`.
- This proposal adds no way to provide a capability value by name over a range (`with Console by log { ... }`), for code deep inside that uses `use Console`. That is left to a later proposal.

### Typing rules

- `use E` and `use F` are the same type when E and F are the same effect, including their type arguments. A label is a different effect, so `use Source` and `use ReadFile` are different types.
- On a value of type `use E`, only calls of the operations of E and taking them as function values (`x.op`) can be written (as now).
- There is no way to inspect a capability by checking its type, by match or by converting it to Dynamic. No trait (Encode, Decode, Show and others) is implemented for `use E`.
- `==` is not defined.

## Specification changes

### `[effect.use]`

Replace the items from "Passing a capability value as an argument, ..." through its sub-items, and the item "The type of a capability value cannot be written in source. ...", with:

> - The type of a capability value of E is written `use E`. E is an effect, with its type arguments, or a label. `use E` is a type, and can be written wherever a type can.
> - A capability value can be passed as an argument, put into a data structure, and returned from a function, as any other value. It cannot leave the with that provides it (8.6).
> - Only the operations of E can be called on a value of type `use E`, or taken as function values. There is no operation that inspects a capability value: no trait is implemented for `use E`, and `==` is not defined for it.

Replace the item "To let a function use the capability of a specific handler, pass a closure that captures that capability. ..." and its example with:

> - To let a function use the capability of a specific handler, pass the capability value.
>
> ```emel
> fn copy(from: use Db, to: use Db) {
>   to.execute(from.query("..."))
> }
>
> with PrimaryDb {
>   primary = use Db
>   with ReplicaDb {
>     replica = use Db
>     copy(replica, primary)
>   }
> }
> ```

### `[effect.escape]`

Replace the first sentence with:

> A capability value, and any value that holds one (a closure or a local fn that captures it, a data structure that contains it), cannot leave the with that provides it.

### `[effect.handler]`

> - A field of a handler can have the type `use E`. The capability it holds belongs to a with that encloses the with of the handler, because the arguments of init are evaluated outside the with (`[effect.with]`).

### `[fiber.escape]`

Replace "Scope is a capability, so the capability rules (8.2, 8.6) apply as they are. Passing it as an argument, putting it in data, or returning it is a compile error (E0408 to E0410)." with:

> - Scope is a capability, so the capability rules (8.2, 8.6) apply as they are. It cannot leave the scope that provides it.

In the table, replace the row "Passing a capability value (including Scope) as an argument, putting it in data, returning it" with:

> | Taking a capability value (including Scope), or a value that holds one, out of its with or scope | Run time (stateful handlers and Scope) | Static |

### `[type.rules]`

> | `use X` | `use X` | {} | { X } |

### `[grammar.type]`

```ebnf
type       = ...
           | "use" effect_item                                (* the type of a capability; 8.2 *)
```

### `[diag.list]`

> | E0408 | Unused (was: a capability value is passed as an argument; retired when `use E` became a type) | |
> | E0409 | Unused (was: a capability value is stored in a data structure; retired when `use E` became a type) | |
> | E0410 | Unused (was: a capability value is returned from a function; retired when `use E` became a type) | |

## Diagnostics

- [ ] E0408, E0409, E0410: retired
- [ ] An operation of F called on a value of type `use E`: E0401 (as now)
- [ ] `use E` where E is not an effect: E0217 or a new code (to be assigned)

## Compatibility

Every program that compiles now keeps its meaning and its results. Programs rejected by E0408 to E0410 now compile. Those among them that take a capability out of its with are compile errors under capture checking.

## Implementation

- emela-syntax: the type `use E`
- emela-types: the type `use E`; remove the checks of E0408 to E0410; display the type of a capability value as `use E`
- emela-core and later: the representation of a capability value does not change; let it flow into data structures
- emela-lsp: show `use E` in hover

## Alternatives

- Allow it only for parameters and fields: the structure alone makes them safe, and no check has to be added. But returning and storing can be done now by wrapping in a closure, so forbidding them adds no safety.
- Introduce it before capture checking, with a check at run time: wrapping in a closure already allows the same, so no safety is lost, but the R of a function that receives the resource of a handler member as a parameter cannot be satisfied (the capability elements of R come with capture checking). The capability of a handler without state taken out of its with would also go unnoticed.
- Write the type as `cap E` or `&E`: `use E` is the same word as the expression `use E` that makes the value, and reads as "the type of the value obtained by `use E`".
- Provide a passed capability by name over a range (`with Console by log { ... }`): useful when code deep inside uses `use Console`, but it is a separate question from the type, and is left to a later proposal.

## Prior art

- Second-class values: values that can be passed down but not returned or stored. This proposal does not make capabilities second-class; it makes them first-class within their with, and confines them with the same rule as closures.
- Capture tracking: tracking which capabilities a value holds as part of its type, so that a value holding a capability cannot outlive it. Scala 3 has capture checking (`^{c}`).
- Object capabilities: authority is held as an unforgeable reference and passed explicitly.

## Open questions

## Decision
