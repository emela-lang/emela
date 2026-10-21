# Vendored WASI 0.3 WIT

`deps/` holds the `wasi:cli@0.3.0` package and the packages its worlds refer to
(`wasi:clocks`, `wasi:filesystem`, `wasi:random`, `wasi:sockets`). They are
copied from the p3 WIT of `wasmtime-wasi` 50.0.0-rc.1
(`src/p3/wit/deps/*.wit`), which implements the same WASI 0.3 interfaces as
wasmtime 49. The files are Apache-2.0 WITH LLVM-exception, like wasmtime; the
license text is in `LICENSE`.

`emela.wit` is this compiler's command world: it imports only the interfaces
the runtime uses (stdout, stderr, exit) and exports `wasi:cli/run@0.3.0`.
