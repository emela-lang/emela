---
title: License of a Pome in Pome.toml
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [module.pome, pome.manifest, cmd.pome]
requires: []
related: [pome-toml-diagnostics]
---

# License of a Pome in Pome.toml

## Summary

Add `license` to `[pome]`, so that a Pome declares its license as an SPDX license expression. It can be omitted. Tools such as the indexer of pkgs.emela.dev read this value instead of guessing the license from the files of the repository.

## Motivation

- `Pome.toml` has no place to say under which license a Pome is distributed. A user has to open the repository of each dependency and read its `LICENSE`.
- The indexer of pkgs.emela.dev builds the rows of versions from the `.toml` served by the proxy (`emela` of `pome_versions`, for example). With the license in `Pome.toml`, it can be listed and searched through the same path. Deciding the license from the text of `LICENSE` is unreliable, and makes the indexer keep rules of its own for it.
- The author of an application wants to see the licenses of the whole dependency graph, to check the conditions of distribution. The `Pome.toml` of every Pome in `Pome.lock` is verified and in the cache (`<cache>/download/<p>/@v/<version>.toml`), so with a machine-readable value they can be listed locally.

Workaround today: `license = "MIT"` can already be written, because unknown keys of `[pome]` are ignored (with emela 0.26.1, a `Pome.toml` with `license = "MIT"` and `foo = 1` passes `emela check`). The key is not in the specification, so neither its form nor its meaning is fixed, and tools cannot rely on it.

```toml
[pome]
name = "github.com/emela-lang/quince"
version = "0.3.2"
emela = "0.20.1"
license = "MIT OR Apache-2.0"
```

## Proposal

### Syntax

`[pome].license` is a string holding an SPDX license expression (SPDX specification, annex "SPDX License Expressions").

- One license: `"MIT"`
- A choice between licenses: `"MIT OR Apache-2.0"`
- Both licenses apply: `"Apache-2.0 AND MIT"`
- A license with an exception: `"Apache-2.0 WITH LLVM-exception"`
- A license not in the SPDX License List: `"LicenseRef-Acme-Internal"`

### Meaning

- `license` is the author's declaration of the license under which the contents of that version are distributed. Neither the compiler nor the proxy checks it against `LICENSE` or any other file.
- It can be omitted. An omitted `license` means that the Pome declares no license, not any particular license.
- `license` does not affect how any Pome is built. It is read also when the Pome is depended on, because the value of a dependency is what tools show.
- Only the syntax of the expression is checked. Whether an identifier is in the SPDX License List is not: licenses vary, and tying the check to the list would require a compiler release for each update of the list.
- A value that is not a string, or a string that does not follow the syntax, is a warning, reported by `check`, `build`, `run`, `test` and the language server for the Pome being built, at the value. It does not stop the build. In a dependency, it is not reported, and that dependency is treated as declaring no license (a published version cannot be changed).
- `emela pome publish` treats such a value as an error, and warns when `license` is omitted.
- The conditions for ingestion by the proxy do not change.

### Typing rules

None.

## Specification changes

### `[module.pome]`

Add a line to `[pome]` in the example:

```toml
[pome]
name = "github.com/emela-lang/quince"   # source path
version = "0.3.2"
emela = "0.20.1"                         # minimum compiler version required
module = "Quince"                        # root name when depended on (optional)
license = "MIT OR Apache-2.0"            # SPDX license expression (optional)
```

Change the item of "Identity":

> - In `[pome]`, `name`, `version`, and `emela` are required, and `module`, `targets`, and `license` can be omitted.

Add after the items of "Identity":

> - `license` in `[pome]` declares the license under which the contents of that version are distributed, as an SPDX license expression. It is the author's declaration: no tool checks it against the files of the Pome. When it is omitted, the Pome declares no license; this does not mean any particular license. Unlike `[replace]` and `[build]`, `license` is read also when the Pome is depended on. It does not affect how any Pome is built.

### `[pome.manifest]`

Add:

> - `[pome].license` is a string in the syntax of an SPDX license expression (SPDX specification, annex "SPDX License Expressions"): license identifiers and `LicenseRef-` references, combined with `AND`, `OR`, `WITH` and parentheses. Whether an identifier is in the SPDX License List is not checked.
> - A `[pome].license` that is not a string, or does not follow this syntax, is a warning (code to be assigned) at the value, when building that Pome (`check`, `build`, `run`, `test`, and the language server). It does not stop the build. In the `Pome.toml` of a dependency, it is not reported, and the dependency is treated as declaring no license. It is not a condition for ingestion.

### `[cmd.pome]`

Add to the items on `publish`:

> - When `[pome].license` of the version does not follow the syntax of `[pome.manifest]`, `publish` reports an error. When `[pome].license` is omitted, `publish` warns that the Pome declares no license, and continues.

## Diagnostics

- [ ] A warning: `[pome].license` is not a string or not an SPDX license expression. The location is the value. Code to be assigned
- [ ] `publish`: an error for the same value, and a warning when `[pome].license` is omitted. Codes to be assigned

## Compatibility

Every `Pome.toml` that is accepted now is still accepted. A `[pome].license` that does not follow the syntax, which is ignored now, becomes a warning when building that Pome.

## Implementation

- emela-pome: add `license` to `PomeSection` and `Manifest`, read as any TOML value so that a value that is not a string does not fail the parse, and check the syntax of the expression. The syntax is small, so a parser of its own is enough; a crate that embeds the SPDX License List would tie the compiler to the version of the list.
- emela-driver and emela-lsp: report the warning for the Pome being built, at the value in `Pome.toml`.
- `emela pome publish` is in the specification (`[cmd.pome]`) but not implemented: emela 0.26.1 and `emela-cli` on main have no `publish` command. The error and the warning of `publish` are added when `publish` is implemented. Until then, only the warning of `check` and the other commands is in effect.
- Outside the specification, after the proposal is accepted: the indexer of pome-registry and pkgs-api add a `license` column and copy it from the `.toml`.
- A command that lists the licenses of dependencies is not part of this proposal.

## Alternatives

- Deciding the license from the text of `LICENSE`: every tool would need its own rules (which file names to look at, how to treat variations of the text), and the result would be unreliable. With the declaration in one place, tools only read it.
- A free-form string, not checked: values such as `"MIT/Apache"` would appear, and the value would not be machine-readable, which is the purpose of the key.
- Checking identifiers against the SPDX License List: the compiler would embed the list and need a release for each update of it.
- Stopping the build on an invalid value: `license` does not affect the build, so there is no reason to stop it. A warning is enough to notice the value before publishing.
- Reporting only in `publish`: `publish` is not a required step, since pushing a tag publishes a version, so the value could go unnoticed until it is published and can no longer be changed.
- `license-file`, pointing at the text of the license: a license that is not in the SPDX License List can be written with `LicenseRef-`. A key for a file is decided separately if a use for it appears.
- Making `license` required: every existing Pome would be rejected, and private Pomes with no license decided would have to write one.

## Prior art

- SPDX license expressions and the SPDX License List. Declaring the license in the package manifest as a machine-readable SPDX expression is done by Cargo (`package.license`), npm (`license`) and PEP 639 (`project.license`); PEP 639 also rejects identifiers that are not in the list.

## Open questions

## Decision
