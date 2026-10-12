# Emela

Emela is a functional language with typed errors and typed effects, compiling to
JavaScript and WebAssembly.

## Status

This branch is the 0.20 rebuild. The current release is **0.20.0-alpha.3**:
the pure subset, typed errors and effects, and suspending effects compile to
JavaScript and run on Node.js. The 0.11
compiler lives on the [`legacy`](https://github.com/emela-lang/emela/tree/legacy)
branch.

| Release        | Adds                                                                                                                       |
| -------------- | -------------------------------------------------------------------------------------------------------------------------- |
| 0.20.0-alpha.1 | Functions, records, enums, `opaque` types, pattern matching, tail calls, pipes, `List` / `Option`, modules, `@test`, `dbg` |
| 0.20.0-alpha.2 | Typed errors and effects: `error`, `fail`, `escape`, `effect`, `handler`, `use`, `with`, `@external`, `Console`            |
| 0.20.0-alpha.3 | `suspend fn`, `Async`, `Immediate`, `Clock`, `Random`, `Env`, `Instant` / `Duration`                                       |
| 0.20.0-alpha.4 | Handlers with fields, `init` and `release`, `layer`, `State`                                                               |
| 0.20.0-alpha.5 | Fibers: `Fiber.scope`, `fork`, interruption                                                                                |
| 0.21 and later | `trait`, `impl`, `derive`, `Map`, `Set`, WebAssembly output                                                                |

The 0.20 language specification is still being written. The
[`emela-lang/specification`](https://github.com/emela-lang/specification)
repository describes the 0.1x language, not 0.20.

## Install

Prebuilt binaries are published for macOS (Apple Silicon) and Linux (x86_64).
Running the generated JavaScript (`emela run`, `emela test`) needs Node.js.

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | EMELA_VERSION=v0.20.0-alpha.3 sh
emela --version
```

This installs into `$HOME/.emela/bin`; set `EMELA_INSTALL_DIR` to change it.

## A tour

A project is a directory with a `Pome.toml` (empty is fine) and its sources under
`src/`, with the entry point in `src/main.emel`. Each module is one file:
`src/email.emel` is imported as `Email`. This is
[`examples/tour`](examples/tour):

```
tour/
├── Pome.toml
└── src/
    ├── main.emel
    └── email.emel
```

`src/main.emel`:

```emela
# A tour of the pure part of the language. `dbg` prints to stderr.
import Email

# Variants carry labelled fields.
enum Shape {
  Circle(radius: Float)
  Rect(width: Float, height: Float)
}

# A record type.
type Todo(id: Int, title: String, done: Bool)

fn area(shape: Shape) -> Float {
  match shape {
    Circle(radius:)       -> 3.14 * radius * radius
    Rect(width:, height:) -> width * height
  }
}

# Self tail calls compile to loops, so deep recursion is fine.
fn sum(xs: List[Int], acc: Int) -> Int {
  match xs {
    []          -> acc
    [x, ..rest] -> sum(rest, acc + x)
  }
}

fn fizzbuzz(n: Int) -> String {
  match (n % 3, n % 5) {
    (0, 0) -> "FizzBuzz"
    (0, _) -> "Fizz"
    (_, 0) -> "Buzz"
    _      -> Int.to_string(n)
  }
}

fn main() {
  todo = Todo(id: 1, title: "Write docs", done: False)
  finished = Todo(done: True, ..todo)
  dbg(finished) # Todo(id: 1, title: "Write docs", done: True)

  dbg(area(Rect(width: 2.0, height: 3.0))) # 6.0

  dbg(sum(List.range(1, 10001), 0)) # 50005000

  # "1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz"
  List.range(1, 16)
    |> List.map(fizzbuzz)
    |> String.join(" ")
    |> dbg

  dbg(Email.parse("alice@example.com") |> Option.map(Email.to_string)) # Some("alice@example.com")
  dbg(Email.parse("not an email")) # None
}

# Run with `emela test`.
@test
fn fizzbuzz_of_15() {
  assert fizzbuzz(15) == "FizzBuzz"
}
```

`src/email.emel`:

```emela
# `opaque` hides the constructor and fields outside this module,
# so every Email goes through `parse`.
pub opaque type Email(value: String)

pub fn parse(input: String) -> Option[Email] {
  if String.contains(input, "@") { Some(Email(value: input)) } else { None }
}

pub fn to_string(email: Email) -> String {
  email.value
}
```

```console
$ emela run
Todo(id: 1, title: "Write docs", done: True)
6.0
50005000
"1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz"
Some("alice@example.com")
None
$ emela test
running 1 test
test fizzbuzz_of_15 ... ok

test result: ok. 1 passed; 0 failed; 0 filtered out
```

## Commands

Each command takes a project directory or an entry file, and defaults to `.`.

| Command       | Does                                                    |
| ------------- | ------------------------------------------------------- |
| `emela check` | Reports diagnostics without compiling                   |
| `emela run`   | Compiles to JavaScript and runs it with Node.js         |
| `emela build` | Compiles to JavaScript under `target/emela/js`          |
| `emela test`  | Runs the functions marked `@test`                       |
| `emela fmt`   | Formats the sources in place; `--check` only lists them |
| `emela lsp`   | Starts the language server over stdio                   |

The language server reports diagnostics and offers hover (types, `fails`, `use`),
inlay hints, completion, go to definition, references, rename, signature help,
formatting, and a code action that writes inferred annotations.

Programs print with `dbg`, which writes to stderr. The value `main` returns is
not printed.

Side effects go through effects. The bundled `Console` effect writes lines, and
`SystemConsole` provides it on the host. Both live in `Std.Console` and are
imported. This is [`examples/hello`](examples/hello):

```emela
import Std.Console.{Console, SystemConsole}

fn greet(name: String) {
  console = use Console
  console.print_line("Hello, #{name}!")
}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line("hi")
    List.map(["Emela", "world"], greet)
    console.print_error_line("done")
  }
}
```

```console
$ emela run
hi
Hello, Emela!
Hello, world!
done
```

Errors are values declared with `error`. A function's errors (`fails`) and
effects (`use`) are part of its type and inferred unless written. `escape`
handles some errors and lets the rest propagate; an error that reaches `main`
exits with status 1. This is [`examples/errors`](examples/errors):

```emela
# Typed errors and effects. `fails` and `use` are inferred unless written.

