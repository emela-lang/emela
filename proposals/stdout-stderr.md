---
title: Stdout and Stderr in Std.Process, the output streams of the process
authors: [Alliana]
status: implemented
pr: 566
issue: 582
decided: 2026-11-15
touches: [api.std.process, api.std.console, api.std.platform, lib.layers, cmd.build, target.native.run, target.wasm.external, open.list]
requires: [std-process]
related: []
---

# Stdout and Stderr in Std.Process, the output streams of the process

## Summary

Add to `Std.Process` two effects for the output streams of the process, `Stdout` and `Stderr`. Each has two operations: `print`, which writes without a line ending, and `print_line`, which writes a line ending after the text. They do not fail and are Immediate. The system handlers are `SystemStdout` and `SystemStderr`, and they are part of `Platform`. The test handlers `TestStdout` and `TestStderr` append what is written to `State[StdoutText]` and `State[StderrText]`. `Std.Console` does not change: `Console` writes lines meant for people, and `Stdout` and `Stderr` are the streams of the process.

```emel
import Std.Platform.{Platform}
import Std.Process.{Stdout}

fn main() {
  with Platform {
    (use Stdout).print("Hello, ")
    (use Stdout).print_line("alice!")
  }
}
```

## Motivation

- There is no way to write without a line ending (`[open.list]` #45). A prompt (writing `name? ` and waiting for input on the same line) and output that grows on one line cannot be written. Now that `Stdin` is in `Std.Process` (`std-process`), prompts are needed right away.
- A CLI tool uses its standard output as the exit for its data. Output that goes to a pipe or a file should be written as the text written, not split into lines.
- Standard output and standard error should be required and replaced separately.
  - In a tool that writes its results to standard output and its progress and warnings to standard error, the type of a function should tell which one it writes to.
  - `TestConsole` discards `print_error_line`. To check in a test what was written to standard error, one has to write a handler of `Console` by hand.
- `Console` stays. It is widely used, and its meaning, writing one line for a person to read, is clear.

The workaround in the current specification: write a handler with `@external` that calls `process.stdout.write`. It works only in projects that allow `[external]`, and has to be written for each target (`[ext.decl]`).

## Proposal

### Syntax

No change.

### Meaning

Add the following to `Std.Process` (declarations, with the bodies of the handlers left out).

```
pub effect Stdout {
  fn print(text: String) -> ()
  fn print_line(text: String) -> ()
}

pub effect Stderr {
  fn print(text: String) -> ()
  fn print_line(text: String) -> ()
}

pub handler SystemStdout: Stdout
pub handler SystemStderr: Stderr

pub type StdoutText(text: String)
pub type StderrText(text: String)

pub handler TestStdout: Stdout # State[StdoutText]
pub handler TestStderr: Stderr # State[StderrText]
```

- `print(text)` writes `text` as is. `print_line(text)` writes `text` and a line ending (`\n`). Line endings inside `text` are not converted.
- The operations do not fail and do not suspend (Immediate).
- What is written to one stream arrives in the order written. Mixing `Stdout` with `Console.print_line`, or `Stderr` with `Console.print_error_line` or `dbg`, keeps the order (see "Relation to Console" below).
- Before `Stderr`, `Console.print_error_line` or `dbg` writes to standard error, what has been written to standard output is handed to the host. With `2>&1` directing both to the same destination, the output is in the order written. The C `print_error_line` already writes after `fflush(stdout)`, so this puts the current behavior into the specification. JS writes synchronously to the file descriptor and has no buffer of its own, so it holds without any extra work.
- Before `Stdin` (`std-process`) waits for input, what has been written to `Stdout` is handed to the host. Calling `read_line` right after `print("name? ")` waits for input after the prompt is visible.
- When the program ends (in any of the ways in the table of exit codes of `[cmd.build]`), everything written so far is written out. This is the guarantee `Console` has now.
- A write to a standard output whose reader has closed it ends the program. Writes to a closed standard error are discarded and the program continues (see "Closed pipes" below).

#### Relation to Console

`Console` does not depend on `Stdout`. If `SystemConsole` were a handler that requires `Stdout` and `Stderr`, code written with only `with SystemConsole { ... }` would no longer compile, and `Console` could not be used on hosts without a standard output. The two make different promises.

| | `Console` | `Stdout`, `Stderr` |
|---|---|---|
| Meaning | One line for people | The output streams of the process |
| Unit | A line (always with a line ending) | Text (`print` adds no line ending) |
| Hosts | Every host. On a host without a standard output, the log of the host (`console.log` and `console.error` on JS) | Only hosts with a standard output and a standard error |
| Used for | Logs, diagnostics, output of HTTP services | Data output of CLI tools, prompts |

- Two effects can write to the same destination, so the order of mixed writes is guaranteed. `SystemConsole` and `SystemStdout` (`SystemStderr`) write through the same buffer of the same stream.
  - JS: on a host with a standard output, `SystemConsole`, `SystemStdout`, `SystemStderr`, `dbg` and the `error:`, `defect:` and `interrupted` lines that the runtime writes all go through one path that writes synchronously to the file descriptor (`writeAll` below). Mixing `console.log` with synchronous writes loses the order in Node when the reader is slow (see the prototype below), so they are not mixed. The order relative to output that the user writes by calling `console.log` directly through `@external` is not guaranteed.
  - C: both write to the `FILE *` of `stdout` and `stderr`, so the order holds.
  - WASM: both write to the one stream the runtime holds (`[target.wasm.output]`), so the order holds.
- `Console` stays, so nothing changes for programs that do not use `Stdout`.

#### Hosts without a standard output

- The hosts the JS target assumes are Node and Bun, and both have `process.stdout` (`[target]`). The browser is not a target, and the behavior of the platform handlers there is not decided. Things specific to a host, such as Cloudflare Workers, belong in a target-specific Pome (`[ext]`).
- So in the specification, a host without a standard output is outside the JS target, and the behavior of `SystemStdout` and `SystemStderr` there is not specified. Unlike `Console`, they do not fall back to the log of the host. `print` cannot be split into lines, so sending it to a log would change the shape of the output.
- What can be rejected at compile time is limited to the target (E0412, `[lib.layers]`). Within the JS target there is no way to tell Node from Workers, so "a compile error when reached" is not possible. Making it a defect at run time would bring rules for identifying hosts into the specification, so that is not done either.
- When the default export of `main.mjs` runs as an HTTP service on another host, `Console` works everywhere. `Stdout` is for running as a CLI.

#### Tests

`TestStdout` and `TestStderr` append what is written to `text` of their state. `print_line` appends the line ending as well. So that the two do not share one `State[String]`, the text is wrapped in the types `StdoutText` and `StderrText` (`[effect.state]`).

```emel
import Main.{greet}
import Std.Process.{StderrText, StdoutText, TestStderr, TestStdout}
import Std.State.{State}

@test
fn greets() {
  ((_, err), out) = State.run(StdoutText(text: ""), || {
    State.run(StderrText(text: ""), || with TestStdout, TestStderr { greet("alice") })
  })
  assert out.text == "Hello, alice!\n"
  assert err.text == "greeted alice\n"
}
```

- The state is text rather than a list of lines because of `print`. `print("a")` followed by `print_line("b")` is one line `ab`. With a list of lines, whether the last line is closed would have to be kept separately.
- `TestConsole` does not change.

#### Closed pipes

In `tool | head -1`, once `head` has read one line and exits, the reading end of the pipe is closed. When `tool` writes after that, there is nowhere to write to.

- Discarding silently and continuing is not chosen. A program that keeps writing (one that produces lines forever, for example) would never stop, with nowhere for its output to go.
- Sending an interruption (`[fiber.interrupt]`) alone is not enough. An interruption takes effect at the next suspension point, but `Stdout` is Immediate, so a loop that only writes never reaches one.
- So the write itself is the point where the program stops.
  1. When a write to standard output fails because the reader has closed it, the fiber that wrote does not return from the operation, and unwinds as interrupted from that point (the form "unwinds as interrupted on the spot" of `[dyn.suspend]`). Releases run from the inside out.
  2. Every other fiber of the program is interrupted as well (taking effect at its next suspension point).
  3. After that, writes to standard output are discarded (writes inside releases, for example).
  4. When the unwinding of main finishes, the program exits with exit code 141 and writes nothing to standard error. No `interrupted` line is written. Cutting output with `| head` is ordinary use, and a line every time would be noise. 141 is the value the C target exits with on SIGPIPE today (128 + 13), so a shell can tell that the program ended because its pipe was closed.
  5. Writes of `Console` and `dbg` are treated the same way. They write to the same stream, so continuing only one of them has no use.
  6. When only standard error is closed, the program does not end. Later writes to standard error (`Stderr`, `Console.print_error_line`, `dbg`, and lines the runtime writes such as `error:`) are discarded. Closing the exit for logs alone does not stop the program.
- The operations do not fail (they have no `fails`), so this is not a failure. Making it one would add an error to every function that uses `Stdout`.
- Inside `emela test`, `TestStdout` and the other test handlers do not write to a pipe, so this does not happen. What is written with `SystemStdout` follows how the test runner handles output (`[cmd.test.output]`).
- Prototype (Node 22.23, Bun 1.4.4, macOS, 2026-11-14): `fs.writeSync(1, ...)` threw `EPIPE` synchronously on a closed pipe (at line 377 on Node, at line 2 on Bun). `process.stdout.write` reports the error later through an event, so a loop that only writes cannot notice it. The JS `SystemStdout` uses the path that writes synchronously to the file descriptor.

#### Implementation on each target

| | `SystemStdout` | `SystemStderr` |
|---|---|---|
| JS | Writes synchronously to file descriptor 1 (`fs.writeSync`) | Writes synchronously to file descriptor 2 |
| C | `fputs` to `stdout` | `fflush` of `stdout`, then `fputs` to `stderr` (as `print_error_line` does now) |
| WASM | `write-via-stream` of `wasi:cli/stdout@0.3.0` | `write-via-stream` of `wasi:cli/stderr@0.3.0` |

#### Prototype (Node 22.23, Bun 1.4.4, macOS, 2026-11-14)

- Wrote 100,000 lines alternating one by one between `console.log` and `process.stdout.write`, and every 1,000 lines wrote alternately with `console.error` and `process.stderr.write`.
  - With standard output directed to a pipe, to a file, and to the same pipe as standard error with `2>&1`, no line was out of order on either host. Each line of standard error came right after the line last written to standard output.
- After writing 50,000 lines to a pipe, calling `process.exit` in the callback of a write to standard error (how the launcher ends on a defect) delivered all 50,000 lines on both hosts. The current launcher keeps the guarantee that everything is written out.
- Writing to a pipe whose reader closed first (`| head -1`), `process.stdout.write` crashed with an `EPIPE` exception. `console.log` discarded silently. With `process.stdout.on("error", ...)` attached, it also discarded silently. C still ends on SIGPIPE (exit code 141).
- Once `process.stdout` has been initialized (`console.log` does that), file descriptor 1 is non-blocking. Then, with a slow reader (`| (sleep 1; wc -c)`), `fs.writeSync(1, ...)` throws `EAGAIN`: about 150,000 times on Node and about 760,000 times on Bun for 13 MB written. Without initializing `process.stdout`, zero times.
- With `writeAll`, which on `EAGAIN` waits 1 ms with `Atomics.wait` and writes the rest again, everything arrived on both hosts. CPU time was about 35 ms, with no busy loop. With `| head -1`, `EPIPE` was thrown synchronously, and writing stopped after 83 lines on Node and 231 lines on Bun.
- Writing alternately one line at a time with `console.log` and `writeAll` to a slow reader, 19,933 of 20,000 lines were out of order on Node (0 on Bun). With a fast reader, 0 on both. On `EAGAIN`, `console.log` queues the write in libuv and writes later, so synchronous writes overtake it. That is why, on a host with a standard output, every write of the runtime goes through `writeAll`.
- `Atomics.wait` blocks the event loop while it waits, which does not conflict with the design because `Stdout` is Immediate and its writes are synchronous. With a slow reader the writer waits, so no buffer grows. `Atomics.wait` is not available on the main thread of a browser, but that is a host without a standard output, where the behavior is not decided.
- `Stdout`, `Stderr`, `TestStdout` and `TestStderr` were written as a user module (on JS, with an `@external` that calls `process.stdout.write`), and `emela check`, `emela test` and `emela run` passed (emela 0.27.0).

### Typing rules

No new rules. `Stdout` and `Stderr` are typed by the same rules as any user-defined effect.

## Specification changes

### `[api.std.process]`

Add to the table of `Std.Process` (`std/process.emel`, `std/process.mjs`, `std/process.c`):

| Declaration | Contents | Description |
|---|---|---|
| `Stdout` | `effect Stdout { fn print(text: String) -> (); fn print_line(text: String) -> () }` | Writes to the standard output of the process. `print` writes `text` as is, and `print_line` writes `text` followed by `\n`. Does not fail |
| `Stderr` | `effect Stderr { fn print(text: String) -> (); fn print_line(text: String) -> () }` | The same for the standard error of the process |
| `SystemStdout` | `handler SystemStdout: Stdout` | File descriptor 1, written synchronously, on JS, `stdout` on native, and `write-via-stream` of `wasi:cli/stdout@0.3.0` on WASM. Has no state |
| `SystemStderr` | `handler SystemStderr: Stderr` | File descriptor 2, written synchronously, on JS, `stderr` on native, and `write-via-stream` of `wasi:cli/stderr@0.3.0` on WASM. Has no state |
| `StdoutText` | `type StdoutText(text: String)` | The state of TestStdout: everything written so far |
| `StderrText` | `type StderrText(text: String)` | The state of TestStderr: everything written so far |
| `TestStdout` | `handler TestStdout: Stdout` | Appends what is written to `text` of `State[StdoutText]`, with `\n` after the text of `print_line` |
| `TestStderr` | `handler TestStderr: Stderr` | Appends what is written to `text` of `State[StderrText]`, with `\n` after the text of `print_line` |

- Stdout and Stderr are the output streams of the process; Console (`[api.std.console]`) writes lines meant for people. Stdout and Stderr write text without adding a line ending (`print`), and are meant for data that goes to a pipe or a file and for prompts.
- Stdout and Stderr are Immediate (9.5): their operations do not suspend.
- What is written to one stream arrives in the order written. SystemStdout and `print_line` of SystemConsole write to standard output through the same path, and SystemStderr, `print_error_line` of SystemConsole and `dbg` write to standard error through the same path, so writes mixed between them keep their order. The order relative to output that an external function writes to the host directly is not specified.
- Before SystemStderr, `print_error_line` of SystemConsole or `dbg` writes to standard error, what has been written to standard output is handed to the host, so that the two streams directed to the same destination keep the order written. Before an operation of SystemStdin waits for input, what has been written to standard output is handed to the host.
- When the program ends, in any of the ways of `[cmd.build]`, everything written so far is written out before the exit code is set.
- `\n` inside `text` is written as is; it is not converted to another line ending.
- SystemStdout and SystemStderr need a host that gives the process a standard output and a standard error. The hosts of the JS target (`[target]`) do. Their behavior on other hosts is not specified.
- When a write to standard output fails because the reader has closed it (a pipe whose reading end is closed), the operation does not return. The fiber that wrote unwinds as interrupted from that point, running the releases of its handlers from the inside out, and every other fiber of the program is interrupted (`[fiber.interrupt]`). After that, writes to standard output are discarded. When the unwinding of main finishes, the program exits with exit code 141 and writes nothing to standard error (no `interrupted` line). The same holds for writes of `print_line` of SystemConsole to standard output.
- When a write to standard error fails because the reader has closed it, the program continues. That write and every later write to standard error (SystemStderr, `print_error_line` of SystemConsole, `dbg`, and the lines that the runtime writes when execution stops) are discarded.

```emel
import Std.Platform.{Platform}
import Std.Process.{Stdout}

fn main() {
  with Platform {
    (use Stdout).print("Hello, ")
    (use Stdout).print_line("alice!")
  }
}
```

### `[api.std.console]`

Add after the table:

- Console writes one line meant for people at a time. To write to the output streams of the process without a line ending, use Stdout and Stderr of `Std.Process` (`[api.std.process]`). Writes mixed between SystemConsole and SystemStdout or SystemStderr keep their order.

### `[api.std.platform]`

Replace the row of `Platform`. The list has the handlers of `std-process` as well:

| Declaration | Contents | Description |
|---|---|---|
| `Platform` | `layer Platform { SystemConsole, SystemArgs, SystemStdin, SystemStdout, SystemStderr, SystemClock, SystemRandom, SystemEnv, SystemHttp, SystemFileSystem }` | A bundle of the System handlers. To replace some of them, list the replacements after it, as in `with Platform, TestClock { ... }` |

### `[lib.layers]`

In the table of layers, the platform row:

| platform | Effect declarations and the host handlers | Console, Process (Args, Stdin, Stdout, Stderr), Clock, Random, Env, FileSystem, Http |

Replace the bullet on names:

- A system handler that uses the host has a name prefixed with `System` (`SystemConsole`, `SystemStdout`, `SystemStderr`, `SystemClock`, `SystemRandom`, `SystemEnv`, `SystemFileSystem`, `SystemHttp`). A test handler is prefixed with `Test` (`TestClock`, `TestConsole`, `TestStdout`, `TestStderr`). Both are placed in the same module as their effect (`import Std.Clock.{Clock, SystemClock, TestClock}`).

After the bullet on Console, add:

- Stdout and Stderr (`Std.Process`) each have two operations, `print(text: String) -> ()` and `print_line(text: String) -> ()`. `print_line` writes a trailing newline and `print` does not. They do not fail. The system handlers are `SystemStdout` and `SystemStderr`.

### `[cmd.build]`

Add a row to the table of how execution stops, after the row of `interrupted`:

| How it stops | Standard error | Exit code |
|---|---|---|
| A write to a closed standard output (`[api.std.process]`) | nothing | 141 |

### `[target.native.run]`

Add:

- The executable ignores SIGPIPE. A write to a closed standard output or standard error is detected from the result of the write and handled as in `[api.std.process]`: for standard output, the program unwinds and exits with exit code 141, writing nothing to standard error; for standard error, the write is discarded and the program continues.

### `[target.wasm.external]`

Add a row to the table:

| `SystemStdout` (`print`, `print_line`), `SystemStderr` (`print`, `print_line`) | `write-via-stream` of `wasi:cli/stdout@0.3.0` and `wasi:cli/stderr@0.3.0`, on the same streams as `SystemConsole`. The text, then a newline for `print_line` |

### `[open.list]`

Mark #45 as decided:

| 45 | ~~Console has two operations, `print_line` and `print_error_line`. Whether to add output without a newline, and reading input~~ Decided (<date>): Console stays as it is. Output without a newline is `print` of Stdout and Stderr, and reading input is Stdin, both in Std.Process | `[api.std.process]` |

## Diagnostics

- [x] No new diagnostics.

## Compatibility

- Existing code keeps working. `Console`, `SystemConsole` and `TestConsole` do not change.
- `SystemStdout` and `SystemStderr` are added to `Platform`. An effect the user declares with the name `Stdout` or `Stderr` does not conflict unless `Std.Process`'s is imported (`[module.resolution]`). When a handler of the same effect is listed after it, as in `with Platform, MyStdout`, the later one is used (8.3).
- `Std.Process` is the module that `std-process` creates, so no new reserved name is added (`[lib.layers]`).

## Implementation

- Add the declarations to `crates/emela-core/lib/std/process.emel` (the file that `std-process` creates).
- JS: put functions that write synchronously to file descriptors 1 and 2 (`fs.writeSync`) in `std/process.mjs`. On `EPIPE`, tell the runtime, and unwind the fiber that wrote as interrupted on the spot. When the file descriptor is non-blocking (after `process.stdout` has been initialized, or when the parent process made it so), wait 1 ms with `Atomics.wait` on `EAGAIN` and write the rest again (`writeAll`, checked in the prototype). On a host with a standard output, also move `SystemConsole`, `dbg` and the `error:`, `defect:` and `interrupted` lines of the runtime to the same path, for the rule on closed pipes and the guarantee of order. On a host without a standard output, keep `console.log` and `console.error` as now.
- C: put them in `std/process.c`. `SystemStderr` writes after `fflush(stdout)`. `SystemStdin` also calls `fflush(stdout)` before waiting. The runtime ignores SIGPIPE at startup and detects a closed pipe from `EPIPE` of `fputs` or `fflush`.
- WASM: use the stdout and stderr streams the runtime already holds. `print` needs only a branch that adds no line ending. How `write-via-stream` reports a closed stream has not been checked. Streams on WASM are also out of scope in `std-process`, so the rule on closed pipes is matched on WASM after checking it in the implementation.
- Add the two handlers to `Platform` of `Std.Platform`.

## Alternatives

### Add `print` and `print_error` to `Console` instead of adding `Stdout`

- There is one effect per destination, so two effects never write to the same destination.
- Every user-defined handler of `Console` breaks (E0404).
- Standard output and standard error still cannot be required or replaced separately.
- `Console` would no longer be "line output that works on every host". `print` cannot go to the log of a host without a standard output.

### Replace `Console` with `Stdout` and `Stderr`

- There is one effect per destination, so two effects never write to the same destination.
- `Console` is used in almost every `main`, so every program would have to be rewritten.
- The meaning of `Console`, one line for people, and its use on hosts without a standard output would be lost. The author did not take this and decided to keep `Console`.

### One effect `Output`, with the labels `Stdout` and `Stderr`

```emel
pub effect Output {
  fn print(text: String) -> ()
  fn print_line(text: String) -> ()
}

pub effect Stdout of Output

pub effect Stderr of Output
```

- One function that takes an output can be directed to standard output, to standard error, or to a file output the user writes (`Stdout by FileOutput`).
- A label cannot have a handler written for it (E0433). `SystemOutput` does not know which stream to write to, so the stream would be passed as an argument of the handler, as in `Stdout by SystemOutput(1)`. The contents of `Platform` become complicated, and the mistake of putting `SystemOutput(2)` into `Stdout` can be written.
- There would be one test handler too, but putting the same `TestOutput` into both labels makes them share one `State`.
- With separate effects, the user can still make a label such as `effect AuditLog of Stderr`. A common `Output` can be added later.

### Make writes `suspend fn`

- The writer could be made to wait when the other end of the pipe does not read (backpressure).
- Every function that uses `Stdout` would require Async, and output would be impossible where Immediate is required (`[effect.async]`).
- JS writes synchronously and waits on the spot on `EAGAIN`. C `stdio` also waits in the write when its buffer is full. Both stop the writer and apply backpressure, so the operations are Immediate, as in `Console`. Other fibers also stop while it waits.

### Other ways to end on a closed pipe

- Discard silently and continue. A program that keeps writing would never stop, so this is not taken.
- Exit with exit code 0, treating cutting with `head` as ordinary use. But then the user cannot check from the exit code that the output really could not be written out. 141 is the value the C target exits with on SIGPIPE today (128 + 13), and a user of `set -o pipefail` can choose how to handle it.
- Exit with exit code 130 (as for an interruption). It could not be told apart from stopping with Ctrl-C.
- Write the `interrupted` line. A line every time output is cut with `| head` is noise.
- Stop the process on the spot without running releases. Simple, but releases do not run. It unwinds as other interruptions do.
- Not apply it to `Console` and `dbg`. They write to the same stream, so continuing only one of them has no use.
- End the program when standard error is closed too. Stopping the program only because the exit for logs closed is a problem, so writes are discarded and the program continues.

## Prior art

- Giving a process three standard streams (input, output and error), each handled as a separate handle, is found in the file descriptors 0, 1 and 2 of POSIX and in the `wasi:cli/stdin`, `wasi:cli/stdout` and `wasi:cli/stderr` interfaces of WASI 0.3. WASI makes the three separate interfaces, and a component imports only the ones it uses.
- Separating the destinations that can be written to by the type of an effect lets a type signature tell which stream a function writes to. It is an instance of capability-based I/O.

## Open questions

## Decision

- A program cannot write without a newline today, so it cannot show a prompt before `Stdin.read_line` or build a line piece by piece (`[open.list]` #45). `Stdout` and `Stderr` add `print` and `print_line` to `Std.Process`, next to `Args` and `Stdin`.
- `Console` stays as a line meant for a person, and it works on any host. `Stdout` and `Stderr` are the streams of the process, and they are separate effects, so the type of a function shows which stream it writes to and a test can replace each one. `Console` does not depend on them, so existing programs and handlers do not change.
- Writing to the same stream through `Console`, `Stdout`, `Stderr`, `dbg` and the runtime keeps the order in which the program wrote. On JS, every write goes through one path that writes to the fd synchronously, because mixing it with `console.log` reorders lines in Node when the reader is slow.
- The operations are Immediate and do not fail. A writer waits while the reader is slow, so the buffer does not grow.
- When the reader closes standard output, the program unwinds as an interruption, runs its releases, and ends with exit code 141, which is the same code a C program gets from SIGPIPE, and prints no line. When only standard error is closed, its writes are discarded and the program goes on.
- What `SystemStdout` and `SystemStderr` do on a host without stdout is unspecified. Such a host is outside the JS target, and `Console` covers it.
