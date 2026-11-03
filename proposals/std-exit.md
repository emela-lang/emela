---
title: Move attempt and Exit to Std.Exit
authors: [Alliana]
status: draft
pr: 489
issue:
decided:
touches: [fiber, fiber.cause, error.defect, lib.layers, api.std.fiber, api.std.exit (new), api.std.cause, target.c.unsupported]
requires: []
related: []
---

# Move attempt and Exit to Std.Exit

## Summary

`attempt` and the enum `Exit` (`Success`, `Failure`) move from `Std.Fiber` to a new module `Std.Exit`. `Fiber.attempt(f)` becomes `Exit.attempt(f)`. `Cause` stays in `Std.Cause`, and `Fiber.await` returns the `Exit` of `Std.Exit`. The old names are removed without a deprecation period.

## Motivation

`attempt` starts no fiber. It runs `f` on the spot and turns its Fail and Die into a value (`[fiber.cause]`). It has nothing to do with concurrency, but a program that only wants to turn a defect into a value has to import the concurrency module and write `Fiber`:

```emel
import Std.Cause
import Std.Fiber.{Fiber, Success, Failure}

fn transaction[A, E](body: fn() -> A fails E use { Sql }) -> A fails E use { Sql } {
  (use Sql).execute("BEGIN")
  Fiber.attempt(body) match {
    Success(value) -> {
      (use Sql).execute("COMMIT")
      value
    }
    Failure(cause) -> {
      (use Sql).execute("ROLLBACK")
      Cause.raise(cause)
    }
  }
}
```

A reader of a transaction stops at `Std.Fiber` and looks for the fibers. In the repositories of emela-lang, every program that imports `Std.Fiber` outside the compiler's own tests uses only `attempt`: the transactions of `x/sql`, `deploy/control/src/sqlite.emel`, the property runner of `brix` (`src/pbt/property.emel`), and `launch` of `Std.Http` (`[lib.http-service]`).

The dependency between the modules also points the wrong way. `Exit` describes how a computation ended, and `Fiber.await` is one of its producers. Moving `Exit` and `attempt` out makes the dependency one-directional: `Std.Fiber` → `Std.Exit` → `Std.Cause`.

## Proposal

```emel
import Std.Cause
import Std.Exit.{Success, Failure}

fn transaction[A, E](body: fn() -> A fails E use { Sql }) -> A fails E use { Sql } {
  (use Sql).execute("BEGIN")
  Exit.attempt(body) match {
    Success(value) -> {
      (use Sql).execute("COMMIT")
      value
    }
    Failure(cause) -> {
      (use Sql).execute("ROLLBACK")
      Cause.raise(cause)
    }
  }
}
```

### Syntax

No change.

### Meaning

- The new module `Std.Exit` declares `enum Exit[A, E] { Success(A), Failure(Cause[E]) }` and the intrinsic `attempt[A, E, R](f: fn() -> A fails E use R) -> Exit[A, E] use R`. Their meaning does not change.
- `Std.Fiber` no longer declares `Exit` or `attempt`. `Fiber.await` returns `Exit[A, {}]` of `Std.Exit`.
- `import Std.Exit.{Success, Failure}` brings the module `Exit` into scope as well (`[module.resolution]`), so `Exit.attempt` needs no other import.

### Typing rules

No change.

## Specification changes

### `[fiber]`

Replace the last sentence of the first paragraph with:

> The concurrency tools are in `Std.Fiber` (Scope, Fiber), `Std.Exit` (Exit, attempt), `Std.Cause` and `Std.Deferred`.

In the paragraph that follows, `attempt` stays in the list of the `@intrinsic` foundation.

### `[fiber.cause]`

Replace the code block after "After raising it again, ..." with:

```emel
pub enum Exit[A, E] { Success(A), Failure(Cause[E]) } # Std.Exit

pub opaque type Cause[E] # Std.Cause
pub enum CauseKind { Fail, Die, Interrupt }
pub fn kind[E](c: Cause[E]) -> CauseKind
pub fn to_string[E](c: Cause[E]) -> String # error: ... / defect: ... / interrupted
pub fn raise[A, E](c: Cause[E]) -> A fails E # Fail fails, Die is a defect, Interrupt unwinds as an interruption
```

In the table of the four ways to receive a result, the row of `attempt` becomes:

> | `Exit.attempt` | `attempt[A, E, R](f: fn() -> A fails E use R) -> Exit[A, E] use R` | Runs f in place and turns Fail and Die into values. It starts no fiber |

In the example, replace `Fiber.attempt(fn() { handle(conn) })` with `Exit.attempt(fn() { handle(conn) })`.

### `[error.defect]`

