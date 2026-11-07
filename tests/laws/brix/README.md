# brix (copy)

A copy of the Pome `github.com/emela-lang/brix` that `tests/laws` builds against through
`[replace]` in `../Pome.toml`. The laws check the compiler, not a published brix, so a change to the
language that breaks brix updates this copy in the same pull request.

- Taken from brix v0.3.0 (commit `54932ed2d540563e796f4fde4dc3d80073685479`): `Pome.toml` and
  `src/` only.
- Changed since: `attempt` and `Exit` imported from `Std.Exit` (proposal `std-exit`), and
  anonymous functions written `|x| e` (proposal `closure-syntax`).

Changes to brix itself are made in its own repository and brought here by hand; changes made here
for the language go back to it once a compiler with them is released.
