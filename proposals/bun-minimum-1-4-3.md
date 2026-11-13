---
title: The minimum version of Bun is 1.4.3
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [target, target.js.host, cmd.build]
requires: [js-host-variable]
related: []
---

# The minimum version of Bun is 1.4.3

## Summary

The JS target assumes Bun 1.4.3 or later, not Bun 1.3 or later. Before 1.4.3, Bun does not run the work that a `beforeExit` listener schedules as Node does, and `emela test` gives different results there.

## Motivation

- `emela test` finds a test left suspended when the event loop empties (`beforeExit`), reports it as interrupted, and goes on to the next test from the listener (`[cmd.test.outcome]`, 14.5).
- Bun 1.3.x does not report an exception thrown by a timer that a `beforeExit` listener started to `uncaughtException`, and does not run the microtasks that such a listener queues. Bun 1.4.0 to 1.4.2 report the exception but do not run the microtasks. Bun fixed the microtasks in oven-sh/bun#43826, released in 1.4.3; its description says "Node runs them".
- On Bun 1.3.8, the version that `[target.js.host]` names as checked, two tests of the compiler give results different from Node: a defect in a host callback in the test after an interrupted test stops the test runner, and the test runner exits with 0 while it waits for a stalled test. Bun 1.4.3 and 1.4.4 give the same results as Node.
- `emela run` is not affected by these cases in the tests of the compiler, but the launcher relies on `beforeExit` in the same way when a release suspends while main unwinds.

Checked on 2026-11-14 with the tests `an_attempt_after_a_stalled_test_catches_a_defect` and `suspended_tests_are_awaited_or_interrupted` of `crates/emela-driver/tests/test_runner.rs`:

| Bun | Result |
|---|---|
| 1.3.8, 1.3.14 | both fail |
| 1.4.0, 1.4.1, 1.4.2 | one fails |
| 1.4.3, 1.4.4 | both pass |

## Proposal

### Meaning

- The minimum version of Bun is 1.4.3. The version check of `[cmd.build]` compares the major, minor and patch versions written by the host.

## Specification changes

### `[target]`

Replace "Node 22 or later, or Bun 1.3 or later" with "Node 22 or later, or Bun 1.4.3 or later".

### `[target.js.host]`

Replace the informative item with: "Informative: suspension and resumption, yielding, AbortSignal, and the order of virtual time (`[target.js.event-loop]`, `[target.js.abort]`) have been checked to give the same results on Node 22.14 and Bun 1.4.3 (2026-11-14). Bun before 1.4.3 does not run the microtasks that a `beforeExit` listener queues, and `emela test` gives different results on it after a test left suspended."

### `[cmd.build]`

In the item on the version check, replace the note `install Node.js 22 or later or Bun 1.3 or later, or set EMELA_JS_HOST to a newer JS host` with `install Node.js 22 or later or Bun 1.4.3 or later, or set EMELA_JS_HOST to a newer JS host`.

## Diagnostics

- [ ] E0904: the note and the minimum of Bun in the message (`needs Bun 1.4.3 or later`)

## Compatibility

- Bun 1.3.x and 1.4.0 to 1.4.2 are refused with E0904 by `emela run` and `emela test`. Bun 1.4.3 was released on 2026-10-10.

## Implementation

- emela-driver: the minimum of `Kind::Bun` in `js_host.rs` becomes 1.4.3, and the comparison reads the patch version as well (a missing part counts as 0). The note of E0904 and the help of `emela doctor` follow.
- CI: nothing changes. The `test-bun` job already runs Bun 1.4.3 (#559).

## Alternatives

- Keep Bun 1.3 and run CI on 1.4.3, as now. `[target.js.host]` names a version on which the results differ, and `emela run` and `emela test` accept a Bun on which they do.
- Work around it in the test runner: stop the runner when a test is left suspended and let the driver start it again for the remaining tests. It changes how tests are run, for a defect that Bun has fixed.

## Prior art

- oven-sh/bun#43826 (merged 2026-09-24, in Bun 1.4.3): microtasks queued by a `beforeExit` listener now run.

## Open questions

## Decision
