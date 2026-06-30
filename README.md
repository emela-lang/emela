# Emela

Emela is an experimental functional language intended to compile to native code
and WebAssembly. This repository contains the early Emela CLI and compiler for
the minimal core language. The current build type-checks the core language and
generates JavaScript.

The language specification lives in the separate `emela-lang/specification`
repository. This README documents what the compiler in this repository actually
implements today, which is a small subset of the full language.

## What the compiler supports

- top-level `fn` definitions
- a `main` entry point (no parameters)
- block expressions and immutable `let` bindings, with optional type annotations
- primitive types `Unit`, `Bool`, `Int`, `Float`, and `String`
- `Array<T>` literals, including nested arrays
- function types such as `(Int) -> Int` and `(Int, Int) -> Int uses { ... }`
- first-class functions: function values, `fn` lambda expressions, closures, and
  higher-order functions
- numeric arithmetic `+`, `-`, `*` on matching `Int` or `Float` operands
- comparisons `==` and `<` on matching numeric operands, producing `Bool`
- effect rows declared with `uses { ... }`, checked so a body's effects are a
  subset of the function's declared effects
- `module`, `pub`, and `import` for splitting code across files and source
  packages
- line comments starting with `--`
- JavaScript code generation, plus a textual IR dump for inspection

The type names `Record`, `Enum`, and `Function` are accepted in signatures, but
there is no literal or constructor syntax for their values yet, so they cannot be
used in runnable code.

## Not yet implemented

To set expectations, the following are **not** part of this build:

- no `if`, `match`, or other control flow beyond function calls and blocks
- no `struct`, `enum`, `trait`, or `impl` declarations
- no string concatenation or boolean operators
- no native or WebAssembly code generation; the only backend is JavaScript
- no platform capability checking; effect names are opaque labels
- no project manifest, dependency fetching, or external backend processes

## Requirements

- Rust toolchain with Cargo, edition 2021 compatible (tested with `rustc 1.84.1`)
- `rustfmt`, normally installed with the Rust toolchain
- Node.js to run the generated JavaScript

The compiler depends on `serde` and `serde_json` for reading package manifests.

## Build and test

```sh
cargo build
cargo fmt
cargo test
```

Run the compiler through Cargo with `cargo run --bin emela -- <args>`, or use the
installed `emela` binary directly.

## CLI usage

```text
emela check [--backend js-node] [--package DIR] FILE
emela build [--backend js-node] [--package DIR] [-o FILE] FILE
emela ir            [--package DIR] [-o FILE] FILE
emela --version
```

- `check` type-checks a program without producing output.
- `build` emits JavaScript. Without `-o`/`--output` it prints to stdout; with it,
  the JavaScript is written to the given file.
- `ir` prints the lowered intermediate representation as text.
- `--backend` is optional and only accepts `js-node` (alias `js`).
- `--package DIR` adds a source package root (see [Packages](#packages)).

Type-check an example:

```sh
cargo run --bin emela -- check --backend js-node examples/maximal.emel
```

Build and run an example with Node.js:

```sh
cargo run --bin emela -- build --backend js-node examples/add.emel | node
# prints 42
```

Inspect the lowered IR:

```sh
cargo run --bin emela -- ir examples/add.emel
```

## Examples

All files under `examples/` type-check and build with this compiler. Each
standalone example below is run with:

```sh
cargo run --bin emela -- build --backend js-node examples/<file>.emel | node
```

| File | Demonstrates | Output |
| --- | --- | --- |
| `minimal.emel` | the smallest valid program | _(none; returns `Unit`)_ |
| `add.emel` | functions, typed parameters, calls | `42` |
| `string.emel` | `String` values and `let` bindings | `Hello, Emela!` |
| `function_values.emel` | function values, higher-order functions, closures | `63` |
| `effects.emel` | `uses { ... }` effect rows and propagation | _(none; returns `Unit`)_ |
| `maximal.emel` | the largest subset that compiles, combined | `44` |
| `imports/main.emel` | `module` / `pub` / `import` across files | `37` |

`imports/main.emel` imports from the sibling module `imports/geometry.emel`. The
module file has no `main`, so it is consumed via `import` rather than checked on
its own.

## Language tour

Minimal program:

```emela
fn main() -> Unit {
}
```

Functions and calls:

```emela
fn add(x: Int, y: Int) -> Int {
  x + y
}

fn main() -> Int {
  add(20, 22)
}
```

`let` bindings and blocks (blocks are expressions; the last expression is the
value):

```emela
fn main() -> Int {
  let base: Int = 20
  let computed = {
    let stepped = base + 1
    stepped * 2
  }
  computed
}
```

Function values and closures:

```emela
fn apply(f: (Int) -> Int, x: Int) -> Int {
  f(x)
}

fn make_adder(n: Int) -> (Int) -> Int {
  fn (x: Int) -> Int {
    x + n
  }
}

fn main() -> Int {
  let add10 = make_adder(10)
  apply(add10, 32)
}
```

Effects:

```emela
fn log_line() -> Unit uses { Stdout } {
  ()
}

fn main() -> Unit uses { Stdout } {
  let printed: Unit = log_line()
  ()
}
```

## Packages

`--package DIR` adds a source package root. `DIR` must contain
`emela-package.json`:

```json
{
  "name": "math",
  "source": "src"
}
```

With that package, `import math.ops.add_one` loads `DIR/src/ops.emel` and imports
the public function `add_one`. The module file must declare a matching
`module ops`, and only `pub` functions can be imported.

Imports that do not name a package are resolved relative to the importing file.
For example, `import geometry.square` loads `geometry.emel` from the same
directory, which must declare `module geometry`.

## Install

Dogfooding builds are published from `main` as timestamped prereleases. They are
intended for quickly trying the current compiler state, not for stable production
use.

Install the latest dogfooding build:

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | sh
```

By default this installs `emela` into `$HOME/.emela/bin`. Set `EMELA_INSTALL_DIR`
to choose another directory, and `EMELA_VERSION` to install a specific release
tag.

Check the installed version:

```sh
emela --version
```
