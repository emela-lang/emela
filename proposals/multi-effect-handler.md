---
title: Handlers that implement several effects
authors: [Alliana]
status: implemented
pr: 625
issue:
decided: 2026-11-18
touches: [effect.handler, effect.with, effect.layer, effect.label, effect.use, type.rules, grammar.decl, grammar.type, diag.list]
requires: [capability-type, handler-member]
related: []
---

# Handlers that implement several effects

## Summary

One handler can implement several effects (`handler H: ReadFile + WriteFile`). All the effects it implements share the same `self`. The type of a capability can be written `use ReadFile + WriteFile`, and a capability of a set of effects can be passed as the type of fewer of them. This stays a separate concept from layers.

## Motivation

A resource can have several faces that have to share state.

- A locked file, a temporary file: the face that reads and the face that writes use the same file handle.
- A transaction of a Db: a function that only reads should be written `use Query` and one that writes `use Execute`, but inside a transaction both have to use the same connection and the same transaction.

A handler implements exactly one effect (`[effect.handler]`). Now, one writes a handler of an effect with every operation (`Tx`), and for each face a handler that only forwards to `Tx` (with `Tx` as a requirement), and lists them in the same with.

```emel
with PostgresTxAll(url: "..."), TxQuery, TxExecute { ... }
```

Each face needs a forwarding handler.

Together with handler members (`handler-member`), the original example (read only, write only, locked, temporary) is written as it is.

```emel
pub effect TemporaryFile {
  fn path() -> String
  suspend fn persist(to: String) -> ()
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
}

pub effect Files {
  handler Open(path: String): ReadFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Create(path: String): WriteFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Lock(path: String): ReadFile + WriteFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Temporary(): ReadFile + WriteFile + TemporaryFile fails FileAccessDenied | FileSystemFailure
}

fn save(path: String, text: String) -> ()
  fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure uses { Files } {
  with guard = Files.Lock("#{path}.lock"), tmp = Files.Temporary() {
    tmp.write(Bytes.encode_utf8(text))
    tmp.sync()
    tmp.persist(path)
  }
}

with src = Files.Open("in.txt"), tmp = Files.Temporary() {
  copy(src, tmp) # tmp is passed as `use WriteFile`
}
```

| The type wanted | The type of the capability |
|---|---|
| ReadableFile | `use ReadFile` |
| WritableFile | `use WriteFile` |
| LockedFile | `use ReadFile + WriteFile` |
| TemporaryFile | `use ReadFile + WriteFile + TemporaryFile` |

## Proposal

### Syntax

```emel
handler SystemTx(conn: Connection): Query + Execute {
  init(url: String) { ... }
  release { ... }
  suspend fn query(sql, resume) { ... }
  suspend fn execute(sql, resume) { ... }
}
```

- After the colon of a handler, effects can be joined with `+`.
- The type of a capability can be written `use E1 + E2`.

### Meaning

- A handler implements each operation of every effect it lists, exactly once. Listing two effects that have an operation of the same name is a compile error, since it would not be decided which implementation is which. A syntax that qualifies an operation by its effect can be added later without breaking code.
- The operations of all the effects it implements share the same `self`.
- Written in a with, it provides all the effects it lists. Its init and its release run once each.
- When the implementation of one effect uses an operation of another effect it implements, the call does not reach its own implementation without going through a with. A `use E` in the body of an operation is a requirement, as now, and a requirement on its own effect is provided outside (`[effect.with]`). State is shared through `self`.
- It stays a separate concept from layers. A layer wires separate handlers in a with, and resolves the requirements of its members at the with (`with AppLive, FixedClock` replaces Clock, and PostgresUsers uses FixedClock too). A handler of several effects shares one `self`, and is replaced as a whole.
- In `L by H`, H serves L through the effect it lists that is the underlying effect of L. H lists each effect at most once and a label has one underlying effect, so there is at most one such effect.
- Passing the same capability as two faces (`copy(tmp, tmp)`) is not stopped by types. It is the lint of `handler-member` (the same capability variable passed to two parameters of one call).
- A property that a type has to guarantee (the contents do not change, the access is exclusive) is expressed by a separate effect, not included in another. A capability can be narrowed but not widened, so a separate effect cannot be reached from a capability that does not have it.

