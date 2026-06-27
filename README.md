# Emela Compiler

Emela is an experimental functional language intended to compile to native code and WebAssembly.
This repository contains the early compiler implementation for the minimal core language.

The current compiler supports:

- top-level `fn` definitions
- `main` and `main!` executable entry points
- block expressions and immutable local bindings
- `I32`, `Bool`, and `Unit`
- required type annotations on function parameters, function returns, and local bindings
- single-field `struct` declarations and field access
- `enum` declarations with zero or one payload value per variant
- `Result`-style enums with `match` over variant patterns
- function calls
- function type annotations and function values for type-checking
- forward pipeline calls with `|>`
- primitive method calls such as `x.add(y)`
- operators backed by primitive trait-style methods: `+`, `-`, `*`, `==`, `<`
- `match` expressions over integer, boolean, unit, and wildcard patterns
- effect markers with `!`
- top-level `import` declarations for compiler-known external functions
- platform capability declarations with `#[requires(...)]`
- platform capability checking from either built-in target defaults or `--platform` manifests
- native assembly generation for `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`
- JavaScript generation for the current core subset with manifest-provided external bindings
- library checking mode for compilation units without `main` / `main!`

The language specification lives in the separate `emela-lang/specification` repository.

## Requirements

Development requires:

- Rust toolchain with Cargo, edition 2021 compatible; currently tested with `rustc 1.84.1`
- `rustfmt`, normally installed with the Rust toolchain
- Apple arm64 macOS or x86_64 Linux for native executable builds
- A system C compiler available as `cc` for assembling and linking generated native assembly when building executables

The native backend can emit assembly with `--emit-asm` without invoking `cc`.
Building an executable invokes the host `cc`, so native executable builds require a matching host for the selected target.

The compiler currently has no third-party Rust crate dependencies.

## Supported Targets

The compiler recognizes these target triples:

| Target | Capability checking | Code generation |
| --- | --- | --- |
| `aarch64-apple-darwin` | Yes | Native arm64 assembly |
| `x86_64-unknown-linux-gnu` | Yes | Native x86_64 System V assembly |
| `wasm32-unknown-unknown` | Yes | Not implemented |
| `wasm32-wasi` | Yes | Not implemented |

Target capability sets currently follow SPEC-0003:

- `aarch64-apple-darwin`: `Stdout`, `Stdin`, `Stderr`, `FileRead`, `FileWrite`, `Clock`, `Random`, `Env`, `Process`, `Network`
- `x86_64-unknown-linux-gnu`: `Stdout`, `Stdin`, `Stderr`, `FileRead`, `FileWrite`, `Clock`, `Random`, `Env`, `Process`, `Network`
- `wasm32-unknown-unknown`: no platform capabilities
- `wasm32-wasi`: `Stdout`, `Stdin`, `Stderr`, `FileRead`, `FileWrite`, `Clock`, `Random`, `Env`

If `--target` is omitted, the compiler uses the host target. At the moment, automatic host target detection accepts Apple arm64 macOS and x86_64 Linux.

`--platform PATH` selects a platform manifest instead of the built-in target-default platform capability and external function registry. The manifest file is JSON:

```json
{
  "name": "node",
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {
        "js": {
          "callee": "console.log"
        }
      }
    }
  ]
}
```

`--stdlib DIR` selects the standard library root. If omitted, the compiler uses
`../stdlib` relative to the Cargo manifest directory. Imports such as
`import std.io.print_i32!` load Emela source from `DIR/std/io.emel`; stdlib
wrappers then call `platform.*` imports supplied by the selected platform.

## Common Commands

Format the code:

```sh
cargo fmt
```

Type-check and run tests:

```sh
cargo check
cargo test
```

Check an Emela source file without building:

```sh
cargo run -- --check examples/maximal.emel
```

Check a library source file without requiring `main` / `main!`:

```sh
cargo run -- --library --check ../stdlib/std/io.emel --platform ../stdlib/platform/native-libc.json
```

Check against a specific platform target:

```sh
cargo run -- --target wasm32-wasi --check examples/maximal.emel
```

Emit native assembly:

```sh
cargo run -- --emit-asm /tmp/emela-maximal.s --check examples/maximal.emel
```

Build a native executable on a matching host:

```sh
cargo run -- --target aarch64-apple-darwin examples/maximal.emel -o /tmp/emela-maximal
```

Emit x86_64 Linux assembly from any supported development host:

```sh
cargo run -- --target x86_64-unknown-linux-gnu --emit-asm /tmp/emela-maximal-x86_64.s --check examples/maximal.emel
```

Emit JavaScript using a platform manifest:

```sh
cargo run -- --platform /path/to/emela-platform.json --emit-js /tmp/emela.js --check examples/maximal.emel
```

Use the stdlib from user code:

```emela
import std.io.print_i32!

fn main!() -> Unit {
  42 |> print_i32!()
}
```

```sh
cargo run -- --platform ../stdlib/platform/node.json --emit-js /tmp/emela.js --check examples/std-print.emel
```

Run it and inspect the process exit code:

```sh
/tmp/emela-maximal
echo $?
```

`examples/add.emel` and `examples/maximal.emel` currently exit with code `42`.

## Examples

Minimal program:

```emela
fn main() -> Unit {
}
```

Integer computation:

```emela
fn add(x: I32, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
```

Effectful entry point with a platform capability:

```emela
#[requires(Stdout)]
fn tick!() -> Unit {
  ()
}

fn main!() -> I32 {
  tick!()
  42
}
```

## Current Limitations

- Native executable building uses the host `cc`; cross-target native builds are not implemented.
- WebAssembly targets are capability-checked only; WASM code generation is not implemented.
- The native backend supports the current core language subset only.
- Function values are type-checked, but native lowering is not implemented yet.
- Runtime implementations for real I/O capabilities are not connected yet.
- Imported external functions are type-checked and capability-checked. Native lowering uses manifest-provided runtime symbols.
- JavaScript external lowering requires a `bindings.js.callee` entry for each imported external function.
- Library mode can check stdlib source files, and user programs can import `std.*` modules from the configured stdlib root.
- User-defined traits, trait declarations, and impl declarations are not implemented.
- Effect handlers and error values are not implemented.
- Structs and enums are currently limited to the first draft subset: one field per struct, at most one payload per variant, and no generics.
