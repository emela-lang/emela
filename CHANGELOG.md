# Changelog

All notable changes to the Emela compiler are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/) (in the `0.y.z` range, a minor
bump may include breaking language changes while the language stabilizes).

## [Unreleased]

## [0.8.1](https://github.com/emela-lang/emela/compare/v0.8.0...v0.8.1) - 2026-07-21

### Fixed

- implement Eq, Show Trait for http.Method ([#80](https://github.com/emela-lang/emela/pull/80))

## [0.8.0](https://github.com/emela-lang/emela/compare/v0.7.1...v0.8.0) - 2026-07-21

### Added

- 関数値の effect subsumption (spec 0023) ([#79](https://github.com/emela-lang/emela/pull/79))
- demote Option to a Core-Prelude enum (spec 0041/0042) ([#78](https://github.com/emela-lang/emela/pull/78))
- add the lang-item attribute for role binding (spec 0039/0041) ([#77](https://github.com/emela-lang/emela/pull/77))
- capability manifest & embedder-defined capabilities (spec 0025/0026) ([#73](https://github.com/emela-lang/emela/pull/73))

### Fixed

- remove non-conformant `?`-on-Option (spec 0011/0042) ([#76](https://github.com/emela-lang/emela/pull/76))
- 改行後の行頭二項演算子を式の継続としてパース ([#62](https://github.com/emela-lang/emela/pull/62)) ([#75](https://github.com/emela-lang/emela/pull/75))

## [0.7.1](https://github.com/emela-lang/emela/compare/v0.7.0...v0.7.1) - 2026-07-20

### Fixed

- close HTTP server connections gracefully to avoid client resets ([#68](https://github.com/emela-lang/emela/pull/68))

## [0.7.0](https://github.com/emela-lang/emela/compare/v0.6.0...v0.7.0) - 2026-07-20

### Added

- ARC — deterministic reference counting for the wasm backend (spec 0048) ([#67](https://github.com/emela-lang/emela/pull/67))
- HTTP client and server (specs 0043–0046) ([#61](https://github.com/emela-lang/emela/pull/61))
- Monoid trait with return-position Self dispatch (spec 0047) ([#65](https://github.com/emela-lang/emela/pull/65))
- [**breaking**] move Char/String/Array builtins to intrinsics (spec 0021) ([#64](https://github.com/emela-lang/emela/pull/64))

## [0.6.0](https://github.com/emela-lang/emela/compare/v0.5.0...v0.6.0) - 2026-07-19

### Added

- attributes and unit testing (specs 0039/0040) ([#60](https://github.com/emela-lang/emela/pull/60))
- [**breaking**] module-unit imports and first-class effects (spec 0037) ([#57](https://github.com/emela-lang/emela/pull/57))
- adopt clap for a friendly `emela --help` (closes #17) ([#58](https://github.com/emela-lang/emela/pull/58))

## [0.5.0](https://github.com/emela-lang/emela/compare/v0.4.0...v0.5.0) - 2026-07-18

### Added

- reserve embedded std module names (spec 0038) ([#56](https://github.com/emela-lang/emela/pull/56))
- intrinsic single-declaration rule (spec 0038) ([#54](https://github.com/emela-lang/emela/pull/54))
- embed std.io/clock/string/float as compiler-resolved modules (spec 0038) ([#53](https://github.com/emela-lang/emela/pull/53))

## [0.4.0](https://github.com/emela-lang/emela/compare/v0.3.0...v0.4.0) - 2026-07-12

### Added

- string/array/sqrt primitives and multi-module fixes for stdlib ([#52](https://github.com/emela-lang/emela/pull/52))
- add effect declarations (spec 0036) ([#50](https://github.com/emela-lang/emela/pull/50))

## [0.3.0](https://github.com/emela-lang/emela/releases/tag/v0.3.0) - 2026-07-12

### Added

- implement pipeline operator `|>` (spec 0019) ([#43](https://github.com/emela-lang/emela/pull/43))

## [0.2.1](https://github.com/emela-lang/emela/releases/tag/v0.2.1) - 2026-07-12

### Fixed

- preserve non-tail block expressions during lowering ([#38](https://github.com/emela-lang/emela/pull/38))

## [0.2.0](https://github.com/emela-lang/emela/releases/tag/v0.2.0) - 2026-07-05

### Added

- qualify enum variants and conversions with `::` (spec 0018 R7)
- qualified imports and calls (spec 0018)
- language primitives for pure to_string (if, /, %, Char, ++)
- implement throws-based error handling (spec 0011)
- platform functions resolved by the backend runtime (spec 0013)
- *(backend-wasm)* compile the full IR to WebAssembly (Tier 1)
- *(codegen)* add external-process backend plugins

### Other

- v0.2.0 ([#31](https://github.com/emela-lang/emela/pull/31))
- Merge generic functions (spec 0014) into feat/new-spec
- clock platform function (JS) and browser playground scaffolding
- split into a Cargo workspace with a published codegen core

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
