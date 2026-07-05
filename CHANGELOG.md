# Changelog

All notable changes to the Emela compiler are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/) (in the `0.y.z` range, a minor
bump may include breaking language changes while the language stabilizes).

## [Unreleased]

### Added
- Language server: `emela lsp` (spec 0033) speaks LSP over stdio — diagnostics
  on open/change/save covering every compiler error, and context-aware
  completion (import paths, `match`/`catch` enum variants, `uses` effect names,
  `::` type paths, keywords, in-scope functions and locals). Editor setup lives
  in `docs/lsp.md`, with a VSCode client under `editors/vscode/`.
- Multi-error reporting (spec 0033): the frontend collects errors across
  declarations instead of stopping at the first — the lexer skips bad
  characters, the parser recovers at top-level declarations, imports and the
  type checker report per item — and `emela check` prints them all.
- Comparison operators `!=`, `>`, `<=`, `>=`, desugaring to `Eq`/`Ord` (spec 0027).
- Short-circuiting logical operators `&&`, `||`, and prefix `!` (spec 0027).
- Generic `enum` declarations with type parameters, including recursive types
  such as `List<T>` (spec 0028); type arguments are inferred at construction and
  each instantiation is monomorphized.
- Cross-module type imports: an imported module's `enum`/`trait`/`impl`
  declarations travel with its functions, so a package can export a type.
- `check --library` (alias `--lib`): type-checks a module that has no `main`.
- Core Prelude instances `Eq`/`Show for Bool` and `Eq`/`Ord for String`
  (the latter backed by new `string_eq` / `string_lt` intrinsics).
- Example standard library modules: `std.list`, `std.ord`, `std.int`, and a
  `std.option` starter.
- Packaging: **Pomes** and decentralized dependency management (spec 0032).
  `emela new <name>` scaffolds an entry Pome; `emela pome add|remove|list|update|
  install|search` manages dependencies. A Pome is any Git repository identified
  by its `host/path` source path (`github:acme/util` shorthands normalize to it),
  versioned by `v`-prefixed semver git tags and pinned to a commit + content hash
  in `Pome.lock`. There is no central registry — resolution fetches straight from
  the source-path repository. `emela pome add` computes and shows the capability
  set the added Pome and its transitive dependencies require, from source (0025),
  before writing. Workspaces (`Bushel.toml`) share a single lock. Building inside
  a Pome puts each locked dependency on the import search path:
  `import <root>.<module>.<item>` resolves against the fetched source, where
  `<root>` is the dependency's source-path leaf (`github.com/acme/mathlib` →
  `mathlib`) unless the Pome overrides it with `[pome].module` (spec 0032 M2) —
  so `github.com/emela-lang/stdlib` declaring `module = "std"` is imported as
  `std.io.print` — and its modules live under `src/`.

### Changed
- Shared IR traversal and intrinsic coverage checks moved into `emela-codegen`
  so the JS and wasm backends no longer duplicate them.

<!--
Release process:
  1. Land changes on `dev` (nightly prereleases publish automatically).
  2. Promote `dev` -> `main`, move this section under a new `## [x.y.z]` heading,
     and bump `version` in the workspace Cargo.toml.
  3. Tag `main`: `git tag vX.Y.Z && git push origin vX.Y.Z` -> stable release.
-->