import Std.Console.{Console, SystemConsole}

error NotFound(id: Int)
error Invalid(reason: String)

type User(id: Int, name: String)

# An effect declares operations; handlers implement them.
effect Users {
  fn find(id: Int) -> User fails NotFound
}

handler SampleUsers implements Users {
  fn find(id) {
    match id {
      1 -> User(id: 1, name: "alice")
      2 -> User(id: 2, name: "bob")
      _ -> fail NotFound(id:)
    }
  }
}

# Inferred: fails NotFound | Invalid, use Users.
fn greeting(id: Int) {
  if id < 0 { fail Invalid(reason: "negative id") }
  user = (use Users).find(id)
  "Hello, #{user.name}!"
}

# `escape` handles NotFound; Invalid still propagates.
fn greeting_or_guest(id: Int) {
  greeting(id) escape {
    NotFound(_) -> "Hello, guest!"
  }
}

fn main() {
  with SystemConsole, SampleUsers {
    console = use Console
    console.print_line(greeting_or_guest(1))
    console.print_line(greeting_or_guest(3))
    console.print_line(greeting_or_guest(-1))
  }
}

# Tests swap the handler instead of mocking.
handler NoUsers implements Users {
  fn find(id) { fail NotFound(id:) }
}

@test
fn unknown_users_get_the_guest_greeting() {
  with NoUsers {
    assert greeting_or_guest(1) == "Hello, guest!"
  }
}
```

```console
$ emela run
Hello, alice!
Hello, guest!
error: Invalid(reason: "negative id")
$ emela test
running 1 test
test unknown_users_get_the_guest_greeting ... ok

test result: ok. 1 passed; 0 failed; 0 filtered out
```

A `suspend fn` operation may pause its caller; the handler decides when to
resume. Callers write ordinary calls, so the same `countdown` runs on a real
clock in `main` and on an instant one in the test. This is
[`examples/clock`](examples/clock):

```emela
# Suspending effects. `sleep` is a `suspend fn`: calls to it look like any other
# call, and the handler decides when the caller resumes.

import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}

fn countdown(n: Int) {
  console = use Console
  clock = use Clock
  List.map(List.range(0, n), fn(i) {
    clock.sleep(100)
    console.print_line("#{n - i}")
  })
  console.print_line("liftoff")
}

fn main() {
  with SystemConsole, SystemClock {
    countdown(3)
  }
}

# Resumes on the spot, so tests do not wait and need no `Async`.
handler InstantClock implements Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { Instant.from_unix_milliseconds(0) }
}

@test
fn countdown_does_not_wait() {
  with SystemConsole, InstantClock {
    countdown(3)
  }
}
```

```console
$ emela run
3
2
1
liftoff
```

The bundled std lives under `Std` and is imported: the platform effects
`Std.Console`, `Std.Clock`, `Std.Random` and `Std.Env`, each with its host
handler (`SystemConsole`, `SystemClock`, `SystemRandom`, `SystemEnv`), the layer
`Std.Platform.Platform` that bundles them, `Std.State`, `Std.Instant` and
`Std.Duration`. The test handlers `TestConsole` and `TestClock` keep their state
in `State`. The core modules (`List`, `Option`, `String`, `Int`, `Int64`,
`Float`) need no import.

Not yet supported: fibers.

## Crates

| Crate                | Role                                                                     |
| -------------------- | ------------------------------------------------------------------------ |
| `emela-syntax`       | Lexer, parser, lossless syntax tree, error recovery                      |
| `emela-fmt`          | Formatter                                                                |
| `emela-resolve`      | Module loading, imports, name resolution, `pub` / `opaque`, naming rules |
| `emela-types`        | Type inference, `fails` / `use` inference, traits, exhaustiveness        |
| `emela-core`         | Core IR and lowering: `derive`, tail-call loops, `layer` wiring          |
| `emela-codegen-js`   | JavaScript output and the embedded JS runtime                            |
| `emela-codegen-wasm` | WebAssembly output and the embedded WASM runtime                         |
| `emela-driver`       | Pipeline, diagnostics, `Pome.toml`                                       |
| `emela-lsp`          | Language server                                                          |
| `emela-cli`          | The `emela` binary                                                       |

```
syntax → resolve → types → core → codegen-js / codegen-wasm
   ↓                                      ↓
  fmt                                  driver → lsp → cli
```

## Building

```sh
cargo build
cargo test
```
