# Emela

Emela is a functional language with typed errors and typed effects, compiling to
JavaScript and WebAssembly.

This branch is the 0.20 rebuild and does not compile programs yet. The 0.10
compiler lives on the [`legacy`](https://github.com/emela-lang/emela/tree/legacy)
branch. The language design lives in
[`emela-lang/specification`](https://github.com/emela-lang/specification).

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
| `emela-driver`       | Pipeline, diagnostics, `emela.toml`                                      |
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
