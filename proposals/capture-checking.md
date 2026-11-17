---
title: Capture checking
authors: [Alliana]
status: draft
pr: 621
issue:
decided:
touches: [intro.experimental, effect.escape, effect.use, fiber.escape, type.fn, type.rules, lsp.hover, diag.list, open.next]
requires: [experimental-features]
related: []
---

# Capture checking

## Summary

Infer, together with its type, the set of capabilities that a value holds (its capture set), and report a value that leaves the with that provides one of them. The R of a function gets a second kind of element: besides the name of an effect (`Db`), a capability itself (`primary`, a parameter `from`). The capture set is not written in source; the language server and the diagnostics show it. In 0.28 the inference, the display and the new elements of R always apply, and leaving a with is a warning. With the experimental feature `capture-checking` (`[intro.experimental]`) it is a compile error. A later version makes the error the default.

## Motivation

`[effect.escape]` says that a value that holds a capability cannot leave the with that provides it, but the check is done at run time, and the text says that it becomes static "in 0.22". The current version is 0.27, and it has not.

- A handler without state is not checked at all: its capability keeps working after it is taken out of the with.
- For a handler with state, the value is found only when it is called, as a defect. A value that is never called is never found.
- An operation that returns a closure capturing `self` lets the fields be read after release (the draft `handler-self-after-release`).
- Leaving a `Fiber.scope` is stopped by whether the type of the value names `Fiber` and the like (E0425), a check by the name of the type.

R has only the granularity of the name of an effect, and that hides which handler a call reaches.

```emel
with Plain {
  p = use Greeter
  with Shout {
    run_greeter(|| p.greet("b")) # "hello b": the closure reaches Plain
  }
}
```

The type of the closure is `fn() -> String uses { Greeter }`, and the Greeter in the R of `run_greeter` is satisfied by Shout. The call reaches Plain. R does not tell "what has to be provided by name where the function is called" from "the handler the function captured".

The proposals that make capabilities values (`capability-type`, `handler-member`) need this distinction. A resource of a handler member is not provided by name, so putting `ReadFile` in the R of `copy(src: use ReadFile, ...)` leaves no handler to satisfy it.

## Proposal

### What is tracked

- An element of a capture set is a root. A root is one of:
  - the handler of a with item,
  - the Scope of a `Fiber.scope`,
  - `self` in the operations of a handler (the handler itself).
- `use E` refers to the root of the innermost with that provides E at that position. A `use E` in a function that no with encloses refers to a root passed by the caller, and is written as the name of the effect, E.
- The capture set of a value is the set of roots it may reach.
  - A closure or a local fn: what its captured variables hold.
  - A data structure: what its elements hold.
  - The result of a call: what the return value of the callee holds, with its parameters replaced by the arguments.
- A value whose type cannot contain a capability holds nothing. Such a type contains no function type, no capability, no type parameter, and none of the types that belong to a scope (Fiber, Deferred, Semaphore, Queue).

### The check

- If the value of a with body holds a root of that with, it is a warning (W0402).
- If the value of a `Fiber.scope` holds its Scope, or a Fiber, Deferred, Semaphore or Queue made by it, it is a warning (W0402).
- The return value of an operation that holds `self` cannot leave the with of the handler, by the first rule.
- Experimental (`capture-checking`). Instead of W0402, these are a compile error (E0444).
- The run-time check of `[effect.escape]` (a released handler used is a defect) and E0425 stay as they are.

### The elements of R

- R has two kinds of elements: the name of an effect (`Db`) and a capability (`primary`, a parameter `from`).
  - `use Db`, and calling an operation of a capability obtained by `use Db`, add `Db` to R, as now.
  - Calling an operation of a capability received as a parameter, bound by a with item, or captured, adds that capability to R.
- The type of `|| p.greet("b")` becomes `fn() -> String uses { p }`.
- A capability element can be widened to the name of its effect. `fn() uses { p }` can be passed where `fn() uses { Greeter }` is expected: it requires less. The `run_greeter` example above compiles as now.
- At a call, the capability elements of the callee's R that are parameters are replaced by the elements of the arguments, and added to the caller's R. A capability element is removed from R where the with (or the binding) that made its root ends.
- Whether a call may suspend (E0417, Immediate) is decided, for a capability element, by its effect.

### Inference and display

- The capture set and the capability elements of R are inferred. They are not written in source (as `[effect.escape]` says now).
- The capture set of the return value of a function is inferred from its body. Within a recursive group it is monomorphic, as R is (`[type.inference]`).
- A declaration without a body (an effect operation, a trait function, an `@external` fn, the function type of a parameter) uses these defaults:
  - its return value may hold what its arguments hold,
  - the return value of an effect operation may also hold its handler,
  - a function type inside a declaration is pure, as now (`[type.fn]`), so it holds nothing.
