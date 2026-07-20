# Emela

Emela is an experimental functional language that compiles to **WebAssembly**
(Tier 1) and **JavaScript** (Tier 2). This repository holds the CLI, the compiler
for the current core subset, and the editor tooling (LSP and syntax highlighting).
The full language spec lives in
[`emela-lang/specification`](https://github.com/emela-lang/specification).

The language is pre-1.0 and moving quickly: within the `0.y.z` range a minor bump
may include breaking language changes. Stable releases are published from `main`
(see the [CHANGELOG](CHANGELOG.md)); dev prereleases are published from `dev`.

- **[Install](#install)** · **[Quick start](#quick-start)** ·
  **[CLI](#cli)** · **[Language features](#language-features)** ·
  **[Syntax by example](#syntax-by-example)** ·
  **[Editor support](#editor-support)** · **[Packages & Pomes](#packages)**

## Install

### Release binary (curl)

Prebuilt binaries are published for macOS (Apple Silicon) and Linux (x86_64). The
installer downloads the latest **stable** release:

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | sh
emela --version
```

By default this installs into `$HOME/.emela/bin`. Environment variables:

| Variable            | Effect                                                     |
| ------------------- | ---------------------------------------------------------- |
| `EMELA_INSTALL_DIR` | install location (default `$HOME/.emela/bin`)              |
| `EMELA_VERSION`     | pin an exact release tag, e.g. `0.7.1`                     |
| `EMELA_CHANNEL`     | `stable` (default) or `nightly` (latest `dev` prerelease) |

### Nix (flake)

The [`emela-lang/emela.nix`](https://github.com/emela-lang/emela.nix) flake
provides `emela` with no Rust toolchain required. By default it fetches the
prebuilt release binary; `emela-source` builds `main` HEAD from source with the
pinned toolchain.

```sh
nix run github:emela-lang/emela.nix                 # run the prebuilt binary
nix profile install github:emela-lang/emela.nix#emela
nix run github:emela-lang/emela.nix#emela-source    # build main HEAD from source
```

Use it as an overlay to get `pkgs.emela` (and `pkgs.emela-source`), e.g. on NixOS
or home-manager via `environment.systemPackages = [ pkgs.emela ];`. Prebuilt
binaries cover `aarch64-darwin` and `x86_64-linux`; `aarch64-linux` and
`x86_64-darwin` fall back to a source build. See the flake's README for the
overlay, `override { fromSource = true; }`, and the Rust `devShell`.

### From source

Build with Cargo (edition 2024, Rust 1.85+) — see [Building](#building).

## Requirements

- To install a release: nothing beyond `curl` and `tar` (or Nix).
- To run generated JavaScript: Node.js.
- To build from source: a Rust toolchain with Cargo (edition 2024, Rust 1.85+).
- Optional: a WASI runtime (`wasmtime` or WAMR's `iwasm`) — only to run a built
  `.wasm` artifact directly. `emela run` embeds its own pure-Rust WASI runtime
  ([`wasmi`]), and building needs no external wasm tools.

[`wasmi`]: https://github.com/wasmi-labs/wasmi

## Quick start

```sh
emela new hello          # scaffold hello/ with a Pome.toml
cd hello
emela run src/main.emel  # build to wasm and run it in-process
```

`emela run` builds with the `wasm-wasi` backend and executes the module in
process — no external runtime needed. `main`'s `Int` result is the process exit
code:

```sh
emela run examples/add.emel; echo $?     # 42
emela run examples/hello.emel            # Hello, Emela!
```

The same program builds to either backend. `main`'s `Int` result is the exit code
for wasm; the JS backend prints to stdout via Node:

```sh
emela build --backend js-node examples/add.emel | node     # 42

emela build --backend wasm-wasi -o /tmp/add.wasm examples/add.emel
wasmtime /tmp/add.wasm; echo $?                             # 42
```

The generated `.wasm` is a plain WASI preview1 module, so the built artifact also
runs under `wasmtime` or WAMR's `iwasm`. Programs that do real I/O import the
embedded std modules (spec 0038) — the `Io` effect ships inside the compiler, so
no `--package` is needed. Every file under `examples/` type-checks and builds.

## CLI

Run the installed `emela` binary (or `cargo run --bin emela -- <args>` from a
source checkout):

```text
emela new <name>                        # scaffold a new Pome
emela check [--library] FILE            # type-check only
emela run   [--package DIR] FILE        # build to wasm and run it in-process
emela build [--backend NAME] [-o OUT] FILE
emela ir    FILE                        # print the typed IR
emela backends                          # list backends (wasm-wasi, js-node)
emela pome  <add|remove|list|update|install|search> ...   # dependencies
emela lsp   [--package DIR]             # LSP server over stdio (docs/lsp.md)
```

`--emit text` prints WAT for the wasm backend; `emela ir` prints the IR.

## Language features

Implemented in the current compiler:

- top-level `fn`, a `main` entry point, block expressions, immutable `let`
- primitives `Unit`, `Bool`, `Int`, `Float`, `String`, `Char`, and `Array<T>`
- arithmetic `+ - * /` (and `%` on `Int`), comparisons `== != < > <= >=`,
  short-circuiting `&& || !`, `String` concatenation `++`
- the pipeline operator `|>`: `x |> f |> g(a)` is `g(f(x), a)` (spec 0019)
- `if / else` as an expression
- first-class functions: function values, `fn` lambdas, closures, higher-order
- generic functions `fn f<T>(...)` — type arguments inferred, then monomorphized
- `record` declarations with named fields and `.` field access (spec 0006)
- `enum` + exhaustive `match` with pattern guards, including generic and
  recursive enums (`enum List<T> { Nil, Cons(T, List<T>) }`)
- `trait` / `impl` with monomorphized dispatch, a Core Prelude (`Show`, operator
  traits), and receiver method-call sugar `x.f()` for `f(x)` (spec 0020/0021)
- error handling: `throws E`, `throw`, the `?` propagation operator, `try` /
  `catch`, `panic`; `Option<T>` for absent values (there is no built-in `Result`)
- first-class effects: `effect Name { ... }`, granted with `uses { Name }` and
  called as `Name.op(...)`, checked against each function body (spec 0037)
- the `Http` capability for synchronous HTTP requests (specs 0043–0046)
- `module` / `pub` / `import` across files and source packages, and **Pomes** for
  Git-based distribution and dependencies (spec 0032)
- WebAssembly and JavaScript backends (in-process or external plugin), with
  deterministic reference counting (ARC) on the wasm backend (spec 0048)

Conventions: enum variants and trait/record types are **type paths written with
`::`** (`List::Nil`, `Color::Red`); `.` is reserved for module, field, and
receiver access. Identifiers use `snake_case`; types, records, and enum variants
use `PascalCase`. The primitive conversions and array operations are bare
intrinsic functions (`char_from_code`, `string_from_char`, `array_get`, …).

Not yet implemented: explicit type arguments, generic function values,
effect/error-row polymorphism, and a native backend.

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
    "digit " ++ string_from_char(char_from_code(48 + n))
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

Records — named fields, constructed with a record literal, read with `.`:

```emela
record User {
  id: Int
  name: String
}

fn greet(u: User) -> String {
  "hi " ++ u.name
}

fn main() -> String {
  greet(User { id: 7, name: "ada" })
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

Traits and `impl`, dispatched by the argument type — a value `c` can call an
`impl` method as either `to_string(c)` or the receiver form `c.to_string()`:

```emela
enum Color {
  Red
  Green
  Blue
}

impl Show for Color {
  fn to_string(c: Color) -> String {
    match c {
      Red -> "red"
      Green -> "green"
      Blue -> "blue"
    }
  }
}

fn main() -> String {
  let c = Color::Green
  c.to_string()
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

Effects are first-class (spec 0037): `effect Name { ... }` declares one, its
operations are called as `Name.op(...)`, and `uses { Name }` grants a function
access. Effects propagate to callers, which must declare at least the same set:

```emela
effect Stdout {
  pub fn log_line() -> Unit { () }
}

fn announce() -> Unit uses { Stdout } {
  Stdout.log_line()
}

fn main() -> Unit uses { Stdout } {
  announce()
}
```

Real side effects enter through **platform functions** (`extern fn`) inside a
stdlib effect, resolved by the selected backend's runtime — app code never names
a backend. `std.io` (embedded in the compiler) owns the `Io` effect:

```emela
import std.io

fn main() -> Unit uses { Io } {
  Io.print("Hello, Emela!\n")
}
```

## Editor support

The compiler binary doubles as a language server, and this repository ships
syntax definitions for several editors. Status:

| Editor / tool     | Syntax highlighting              | LSP (`emela lsp`)        |
| ----------------- | -------------------------------- | ------------------------ |
| Neovim / Vim      | ✅ `editors/nvim/`               | ✅ built-in client       |
| VS Code           | ✅ TextMate grammar + extension  | ✅ `editors/vscode/`     |
| Sublime / `bat`   | ✅ `.sublime-syntax` (syntect)   | —                        |
| Any LSP client    | —                                | ✅ stdio, `.emel` files  |
| Tree-sitter       | ⬜ not yet (`tree-sitter-emela`) | —                        |

**LSP** (`emela lsp`, spec 0033) speaks LSP over stdio and provides:

- **Diagnostics** on open/change/save, covering every compiler error — lexer,
  parser, imports, and the type checker (type errors, missing trait methods,
  unhandled effects, `throws` mismatches, non-exhaustive matches, …). Errors are
  collected across declarations, so independent mistakes all show at once, and
  unsaved buffers drive cross-file resolution.
- **Completion**, context-aware: `import` paths, `match`/`catch` variants (typed
  from the scrutinee), effect names in `uses { … }`, variants after `Enum::`, and
  the default mix of keywords, types, functions, and locals.

Setup instructions per editor are in **[docs/lsp.md](docs/lsp.md)** and
**[docs/syntax-highlight.md](docs/syntax-highlight.md)**.

## Packages

`--package DIR` adds a source root; `DIR` needs an `emela-package.json`:

```json
{ "name": "math", "source": "src" }
```

Then `import math.ops.add_one` loads `DIR/src/ops.emel` (which must declare
`module ops`) and imports the `pub` function `add_one`. Imports without a package
name resolve relative to the importing file.

## Pomes: distribution and dependencies

A **Pome** is Emela's unit of distribution — one or more modules supplied as a
Git repository (spec 0032). There is no central registry: a Pome is identified by
its source path `host/path` and fetched straight from that repository, versioned
by `v`-prefixed semver git tags.

```sh
emela new hello                        # scaffold hello/ with a Pome.toml
cd hello
emela pome add github:emela-lang/stdlib   # fetch, pin in Pome.lock, audit capabilities
emela pome list                            # print the resolved dependency tree
emela build src/main.emel                  # deps are on the import path automatically
```

Once a Pome is a dependency, building any file inside your Pome puts its modules
on the import path — no `--package` needed. The import root is the dependency's
source-path leaf by default (a Pome may override it with `[pome].module`, spec
0032 M2) and its modules live under `src/`, so `github.com/emela-lang/stdlib`
declaring `module = "std"` and exposing `src/io.emel` (`module io`) is used as:

```
import std.io.print         -- callable as print, io.print, or std.io.print
```

`emela pome add` records the dependency in `Pome.toml` under its canonical source
path, pins the resolved tag + commit + content hash in `Pome.lock`, and — since a
Pome's required capabilities are computable from source (spec 0025) — prints the
capability set the added Pome and its transitive dependencies require *before*
committing, so `net`/`fs`/`clock` growth is auditable at add time.

```toml
# Pome.toml
[pome]
name = "github.com/emela-lang/json"
version = "1.2.0"
emela = "0.1"

[dependencies]
"github.com/emela-lang/parser" = "^2.0"
```

Publishing is just tagging: `git tag v0.1.0 && git push origin v0.1.0`. Several
Pomes developed together can share a workspace via `Bushel.toml`. To resolve
against a local checkout or mirror (offline development, CI), set
`EMELA_POME_REPLACE="host/path=/local/or/url"`; `EMELA_POME_CACHE` redirects the
fetch cache.

## Building

```sh
cargo build
cargo test
cargo fmt        # format
```

Building needs no external wasm tools, and `emela run` bundles a WASI runtime, so
an external runtime is only needed to *run* a `.wasm` file yourself.

## Specification

The language design lives in numbered SPECs under
[`emela-lang/specification`](https://github.com/emela-lang/specification). This
repository implements the current core subset; the SPEC referenced next to each
feature above is the authoritative description of its semantics.
