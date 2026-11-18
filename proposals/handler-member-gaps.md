---
title: Suspending handler members, bound layers, handler members of labels, and capability fields
authors: [Alliana]
status: implemented
pr: 667
issue: 656
decided: 2026-11-19
touches: [effect.decl, effect.use, effect.handler, effect.with, effect.label, type.effect-bound, grammar.decl, diag.list]
requires: [handler-member]
related: [multi-effect-handler]
---

# Suspending handler members, bound layers, handler members of labels, and capability fields

## Summary

Four gaps in `handler-member`, found while implementing it (#656).

1. A handler member is declared `suspend handler` when the init or the release of its handlers may suspend. An effect with a suspend handler member is not Immediate. The init and the release of an inner handler of a handler member without `suspend` are checked as the body of an fn operation is (E0417).
2. `with x = L { ... }`, where L is a layer, is a compile error, with the same code as a handler member item written without `x =` or as an item of a layer.
3. A label of an effect has the handler members of its underlying effect.
4. Calling an operation of a capability held in a field of a handler (`self.inner.read(max)`) adds nothing to R.

## Motivation

### 1. Suspension is not visible in the type

An effect is Immediate when it has no suspend fn and is not Async (`[type.effect-bound]`). `Files` below has no suspend fn, so it is Immediate. The init of `SystemFiles.Open` calls a host function, which is an `@external suspend fn` (as every host function of `[api.std.file-system]` is), so it suspends. Its R is a requirement of `SystemFiles` (`[effect.handler]`), and the R of `with f = Files.Open(p)` is only `{ Files }`.

```emel
effect Files {
  handler Open(path: String): ReadFile
}

fn on_click[R: Immediate](f: fn() -> () uses { R }) -> () { ... }

fn g() -> () uses { Files } {
  on_click(|| with f = Files.Open("a") { count(f) }) # R = { Files } is Immediate, but the init suspends
}
```

The type checker accepts a suspending callback where Immediate is required, and the compiler cannot tell from the types whether `with f = Files.Open(p)` is a call that may suspend. For operations, the same hole is closed by `suspend fn` on the effect side and E0417 on the implementation side. Handler members have neither.

### 2. The binding of a layer has no type

`[effect.with]` says that x is the capability of "the effect that Item provides". A layer provides several effects, so `with x = AppLayer { ... }` gives x no type. `handler-member` says "a layer has no binding" in its Meaning, but the specification has no rule that rejects it.

### 3. Labels and handler members

`effect L of E` has the same operations as E (`[effect.label]`). A handler member is not an operation, so whether `L.Open(p)` or `c.Open(p)` with `c: use L` can be written is not determined. An effect that has handler members and no operations cannot be the underlying effect of a label at all ("MUST be a user effect that has operations").

### 4. Wrapping a provider requires the wrapped effect by name

`[effect.use]` says that calling an operation of a capability obtained by `use E` adds E to R, and that calling an operation of a captured capability adds that capability. A field of `self` is neither, and the implementation (capability-type, #665) adds the effect E. In the `TracedFiles` example of `handler-member`, the inner `Open` calls `self.inner.read(max)`, so `ReadFile` becomes a requirement of `TracedFiles`.

```emel
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) {
      Self(inner: Files.Open(path))
    }

    suspend fn read(max, resume) {
      resume(self.inner.read(max)) # adds ReadFile to the R of TracedFiles
    }
  }
}
```

`with TracedFiles { ... }` then asks for `ReadFile` by name from outside (E0402), although the capability it reads is the one its init opened.

## Proposal

### Syntax

```emel
pub effect Files {
  suspend handler Open(path: String): ReadFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Memory(): ReadFile
}
```

`suspend` can be written before `handler` in an effect, as before `fn`.

### Meaning

- A handler member declared `suspend handler` may suspend when it is opened (the init) and when what it opened is released (the release).
- An effect is Immediate when it has no suspend fn, no suspend handler member, and is not Async.
- For a handler member without `suspend`, the init and the release of every inner handler that implements it MUST satisfy the condition on the body of an fn operation (`[effect.decl]`, E0417): its R is Immediate, or it contains no call that may suspend.
- Opening a suspend handler member (a with item `x = E.M(a)` or `x = c.M(a)`, or `E.M(a)` and `c.M(a)` inside an init) is a call that may suspend. Opening a handler member without `suspend` is not.
- `with x = L`, where L is a layer, is a compile error. It has the same code as a handler member item written without `x =` and a handler member written as an item of a layer.
- A label L of E has the handler members of E. `with x = L.M(a)` opens M of the handler that provides L, and requires L. `c.M(a)` with `c: use L` opens M of the handler that provides c. The underlying effect of a label MUST have operations or handler members.
- Calling an operation of a capability held in a field of the handler (`self.f.op(...)`, in init's value, the operation bodies and release) adds nothing to R. The capability belongs to a with that encloses the with of the handler, or is a handler member opened in its init, and both live at least as long as the handler (`[effect.handler]`). A capability taken out of the field into a variable follows the rules for variables.

### Typing rules

No new rules. In the rows of `handler-member`, E may be a label.

## Specification changes

### `[effect.decl]`

Replace the item on handler members with:

> - An effect MAY declare handler members (`handler Open(path: String): ReadFile fails ...`). A handler member is not an operation: it names a handler that every handler of the effect provides. Its parameters are the parameters of that handler's init, and its `fails` is the E of that init. The effect after the colon is the effect that the handler implements.
> - A handler member declared `suspend handler` may suspend when it is opened and when the handler opened is released. The init and the release of a handler that implements a handler member without `suspend` MUST satisfy the same condition as the body of an fn operation below (E0417).

In "Whether a call may suspend is determined as follows", add:

> - Opening a handler member (`x = E.M(arguments)` in a with, or `E.M(arguments)` inside an init, 8.3, 8.4) is determined by the declaration of the handler member. If it is a `suspend handler`, it may suspend. For a with item, the end of the with, where the handler opened is released, may suspend too.

### `[effect.use]`

Add after the item "Calling an operation of a capability value obtained by `use E` adds E to R. ...":

> - Calling an operation of a capability held in a field of a handler, through `self` (`self.inner.read(max)`), adds nothing to R. The capability belongs to a with that encloses the with of the handler, or to a handler member opened in its init (8.4); both outlive the handler.

### `[effect.handler]`

Add to the item "A field of a handler can have the type `use E`. ...":

> Calling its operations through `self` adds nothing to R (8.2).

Add to the item on inner handlers:

> If the handler member is not a `suspend handler`, the init and the release of the inner handler are checked as the body of an fn operation (E0417, 8.1).

### `[effect.with]`

Replace "A handler member cannot be an item of a layer." with:

> - A handler member cannot be an item of a layer, and a layer cannot be bound (`with x = AppLayer` is a compile error), since it provides more than one effect.

### `[effect.label]`

Replace "The underlying effect MUST be a user effect that has operations." with "The underlying effect MUST be a user effect that has operations or handler members." Add:

> - A label L of E has the handler members of E. `x = L.M(arguments)` in a with opens M of the handler that provides L, and requires L. On `c: use L`, `c.M(arguments)` opens M of the handler that provides c.

### `[type.effect-bound]`

Replace "An effect is Immediate when it has no suspend fn and is not Async." with:

> - An effect is Immediate when it has no suspend fn, no suspend handler member, and is not Async.

Replace the last item with:

> - Whether an effect is Immediate is determined only by its declaration and does not depend on handlers. In the implementation of a fn operation, and in the init and the release of a handler that implements a handler member without `suspend`, the R is Immediate or the body contains no call that may suspend (8.1, E0417), so a call of a fn operation, or opening a handler member without `suspend`, does not suspend whichever handler it reaches, and an operation or a handler member of an Immediate effect never suspends.

### `[grammar.decl]`

```ebnf
member_sig   = [ "suspend" ] "handler" type_name "(" [ params ] ")" ":" type_ref [ "[" type { "," type } "]" ]
               [ "fails" error_set ]
```

### `[diag.list]`

E0417: add a reason for the init or the release of an inner handler (``the init of `Open` may suspend, but handler member `Open` of `Files` is not a `suspend handler` ``, help ``= help: declare `Open` as a `suspend handler` in the effect``).

## Diagnostics

- [ ] The init or the release of an inner handler of a handler member without `suspend` may suspend: E0417 (extended)
- [ ] `with x = Layer`: the code assigned to "a handler member item is written without `x =`, or as an item of a layer" in `handler-member`

## Compatibility

Only adds to `handler-member`, which is not implemented yet.

## Implementation

Together with `handler-member` (#656).

- emela-syntax: `suspend` before `handler` in an effect
- emela-types: Immediate of effects with suspend handler members, E0417 on inner handlers, the binding of a layer, handler members of labels, operations on capability fields of `self` add nothing to R
- emela-core: opening a suspend handler member is a suspending call

## Alternatives

- An effect with any handler member is not Immediate. One rule, no syntax. A provider whose resources open without waiting (an in-memory table) would still make every user of the effect possibly suspending, and the effect cannot be used under `R: Immediate`.

## Prior art

## Open questions

## Decision

Adopted as a supplement to handler-member, found while implementing it (#656).

- A handler member that may suspend when it is opened or released is declared `suspend handler`. An effect with one is not Immediate, and the init and release of a handler that implements a member without `suspend` are checked as the body of an fn operation (E0417). Suspension stays visible in the declaration of the effect, as for operations, and a provider whose resources open without waiting keeps the effect usable under `R: Immediate`.
- `with x = Layer` is a compile error, with the same code as a handler member item without `x =` or in a layer: a layer provides more than one effect, so x has no type.
- A label has the handler members of its underlying effect, so an effect with only handler members can have labels.
- Calling an operation of a capability held in a field of a handler adds nothing to R. The capability outlives the handler, and without this rule a provider that wraps another provider (`TracedFiles`) would have to be given the wrapped effect by name.
