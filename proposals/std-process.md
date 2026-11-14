---
title: Std.Process, the arguments and the standard input of the process
authors: [Alliana]
status: accepted
pr: 565
issue:
decided: 2026-11-14
touches: [lib.layers, api.std.process (new), api.std.platform, cmd.build, target.c.run, target.wasm.output, target.wasm.external, cmd.test.output]
requires: []
related: []
---

# Std.Process, the arguments and the standard input of the process

## Summary

Add a module `Std.Process` to the platform layer of std, for what belongs to the process, and put two effects in it first.

- `Args`: reads the arguments given to the process. `get(index)` takes one by position, and `list()` returns all of them as a `List[String]`. The name of the program is not included
- `Stdin`: reads standard input. `read_line` reads one line, and `read_text` reads all the rest. Both are `suspend fn`

The system handlers `SystemArgs` and `SystemStdin` are added to `Platform`. For tests, `TestStdin` reads from `State[StdinLines]`. There is no test handler for `Args`. `Console` does not change (`Stdout` and `Stderr` are added to this module by the separate proposal stdout-stderr, and the exit code type `ExitCode` by the separate proposal exit-code).

```emel
import Std.Console.{Console}
import Std.Platform.{Platform}
import Std.Process.{Args, Stdin, InvalidInput, StdinFailure}

fn quote(prefix: String) -> () fails InvalidInput | StdinFailure uses { Console, Stdin } {
  (use Stdin).read_line() match {
    None -> ()
    Some(line) -> {
      (use Console).print_line("#{prefix}#{line}")
      quote(prefix)
    }
  }
}

fn main() {
  with Platform {
    prefix = (use Args).get(0) match {
      Some(p) -> p
      None    -> "> "
    }
    quote(prefix)
  }
}
```

## Motivation

- An Emela program cannot take input from outside today. `emela run` passes the arguments after `--` and lets the program inherit standard input (`[cmd.build]`, `[target.c.run]`, `[target.wasm.run]`), but there is no API to read them inside the program. Notes from porting programs to Emela record: "there is no argv and no stdin; input can only be embedded as constants"
- A CLI tool needs its arguments first. Parsing them (flags, subcommands, printing usage) can be left to a library in a Pome, but std has to provide the means to receive the `List[String]` that such a library works on
- Many CLI tools sit in the middle of a pipe (`cat data.json | tool`, `git diff | tool`). Without reading standard input, the only way to get input is to take a file path as an argument and read it with `Std.FileSystem`
- Interactive programs (a REPL, a confirmation prompt) cannot be written either
- The workaround is a Pome that calls `process.argv` or `node:readline` through `@external`. It works only on JS, and its users have to allow it in `[external]` (`[module.pome]`). `[ext.decl]` says that a host feature that many programs use is added to std as a platform effect

## Proposal

### Meaning

```emel
# Std.Process
pub effect Args {
  fn get(index: Int) -> Option[String]
  fn list() -> List[String]
}

pub handler SystemArgs: Args

pub error InvalidInput
pub error StdinFailure(message: String)

pub effect Stdin {
  suspend fn read_line() -> Option[String] fails InvalidInput | StdinFailure
  suspend fn read_text() -> String fails InvalidInput | StdinFailure
}

pub handler SystemStdin: Stdin

pub type StdinLines(lines: List[String])
pub handler TestStdin: Stdin
```

#### The module

- `Std.Process` is the module for what the process has exactly one of (its arguments and its standard input, and, in later proposals, its standard output, its standard error and its exit code)
- In std today, each effect has a module of the same name (`Std.Clock.Clock`). `Std.Process` puts several effects in one module. System handlers and test handlers are placed in the same module as their effect, as now

#### Args