### Typing rules

- provides(H) is all the effects it lists.
- A capability of `use E1 + ... + En` can be passed to a parameter of `use F1 + ... + Fm` when {F1, ..., Fm} ⊆ {E1, ..., En}. Only the operations of the F can be called on it there.
- There is no way to widen the type of a capability (checking its type, match, conversion to Dynamic). The operations of the original effects cannot be called on a narrowed capability.
- `use E1 + E2` extends `use E` of `capability-type`, and can likewise be written wherever a type can. Calling an operation adds that effect to R.

## Specification changes

### `[effect.handler]`

Replace "A handler is a declaration, and it implements each operation of one effect exactly once." with:

> - A handler is a declaration, and it implements each operation of the effects after its colon exactly once (`handler SystemTx(conn: Connection): Query + Execute`). All of them share the same `self`. Listing two effects that have an operation of the same name is a compile error (code to be assigned).

### `[effect.with]`

> - A handler that implements several effects provides all of them. Its init and release run once.

### `[effect.layer]`

> - A handler that implements several effects counts as providing each of them for E0421.

### `[effect.label]`

> - In `L by H`, where H implements several effects, H serves L through the effect that is the underlying effect of L. If none of them is, it is E0431.

### `[effect.use]`

> - The type of a capability of several effects is written `use E1 + ... + En`. A capability can be passed to a parameter of type `use F1 + ... + Fm` when every Fi is one of the Ej. Only the operations of the Fi can be called through the parameter. There is no way to recover the other effects from it.
> - A property that a type has to guarantee, such as contents that do not change or exclusive access, is expressed by a separate effect that no handler implements together with an effect that breaks the property.

### `[type.rules]`

> - provides(H) is all the effects that H implements.

### `[grammar.decl]`

```ebnf
handler_decl = "handler" type_name [ "(" [ fields ] ")" ]
               ":" effect_ref { "+" effect_ref } "{" { handler_item NL } "}"
effect_ref   = type_ref [ "[" type { "," type } "]" ]
member_sig   = "handler" type_name "(" [ params ] ")" ":" effect_ref { "+" effect_ref }
               [ "fails" error_set ]
```

### `[grammar.type]`

```ebnf
type       = ...
           | "use" effect_ref { "+" effect_ref }
```

## Diagnostics

- [x] Two effects listed on a handler have an operation of the same name: E0450
- [x] The same effect listed twice on a handler: E0451
- [x] A capability passed to `use F...` that lacks some Fi: E0452

## Compatibility

The change only adds.

## Implementation

- emela-syntax: a list of effects joined by `+`
- emela-types: provides(H) as a set, the check of repeated operation names, inclusion between the types of capabilities
- emela-core: the tables of operations of several effects from one handler value

## Alternatives

- A forwarding handler per face (what the current specification allows): a handler and a label per face.
- Sugar for delegation (a syntax that forwards the remaining operations outward): less forwarding to write, but the faces still cannot share state.
- Replacing layers with handlers of several effects: the wiring happens at the opposite time, and when a test replaces some of the effects, the inside of the handler still uses the original implementation.
- No inclusion between the types of capabilities: a LockedFile or a TemporaryFile could not be passed to a parameter of `use ReadFile`, and a function would be written for each face.
- Qualifying operations by effect in their implementations, so that two effects with an operation of the same name can be listed: not needed for the motivating cases, and can be added later.

## Prior art

- Interface subsumption in object-capability systems: attenuating authority by passing a reference through a narrower interface.
- Handlers for several effects at once: Koka and Effekt let one handler handle several operations of different effects.

## Open questions

## Decision

Adopted, on top of the type of a capability (#622) and handler members (#624), which this requires.

- One handler can implement several effects that share one self, so the faces of one resource (a locked file, a temporary file, a transaction) share their state without forwarding handlers.
- A capability of several effects can be narrowed to fewer of them, and never widened. A property a type has to guarantee is expressed by a separate effect.
- This stays apart from layers: a layer wires separate handlers, a handler of several effects is replaced as a whole.
- Listing two effects with an operation of the same name stays an error; qualifying operations by effect can be added later.
