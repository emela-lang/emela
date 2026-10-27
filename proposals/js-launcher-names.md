---
title: The JavaScript launcher names
authors: [isoshigi]
status: draft
pr:
issue:
decided:
touches: [target.js.module, ext.export, diag.list]
requires: []
related: []
---

# The JavaScript launcher names

## Summary

`main.mjs` exports `main` and `$start` for the launcher in addition to the host exports. A two-argument `@export(js, "name")` that would duplicate one of them is E0438. The rule is conditional on the name being exported: `main` when the entry has a `main`, and `$start` when the entry's `main` or a host export of the `js` target may suspend.

## Motivation

`[target.js.module]` says that only the fns with `@export(js, ...)` and the exports made by `@entry` are exports of `main.mjs` for the host, and that the launcher and other files the compiler writes also export `main` and `$start`. It also says that their names and forms are not specified for other uses. That leaves a gap: a program may write `@export(js, "main")` or `@export(js, "$start")`, and then `main.mjs` has two exports with the same name, which is not a valid JavaScript module.

The rest of the specification already has the pieces for this rule:

- `@export(js, "default")` is E0146, because `default` is the default export itself (`[target.js.module]`).
- Two exports with the same name for the same target are E0438 (`[ext.export]`).

The launcher's exports are the remaining names that the module already uses for something else. A user export that takes one of them is a duplicate of an export of the module, not an arbitrary reserved word.

The rule must match what `main.mjs` actually exports. A build does not include test functions (`[test.attr]`). `$start` is exported only when the launcher or a host export may suspend. An `@export(wasm, ...)` or `@export(c, ...)` does not affect the `js` module. A library-only build produces no `main.mjs`. The current emitter also drops the launcher's `main` when the entry's `main` itself has a `js` `@export`; that suppression is inconsistent with the launcher's contract and with this rule.

## Proposal

### Meaning

`main.mjs` exports:

- the host exports: the fns with `@export(js, ...)` and the exports made by `@entry` (`[target.js.module]`);
- `main` when the entry has a `main`, even when that fn also has a `js` `@export`;
- `$start` when the entry's `main` or a host export of the `js` target may suspend.

This is the build's `main.mjs`. `emela test` writes a separate module that exports the selected test functions and `$start`, but not the host exports, so a host export does not collide there.

A two-argument `@export(js, "main")` or `@export(js, "$start")` that would duplicate one of these is E0438. The diagnostic is reported at the export, or at the `@entry` for an export it makes. If several user exports take the same launcher name, the first is the launcher collision and the later ones are duplicates of each other; each is E0438.

The three-argument form `@export(js, "default", "name")` is a property of the default export object, not a named export, and is not affected. `@export(js, "default")` stays E0146.

`main` is not reserved when the entry has no `main`. A `@test fn main` is a test, not the entry's `main`, and the build does not include it. `$start` is not reserved for a suspending test, for a suspending `@export` of another target, or in a library-only build. A fn may suspend when its R contains `Async` (`[ext.export]`). An open set is not known to be empty, so it counts as may suspend; this is conservative and only affects this check.

A bare entry adapter is not exposed by itself (`[ext.entry]`); its `@export` reserves a launcher name only when an `@entry` uses it.

The form of the launcher's exports (how the launcher calls them) stays unspecified for other uses. Only their names become part of the module interface for the purpose of this duplicate check.

### Typing rules

None. The rule is a constraint on `@export` names.

## Specification changes

### `[target.js.module]`

Replace the bullet

> `main.mjs` also has exports that the runtime's launcher and other files the compiler writes use (`main`, `$start`). Their names and forms are not specified for other uses.

with

> `main.mjs` also has exports that the runtime's launcher and other files the compiler writes use: `main` when the entry has a `main`, and `$start` when the entry's `main` or a host export of the `js` target may suspend. Their forms are not specified for other uses. A two-argument `@export(js, "main")` or `@export(js, "$start")` that would duplicate one of them is E0438.

### `[ext.export]`

Replace the bullet

> Two exports with the same name for the same target, from `@export` or `@entry`, are a compile error (E0438).

with

> Two exports with the same name for the same target, from `@export`, `@entry` or the launcher of `main.mjs` (`[target.js.module]`), are a compile error (E0438).

### `[diag.list]`

Change the row of E0438 from

> Two exports with the same name for the same target (12.7)

to

> Two exports with the same name for the same target (12.7, 16.6.2)

## Diagnostics

- [x] E0438: a two-argument `@export(js, "main")` or `@export(js, "$start")` that duplicates a launcher export

## Compatibility

A program that takes `main` or `$start` when the launcher does not export that name still compiles. A program that would make `main.mjs` export the same name twice now fails to compile, instead of producing an invalid module. The JS emitter also stops dropping the launcher's `main` when the entry's `main` has a `js` `@export`; `emela run` then calls `main` for a program such as `@export(js, "run") fn main`, which currently does nothing.

## Implementation

- `crates/emela-types/src/export.rs` checks the host exports after inference, where the signatures are known, and reports E0438 for the names that `main.mjs` will export. `CheckOptions` carries the entry module, and a library-only build passes none.
- `crates/emela-driver/src/frontend.rs` keeps a `@test fn main` out of the entry's `main`.
- `crates/emela-codegen-js/src/emit.rs` exports the launcher's `main` from the entry even when that fn has host exports.
- Tests cover the two launcher names, the three-argument form, suspending tests, suspending `wasm` and `c` exports, a `@test fn main`, a library-only build, an `@entry`-made export, and a real duplicate. A run test covers `@export(js, "run") fn main`.

## Alternatives

- Reserve `main` and `$start` unconditionally. Rejected: it rejects programs whose `main.mjs` would not export those names, such as a program without `main`, a program whose only suspending exports are for other targets, and a library-only build.
- Rename the launcher's exports to names that a user cannot write, or move the launcher to another module. This keeps the user namespace free, but changes the launcher and test-runner contract. A future proposal may do it; this proposal keeps the current module interface and makes the collision a compile error.
- Rename the user's export when it collides. Rejected: the host must see the name the program wrote (`[ext.export]`).
- Emit the duplicate and let the JavaScript engine reject it. Rejected: the compiler can report it with a source location, and the output would be invalid.
- Reserve `default` here too. Not needed: the two-argument form is already E0146.

## Prior art

An ECMAScript module has a single export name space: two `export` declarations with the same name are a syntax error. This rule rejects that collision in Emela, before the module is written, and only for the names the generated module already exports.

## Open questions

## Decision
