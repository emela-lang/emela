---
title: A wildcard arm for escape
authors: [Alliana]
status: accepted
pr: 420
issue:
decided: 2026-10-29
touches: [error.escape, type.rules, module.resolution, diag.list]
requires: []
related: []
---

# A wildcard arm for escape

## Summary

An escape arm MAY be the wildcard `_`. It handles every error of the original expression, including the errors in opaque set variables, and removes all of them from E. It is meant for falling back to a default, doing nothing, or turning a failure into a defect. `_` cannot have a guard. A variable pattern stays a compile error, because a value of "one of E" cannot be typed (`[error.decl]`).

## Motivation

Today every arm of escape MUST be an error constructor (`[error.escape]`, E0320). To recover from any failure, every error is written out.

```emel
fn outcome[E](body: fn() -> () fails E use { FileSystem }) -> String fails E use { FileSystem } {
  {
    body()
    "ok"
  } escape {
    FileNotFound(..)      -> "FileNotFound"
    IsADirectory(..)      -> "IsADirectory"
    NotADirectory(..)     -> "NotADirectory"
    DirectoryNotEmpty(..) -> "DirectoryNotEmpty"
    InvalidText(..)       -> "InvalidText"
  }
}
```

- When the arms all do the same thing (a default value, a fallback, a log line), listing the errors adds length and nothing else.
- The list has to follow the callee. Adding an error to a callee breaks every escape that meant "recover from anything" (E0318 at the caller).
- A function that recovers from every error of a function it receives cannot be written. The errors of `f` are an opaque set variable, and escape removes nothing from it (`[error.escape]`). `fn or_else[A](f: fn() -> A, default: A) -> A` is impossible today.

The only workaround is `Fiber.attempt` (`[fiber.cause]`), which also turns defects into values. A program that only wants to recover from errors then silently swallows bugs.

```emel
Fiber.attempt(fn() { load(id) }) match {
  Success(data) -> data
  Failure(_)    -> default_data() # also reached by panic and division by zero
}
```

## Proposal

`_` swallows every error that reaches it, so it is a form to use with care. It is meant for the cases where the error itself is not needed: falling back to a default value, doing nothing, or turning the failure into a defect with `panic`. To look at the error (to display it or log it), the errors are handled by their constructors as before.

### Syntax

No change. `arm = pattern [ "if" expr ] "->" expr` already admits `_`.

### Meaning

- An arm `_` handles every error that reaches it. The arms above it are tried first.
- A defect and an interruption are not errors and pass through `_` as before (`[error.defect]`, `[fiber.cause]`).
- `_` cannot have a guard (E0320). `_` binds nothing, so a guard could only test values outside the error, and the arm would remove nothing from E.

### Typing rules

- After an arm `_`, the E of the escape is `Eei` (the errors of the arms) only. Every tag and every set variable of `Ee`, opaque or not, is removed.
- A variable pattern is still E0320. A value of one of several errors has no type (`[error.decl]`).

```emel
fn or_else[A](f: fn() -> A, default: A) -> A {
  f() escape { _ -> default }
}
```

The type of `or_else` is `∀ A, ε. fn(fn() -> A fails ε, A) -> A`, and a call has no errors.

## Specification changes

### `[error.escape]`

Replace the first item of the list with:

- `expr escape { arms }` handles errors raised while evaluating the expression. The pattern of an arm MUST be an error constructor or the wildcard `_`. A variable cannot be written, because a value of one of several errors has no type (7.1). A constructor that is not an error at the top level is E0217, and a variable or `_` with a guard is E0320. An escape with such a mistake is checked further as if it handled all the errors of the original expression (E0318 is not reported in addition). Exhaustiveness of the arms is not required.

Add after the item "A tag is removed only when ...":

- An arm `_` handles every error that reaches it, and removes every tag and every set variable, opaque or not, from E. Arms below it are warned about as redundant (W0301). `_` cannot have a guard.
- `_` discards the error. Use it when the error itself is not needed: to fall back to a default value, to do nothing, or to turn the failure into a defect with `panic`. To look at the error, handle it by its constructor.
- A defect and an interruption are not errors, so `_` does not handle them (7.5, 11.5).

Add to the examples:

```emel
# Fall back to a default on any error
config = load_config(path) escape {
  _ -> default_config()
}

# Turn any error into a defect
config = load_config(path) escape {
  _ -> panic("the bundled config must load")
}

# Fall back on any error of a function received as an argument
fn or_else[A](f: fn() -> A, default: A) -> A {
  f() escape { _ -> default }
}
```

### `[type.rules]`

Replace the E cell of the escape row with:

(Ee ∖ {Ti}) ∪ Eei (Ti are the tags whose arm accepts every value. They are not removed from set variables. When there is an arm `_`, the E is Eei alone. 7.3)

### `[module.resolution]`

- The top-level constructor pattern of an escape arm is limited to errors (E0217). `_` is allowed (7.3). A variable passes name resolution but is a compile error in type checking (E0320, 7.3).

### `[diag.list]`

- E0320: "The pattern of an escape arm is a variable, or `_` has a guard (7.3)". Messages ``escape arms must match an error constructor or `_` `` and ``a `_` escape arm cannot have a guard``.
- W0302: also applies to an arm `_` when the E of the original expression is empty (`no error is raised here`).

## Diagnostics

- [ ] E0320 narrowed to variables, message changed
- [ ] E0320 for `_` with a guard
- [ ] W0302 for `_` on an expression that raises nothing
- [ ] W0301 for arms below `_`

## Compatibility

Programs that compiled before compile the same. `_` was E0320.

## Implementation

- emela-types: handle `_` in the escape rule, clear E including opaque set variables.
- emela-core and backends: an arm `_` lowers to a catch of every error tag, before the rethrow.

## Alternatives

- Keep listing every error. Rejected: a function that falls back on every error of a function it receives cannot be written at all.
- `Fiber.attempt` as the catch-all. Rejected: it also turns defects into values, which `[error.defect]` keeps out of escape on purpose.
- Bind the error to a variable with an opaque type (`e -> log(Cause.to_string(e))`). Rejected: to look at the error, handle it by its constructors. `_` is only for the cases where the error is not needed.
- A lint that warns on `_`. Not now: it is more than the problem needs.
- A separate diagnostic code for `_` with a guard. Rejected: it is a mistake in the pattern of an escape arm, the same as a variable, so it shares E0320 with its own message.
- Allow `_ if cond`, removing nothing from E. Rejected: `_` binds nothing, so the guard can only test values outside the error, and an arm that looks like a catch-all would let errors through.

## Prior art

## Open questions

## Decision

Recovering from every error meant listing each constructor, and the list broke whenever a callee gained an error. A function that falls back on any error of a function it receives could not be written at all, because escape removes nothing from an opaque set variable. Fiber.attempt was the only way out, and it also turns defects into values. `_` handles every error, including opaque set variables, and still lets defects and interruptions through, so `or_else` can be written without swallowing bugs. A variable pattern stays an error because a value of one of several errors has no type; to look at the error, handle it by its constructors. `_` with a guard is rejected because it binds nothing and would look like a catch-all while letting errors through. The change is additive: `_` was E0320 before.
