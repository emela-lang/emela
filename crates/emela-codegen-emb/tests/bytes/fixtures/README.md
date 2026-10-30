# Bytes corpus

`bytes.emel` and `bytes.expected` were copied without changes from
`crates/emela-codegen-c/tests/programs/` at `b5d893a`.
The embedded tests own these copies so they do not read another backend's
private test files. The test harness bounds the long hexadecimal result to
the embedded String capacity before compiling it.
