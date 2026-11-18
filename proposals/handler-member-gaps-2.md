---
title: Effects on the left of `.`, E0417 for handler members, and capability fields in suspension
authors: [Alliana]
status: draft
pr:
issue: 656
decided:
touches: [module.resolution, effect.use, effect.handler, diag.list]
requires: [handler-member, handler-member-gaps]
related: []
---

# Effects on the left of `.`, E0417 for handler members, and capability fields in suspension

## Summary

Three more gaps in `handler-member` and `handler-member-gaps`, found while implementing them (#656).

1. An effect can be written on the left of `.`, only to name its handler members (`Files.Open`). When a module and an effect of the same name are both found, `X.M` names the handler member M of the effect if it has one, and otherwise the definition M of the module.
2. E0417 on the init and the release of an inner handler is reported once for each call that may suspend, as for an fn operation, with a fixed message and help. Opening a `suspend handler` member is a new reason for a call that may suspend.
3. Rule 4 of `handler-member-gaps` is refined. Calling an operation of a capability held in a field of a handler through `self` (`self.inner.read(max)`) in an operation body does not require its effect by name. It still counts as a call of its effect for whether the body may suspend. In release, its effect stays a requirement of the handler.

## Motivation

### 1. Effects on the left of `.`

`[module.resolution]` looks up only modules and Traits on the left of `.`, and says "An effect cannot be written on the left of `.` (`Logger.info` is a compile error. 8.2)". `handler-member` writes `Files.Open(p)` (`[effect.with]`, `[effect.handler]`), which this rule rejects.

A module and an effect can also have the same name (`State`, as in `State.run(...)`). If an effect were looked up on the left of `.` with the rule for a module and a Trait at the same level (E0218), `State.run` would be ambiguous and existing programs would break.

### 2. The form of E0417 for handler members

`handler-member-gaps` checks the init and the release of an inner handler of a handler member without `suspend` as the body of an fn operation (E0417), and gives one message for the whole init (``the init of `Open` may suspend, but ...``). E0417 on an fn operation is reported for each call that may suspend, with the reason for that call. The two forms differ, and opening a `suspend handler` member, which may suspend, has no reason of its own.

### 3. Capability fields and suspension

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

`with TracedFiles { ... }` then asks for `ReadFile` by name from outside (E0402), although the capability it reads is the one its init opened. `handler-member-gaps` fixes this by saying that such a call "adds nothing to R".

Taken literally, that rule also removes the call from the check of suspension. An fn operation could then call a suspend fn through a field and pass E0417. In release, a call that may suspend would not show in the R of the with.

## Proposal

### Syntax

No changes.

### Meaning

- On the left of `.`, effects are looked up too, only to name their handler members (`Files.Open`). Naming an operation on an effect (`Logger.info`) stays a compile error. A label names the handler members of its underlying effect (`handler-member-gaps`).
- When a module and an effect of the same name are both found at the same level, `X.M` names the handler member M of the effect if the effect has one of that name, and otherwise the definition M of the module.
- E0417 on the init and the release of an inner handler of a handler member without `suspend` is reported once for each call that may suspend, as for an fn operation. Opening a `suspend handler` member is a call that may suspend, with its own reason.
- In an operation body of a handler, calling an operation of a capability held in a field through `self` (`self.inner.read(max)`) does not require its effect by name: the effect is not a requirement of the handler. The capability belongs to a with that encloses the with of the handler, or is a handler member opened in its init, and both live at least as long as the handler (`[effect.handler]`). The value of init has no `self`, so the rule does not apply there.
- Such a call still counts as a call of its effect for whether the body may suspend. It is a call that may suspend if the operation is a suspend fn, and the R of the operation body is not Immediate if the effect is not (E0417, `[type.effect-bound]`).
- In release, the effect of such a call is a requirement of the handler as before, so that a release that may suspend shows in the R of the with.
- A capability taken out of the field into a variable follows the rules for variables.

### Typing rules

No new rules.

## Specification changes

The changes of `handler-member-gaps` to `[effect.use]`, `[effect.handler]` and `[diag.list]` are not yet applied to `spec/`. Where a change below names text of `handler-member-gaps`, it replaces that text, and is applied together with it.

### `[module.resolution]`

Replace "On the left of `.`, only modules and Traits are looked up." with:

> On the left of `.`, modules and Traits are looked up, and effects, only to name their handler members (`Files.Open`, 8.3).

Replace "If both a module and a Trait are found at the same level, it is a compile error (E0218)." with:

> If both a module and a Trait are found at the same level, it is a compile error (E0218). If both a module and an effect are found, `X.M` names the handler member M of the effect if it has one, and otherwise the definition M of the module.

Replace "An effect cannot be written on the left of `.` (`Logger.info` is a compile error. 8.2)." with:

> An operation cannot be named on an effect (`Logger.info` is a compile error, 8.2); only its handler members can.

### `[effect.use]`

Replace the item that `handler-member-gaps` adds after "Calling an operation of a capability value obtained by `use E` adds E to R. ..." with:

> - In an operation body of a handler, calling an operation of a capability held in a field through `self` (`self.inner.read(max)`) does not make its effect a requirement of the handler. The capability belongs to a with that encloses the with of the handler, or to a handler member opened in its init (8.4); both outlive the handler. The call still counts as a call of its effect for whether the body may suspend: it may suspend if the operation is a suspend fn, and the R of the operation body is not Immediate if the effect is not (8.1, E0417). In release, the effect of such a call is a requirement of the handler, so that a release that may suspend shows in the R of the with.

### `[effect.handler]`

Replace the sentence that `handler-member-gaps` adds to the item "A field of a handler can have the type `use E`. ..." ("Calling its operations through `self` adds nothing to R (8.2).") with:

> Calling its operations through `self` in an operation body does not make its effect a requirement of the handler, but counts for whether the body may suspend (8.2).

### `[diag.list]`

Replace the change of `handler-member-gaps` to E0417 with:

> E0417: add the init and the release of an inner handler of a handler member without `suspend`. As for an fn operation, it is reported once for each call that may suspend. The message is the reason for the call followed by ``, but handler member `Open` of `Files` is not a `suspend handler` `` (``operation `sleep` of `Clock` is a `suspend fn` and may suspend, but handler member `Open` of `Files` is not a `suspend handler` ``), and the help is ``= help: declare `Open` as a `suspend handler` in the effect``.

Add a reason for a call to E0417:

> Opening a `suspend handler` member: ``handler member `Open` of `Files` is a `suspend handler` and may suspend``.

## Diagnostics

- [ ] A call that may suspend in the init or the release of an inner handler of a handler member without `suspend`: E0417 (extended), once per call
- [ ] Opening a `suspend handler` member as a reason of E0417: ``handler member `Open` of `Files` is a `suspend handler` and may suspend``
- [ ] A call of a suspend fn through a capability field of `self` in an fn operation: E0417 (existing)
- [ ] An operation named on an effect (`Logger.info`): stays a compile error

## Compatibility

Only changes `handler-member` and `handler-member-gaps`, which are not implemented yet. Rule 1 keeps the meaning of `X.M` for a module X when no effect named X has a handler member M, so `State.run(...)` is unchanged.

## Implementation

Together with `handler-member` (#656).

- emela-resolve: effects on the left of `.` for handler members, a module and an effect of the same name
- emela-types: operations on capability fields of `self` are not requirements of the handler but count for suspension (except in release); E0417 per call on inner handlers, with the reason for opening a `suspend handler` member

## Alternatives

## Prior art

## Open questions

## Decision
