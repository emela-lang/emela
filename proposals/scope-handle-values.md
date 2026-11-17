---
title: Values taken out of the handles of a scope
authors: [Alliana]
status: accepted
pr: 645
issue:
decided: 2026-11-18
touches: [effect.escape, fiber.escape, open.next]
requires: []
related: [capture-checking]
---

# Values taken out of the handles of a scope

## Summary

A value taken out of a Fiber, Deferred, Semaphore or Queue (`Fiber.join`, `Deferred.await`, `Queue.take`, `Semaphore.with_permit`, ...) does not hold the root of the scope that made the handle, unless its type contains one of these types. Without this, the defaults of capture sets (`[effect.escape]`) report W0402 in correct programs, including `Fiber.all`, `race`, `both`, `map` and `timeout` of the standard library.

## Motivation

The proposal `capture-checking` gives a declaration without a body this default: its return value holds what its arguments hold. A Fiber, Deferred, Semaphore or Queue holds the root of the scope that made it (`[fiber.escape]`). `Fiber.join`, `Fiber.await`, `Deferred.await`, `Deferred.poll`, `Queue.take`, `Queue.poll` and `Semaphore.with_permits` are `@intrinsic` fns without a body, so the value they return holds the root of the scope.

```emel
fn run[A](f: fn() -> A) -> A {
  Fiber.scope(|| Fiber.join(Fiber.fork(f))) # W0402 by the current rules
}
```

- `Fiber.fork(f)` holds the root of the scope: its body is `(use Scope).fork(f)`, and the return value of an operation holds its handler.
- `Fiber.join` returns what its argument holds, so the root of the scope.
- `A` is a type parameter, so the value may hold capabilities, and the value of `Fiber.scope` holds its own root: W0402, and E0444 with `capture-checking`.

The program is correct: the value that `join` returns is the value `f` returned, and it does not belong to the scope. `Fiber.all`, `race`, `both`, `map` and `timeout` of the standard library have the same shape, so the standard library itself gets W0402. User code gets it whenever the type of the value is a type parameter or contains a function type.

## Proposal

- When a declaration without a body is called, an argument whose type is Fiber, Deferred, Semaphore or Queue passes on to the return value what it holds, except the roots of scopes.
- The exception does not apply when the type of the return value at the call contains Fiber, Deferred, Semaphore or Queue. A handle taken out of a handle (`Fiber.join` of a `Fiber[Fiber[A]]`) keeps the root.
- Other roots are passed on as before. A Fiber forked from a closure that captured `log` holds `log`, and so does the value `join` returns.

```emel
fn run[A](f: fn() -> A) -> A {
  Fiber.scope(|| Fiber.join(Fiber.fork(f))) # holds nothing of the scope
}

fn leak() {
  Fiber.scope(|| Fiber.join(Fiber.fork(|| Fiber.fork(|| 1)))) # W0402: a Fiber of the scope
}
```

### What stays untracked

- A value put into a Deferred or a Queue (`Deferred.complete`, `Queue.offer`) and taken out again. The return value of `complete` and `offer` holds nothing, so the handle does not record what was put in. This is the same under the rules of `capture-checking` without this proposal: taking the value out returned only what the handle held, which is the root of the scope. The run-time checks of `[effect.escape]` and `[fiber.escape]` catch a capability that leaves through this path now.
- The change for 0.29 that removes the run-time checks MUST decide how this path is checked, together with the callbacks kept by the host (`[open.next]`).

## Specification changes

### `[effect.escape]`

Replace the item "The capture set of the return value of a function is inferred from its body. For a declaration without a body ..." with:

> - The capture set of the return value of a function is inferred from its body. For a declaration without a body (an effect operation, a trait function, an `@external` fn, the function type of a parameter), the return value holds what its arguments hold, and the return value of an operation also holds the handler. An argument whose type is Fiber, Deferred, Semaphore or Queue passes on what it holds except the roots of scopes, unless the type of the return value at the call contains one of these types (11.8).

### `[fiber.escape]`

Add after the item on W0402:

> - A value taken out of a handle (`join`, `await`, `poll`, `take`, `with_permit`, and the other functions of `Std.Fiber`, `Std.Deferred`, `Std.Queue` and `Std.Semaphore` that receive one) does not hold the root of the scope, unless its type contains Fiber, Deferred, Semaphore or Queue (8.6). A value put into a Deferred or a Queue is not tracked through it; the checks at run time catch a capability that leaves this way.

