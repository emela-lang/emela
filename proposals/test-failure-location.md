---
title: Show where a test failed
authors: [Alliana]
status: accepted
pr: 413
issue:
decided: 2026-10-24
touches: [cmd.test.output, cmd.test.outcome, test.assert]
requires: []
related: []
---

# Show where a test failed

## Summary

A defect from a failed assert carries the source location of the assert expression (the path, the line and the column). `emela test` shows that location in the details of the failure.

## Motivation

The details of a failure (`[cmd.test.outcome]`) show only the message of the defect.

```
---- adds_wrong ----
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4
```

They do not show which assert, in which file and on which line, failed.

- A person finds the location by searching the source for the text of the expression (`add(1, 2) == 4`). When several asserts have the same text, the search does not tell which one failed.
- A coding agent searches the source before it can fix the failure, which costs turns and context.
- Diagnostics (`[diag]`) show their location with a code frame; test failures do not.

The location of an assert is known at compile time. The message of an assert (`assertion failed: <source text>`) is also built at compile time (`[test.assert]`). Adding the location needs no tracking at run time.

## Proposal

- A defect from a failed assert has the location of the assert expression.
- `emela test` shows the location in the details of a failure that has one. In `plain`, the line ` --> <path>:<line>:<column>` follows the line `---- <name> ----`.

```
---- adds_wrong ----
 --> src/main.emel:13:3
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4
```

- The path is relative to the project directory (in single-file mode, to the directory that contains the file). For an assert in a dependency Pome, the path is the path of the Pome's source file, not of the build output (`emela_pomes/...`). Lines and columns are counted from 1. Columns are counted as in diagnostics.
- A defect that reaches `main` under `emela run` is shown without a location. The line `defect: <message>` of `[error.defect]` does not change.
- Defects other than those from asserts (`panic`, division by zero, exceptions from foreign functions, and so on), errors, and interruptions have no location. The locations of `panic` and `todo` are left to a separate proposal.

If `test-reporters` is accepted, `agent` prints `FAIL <name> <path>:<line>:<column>`, and `pretty` prints a code frame that includes the location.

## Specification changes

### `[test.assert]`

Add:

```md
- A defect from a failed assert has the location of the assert expression: the path of the file, the line and the column. The location is shown by the test runner (`[cmd.test.outcome]`). It is not part of the defect message.
```

### `[cmd.test.outcome]`

Add:

```md
- A failure has a location when it is a defect from a failed assert (`[test.assert]`). Other failures have no location.
- A location is shown as `<path>:<line>:<column>`. The path is relative to the project directory, and in single-file mode to the directory that contains the file. For an assert in a dependency Pome, the path is the path of its source file, not of the build output.
- The location is shown only by the test runner. A defect that reaches `main` is displayed as in `[error.defect]`, without a location. Lines and columns are counted from 1, as in diagnostics (`[diag]`).
```

### `[cmd.test.output]`

Replace the item on the details of failures with the following, and update the example to match.

```md
- If there is at least one failure, then after a blank line and `failures:`, the command prints, for each failed test, a blank line, the line `---- <name> ----`, the line ` --> <location>` if the failure has a location, and the details of the failure ("Kinds of results" below). Then it prints a blank line, `failures:`, and the names of the failed tests, one per line, each preceded by four spaces.
```

## Diagnostics

None.

## Compatibility

- The meaning of the language does not change. Defect messages do not change either (the location is kept outside the message).
- The `plain` output of `emela test` gains one line per failure. Tests that compare the output (`crates/emela-driver/tests/snapshots/test_runner__*.snap`) need updating.

## Implementation

- `crates/emela-lir/src/from_core.rs` lowers the defect of an assert to `Term::Defect(message)`. Add the location there (`Term::Defect` carries the location as a constant). `crates/emela-core/src/mono.rs` changes in the same way.
- Give the defects of the JS, C and WASM runtimes a location, and add `location` to the JSON of the test runner's result line (`Event::Result`).
- The location is a compile-time constant, so no tracking at run time is needed.

## Alternatives

### Put the location in the defect message

For example `assertion failed: add(1, 2) == 4 at src/main.emel:13:3`. The runtimes would not change, but the display of a defect that reaches `main` under `emela run` and the message received by `Fiber.attempt` would change too. Reporters could not handle the location separately (`pretty` could not use it for a code frame).

### Include `panic` and `todo`

`panic` is a function. Knowing where it was called needs a mechanism that passes the caller's location as an implicit argument, which is a change to the language. The compiler sees the form of an assert expression (`[test.assert]`), so asserts need no such mechanism. `panic` and `todo` are left to a separate proposal.

## Prior art

- Test runners that show the source location and a code frame of the failed assert (Vitest; `cargo test` with `panicked at src/lib.rs:10:5`).
- Functions that receive the caller's location implicitly (Rust's `#[track_caller]`). This is the mechanism `panic` would need to carry a location.

## Open questions

## Decision

A test failure does not say which assert failed, so people and agents search the source for the text of the expression, and the search is ambiguous when several asserts have the same text. Diagnostics show their location; test failures should too.