- `list` returns the arguments given to the process, in the order given. The name of the program (the host's executable, `main.mjs`, `main.wasm`, `argv[0]` on C) is not included. Without arguments it is `[]`
- `get(index)` returns the element of `list` at `index` (from 0). Out of range (including a negative index) is `None` (out of range is an Option, `[lib.api-rules]`)
- Every call returns the same result. It does not change after the program starts
- What each target returns:
  - `emela run`: the arguments written after `--` (every target)
  - C: `argv[1]` and after, when the executable `main` is started directly
  - JS: the arguments that the runtime's launcher passes to the runtime
  - WASM: the result of `get-arguments` of `wasi:cli/environment@0.3.0` without its first element (the name of the program)
  - An HTTP service: `[]` (`emela run` passes the arguments after `--` to the host, so they do not reach the program)
- An argument that is not valid UTF-8 (possible on C and on JS hosts on POSIX) has its invalid parts replaced with U+FFFD. It does not fail. The reasons are in Alternatives
- Under `emela test` it is `[]`. The arguments that the test runner passes to the executable (to select tests, for example) are not visible
- `SystemArgs` does not require Async. It has no state
- There is no test handler. One can be written in two lines, `get` and `list` (as `Std.Env` has no `TestEnv`)

#### Stdin

- `read_line` returns the next line. The `\n` at the end of the line, and a `\r` immediately before it, are not included. A last line without `\n` is still returned as a line. When the input is exhausted it returns `None`, and after returning `None` it returns `None` on every call
- `read_text` returns all the rest of the input, line terminators included. When the input is exhausted it returns `""`
- The two share one position. `read_text` after `read_line` returns what follows
- When a line (for `read_text`, all the rest) is not valid UTF-8, it is consumed and the operation fails with `InvalidInput`. It is not replaced with the replacement character (the same policy as `[api.std.bytes]` and `InvalidText` of `[api.std.file-system]`). The next `read_line` reads from the following line
- When the host cannot read, it is `StdinFailure(message)`. `message` is the host's text
- When the waiting fiber is interrupted, the read is canceled. Input already received from the host is kept, not discarded, and the next read returns it. No line is lost. The runtime keeps received input in a buffer anyway to split it into lines, so keeping it costs nothing more
- `SystemStdin` requires Async (`Platform` already requires it). Other fibers run while it waits for input
- `emela test` does not pass standard input to the program (`[cmd.test.output]`). `SystemStdin` reads an empty input (`read_line` is `None`, `read_text` is `""`)
- WASM is out of the scope of this proposal. The runtime needs a way to wait for a host stream, which is also why `SystemClock.sleep` does not work on WASM (Implementation)

### Tests

`TestStdin` takes lines one by one from the front of `State[StdinLines]`. `TestConsole` uses `State[List[String]]`, so the lines are wrapped in `StdinLines` to keep the types apart (`[effect.state]`). The name is prefixed with the name of the stream, matching `StdoutText` and `StderrText` of the stdout-stderr proposal. The final state of `State.run` is the lines not read.

```emel
import Std.Console.{TestConsole}
import Std.Process.{Stdin, TestStdin, StdinLines}
import Std.State.{State}

@test
fn quotes_each_line() {
  ((_, rest), lines) = State.run([], || {
    State.run(StdinLines(lines: ["a", "b"]), || with TestStdin, TestConsole { quote("> ") })
  })
  assert lines == ["> a", "> b"]
  assert rest == StdinLines(lines: [])
}
```

- `read_text` of `TestStdin` joins the remaining lines with `\n` and empties the state. It does not raise `InvalidInput` or `StdinFailure`
- Prototype: `Stdin`, `StdinLines` and `TestStdin` were written as a user module, and `emela check` and `emela test` pass. With `Args` (`get` and `list`) and a handler `FixedArgs` written by hand, the form of `main` in the Summary passes `emela check`, `emela run` and `emela fmt` (0.x, 2026-11-14)

### Typing rules

No new rules. `Args` and `Stdin` are typed by the same rules as an ordinary user-defined effect.

## Specification changes

### `[lib.layers]`

In the table of layers, the platform row becomes:

| Layer | Nature | Contents |
|---|---|---|
| platform | Effect declarations and the host handlers | Console, Process (Args, Stdin), Clock, Random, Env, FileSystem, Http |

The paragraph on system and test handlers becomes:

- A system handler that uses the host has a name prefixed with `System` (`SystemConsole`, `SystemArgs`, `SystemStdin`, `SystemClock`, `SystemRandom`, `SystemEnv`, `SystemFileSystem`, `SystemHttp`). A test handler is prefixed with `Test` (`TestClock`, `TestConsole`, `TestStdin`). Both are placed in the same module as their effect (`import Std.Clock.{Clock, SystemClock, TestClock}`).
- A platform module usually declares one effect and has the name of that effect (`Std.Clock`). `Std.Process` declares several effects, one for each thing the process has exactly one of (its arguments and its standard input), and their handlers (`import Std.Process.{Args, Stdin, SystemStdin}`).

### `[api.std.process]` (new, after `[api.std.console]`)

### Std.Process (`std/process.emel`, `std/process.mjs`, `std/process.c`) `[api.std.process]`

The platform layer and the test layer (`[lib.layers]`). What the process has exactly one of.

| Declaration | Contents | Description |
|---|---|---|
| `Args` | `effect Args { fn get(index: Int) -> Option[String]; fn list() -> List[String] }` | The arguments of the process, in order, without the name of the program. `list` returns all of them, and `get` the one at `index` (from 0), or `None` when `index` is out of range |
| `SystemArgs` | `handler SystemArgs: Args` | The arguments given by the host. Has no state |
| error | `InvalidInput`, `StdinFailure(message: String)` | InvalidInput when standard input is not valid UTF-8. StdinFailure when the host cannot read standard input; `message` is the host's text |
| `Stdin` | `effect Stdin { suspend fn read_line() -> Option[String] fails InvalidInput \| StdinFailure; suspend fn read_text() -> String fails InvalidInput \| StdinFailure }` | Reads standard input as text (UTF-8). `read_line` returns the next line without its line terminator, and `None` at the end of the input. `read_text` returns the rest of the input, line terminators included, and `""` at the end of the input |
| `SystemStdin` | `handler SystemStdin: Stdin` | The standard input of the process. `process.stdin` on JS and file descriptor 0 on C. Requires Async. Has no state |
| `StdinLines` | `type StdinLines(lines: List[String])` | The state of TestStdin: the lines not read yet |
| `TestStdin` | `handler TestStdin: Stdin` | Kept in `State[StdinLines]` and resumes immediately. Does not raise InvalidInput or StdinFailure |

Args:

- `list` of SystemArgs returns the same list on every call, and `get(index)` returns its element at `index`, or `None` when `index` is negative or not less than its length. The list is:
  - for `emela run`, the arguments written after `--` (`[cmd.build]`), on every target;
  - on C, `argv[1]` and after;
  - on JS, the arguments that the runtime's launcher passes to the runtime;
  - on WASM, the result of `get-arguments` of `wasi:cli/environment@0.3.0` without its first element;
  - `[]` for an HTTP service (`[lib.http-service]`) and under `emela test` (`[cmd.test.output]`).
- An argument that is not valid UTF-8 is decoded with each maximal invalid subsequence replaced by U+FFFD. Neither operation fails.

Stdin:

- A line ends at `\n`. The `\n`, and a `\r` immediately before it, are not part of the line. The text after the last `\n`, if not empty, is the last line. Once `read_line` has returned `None`, every later call returns `None` and every later `read_text` returns `""`.
- `read_line` and `read_text` read from the same position. `read_text` after `read_line` returns the input that follows the line.
- When a line (for `read_text`, the rest of the input) is not valid UTF-8, it is consumed and the operation fails with InvalidInput. Invalid UTF-8 is not replaced with the replacement character, as in `[api.std.bytes]`. The next `read_line` starts at the following line. U+0000 is read as is on every target.
- When the fiber waiting in an operation of SystemStdin is interrupted (11.4), the operation is canceled. Input that the host has already delivered is kept and returned by the next operation; no input is lost.
- Waiting for input does not stop other fibers on any target.
- SystemStdin does not hold the host open: a program that has no operation of Stdin waiting ends when its main ends, even if standard input is still open (a terminal, for example).
- Under `emela test`, standard input is not passed to the program (`[cmd.test.output]`). SystemStdin reads it as an empty input.
- `read_line` of TestStdin removes the first element of `lines` and returns it, or returns `None` when `lines` is empty. `read_text` returns the elements of `lines` joined with `\n` and sets `lines` to `[]`.

impl (`[trait.derive]`): StdinLines has `derive Eq, Show`. Both errors have `derive Show`.

```emel
import Std.Console.{Console}
import Std.Process.{Args, Stdin, InvalidInput, StdinFailure}

fn quote(prefix: String) -> () fails InvalidInput | StdinFailure uses { Console, Stdin } {
  (use Stdin).read_line() match {
    None -> ()
    Some(line) -> {
      (use Console).print_line("#{prefix}#{line}")
      quote(prefix)
    }
  }
}
```

### `[api.std.platform]`

| Declaration | Contents | Description |
|---|---|---|
| `Platform` | `layer Platform { SystemConsole, SystemArgs, SystemStdin, SystemClock, SystemRandom, SystemEnv, SystemHttp, SystemFileSystem }` | A bundle of the System handlers. To replace some of them, list the replacements after it, as in `with Platform, TestClock { ... }` |

- The order of the list has no meaning (`[effect.layer]`). SystemStdin, SystemClock, SystemHttp, and SystemFileSystem require Async, so Async remains in the R of `with Platform { ... }`. In main and test functions, the runtime provides Async.

### `[cmd.build]`

In "Running the program (JS)", the item on arguments becomes:

- `run` loads the entry module and calls the exported `main` with no arguments. The return value of `main` is discarded. Arguments to the program are passed after `--` as arguments of the JS host, and the launcher passes them to the runtime, where `Args.list` of SystemArgs returns them (`[api.std.process]`).

### `[target.c.run]`

The first item becomes:

- `emela run --target c` starts `main` in the output directory. It passes the arguments after `--`, and the program inherits standard input. `Args.list` of SystemArgs returns `argv[1]` and after (`[api.std.process]`). The exit code is as in `[cmd.build]`.

### `[target.wasm.output]`

In the item on imports, `Std.Env` becomes `Std.Env` or `Std.Process`:

- The component always imports `wasi:cli/stdout@0.3.0`, `wasi:cli/stderr@0.3.0` and `wasi:cli/exit@0.3.0`. It also imports `wasi:cli/environment@0.3.0`, `wasi:random/random@0.3.0` and `wasi:clocks/system-clock@0.3.0`, each only when the entry reaches a host function of `Std.Env` or `Std.Process`, `Std.Random` or `Std.Clock` that uses it (`[target.wasm.external]`). ...

### `[target.wasm.external]`

Add a row to the table of handlers that work on WASM:

| Handler | Host function |
|---|---|
| `SystemArgs` (`get`, `list`) | `get-arguments` of `wasi:cli/environment@0.3.0`, called once per run. The list without its first element |

The last item becomes:

- `SystemClock.sleep`, `SystemStdin`, `Std.FileSystem` and the client of `Std.Http` (`SystemHttp`) are not available on WASM yet. The entry adapter `Http.serve` is (`[lib.http-service]`).

### `[cmd.test.output]`

- What the program writes to standard output while tests run appears as is between the result lines. The program's standard error goes to standard error as is. Standard input is not passed to the program: `SystemStdin` reads an empty input, and `SystemArgs` returns `[]` (`[api.std.process]`).

## Diagnostics

- [ ] No new diagnostics. Reaching an operation of `SystemStdin` on WASM is the existing E0412 (`[lib.layers]`)

## Compatibility

- Existing programs do not change. `Console` does not change either
- `Platform` gains `SystemArgs` and `SystemStdin`. A replacement such as `with Platform, MyStdin { ... }` merely becomes possible. A program that already listed its own handler of `Args` or `Stdin` in the same with as `Platform` keeps its behavior, because the one written later takes precedence
- A user cannot have a module of their own named `Std.Process` (it is under `Std`, `[lib.layers]`). It does not conflict with a user's `process.emel`
- The following proposals stdout-stderr and exit-code add `Stdout`, `Stderr` and `ExitCode` to this module. Whichever is adopted first, each adds rows to the table of `[api.std.process]`
- Starting a JS program without the launcher (`node main.mjs a b`) is not in the specification today, and this proposal does not address it

## Implementation

Args:

- JS: the launcher passes to the runtime the part of `process.argv` after `main.mjs`, which follows `--`. How many leading elements of `process.argv` are the host and the script depends on the host and on how it is started (`-e` or a file), so the runtime does not slice `process.argv` itself
- C: the runtime's `main` saves `argc` and `argv`, and the first call to `SystemArgs` builds the list, decoding UTF-8 with the invalid parts replaced with U+FFFD
- WASM: `get-arguments` is called once. The import is added only when an operation of `SystemArgs` is reached (the same interface as `get-environment`)
- Most JS hosts turn arguments into strings themselves. Node replaces invalid bytes with U+FFFD, so returning them as they are matches C

Stdin:

- JS: the `data` of `process.stdin` is accumulated in a buffer of the runtime, and waiting `read_line` / `read_text` are served from it. When nothing waits, the stream is paused (`pause()`), and even with a non-empty buffer it does not keep the event loop alive (the equivalent of `unref`). A program that never reaches an operation of `Stdin` does not touch `process.stdin` (touching it keeps the program from ending on a TTY). UTF-8 is checked with `TextDecoder("utf-8", { fatal: true })` per line
- C: fd 0 is waited on with `em_on_readable`. epoll cannot register a regular file (`EPERM`), so in that case it is read synchronously (a regular file can be read without waiting, so other fibers are not stopped). A line that contains U+0000 cannot be returned with `em_complete_string`, so it is returned with its length through an internal completion function of the runtime (the public API of `[ext.c-host]` does not change)
- WASM: `read-via-stream` of `wasi:cli/stdin@0.3.0` exists, but the runtime does not yet have a way to wait for a host stream (the same reason as `SystemClock.sleep`). This proposal does not add it

Tests:

- Under `emela test`, `Args.list` returns `[]`, `Args.get(0)` returns `None`, and `read_line` returns `None`
- With `emela run -- a 'b c' ''`, `list` returns `["a", "b c", ""]`, `get(2)` returns `Some("")`, and `get(3)` and `get(-1)` return `None` (JS, C, WASM). On both Node and Bun
- On C, an argument that is not valid UTF-8 has U+FFFD in place of the invalid parts
- On JS and C: `\r\n`, a last line without a line terminator, the line after invalid UTF-8, `read_text` after `read_line`, and no line lost after an interruption

## Alternatives

### The module and the names

- Name the effects by role (`Input`, `Output`, `Diagnostics`). They read well even on hosts without stdin or stdout, such as Workers and browsers. Not adopted because:
  - they are common words, and collide by import with names in users' code
  - what these effects promise is the behavior of the streams given to the process (an end of input, redirection and pipes, a separate exit for diagnostics apart from the output). A role name does not show that promise
- One module per effect (`Std.Args`, `Std.Stdin`). It keeps the current shape of std. Not adopted because: the arguments, standard input and output and the exit code are each something the process has exactly one of, and they are used together. One module is one place to look, and one line imports them: `import Std.Process.{Args, Stdin, Stdout}`
- Add input to the module of `Console` (`Console` and `ConsoleInput`). The names of input and output do not match. `Console` stays as it is, as the output of lines for people to read, and output as a stream of the process is added to `Std.Process` as `Stdout` and `Stderr` by stdout-stderr

### Args

- Add `fn args()` to `Env` of `Std.Env`. In WASI, environment variables and arguments are in the same `wasi:cli/environment`. Not adopted because: every program that writes its own handler of `Env` breaks with E0404, and environment variables and arguments could no longer be replaced separately
- Make it `fn main(args: List[String])`. The arguments would have to be passed by hand from main down to where they are used. It does not match the shape of `@entry` and the entry of an HTTP service. It also departs from the shape of std, in which what touches the host is received through an effect
- Include the name of the program first. Its contents differ by target (the host's path or the script on JS, `main.wasm` on WASM, depending on how it is started on C), so a portable program cannot use it
- Have only one operation (only `list` or only `get`). With only `list`, "if there is an n-th one" has to be written with a list pattern each time. With only `get`, there is no way to know the count when passing everything to a parsing library. Both are kept: `get`, which looks up by position and returns an `Option` as `Env.get` does, and `list`, which returns everything
- Fail on an argument that is not valid UTF-8 (`list() -> List[String] fails InvalidInput`). It would match Stdin, but is not adopted because:
  - arguments are fixed values that the OS passes at startup, and reading them again does not change them. A failure has no way to recover
  - arguments are almost always read right after main starts, and handling a failure there would require an `escape` in every CLI
  - `get-arguments` of WASI is `list<string>`, already turned into strings by the host (wasmtime cannot pass invalid arguments). Node also replaces with U+FFFD before passing them. Failing only on C would make the result differ by target
  - Stdin is used in filters that read and write back, where silent replacement hides that the input changed. Arguments are not used that way
- Return a list of Bytes (`List[Bytes]`). It could handle paths with invalid bytes, but is left to a later proposal, after the handling of Path is decided
- Add a test handler `TestArgs`. It is not added, because one can be written by hand in one line (as with `Std.Env`)

### Stdin

- Add `read_line` to `Console`. Input and output would be one effect, and `with SystemConsole` alone would be enough. Not adopted because:
  - every program that writes its own handler of `Console` (a handler that discards logs, for example) breaks with E0404
  - the operations of `Console` are `fn` and Immediate, but reading waits, so it is a `suspend fn`. Whether the effect is Immediate (`[effect.decl]`) would change, and the property that `TestConsole` can be used without `Async` would no longer hold
  - `TestConsole` would have two States, for output and for input
- Have only `read_line`. `read_text` can be written by repeating `read_line`, but the line terminators cannot be restored (whether there was a last line terminator, `\r\n`). Reading JSON and the like as a whole needs an operation that receives the original input as it is
- Read Bytes (`suspend fn read(max: Int) -> Option[Bytes]`). It handles binary input and input that is not UTF-8, and has the same shape as `read` of the TCP proposal. Not adopted because: a binary filter also needs a Bytes version of `Stdout` on the writing side, and how UTF-8 boundaries are handled when mixed with text reads must be decided. It is left to a separate proposal together with the Bytes version of `Stdout`. Adding it to the same effect later does not break existing calls
- Make it work on WASM too. `read-via-stream` of `wasi:cli/stdin@0.3.0` exists, but the runtime does not yet have a way to wait for a host stream. That is also needed for `SystemClock.sleep` on WASM, so both are left to a separate proposal
- On interruption, discard the input already received from the host. The implementation would be a little simpler, but the runtime keeps received input in a buffer anyway to split lines, so keeping it costs almost the same. Discarding it would make a program that reads with a timeout lose lines
- Replace invalid UTF-8 with U+FFFD. It would match Args, but is not adopted for the reasons above. `Bytes.decode_utf8` and `FileSystem.read_text` do not replace either, and Stdin matches them
- Return the input as a Stream. std has no Stream yet (a later version, in the effect layer of `[lib.layers]`)
- Receive it at the entry, as in `fn main(stdin: ...)`. It could not be read from deep in the call stack, and is hard to replace in tests

## Prior art

- Rust's `std::env::args` includes the name of the program first and panics on an argument that is not valid Unicode. `args_os` returns `OsString`
- Go's `os.Args` is a `[]string` that includes the name of the program first and keeps the bytes as they are
- Node's `process.argv` includes the paths of the host and the script first
- `get-arguments` of `wasi:cli/environment` in WASI 0.3 returns `list<string>`
- Rust's `std::io::Stdin` has `read_line(&mut String)` and `lines()`. `read_line` includes the line terminator. Invalid UTF-8 is an `InvalidData` error
- Go's `bufio.Scanner` removes the line terminator (including the `\r` of `\r\n`)
- Node's `readline` removes the line terminator and treats `\r\n` as one line break
- `wasi:cli/stdin` of WASI 0.3 only returns a stream of bytes through `read-via-stream`, with no notion of lines

## Open questions

## Decision

A CLI tool written in Emela has to receive its arguments and read its standard input; parsing the arguments is left to libraries.

    Things that belong to the process are gathered in one module, Std.Process, so that the arguments, the standard input, and later the output streams and the exit code are found in one place. The effects are named after the streams (Stdin), which state what they promise: end of input, pipes and redirection.
    Args has both get(index), returning an Option, and list(), returning the whole list.
    Stdin starts with read_line and read_text. Reading bytes goes with a bytes version of Stdout in a later proposal, and WASM waits for the runtime to be able to wait on host streams.
    Invalid UTF-8 is replaced in the arguments but fails in the standard input: the arguments are read once at start-up, where there is no place to handle a failure, while standard input is data that must not be changed silently.
