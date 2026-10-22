# 16.8 The WASM target `[target.wasm]`

This section defines what can be observed about the component emitted with `--target wasm` and about running it. The meaning of numbers and strings follows 16.1 and 16.2.

## 16.8.1 Form of the output `[target.wasm.output]`

- The name of the target is `wasm`. It is selected with `--target wasm` on the command, or with `[build].target = "wasm"` in `Pome.toml`.
- `emela build` and `emela run` write exactly one file, `main.wasm`, to the output directory. The runtime is included in the same component, and no other files are written. No external toolchain is used.
- The default output directory is `target/emela/wasm` for `emela build` and `emela run`, and `target/emela/test-wasm` for `emela test`.
- `main.wasm` is a WASI 0.3 component. It exports the interface `wasi:cli/run@0.3.0`, whose `run` calls `pub fn main` of the entry module. If the entry module has no `main`, `run` does nothing and returns.
- The component imports exactly `wasi:cli/stdout@0.3.0`, `wasi:cli/stderr@0.3.0` and `wasi:cli/exit@0.3.0`, whatever the program does. Standard output and standard error are each written through one stream passed to `write-via-stream`, and an exit code other than 0 is passed with `exit-with-code`. The output of `dbg` is written to standard error.
- The bytes of a write are handed to the host before the write returns. If the host stops reading a stream, later writes to it are dropped without an error.

## 16.8.2 Running `[target.wasm.run]`

- `emela run` writes `main.wasm` and then runs it with `wasmtime run -S p3=y main.wasm <args...>`. The arguments written after `--` are passed as they are, after `main.wasm`. Standard input is inherited. wasmtime provides the WASI 0.3 features, which need wasmtime 49 or later.
- The wasmtime used is the executable in the environment variable `EMELA_WASMTIME` if it is not empty, and otherwise `wasmtime` on PATH.
- If wasmtime cannot be started, it is E0909. When it is not found, the message is ``` `<executable>` not found ```, with the note `install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable`. When it cannot be started for any other reason, the message is ``` cannot start `<executable>`: <reason> ```. `<executable>` is the value of `EMELA_WASMTIME` or `wasmtime`.
- Before running, the command reads the version from `<executable> --version`. If its major version is older than 49, it is E0909 and the program is not run: the message is ``` `<executable>` is <version>, but target `wasm` needs wasmtime 49 or later ```, with the note `install Wasmtime 49 or later, or set EMELA_WASMTIME to a newer wasmtime executable`. `<version>` is the output of `--version` without the surrounding whitespace. If the version cannot be read, the program is run. The same check is made by `emela test` (`[cmd.test]`).
- If a defect remains when main ends, one line `defect: <message>` is written to standard error, and the program exits with exit code 101. If an error remains, one line `error: <the error value shown>` is written, and the program exits with exit code 1 (14.3). If neither remains, it exits with exit code 0.

## 16.8.3 External functions `[target.wasm.external]`

- A fn with `@external(wasm, ...)` that is reachable from the entry point is a compile error with no code: `` external function `f` has a `wasm` import, which the WASI 0.3 component target does not support yet ``. A component can only import the interfaces of its WIT world, and declaring the WIT of a host written by a user is not supported yet (`[open.list]` #56). Only this error is reported for such a fn, not also an error for its parameter types, for being a `suspend fn` or for taking a callback. E0427 and E0428 are not used on this target.
- The handlers of the std platform (`Std.Console`, `Std.Clock`, `Std.Random`, `Std.Env`, `Std.FileSystem`) work on WASM. They reach the host through the runtime in `main.wasm`, so the limits above do not apply to them.

## 16.8.4 Constructs not supported yet `[target.wasm.unsupported]`

None. Every construct of the language, including suspension, fibers, the Float conversions, the grapheme cluster operations of String, and the notes for release defects (14.4), can be emitted to WASM. The limits on external functions are in `[target.wasm.external]`.