> - A defect can be observed only at a boundary. A boundary is the top level of the program, the test runner, or `Exit.attempt` (11.5).

### `[lib.layers]`

In the row of the effect layer, the contents become "State, Fiber, Exit, Cause, Deferred (0.20). ...". The rest of the row does not change.

### `[api.std.fiber]`

Remove the rows `Exit` and `attempt`. The row of `await` becomes:

> | `await` | `await[A](fiber: Fiber[A]) -> Exit[A, {}] use Async` | intrinsic | Waits for the child to finish and returns how it ended (`Exit` of `Std.Exit`) |

### `[api.std.exit]` (new)

Placed between `[api.std.fiber]` and `[api.std.cause]`:

> ### Std.Exit (`std/exit.emel`) `[api.std.exit]`
>
> How a computation ended, and `attempt`, which turns errors and defects into that value (`[fiber.cause]`). Used with `import Std.Exit.{Success, Failure}`.
>
> | Declaration | Signature | Implementation | Description |
> |---|---|---|---|
> | `Exit` | `enum Exit[A, E] { Success(A), Failure(Cause[E]) }` | Emela | How a computation ended |
> | `attempt` | `attempt[A, E, R](f: fn() -> A fails E use R) -> Exit[A, E] use R` | intrinsic | Runs `f` in place and turns Fail and Die into values. Starts no fiber. Does not catch an interruption of the calling fiber itself |

### `[api.std.cause]`

> impl (`[trait.derive]`): CauseKind has `derive Eq, Show`. Cause holds functions, so it does not satisfy Eq, Ord, or Show (display it with `to_string` or dbg). For the same reason, Exit of Std.Exit does not satisfy them either.

### `[target.c.unsupported]`

> ... This defect can be caught with `Exit.attempt`, and the releases of the withs on the way also run.

## Diagnostics

None added, changed or removed. `Fiber.attempt` and `import Std.Fiber.{Exit}` (and `Success`, `Failure`) become the existing E0212.

## Compatibility

Programs that use `Fiber.attempt`, or import `Exit`, `Success` or `Failure` from `Std.Fiber`, stop compiling (E0212). The fix is mechanical: write `Exit.attempt`, and import `Exit`, `Success` and `Failure` from `Std.Exit`. Programs that use only `Fiber.await` and match on `Success` and `Failure` import them from `Std.Exit` too.

This is a breaking change without a deprecation period. An enum cannot be re-exported (`[module.resolution]`: imported names are not re-exported), so `Std.Fiber` could keep at most a deprecated `attempt` returning the `Exit` of `Std.Exit`, while the imports of `Success` and `Failure` from `Std.Fiber` would break anyway. Before 1.0 a single break is preferred to a rule kept for a period, as when match became postfix only.

Outside this repository, `x/sql`, `deploy/control/src/sqlite.emel` and `brix` (`src/pbt/property.emel`) need the rewrite. `tests/laws` depends on `brix`, so it passes only after a release of `brix` with the rewrite and an update of its `Pome.lock`.

## Implementation

- emela-core: add `lib/std/exit.emel` and register `Std.Exit` before `Std.Fiber`. Move `attempt` to `("Std.Exit", "attempt")` in the lookup of the concurrency intrinsics. The Exit enum is found from the return type of the intrinsic, so lowering and the IR do not name the module.
- emela-codegen-emb: the check of the enum `Std.Fiber.Exit` by name becomes `Std.Exit.Exit`.
- The JS, C and WASM backends refer to the operation, not to the module. Only comments and the name in a descriptor of the C runtime (`"Fiber.attempt"`) change.
- `Std.Http` (`launch`), the examples, the test programs and the tests that hold Emela source are rewritten mechanically.

## Alternatives

- Keep `attempt` in `Std.Fiber`. Rejected: the module of concurrency stays a requirement of code that has no concurrency, and `Exit` keeps depending on the module that produces it.
- Move `attempt` to `Std.Cause` (`Cause.attempt`). Rejected: `attempt` returns an `Exit`, not a `Cause`, and `Exit` would have to move to `Std.Cause` too, making it the module of everything about failure.
- Put `attempt` and `Exit` in the Prelude. Rejected: turning a defect into a value is meant to happen at a few boundaries (`[error.defect]`), and an import keeps those places visible.
- Keep a deprecated `Fiber.attempt` for one minor version. Rejected in "Compatibility": the imports of the variants break anyway.

## Prior art

- Effect values that describe how a computation ended, separate from the concurrency primitives: `Effect.exit` of Effect-TS returns an `Exit` that holds a `Cause`, and `Fiber.await` returns the same `Exit`.
- ZIO separates `ZIO#exit` (an `Exit` of the effect, without a fiber) from `Fiber#await` in the same way.

## Open questions

## Decision
