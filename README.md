# Emela

Emela is an experimental functional language that compiles to **WebAssembly**
(Tier 1) and **JavaScript** (Tier 2). This repository is the CLI and compiler for
the current core subset; the full spec lives in `emela-lang/specification`.

Editor syntax highlighting: see [docs/syntax-highlight.md](docs/syntax-highlight.md).

## Install

Timestamped dogfooding builds are published from `main` for trying the current
compiler — not for production:

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | sh
emela --version
```

By default this installs into `$HOME/.emela/bin`. Set `EMELA_INSTALL_DIR` to
change the location and `EMELA_VERSION` to pin a release tag. To build from
source instead, see [Build and test](#build-and-test).

## Requirements

- Rust toolchain with Cargo, edition 2024 (Rust 1.85+)
- Node.js — to run generated JavaScript
- A WASI runtime (`wasmtime` or WAMR's `iwasm`) — to run generated wasm

Building needs no external wasm tools; a runtime is only needed to *run* output.

## Build and test

```sh
cargo build
cargo test
cargo fmt        # format
```

## Running programs

Invoke the compiler with `cargo run --bin emela -- <args>` (or the installed
`emela` binary):

```text
emela check [--library] FILE          # type-check only
emela ir    FILE                       # print the typed IR
emela build [--backend NAME] [-o OUT] FILE
emela backends                         # list backends (wasm-wasi, js-node)
```

Build and run as JavaScript (Tier 2):

```sh
cargo run --bin emela -- build --backend js-node examples/add.emel | node
# 42
```

Build and run as WebAssembly (Tier 1) — `main`'s `Int` result is the exit code:

```sh
cargo run --bin emela -- build --backend wasm-wasi -o /tmp/add.wasm examples/add.emel
wasmtime /tmp/add.wasm; echo $?    # 42
```

Programs that do real I/O use the bundled stdlib package via `--package`:

```sh
cargo run --bin emela -- build --backend js-node --package examples/stdlib examples/hello.emel | node
# Hello, Emela!
```

Every file under `examples/` type-checks and builds. `--emit text` prints WAT for
the wasm backend; `emela ir` prints the IR.

## What it supports

- top-level `fn`, a `main` entry point, block expressions, immutable `let`
- primitives `Unit`, `Bool`, `Int`, `Float`, `String`, `Char`, and `Array<T>`
- arithmetic `+ - * /` (and `%` on `Int`), comparisons `== != < > <= >=`,
  short-circuiting `&& || !`, `String` concatenation `++`
- `if / else` as an expression
- first-class functions: function values, `fn` lambdas, closures, higher-order
- generic functions `fn f<T>(...)` — type arguments inferred, then monomorphized
- `enum` + exhaustive `match` with pattern guards, including generic and
  recursive enums (`enum List<T> { Nil, Cons(T, List<T>) }`)
- error handling: `throws E`, `throw`, the `?` propagation operator, `try` /
  `catch`, `panic`; `Option<T>` for absent values (there is no built-in `Result`)
- effect rows `uses { ... }`, checked against each function body
- `module` / `pub` / `import` across files and source packages
- WebAssembly and JavaScript backends (in-process or external plugin)

Enum variants and the built-in conversions are **type paths written with `::`**
(`List::Nil`, `Color::Red`, `Char::from_code`); `.` is reserved for module and
receiver access. Identifiers use `snake_case`; types and enum variants use
`PascalCase`. Not yet implemented: `struct`/`record`, explicit type arguments,
generic function values, effect/error-row polymorphism, a native backend.

## Syntax by example

Functions, `let`, and blocks (a block is an expression; its last line is the value):

```emela
fn add(x: Int, y: Int) -> Int {
  x + y
}

fn main() -> Int {
  let base: Int = 20
  let doubled = {
    let stepped = base + 1
    stepped * 2
  }
  add(doubled, 0)
}
```

`if` expression, operators, and `Char` / `String`:

```emela
fn label(n: Int) -> String {
  if n < 10 && n >= 0 {
    "digit " ++ String::from_char(Char::from_code(48 + n))
  } else {
    "other"
  }
}
```

Function values, closures, and generics (type arguments are inferred):

```emela
fn make_adder(n: Int) -> (Int) -> Int {
  fn (x: Int) -> Int { x + n }
}

fn identity<T>(x: T) -> T { x }

fn main() -> Int {
  let add10 = make_adder(10)
  identity(add10(32))
}
```

Enums and exhaustive `match` (variants are constructed with `::`):

```emela
enum Color {
  Red
  Green
  Blue
}

fn code(c: Color) -> Int {
  match c {
    Red -> 1
    Green -> 2
    Blue -> 3
  }
}

fn main() -> Int {
  code(Color::Red)
}
```

Generic, recursive enums:

```emela
enum List<T> {
  Nil
  Cons(T, List<T>)
}

fn length<T>(xs: List<T>) -> Int {
  match xs {
    Nil -> 0
    Cons(h, t) -> 1 + length(t)
  }
}

fn main() -> Int {
  let xs: List<Int> = List::Cons(1, List::Cons(2, List::Nil))
  length(xs)
}
```

Error handling with `throws` / `throw` / `try` / `catch`, plus `Option`:

```emela
enum ParseError {
  Empty
  BadDigit
}

fn parse_digit(s: String) -> Int throws ParseError uses {} {
  throw ParseError::BadDigit
}

fn parse_or(s: String, fallback: Int) -> Int uses {} {
  try {
    parse_digit(s)
  } catch {
    ParseError::Empty -> 0
    ParseError::BadDigit -> fallback
  }
}

fn unwrap_or(opt: Option<Int>, fallback: Int) -> Int uses {} {
  match opt {
    Some(value) -> value
    None -> fallback
  }
}
```

Effects are declared with `uses { ... }` and checked to be a subset of the body's:

```emela
fn log_line() -> Unit uses { Stdout } { () }

fn main() -> Unit uses { Stdout } {
  let printed: Unit = log_line()
  ()
}
```

Side effects enter only through **platform functions** (`extern fn`), resolved by
the selected backend's runtime. A stdlib module wraps them so app code never
names a backend:

```emela
module io

extern fn write_stdout(s: String) -> Unit uses { io }

pub fn print(s: String) -> Unit uses { io } {
  write_stdout(s)
}
```

## Packages

`--package DIR` adds a source root; `DIR` needs an `emela-package.json`:

```json
{ "name": "math", "source": "src" }
```

Then `import math.ops.add_one` loads `DIR/src/ops.emel` (which must declare
`module ops`) and imports the `pub` function `add_one`. Imports without a package
name resolve relative to the importing file.
