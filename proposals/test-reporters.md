---
title: Separate the output of emela test for people at a terminal and for coding agents
authors: [Alliana]
status: implemented
pr: 412
issue:
decided: 2026-10-24
touches: [cmd.test.output, cmd.test.reporter (new), test.command]
requires: []
related: []
---

# Separate the output of emela test for people at a terminal and for coding agents

## Summary

`emela test` displays its results with one of three reporters: `pretty` for a person reading a terminal, `plain` that keeps the current line format, and `agent` for a coding agent. `--reporter` selects one; when it is omitted, the environment decides. The layout of `pretty` is left to the implementation, not the specification.

## Motivation

The current output (`[cmd.test.output]`) has a single format, and it suits neither people nor agents.

```
running 3 tests
test adds ... ok
test adds_wrong ... FAILED
test names ... FAILED

failures:

---- adds_wrong ----
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

---- names ----
defect: assertion failed: "abc" == "abd"
  left: "abc"
 right: "abd"

failures:
    adds_wrong
    names

test result: FAILED. 1 passed; 2 failed; 0 filtered out
```

For a person reading a terminal:

- There is no color. `ok` and `FAILED` look the same, and failures have to be found by eye.
- When `left` and `right` are long strings or large values, it is hard to see where they differ.
- Tests are not grouped by file; they can be told apart only by the prefix of their names (`Util.`).
- No elapsed time is shown, so slow tests go unnoticed.

For a coding agent:

- Every passed test prints a line. With hundreds of tests, hundreds of lines enter the context to report one failure.
- The name of a failure appears twice (in the heading of its details and in the list `failures:` at the end).
- What passed tests write to standard output is passed through as is.

Both people and agents run `emela test`, and they want opposite things: people want more information, agents want less. A single format cannot serve both.

