# 16.8 The WASM target `[target.wasm]`

This section defines what can be observed about the module emitted with `--target wasm` and about running it. The meaning of numbers and strings follows 16.1 and 16.2.

## 16.8.1 Form of the output `[target.wasm.output]`

- The name of the target is `wasm`. It is selected with `--target wasm` on the command, or with `[build].target = "wasm"` in `Pome.toml`.
- `emela build` and `emela run` write exactly one file, `main.wasm`, to the output directory. The runtime is included in the same module, and no other files are written. No external toolchain is used.
- The default output directory is `target/emela/wasm` for `emela build` and `emela run`, and `target/emela/test-wasm` for `emela test`.
- `main.wasm` is a WASI command. It exports `_start` (a function with no parameters and no return value) and the linear memory `memory`.
- `_start` calls `pub fn main` of the entry module. If the entry module has no `main`, it does nothing and ends.
- The module always imports `fd_write` and `proc_exit` from WASI's `wasi_snapshot_preview1`. Writes to standard output and standard error go through `fd_write`, and the exit code is passed with `proc_exit`. The output of `dbg` is written to standard error (descriptor 2).
- The only other imports are the external functions reachable from the entry point (`[target.wasm.external]`).

## 16.8.2 Running `[target.wasm.run]`

- `emela run` writes `main.wasm` and then runs it with `wasmtime run main.wasm <args...>`. The arguments written after `--` are passed as they are, after `main.wasm`. Standard input is inherited. wasmtime provides the WASI features.
- The wasmtime used is the executable in the environment variable `EMELA_WASMTIME` if it is not empty, and otherwise `wasmtime` on PATH.
- If wasmtime cannot be started, it is E0909. When it is not found, the message is ``` `<executable>` not found ```, with the note `install Wasmtime and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable`. When it cannot be started for any other reason, the message is ``` cannot start `<executable>`: <reason> ```. `<executable>` is the value of `EMELA_WASMTIME` or `wasmtime`.
- If a defect remains when main ends, one line `defect: <message>` is written to standard error, and the program exits with exit code 101. If an error remains, one line `error: <the error value shown>` is written, and the program exits with exit code 1 (14.3). If neither remains, it exits with exit code 0.

## 16.8.3 External functions `[target.wasm.external]`

- A fn with `@external(wasm, "module", "name")` becomes an import of `main.wasm`. The module name and the name of the import are the two strings of the annotation. When several fns have the same pair of module name and name, they share one import.
- Until the WASM output moves to the Component Model and passes values by its canonical ABI (`[open.list]` #56), the boundary types of an external function written by a user are narrower than in 12.4, limited to the following types.

  | Emela type | WASM type |
  |---|---|
  | Int, Bool | `i32` |
  | Int64 | `i64` |
  | Float | `f64` |
  | `()` as the return value | No return value |

- An external function written by a user that uses String, `@external type` or `()` for a parameter is a compile error with no code: `` external function `f` uses the type String, which cannot cross to WASM ``. An external function written by a user that is declared as `suspend fn` is a compile error with no code (`` external function `f` is a `suspend fn`, which target `wasm` does not support yet ``). E0428 is unused.
- An external function that takes or returns a callback is E0427 (`` external function `f` takes a callback, which target `wasm` does not support yet ``, or `returns a callback` for a return value).
- The handlers of the std platform (`Std.Console`, `Std.Clock`, `Std.Random`, `Std.Env`, `Std.FileSystem`) work on WASM. They reach the host through the runtime in `main.wasm`, so the limits above do not apply to them.

## 16.8.4 Constructs not supported yet `[target.wasm.unsupported]`

None. Every construct of the language, including suspension, fibers, the Float conversions, the grapheme cluster operations of String, and the notes for release defects (14.4), can be emitted to WASM. The limits on external functions are in `[target.wasm.external]`.
