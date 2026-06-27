# Backend Profiles

This directory documents reference backend profiles.

Each profile combines a backend implementation with one runtime environment.
For example, `js-node` and `js-bun` share the JavaScript emitter but can provide
different capabilities and platform bindings. `native-aarch64-apple-darwin` and
`native-x86_64-unknown-linux-gnu` share the native emitter but use different
target ABI details.

External backend descriptors use `command`. Built-in descriptors use `builtin`
instead because they are dispatched in-process by the compiler.
