# brix v0.3.0, moved to Std.Exit

A copy of `github.com/emela-lang/brix` at v0.3.0 (commit `54932ed2d540563e796f4fde4dc3d80073685479`),
with `Fiber.attempt` and the imports of `Success` and `Failure` from `Std.Fiber` rewritten to
`Std.Exit` (`proposals/std-exit.md`). Only `src/` and `Pome.toml` are copied.

`tests/laws/Pome.toml` uses it through `[replace]`, because a release of brix that uses `Std.Exit`
needs a released compiler that has it. Once emela 0.27.0 is released, brix v0.4.0 is released with
this change and `emela = "0.27.0"`, the `[replace]` is removed together with this directory, and
`tests/laws/Pome.lock` moves to v0.4.0.
