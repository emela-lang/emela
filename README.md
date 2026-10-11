# Emela

Emela is a functional language with typed errors and typed effects, compiling to
JavaScript and WebAssembly.

## Status

This branch is the 0.20 rebuild. The current release is **0.20.0-alpha.1**: the
pure subset of the language compiles to JavaScript and runs on Node.js. The 0.11
compiler lives on the [`legacy`](https://github.com/emela-lang/emela/tree/legacy)
branch.

| Release        | Adds                                                                                                                       |
| -------------- | -------------------------------------------------------------------------------------------------------------------------- |
| 0.20.0-alpha.1 | Functions, records, enums, `opaque` types, pattern matching, tail calls, pipes, `List` / `Option`, modules, `@test`, `dbg` |
| 0.20.0-alpha.2 | Typed errors and effects: `error`, `fail`, `escape`, `effect`, `handler`, `use`, `with`, `@external`                       |
| 0.20.0-alpha.3 | `suspend`, `Async`, `Console`                                                                                              |
| 0.21 and later | `trait`, `impl`, `derive`, `Map`, `Set`, WebAssembly output                                                                |

The 0.20 language specification is still being written. The
[`emela-lang/specification`](https://github.com/emela-lang/specification)
repository describes the 0.1x language, not 0.20.

## Install

Prebuilt binaries are published for macOS (Apple Silicon) and Linux (x86_64).
Running the generated JavaScript (`emela run`, `emela test`) needs Node.js.

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | EMELA_VERSION=v0.20.0-alpha.1 sh
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
# A tour of what 0.20.0-alpha.1 compiles. `dbg` prints to stderr.
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

Programs print with `dbg`, which writes to stderr. The value `main` returns is
not printed.

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