The missing location of a failure (the path, the line and the column) concerns every reporter, so it is a separate proposal (`test-failure-location`, #413).

## Proposal

### Reporters

`emela test` displays its results with one reporter.

| Reporter | Reader | What the specification fixes |
|---|---|---|
| `pretty` | A person at a terminal | Only when color is used, and that every failure is shown with its details. The layout is left to the implementation |
| `plain` | Logs, CI, tests that compare the output | The current format. The same input gives the same output |
| `agent` | A coding agent | The format. Only failures are printed; neither the lines of passed tests nor their standard output are printed |

The reporter is the first of the following that applies.

1. The one given by `--reporter <pretty|plain|agent>`.
2. `agent`, if the command runs inside a coding agent. It does when the environment variable `AI_AGENT` is not empty. An implementation MAY also look at other environment variables that agents set.
3. `pretty`, if standard output is a terminal.
4. `plain` otherwise.

### An example of `pretty`

The layout is not specified. The following is one example.

```
 src/main.emel
   ✓ adds                                     0ms
   × adds_wrong                               1ms
   × names                                    0ms

──── FAIL  adds_wrong ────
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

──── FAIL  names ────
defect: assertion failed: "abc" == "abd"
  - left:  "abc"
  + right: "abd"
               ^

 Tests  2 failed | 1 passed (3)
 Duration  12ms
```

- Tests are grouped by file; passed tests are marked `✓` (green) and failed tests `×` (red).
- For a comparison assert, where the displays of the two sides differ is marked.
- The counts and the elapsed time are summarized at the end.

### The format of `agent`

When every test passed, only the summary line is printed. It is printed so that the agent can confirm that the tests ran.

```
test result: ok. 3 passed; 0 failed; 0 filtered out
```

When there are failures:

```
FAIL adds_wrong
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

FAIL names
defect: assertion failed: "abc" == "abd"
  left: "abc"
 right: "abd"

test result: FAILED. 1 passed; 2 failed; 0 filtered out
```

- The summary line has the same form as the last line of `plain`. Readers that rely on the current format (for example, looking for the line `test result:`) keep working.
- Neither the leading `running N tests` nor the lines of passed tests are printed.
- What a failed test wrote to standard output is printed in full after the details of its failure. What passed tests wrote is not printed. Truncating by line count can cut off the part that matters, and it would need truncation rules in the specification.
- No color, mark or elapsed time is printed.
- The name of a failure is printed once.
- If `test-failure-location` is accepted, the location follows `FAIL <name>` (`FAIL adds_wrong src/main.emel:13:3`).

## Specification changes

### `[cmd.test.reporter]` (new)

```md
## Reporters `[cmd.test.reporter]`

`emela test` displays its results with one reporter.

| Reporter | Output |
|---|---|
| `pretty` | Results for a person reading a terminal. Its layout is not specified |
| `plain` | The line format of `[cmd.test.output]`. The same input gives the same output |
| `agent` | Only the failures and one summary line |

- `--reporter <pretty|plain|agent>` selects the reporter. Another name is an error in the command-line arguments, reported in the same way as an unknown value of `--target`, and no test is run.
- When `--reporter` is omitted, the reporter is the first that applies of the following.
  1. `agent`, if the command runs inside a coding agent.
  2. `pretty`, if standard output is a terminal.
  3. `plain`.
- The command runs inside a coding agent when the environment variable `AI_AGENT` is set and not empty. An implementation MAY also treat other environment variables that coding agents set as meaning the same.
- Every reporter writes to standard output. The exit code (`[cmd.test.exit]`) does not depend on the reporter.
- Diagnostics found by checking are written to standard error as in `[diag]`, whatever the reporter.
```

### `[cmd.test.output]`

Make the current section the definition of `plain`. Replace its opening sentence with the following, and keep the rest.

```md
The `plain` reporter (`[cmd.test.reporter]`) prints the results to standard output in the following form.
```

Add `pretty` and `agent` at the end of the section.

````md
### pretty

- Every failed test is shown with the details of the failure (`[cmd.test.outcome]`).
- Color is used only when standard output is a terminal and the environment variable `NO_COLOR` is empty or unset.
- The layout, marks, colors, and whether the elapsed time is shown are not part of the specification, and an implementation MAY change them. An example:

```
 src/main.emel
   ✓ adds                                     0ms
   × adds_wrong                               1ms

──── FAIL  adds_wrong ────
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

 Tests  1 failed | 1 passed (2)
 Duration  12ms
```

### agent

```
FAIL adds_wrong
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

test result: FAILED. 1 passed; 1 failed; 0 filtered out
```

- Nothing is printed for a passed test. The header line `running N tests` is not printed.
- For each failure, in run order, the line `FAIL <name>` is printed, then the details of the failure (`[cmd.test.outcome]`), then what the test wrote to standard output. Failures are separated by a blank line.
- What a passed test writes to standard output is not printed. What a failed test writes is printed in full, without truncation.
- The last line is the summary line of `plain`, `test result: <ok|FAILED>. <number passed> passed; <number failed> failed; <number excluded by the filter> filtered out`. It is printed even when every test passed. When a failure was printed, a blank line precedes it.
- No color, mark or elapsed time is printed. The same input gives the same output.
````

### `[test.command]`

Replace the item "Results go to standard output: ..." with the following.

```md
- Results go to standard output, in the form of the selected reporter (`[cmd.test.reporter]`).
```

## Diagnostics

None. An unknown name given to `--reporter` is an error in the command-line arguments, as is an unknown value given to `--target`, and has no diagnostic code.

## Compatibility

- The meaning of the language does not change.
- Running `emela test` at a terminal looks different. Output to a pipe or a file (CI, `emela test | tee`) stays `plain`, in the current format.
- Running in an environment where `AI_AGENT` is set, or inside an agent, selects `agent`: the lines of passed tests and `running N tests` are no longer printed. The summary line is the same as in `plain`, so reading the line `test result:` keeps working. `--reporter plain` restores the current format.

## Implementation

- Add the reporters to `crates/emela-driver/src/testing/report.rs`. The current functions become `plain`.
- Tests run one at a time, and the driver reads the program's standard output line by line and tells result lines apart (`run_tests` in `crates/emela-driver/src/testing.rs`). If the driver keeps the lines before the next result line as belonging to the running test, `agent` can print each test's standard output separately. The runtimes need no change. Standard error is currently inherited, so it cannot be split per test.
- The driver can measure the elapsed time for `pretty` as the difference between the arrival times of result lines. The runtimes need no change.
- Whether standard output is a terminal is given by `std::io::IsTerminal`. The decision on color for standard error is in `crates/emela-cli/src/main.rs`.
- The environment variables looked at besides `AI_AGENT` are listed in the implementation (`CLAUDECODE`, `GEMINI_CLI`, `CODEX_SANDBOX`, `CURSOR_AGENT`, and so on).

## Alternatives

### Keep one format and only add color

Adding color to `plain` is a small change, but it leaves the problem of reducing lines for agents. The lines of passed tests show progress to a person at a terminal, but an agent does not need them. One format cannot satisfy both.

### Do not detect agents; require `--reporter agent`

The specification would not depend on environment variables. But agents usually run plain `emela test`, so a flag they must pass would go unused. Projects would have to write `--reporter agent` into AGENTS.md or similar files.

### List the environment variables of each product in the specification

The specification would tell in which environments `agent` is selected. But the specification would change every time a new agent product appears. The specification names only `AI_AGENT`, which does not depend on a product, and leaves the rest to the implementation.

### Specify the layout of `pretty`

Every change to the layout would need a proposal. Output for people is something to keep improving, and `plain` and `agent` serve the uses that compare the output.

### Add a machine-readable format (JSON) and have agents read it

JSON has structure, but it takes more characters to say the same thing, and an agent reads it as text in the end. What an agent needs is which tests failed and why, and the `agent` format states that in the fewest characters. JSON serves other uses such as editor integration, and is left to a separate proposal.

### Select the reporter with an environment variable `EMELA_TEST_REPORTER`

Having it besides `--reporter` adds precedence rules. For now there is only the flag.

## Prior art

- Test runners with interchangeable reporters (Vitest's `default`, `verbose`, `dot`, `minimal`, `json`, and others). When no reporter is given and Vitest detects that it runs inside an AI coding agent, it uses `minimal` (also named `agent`). `minimal` prints only failures and their errors, and prints neither the console output of passed tests nor the summary. The detection uses the list of environment variables in `std-env` (`AI_AGENT`, `CLAUDECODE`, `GEMINI_CLI`, `CODEX_SANDBOX`, `CURSOR_AGENT`, and others).
- The `NO_COLOR` convention.
- CLIs that change their format by whether the output goes to a terminal (color and layout at a terminal, plain lines to a pipe).

## Open questions

## Decision

`emela test` is run by people at a terminal and by coding agents, and they want opposite things from its output: people need color, grouping and the place where two values differ, while agents need only the failures, with nothing that fills their context. A single format serves neither, so the output is split into reporters selected by the reader.