- The hover of the language server shows the capture set of a return value or a value on a line `holds:`.
- A root, and a capability element of R, is shown by the name attached to it: the variable of a binding (`p`, `src`), the parameter that received it (`from`), the handler of a with item that is not bound (`ConsoleLogger`), or the name of the effect for a `use E` that the caller provides (`Db`). When two roots have the same name, the hover adds where each is provided (`p (with PrimaryDb, line 4)`). A function can return only values that hold its parameters or roots from outside it (a root made inside would leave its with), so a return value always has such a name.
- No syntax is defined for writing a capture set.

```text
fn pick(primary: use Db, replica: use Db, write: Bool) -> use Db uses {}
  holds: primary, replica
```

```text
f: fn() -> Rows uses { primary }
  holds: primary (with PrimaryDb, line 4)
```

### Examples

```emel
fn make_greeter() -> fn() -> () {
  with FileLogger {
    log = use Logger
    || log.info("hi") # W0402, or E0444 with `capture-checking`
  }
}

with PrimaryDb {
  p = use Db
  with ReplicaDb {
    r = use Db
    f = || r.query("...")
    [f] # holds `r`, which is released at the end of `with ReplicaDb`
  }
}
```

```text
warning[W0402]: a value that holds `log` leaves the `with` that provides it
   ╭─[ src/main.emel:4:5 ]
   │
 2 │   with FileLogger {
   │        ─────┬────
   │             ╰── `log` is released at the end of this `with`
 4 │     || log.info("hi")
   │     ───────┬─────────
   │            ╰── the value of the body holds `log`
───╯
```

### Rollout

- 0.28: inference, `holds:`, the capability elements of R and W0402 always apply. E0444 applies with `capture-checking`.
- During the period of warnings, until the next minor version, code that takes the capability of a handler without state out of its with is found and fixed.
- 0.29: E0444 becomes the default. W0402 and E0425 are retired, the run-time check of `[effect.escape]` is removed, and `capture-checking` becomes stable (`[intro.experimental]`). That is a separate change of the specification, recorded when it is made.

## Specification changes

### `[intro.experimental]`

Add a row to the table:

> | `capture-checking` | 0.28 | A value that holds a capability and leaves the with or `Fiber.scope` that provides it is a compile error (E0444) instead of a warning (W0402) (`[effect.escape]`) |

### `[effect.escape]`

Replace the items from "The compiler tracks which capabilities a value holds ..." through "Until the capture set check is added (0.22), ..." with:

> - The compiler infers, together with its type, which capabilities a value holds (its capture set). The elements of a capture set are roots: the handler of a with item, the Scope of a `Fiber.scope`, and `self` inside the operations of a handler. `use E` refers to the root of the innermost with that provides E at that position.
> - A closure or a local fn holds what its captured variables hold. A data structure holds what its elements hold. The result of a call holds what the return value of the callee holds, with its parameters replaced by the arguments. A value whose type cannot contain a capability holds nothing.
> - The capture set of the return value of a function is inferred from its body. For a declaration without a body (an effect operation, a trait function, an `@external` fn, the function type of a parameter), the return value holds what its arguments hold, and the return value of an operation also holds the handler.
> - If the value of a with body holds a root of that with, it is a warning (W0402).
> - Experimental (`capture-checking`). It is a compile error (E0444) instead.
> - The check at run time stays while the error is not the default: calling an operation of a handler that has already been released is a defect (`handler PostgresUsers was used after release`). It applies only to handlers that have fields, init, or release.
> - The capture set is not written in source. The language server shows it (`[lsp.hover]`).

### `[effect.use]`

Replace "Calling an operation of a capability value adds its effect to R" with:

> - Calling an operation of a capability value obtained by `use E` adds E to R. Calling an operation of a capability captured by a closure or a local fn adds that capability to R (`uses { primary }`). A capability in R can be widened to its effect: a function whose R contains `p` can be used where R contains the effect of p. Whether a call may suspend (E0417) is decided by the effect of the capability.

### `[fiber.escape]`

Add after the item on E0425:

> - The Scope of a `Fiber.scope`, and the Fibers, Deferreds, Semaphores and Queues it makes, are roots of that scope (8.6). If the value of `Fiber.scope` holds one of them, it is a warning (W0402).
> - Experimental (`capture-checking`). It is a compile error (E0444) instead.

Replace the paragraph that begins "When capture sets (8.6) arrive in 0.22" with:

> Capture sets (8.6) track handles, Deferreds, Semaphores and Queues as values that hold the root of the scope that created them. Leaving the scope is a warning, and a compile error with the experimental feature `capture-checking`.

