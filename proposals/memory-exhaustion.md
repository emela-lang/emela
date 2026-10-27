---
title: Behavior when memory is exhausted
authors: [isoshigi]
status: draft
pr:
issue:
decided:
touches: [dyn.memory, target.unspecified, error.defect, fiber.cause, target.wasm.run]
requires: []
related: []
---

# Behavior when memory is exhausted

## Summary

The specification does not say what happens when a program cannot obtain the memory it needs. This proposal leaves the result implementation-defined, including whether it is a defect (and therefore whether the releases run and whether the failure is observable at `Fiber.attempt`), what is written and the exit status. A program MUST NOT depend on any of these. A future proposal may fix them.

## Motivation

Allocation failure is not mentioned in the specification:

- `[dyn.memory]` says only that the time of freeing cannot be observed and that it is unspecified.
- `[target.unspecified]` lists the memory representation, traits, continuations and fiber order, but not allocation failure.
- `[error.defect]` lists the sources of a defect, and allocation failure is not one of them. A defect is documented to run releases and to be observable at `Fiber.attempt` (`[error.defect]`, `[fiber.cause]`).

The targets therefore behave in ways that no section describes.

- C: `em_alloc` calls `em_out_of_memory` when `malloc` returns `NULL` (`crates/emela-codegen-c/src/emela_runtime.c:54-56`). It writes `emela: out of memory` to standard error and calls `abort()` (`crates/emela-codegen-c/src/emela_runtime.c:49-52`). The process ends by `SIGABRT` (exit status 134); no release runs and `Fiber.attempt` cannot catch it.
- JS: there is no allocator or out-of-memory handling; memory is managed by the host's garbage collector. Exhausting the V8 heap aborts the process. A size limit instead throws a host error such as `RangeError: Invalid string length`: at the top level it is not a `Defect`, so the launcher prints its stack and exits with 1 (`crates/emela-driver/src/run.rs:227-233`). Inside `Fiber.attempt` it is caught and turned into a `Die` cause (`crates/emela-codegen-js/src/runtime.mjs:853-856`), and a `with` body unwinds through `finally` blocks, so releases run on that path (`crates/emela-codegen-js/src/emit.rs:457-459`).
- WASM (before the implementation below): `heap_reserve` ignored the result of `memory.grow` and stored the new bump pointer unconditionally (`21b4aca:crates/emela-codegen-wasm/src/emit/runtime/alloc.rs:131-155`). `G_HEAP` then pointed past the end of linear memory, and the next store or copy trapped with an out-of-bounds memory access. The program still ended abruptly and not as a defect, but the reported failure named an unrelated store instead of the allocation that failed.

C and WASM agree on an abrupt end of the process, and JS agrees for a fatal heap exhaustion. The JS size-limit path differs: it can be observed at `Fiber.attempt` and can run releases. No target promises a message or an exit status. The specification should state only what they have in common and leave the rest implementation-defined.

## Proposal

### Meaning

If a program cannot obtain the memory it needs, the result is implementation-defined. In particular, whether it is a defect is implementation-defined, and with that whether the releases of handlers run and whether the failure can be observed at `Fiber.attempt`; what, if anything, is written and the exit status are implementation-defined too. A program MUST NOT depend on any of these.

### Typing rules

None. Allocation failure has no representation in types.

## Specification changes

### `[dyn.memory]`

Add this bullet:

- If a program cannot obtain the memory it needs, the result is implementation-defined (`[target.unspecified]`), including whether it is a defect and therefore whether the releases of handlers run and whether the failure can be observed at `Fiber.attempt` as a `Die` cause (`[fiber.cause]`). What, if anything, is written and the exit status are implementation-defined as well. A program MUST NOT depend on any of these.

### `[target.unspecified]`

Add this bullet:

- The result of a program that cannot obtain the memory it needs, including whether it is a defect, whether releases run, whether the failure can be observed at `Fiber.attempt`, what, if anything, is written, and the exit status

### `[error.defect]`

Add this sentence after the table, before the bullet list:

Whether resource exhaustion is a defect is implementation-defined (`[dyn.memory]`); programs MUST NOT depend on its classification.

### `[fiber.cause]`

Add this sentence after the table:

Whether an allocation failure is observable as a `Die` cause is implementation-defined (`[dyn.memory]`).

### `[target.wasm.run]`

Add this bullet:

- In this target, a program that cannot obtain the memory it needs takes the implementation-defined result of `[dyn.memory]` rather than one of the defect, error or normal endings above.

## Diagnostics

- [x] None. Allocation failure has no diagnostic.

## Compatibility

No program that compiles now stops compiling. The specification does not currently promise any behavior when memory is exhausted, so no defined behavior changes. A JS program that hits a host size-limit error inside `Fiber.attempt` currently observes a `Die` cause; the proposal leaves that implementation-defined rather than requiring a change. The WASM output changes: a failed `memory.grow` now traps at the runtime's own `unreachable` after a best-effort report, instead of setting `G_HEAP` past the end of memory and trapping at the next store. The emitted bytes therefore change, but no program's defined behavior does. C and JS are not changed.

## Implementation

- `crates/emela-codegen-wasm/src/emit/runtime/alloc.rs`: `setup` writes the out-of-memory line into static data. `heap_reserve` takes the end offset as an i64 so that the sum of two u32 offsets cannot wrap at 4 GiB; it rejects an end past `u32::MAX` and an end below `G_HEAP`, compares `memory.size * PAGE` as an i64, and on a failed `memory.grow` calls `write_stderr` with the line (best effort) and traps before changing `G_HEAP`. In this backend the failure does not set `G_DEFECT`, does not run `finish`, and does not run releases.
- `crates/emela-codegen-wasm/tests/memory.rs`: structural tests check the grow result comparison, the i64 arithmetic and the guards, the `write_stderr` callee and the data segment, and that `G_HEAP` is not moved on the failure path; runtime tests use `-W max-memory-size` and `-W trap-on-grow-failure=n` and check that the program does not continue, including inside `Fiber.attempt`.
- `crates/emela-codegen-wasm/tests/bytes_stable.rs`: the pinned component sizes and hashes are updated.
- C and JS are not changed.

## Alternatives

- Require allocation failure to be a defect on every target. Rejected: the process may have no memory left to build the defect object or to run releases, and C and WASM currently do not run releases. It would also make the failure observable at `Fiber.attempt`, which no target can do reliably.
- Return an error or a failure value. Rejected: constructing the value needs memory, the error type would have to appear in every signature, and `fail` values are ordinary allocated values.
- Leave the behavior completely unspecified without changing the backend. Rejected: the current WASM failure reports an out-of-bounds store rather than the allocation that failed, which misleads a person reading the trap even though no program can depend on it.
- Specify "no releases and not observable at `Fiber.attempt`" for every target. Rejected for now: the JS size-limit path violates it today, and the author's policy is to align with C and JS where they agree and leave the rest undefined. Normalizing JS would be a separate decision.
- Trap without writing a message. This is what an implementation may still do; the message is best effort and is not required by the specification.
- Specify the message and the exit status now. Not in this proposal: they stay implementation-defined, and a future proposal may narrow them.

## Prior art

Managed runtimes commonly make allocation failure unrecoverable or only partly recoverable: Rust's default allocation error handler aborts the process, Go's runtime reports a fatal `out of memory` error that a program cannot recover from, and Java throws `OutOfMemoryError`, which can be caught. Emela's errors and defects are values, so a catchable allocation failure would need a representation the language does not have.

## Open questions

## Decision