### `[open.next]`

Replace the item for 0.29 with:

> - [ ] 0.29: make E0444 the default, retire W0402 and E0425, and remove the run-time check of 8.6 (`capture-checking`). Before the run-time checks are removed, decide how a callback passed to an `@external` fn and kept by the host (12.4), and a value put into a Deferred or a Queue and taken out again (11.8), are checked

## Diagnostics

- [ ] None added. W0402 and E0444 are no longer reported for a value taken out of a handle.

## Compatibility

- `capture-checking` is not implemented yet, so no released version reports W0402. Nothing that compiles now stops compiling.
- With this change, programs that the rules of `capture-checking` would report and that are correct are no longer reported. A program in which a handle leaves its scope inside another handle is still reported.

## Implementation

- emela-types: in the analysis of capture sets, the rule for arguments of the scope types at calls of declarations without a body. Implemented in the same pull request as `capture-checking` (#642), which has not been implemented yet.

## Alternatives

### Track what a handle contains

A handle carries two sets: what the handle itself holds (the root of its scope, and what the forked function captured), and what a value taken out of it holds (its contents).

- `fork(f)`: the contents are what the return value of `f` holds.
- `deferred()` and `queue()`: the contents are the union of what every value put in by `complete`, `offer` and `try_offer` holds. The handle can be passed to other functions and completed there, so the summary of a function gets a new part: what it puts into the contents of each parameter. The contents are a set per handle made, collected over the whole body, not in order.
- `join`, `await`, `poll`, `take`: return the contents, without the root.
- A handle of handles (`Fiber[Fiber[A]]`) has contents that are themselves handles with contents, so the sets follow the type arguments.

Comparison with this proposal:

| | This proposal | Contents of handles |
|---|---|---|
| False W0402 for `join` and the like | None | None |
| A handle taken out of a handle (`Fiber[Fiber[A]]`) | Reported, by the type of the return value | Reported, by the contents |
| A value put into a Deferred or a Queue and taken out | Not tracked (run-time check) | Tracked |
| A generic function that returns a handle of its own scope through a type parameter (`both(|| Fiber.fork(g), ...)`) | Not tracked (run-time check) | Tracked |
| What a value carries | One set | Two sets per handle, nested by type arguments |
| Summary of a function | What the return value holds | Also what it puts into each parameter |
| Specification text | One sentence in 8.6 and one item in 11.8 | A new kind of element and rules for each function of the four modules |

The paths that this proposal leaves to the run-time checks exist in the rules of `capture-checking` as well. Tracking contents is the larger change, and it can be added before 0.29 if the period of `capture-checking` shows that these paths occur in real code. This proposal does not prevent it: the contents would replace the exception for scope types.

### Rules for each function of the standard library

Keep the default for declarations without a body, and state in `[api.std.fiber]`, `[api.std.deferred]`, `[api.std.queue]` and `[api.std.semaphore]` what the return value of each function holds. It gives the same results as this proposal for the current functions, but each new function that receives a handle needs its own rule, and a user's `@external` fn that receives a handle gets the default and the false W0402.

### Drop the roots of scopes from every return value of a declaration without a body

Simpler, but `Fiber.join` of a `Fiber[Fiber[A]]` would return a handle of the scope without its root, and the leak would be found only by E0425, which is retired in 0.29.

## Prior art

- Capture checking with capture sets for effect handlers and regions (see the proposal `capture-checking`).
- Separating the region of a container from the regions of the values in it: region type systems give a container type both its own region and the regions of its element type, so that taking an element out does not carry the container's region.

## Open questions

## Decision

The defaults of capture sets for declarations without a body make the value that `Fiber.join`, `Deferred.await`, `Queue.take` and `Semaphore.with_permits` return hold the root of the scope, so correct programs, and `Fiber.all`, `race`, `both`, `map` and `timeout` of the standard library, get W0402. The value taken out of a handle does not belong to the scope, so dropping the root there misses no leak that the rules of `capture-checking` would catch, and a handle taken out of a handle is still found by the type of the return value.

Tracking the contents of handles would also follow values put into a Deferred or a Queue, but it needs a second set per handle and a new part in the summary of every function. That path is not tracked by `capture-checking` either, and the run-time checks catch it until 0.29, so it is left as a decision for the change that removes those checks.