In the table, rename the column "0.22" to "With `capture-checking`".

### `[type.fn]`

Add:

> - The `uses` of a function type can contain capabilities as well as effects (`fn() -> Rows uses { primary }`). They are inferred and shown by the language server, and are not written in source.

### `[type.rules]`

Replace the row of the anonymous function:

> | `\|x\| e` | `fn(..) -> T fails Ee uses { Re }`, where Re keeps the captured capabilities as elements | {} | {} |

Add:

> - A call `f(a)` adds to R the elements of the R of f, with each capability element that is a parameter replaced by the elements of the corresponding argument.

### `[lsp.hover]`

Add:

> - On a function or a value that may hold capabilities, the hover shows a line `holds: ...` with the roots it may hold, and for each root the with that provides it. A root is shown by the variable of its binding, the parameter that received it, the handler of an unbound with item, or the name of the effect for a `use E` provided by the caller.

### `[diag.list]`

Add:

> | E0444 | With the experimental feature `capture-checking`, a value that holds a root leaves the with or the `Fiber.scope` that provides it (8.6, 11.8) | ``a value that holds `log` cannot leave the `with` that provides it`` |
> | W0402 | A value that holds a root leaves the with or the `Fiber.scope` that provides it (8.6, 11.8). The labels show where the root is provided and where the value leaves | ``a value that holds `log` leaves the `with` that provides it`` |

### `[open.next]`

Replace the item "0.22: capture set" with:

> - [ ] 0.29: make E0444 the default, retire W0402 and E0425, and remove the run-time check of 8.6 (`capture-checking`)

## Diagnostics

- [x] A value that holds a root leaves its with or `Fiber.scope`: W0402, or E0444 with `capture-checking`
- [x] E0425: kept until the error is the default (0.29)
- [x] The run-time defect `handler X was used after release`: kept until the error is the default, then removed

## Compatibility

- Programs that compile now still compile. Code that takes a capability out of its with now gets W0402.
- With `capture-checking`, that code is a compile error. This includes code that takes the capability of a handler without state out of its with, which runs now, and code that takes one out and never calls it.
- The capability elements of R can be widened to the names of effects, so every R check that passes now still passes.

## Implementation

- emela-types: the inference of capture sets (kept with the types), the check at the end of a with and of a `Fiber.scope`, the capability elements of R and their widening, the replacement at the boundary of a call
- emela-cli, emela-lsp: W0402 and E0444 by the setting of each Pome
- emela-lsp: the `holds:` line in hovers

## Alternatives

- Keep the check at run time: handlers without state, and values never called, are not found.
- Write capture sets in source (`T^{c}`): more to write. Emela has a fixed kind of capability (`use E` and Scope), immutable variables and nested lifetimes, so inference with conservative defaults is expected to be enough. A syntax can be added later for a declaration whose default is too strict.
- Stop by the name of the type (extending E0425): every type has to be walked (`Option[Fiber[User]]`), and two values of the same type from different withs cannot be told apart.
- Keep R to the names of effects: the R of a function that receives the resource of a handler member as a parameter cannot be satisfied, and a captured handler cannot be told apart from the one where the function is called.
- Make the error the default at once: code that runs now breaks in one version, and the defaults of the inference cannot be adjusted against real code first.
- Report a change of the capture set of a pub function in a compatibility check of Pome: the return type, `fails` and `uses` of a pub function can be inferred too (`[module.pub-signature]`), and Pome has no compatibility check for any of them. A check of the inferred signatures of pub functions can cover capture sets together, when one is added.


## Prior art

- Capture checking: tracking the capabilities a value captures as part of its type, so that a value cannot outlive the capabilities it holds. Scala 3 has it, with capture sets written in types (`T^{c}`, `A ->{c} B`) (Odersky, Boruch-Gruszecki et al., "Capturing Types").
- Effects as capabilities, and capture sets for effect handlers: System C tracks the capabilities a computation uses as capture sets, with second-class capabilities by default and boxes for first-class use (Brachthäuser, Schuster, Lee, Boruch-Gruszecki, "Effects, Capabilities, and Boxes", OOPSLA 2022). Effekt is built on it.
- Region-based escape analysis: rejecting values that refer to a region after it ends.

## Open questions

- A callback passed to an `@external` fn can be kept by the host and called after the with ends (`[ext.boundary]`). Its R may contain Immediate effects, so it may hold a capability. The run-time check catches this now. Before the run-time check is removed in 0.29, decide one of: a value passed to an `@external` fn counts as leaving every with that provides a root it holds; or the run-time check stays only for values that the host calls back.

## Decision
