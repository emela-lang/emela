# Vendored WASI 0.3 WIT

`deps/` holds the `wasi:cli@0.3.0` package and the packages its worlds refer to
(`wasi:clocks`, `wasi:filesystem`, `wasi:random`, `wasi:sockets`). They are
copied from the p3 WIT of `wasmtime-wasi` 50.0.0-rc.1
(`src/p3/wit/deps/*.wit`), which implements the same WASI 0.3 interfaces as
wasmtime 49. The files are Apache-2.0 WITH LLVM-exception, like wasmtime; the
license text is in `LICENSE`. `deps/http.wit` (`wasi:http@0.3.0`) is copied
from the p3 WIT of `wasmtime-wasi-http` 50.0.0 (`src/p3/wit/deps/http.wit`);
its dependencies are the same files as above.

`emela.wit` is this compiler's command world. It always imports stdout, stderr
and exit. The `// @optional-imports` marker is replaced by
`src/component.rs` with the environment, random and system clock imports when
the program uses the corresponding bundled `Std` handler. An unused interface
is therefore not an import of the output component. The world exports
`wasi:cli/run@0.3.0`.

`service.wit` is the world of an HTTP service (`@entry(Http.serve)`). It exports
`wasi:http/handler@0.3.0` instead of `wasi:cli/run@0.3.0`, imports
`wasi:http/types@0.3.0`, and imports stdout, stderr and exit like the command
world. It has the same `// @optional-imports` marker.
