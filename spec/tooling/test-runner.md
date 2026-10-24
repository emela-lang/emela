# Test runner `[cmd.test]`

This appendix defines the externally visible behavior of `emela test`. How to write test functions and their constraints, and test names and filters, are in `[test]`.

## Targets `[cmd.test.target]`

- The path of `emela test [path] [filter]` is determined by the same rules as `emela check` (`[cmd.build]`). It defaults to `.`.
- In a project, the command runs the test functions of the modules in the source root and the test root (`<project>/test`). Tests in modules of the test root are also named by the rules of `[test.command]`.
- In single-file mode, only the tests of the modules reachable from the entry through imports are run.
- Test functions of dependency Pomes are not run (`[module.pome]`).
- The filter selects the tests whose names contain it as a substring (case-sensitive) (`[test.command]`).
- If checking finds errors, no test is run, the diagnostics are printed to standard error, and the command ends with exit code 1.
- Every selected test function is either run, or reported with the reason it cannot be run. A test function is never dropped silently. A test function whose inferred type still has type parameters is run with the default instantiation (an ordinary type parameter becomes `Never`, a set type parameter the empty set), on every target.
- `--target <js|c|wasm>` selects the build target. When it is omitted, it is determined as for `build` (`[cmd.build]`). The results are displayed the same way for every target.

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

## Output format `[cmd.test.output]`

The `plain` reporter (`[cmd.test.reporter]`) prints the results to standard output in the following form.

```
running 3 tests
test adds ... ok
test Util.contains ... ok
test Util.new ... FAILED

failures:

---- Util.new ----
defect: assertion failed: left != right
  left: "a"
 right: "a"

failures:
    Util.new

test result: FAILED. 2 passed; 1 failed; 0 filtered out
```

- The first line is `running N tests`. If N is 1, it is `running 1 test`.
- Each time a test finishes, one line, `test <name> ... ok` or `test <name> ... FAILED`, is printed. Tests are run one at a time, in order.
- If there is at least one failure, then after a blank line and `failures:`, the command prints, for each failed test, a blank line, the line `---- <name> ----`, and the details of the failure ("Kinds of results" below). Then it prints a blank line, `failures:`, and the names of the failed tests, one per line, each preceded by four spaces.
- Finally, it prints a blank line and the summary line `test result: <ok|FAILED>. <number passed> passed; <number failed> failed; <number excluded by the filter> filtered out`. If there are no failures, it is `ok`.
- The elapsed time is not printed, so that tests that compare output get the same output from the same input.
- What the program writes to standard output while tests run appears as is between the result lines. The program's standard error goes to standard error as is. Standard input is not passed to the program.

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

## Kinds of results `[cmd.test.outcome]`

| Result | When it happens | Details of the failure |
|---|---|---|
| ok | The test function returned a value | (not printed) |
| defect | The test function stopped with a defect (`[error.defect]`, `[test.assert]`) | `defect: <message>` |
| error | An unhandled error came out of the test function (`[test.constraints]`) | `error: <display of the error value>` |
| interrupted | Ended while suspended without being resumed (`[dyn.suspend]`) | `interrupted` |
| Early exit | The program exited while tests were running | `the test runner exited with code <exit code> while running this test` |

- The display of an error is the same as for an error that reached `main` (`[dyn.error]`). If the error type satisfies Show, the inspect display (`[trait.builtin]`) is printed. Otherwise only the name of the error is printed.
- A test function that can suspend is awaited until it finishes. If nothing is left to wait for while it has not been resumed, the result is interrupted, and the runner proceeds to the next test. Also, when, inside a test function that does not suspend, a continuation is discarded on the spot without being resumed, the result is interrupted.
- An exception thrown inside a host callback while tests are running is a defect of the running test. Continuations of finished tests (a resume called later and so on) are not advanced.
- On an early exit (when an external function terminates the process and so on), the test that was running fails. If tests after it did not run, `; 1 test after it did not run` or `; N tests after it did not run` is appended after the details line. Tests that did not run get no result line.
- After a test with defect, error, or interrupted, the remaining tests continue.

## Exit code `[cmd.test.exit]`

- 1 if there is at least one failure, and 0 otherwise. It is 0 even with zero tests.
- 1 if the diagnostics include an error.

## Output directory `[cmd.test.out-dir]`

- The test output is written to `<project>/target/emela/test` for a project, and to `target/emela/test` in the directory that contains the file in single-file mode. If the target is `c`, the directory is `test-c`, and if `wasm`, `test-wasm`. `--out-dir` changes it.
- The test output exports the selected test functions, and is separate from the normal build output. It does not overwrite the normal build output (`target/emela/js` and so on).
- For the JS target, the command writes to the output directory `main.mjs`, which exports the test functions, the runtime `emela_runtime.mjs`, and the launcher module `emela_test.mjs`, which runs the tests. `emela test` runs the launcher module with node. Test functions of modules other than the entry are exported with the test name (`Util.in_util`) as a string name.
- For the WASM target, the command writes a single `main.wasm` that bundles the selected test functions to the output directory. `emela test` runs it with `wasmtime run -S p3=y main.wasm` (`[target.wasm.run]`). The test functions are run one at a time, in order.
- Files that `@external` points to are copied the same way as in a normal build (`[cmd.build]`). Files other than `.emel` in the test root are copied under `emela_tests/` in the output directory, in the same layout as from the test root.
