---
title: A built-in trait for capability types, Capability
authors: [somnicattus]
status: draft
pr: 650
issue: 616
decided:
touches: [trait.builtin, effect.use, diag.list]
requires: [capability-type]
related: []
---

# A built-in trait for capability types, Capability

## Summary

Add a built-in trait `Capability`. Only capability types `use E` satisfy it, and users cannot implement it (it is determined automatically, as `Immediate` is). `[C: Capability]` says in a type "a capability of any effect".

## Motivation

With `capability-type`, `use E` is an ordinary type and can be a type argument. An ordinary type parameter A can take `use Db`, but A cannot be limited to capabilities.

Some built-in functions take an argument that is a capability of any effect. In the proposal of regions (#651, under review), the function that makes a region takes the capability of a provider, and the function that closes takes the capability of an opened resource. Neither knows the effect, but neither accepts a value that is not a capability.

```emel
# now: the types cannot be written, so the rules are given in prose
Region.scope(use Engine, |enemies| { ... })
enemies.close(a) # `close` accepts a capability of any effect, checked by a special rule
enemies.close(42) # needs its own diagnostic

# with this proposal
pub fn scope[P: Capability, A, E, R](provider: P, body: fn(use Region[P]) -> A fails E uses { Async, R }) -> A
  fails E
  uses { Async, R }

suspend fn close[C: Capability](item: C) -> ()

enemies.close(42) # error[E0326]: `Int` does not satisfy `Capability`
```

With the types written as declarations, no typing rule has to be written separately in prose, and a wrong argument is stopped by the current check of traits (E0326).

## Proposal

### Syntax

No syntax is added. The constraint is written as other trait constraints are (`[C: Capability]`, `[trait.call]`).

### Meaning

- `Capability` is a built-in trait without functions.
- Only `use E` satisfies `Capability` (E is an effect or a label, with its type arguments). A type that contains `use E` (`List[use Db]`, `(use Db, Int)`) and a function type do not.
- Users cannot write an impl of `Capability` (a compile error). An impl of `Immediate` is the same compile error (no diagnostic is defined for it now).
- `Capability` is put on an ordinary type parameter. Putting it on a set type parameter (in the position of `fails E` or `uses { R }`) is a compile error.
- Inside a function with `[C: Capability]`, a value of C can be used as the value of any other type parameter (passed, stored, returned). The effect is not known, so no operation can be called on it. This proposal adds no way to use the capability without knowing its effect.

### Typing rules

- Whether the constraint is satisfied is checked where the function is referred to, as for other trait constraints (`[trait.call]`, E0326).
- The capture set of a capability value is not lost through a type parameter (`capture-checking`, `capability-type`), with or without `[C: Capability]`.

## Specification changes

### `[trait.builtin]`

Add a row to the table:

> | Capability | The type is a capability type `use E` (8.2) | Not allowed (determined automatically) |

Add after the table:

> - Capability has no functions. It is satisfied only by `use E`, where E is an effect or a label, with its type arguments. A type that contains a capability type (`List[use Db]`, `(use Db, Int)`) and a function type do not satisfy it.
> - Capability can be put only on an ordinary type parameter. Putting it on an error set or effect set type parameter is a compile error (code to be assigned).
> - An impl of Capability or Immediate is a compile error (code to be assigned).

### `[effect.use]`

Add to the items on the type `use E`:

> - `use E` satisfies the built-in trait Capability (10.6). A type parameter with `[C: Capability]` accepts a capability of any effect.

### `[diag.list]`

Add rows:

> | (code to be assigned) | Capability on a type parameter that is not an ordinary type (10.6) | `` `Capability` can only constrain ordinary type parameters, but `R` is used in `uses` `` |
> | (code to be assigned) | An impl of a built-in trait that is determined automatically: Immediate or Capability (10.6) | `` `Capability` is determined automatically and cannot be implemented `` |

## Diagnostics

- [ ] A type that does not satisfy Capability: E0326
- [ ] Capability on an error set or effect set type parameter: code to be assigned
- [ ] An impl of Capability or Immediate: code to be assigned

## Compatibility

A type or trait named `Capability` defined by a user overlaps the name. Names of built-in traits are treated as Eq, Ord, Show and Immediate are. Otherwise the change only adds.

## Implementation

- emela-types: Capability in the table of built-in traits; whether a type satisfies it (whether it is `use E`); the diagnostics for a set type parameter and for an impl
- The representation at run time does not change

## Alternatives

- A type parameter for one effect (`[F: effect]`, written `use F`): adds a new kind of type parameter. Kinds are inferred from position (`[type.fn]`), so a rule is needed to tell it apart from a set type parameter when written in `uses { F }`, and it touches `[open.list]` #31 (one type parameter written in both `fails` and `uses`). This proposal uses the fact that `use E` is an ordinary type since `capability-type`, and adds no kind.
- Typing rules in prose for each built-in function: each function needs its own check and diagnostic, and its type cannot be read from its declaration.
- Other names (`IsCapability`, `Cap`): `Capability` names what the type is, as `Immediate` does.

## Prior art

- Marker traits that the compiler implements automatically from the structure of a type: Rust's auto traits (`Send`, `Sync`). In Rust, users can still implement them with `unsafe impl`; here users cannot.
- Capability types as ordinary types in capability-based effect systems (Effekt, Scala 3's capture checking).

## Open questions

## Decision
