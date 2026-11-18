---
title: Experimental features
authors: [Alliana]
status: implemented
pr: 620
issue:
decided: 2026-11-18
touches: [intro.experimental (new), intro.reading, module.pome, pome.manifest, cmd.build, cmd.test, lsp.project, diag.list]
requires: []
related: []
---

# Experimental features

## Summary

Add a category of experimental features to the specification. A Pome enables an experimental feature in the `[experimental]` table of its `Pome.toml`, and a command enables it for one run with a flag `--experimental-<name>`. Experimental features are outside the compatibility guarantees. Their rules are written where they belong, marked "Experimental". The first feature is expected to be capture checking (`capture-checking`).

## Motivation

Capture checking (`capture-checking`) is not compatible with every program that compiles now: code that takes the capability of a handler without state out of its with runs today, and becomes a compile error. Making it the default at once would break that code in one step.

- The inference and its display should come first, and the check should become an error after a period of warnings.
- The defaults of the inference (for declarations without a body) and the text of the diagnostics should be adjusted against real code. Changing them during that period should not count as breaking compatibility.

The specification has no such category.

- Every rule in the specification reads as covered by compatibility.
- There is no command flag or setting for trying a feature. `[intro.reading]` tells informative sections (`[target.impl-hint]` and others) apart, but there is no category for normative rules that may still change.

## Proposal

### Syntax

```toml
# Pome.toml
[experimental]
capture-checking = true
```

- Each key of `[experimental]` is the name of an experimental feature, and its value is a boolean.

```sh
emela run --experimental-capture-checking
emela check --experimental-capture-checking --experimental-other-feature
emela run --experimental-capture-checking hello.emel   # single-file mode
```

- `check`, `build`, `run` and `test` take a flag `--experimental-<name>` for each experimental feature. The name of the flag follows from the name of the feature. To enable several features, write several flags.

### Meaning

- The experimental features are those listed in `[intro.experimental]`, each with the version that introduced it and what it changes.
- A name is lower-case English words joined by `-` (`capture-checking`).
- An enabled feature applies when the modules of that Pome are checked and built. For a dependency, the `[experimental]` of the dependency's own `Pome.toml` applies. Unlike `[replace]` and `[external]`, `[experimental]` is also read when the Pome is depended on.
  - A feature whose diagnostics are warnings or errors depending on where the violation is (capture checking) is therefore decided per Pome.
  - A feature that changes meaning across Pome boundaries (syntax, the representation of types, behavior at run time) decides, in its own rules, what happens when a dependency and the Pome that depends on it differ.
- Experimental features are outside the compatibility guarantees.
  - Any version MAY change or remove any of them.
  - The name of a feature that has become stable is accepted for at least one minor version, with a warning that it has no effect.
  - The name of a removed feature is accepted with a warning that it has no effect.
  - A Pome that enables a feature declares only the minimum compiler version in `[pome].emela`, as now. There is no upper bound.
- An unknown name (a feature of a newer compiler) is a warning. The build does not stop.
- Diagnostics produced by an experimental feature carry the note ``= note: experimental feature `name` ``.
- A flag `--experimental-<name>` enables the feature for that run only, for the Pome being built (in single-file mode, for the file and the files it reaches by import).
  - It adds to the `[experimental]` of `Pome.toml`. A flag cannot disable a feature enabled in `Pome.toml`.
  - It does not apply to dependencies. Each dependency is checked with its own `[experimental]`.
  - Single-file mode has no `Pome.toml`, so a flag is the only way to try a feature there.
  - The language server does not know the flags of a command, and applies only `Pome.toml`. A feature enabled only by a flag does not show in the diagnostics of an editor. No tool reports this difference.
- A flag that starts with `--experimental-` and does not name a known feature is not an unknown-argument error. It is the same warning as an unknown key of `[experimental]`, so an older compiler given the flag of a newer feature still builds.
- `--help` lists the flags of the experimental features that the compiler knows.
- Only `check`, `build`, `run` and `test` take the flags. Whether `fmt`, `doc` and `pome` take them is decided by the proposal of a feature that needs it (a feature that changes the syntax).
- In the specification, the rules of an experimental feature are written where they belong, and each item that belongs to one starts with "Experimental (`name`)." The mark is put per item: within the section of one feature, an item without the mark applies always, and an item with the mark applies only when the feature is enabled.
- Tools do not list the experimental features that dependencies enable, and `emela doctor` does not show them.

### Typing rules

None. Each feature has its own rules.

## Specification changes

### `[intro.experimental]` (new)

Add as 1.5:

> ## 1.5 Experimental features `[intro.experimental]`
>
> An experimental feature is a part of the specification that is not yet stable. A Pome enables it in the `[experimental]` table of its `Pome.toml` (`[module.pome]`), and a command enables it for one run with a flag `--experimental-<name>` (`[cmd.build]`).
>
> - The name of a feature is lower-case English words joined by `-`.
> - The rules of an experimental feature are written where they belong. Each item that belongs to one starts with "Experimental (`name`)." Within the section of a feature, an item without the mark applies always, and an item with the mark applies only when the feature is enabled.
> - Experimental features are outside the compatibility guarantees of this document. Any version MAY change or remove any of them. A Pome that enables one declares only the minimum compiler version in `[pome].emela`.
> - When an experimental feature becomes stable, its rules lose the mark, and its name is accepted in `[experimental]` and as a flag for at least one minor version, with a warning (W0206) that it has no effect. The name of a removed feature is accepted with the same warning.
> - Diagnostics produced by an experimental feature carry the note ``= note: experimental feature `name` ``.
>
> | Name | Since | What it changes |
> |---|---|---|
>
> There is no experimental feature yet. Each proposal that adds one adds a row.

### `[intro.reading]`

Add:

> - An item marked "Experimental (`name`)." applies only when that feature is enabled (`[intro.experimental]`).

### `[module.pome]`

Add to the example:

> ```toml
> [experimental]
> # name = true                                  # experimental features (1.5)
> ```

Add:

> - `[experimental]` enables experimental features (`[intro.experimental]`) for the modules of that Pome. Unlike `[replace]` and `[external]`, it is also read when the Pome is depended on: each Pome in the dependency graph is checked with its own `[experimental]`. A feature that changes meaning across Pome boundaries states in its own rules what happens when the settings of two Pomes differ.

### `[pome.manifest]`

Add:

> - The keys of `[experimental]` are names of experimental features, and the values are booleans. An unknown name, or the name of a feature that is stable or removed, is a warning (W0206) at the key, when building that Pome. A value that is not a boolean is a warning (W0207), and the key has no effect.

### `[cmd.build]`

Add:

> - `check`, `build` and `run` take a flag `--experimental-<name>` for each experimental feature `name` (`[intro.experimental]`). It enables that feature for that run, for the Pome being built (in single-file mode, for the files read), in addition to the `[experimental]` of its `Pome.toml`. It does not apply to dependencies, and it cannot disable a feature enabled in `Pome.toml`. A flag that starts with `--experimental-` and does not name a known experimental feature, or names a stable or removed one, is not an error: it is the warning W0206. `--help` lists the flags of the experimental features the compiler knows.
> - The language server applies only `Pome.toml` (`[lsp.project]`), so a feature enabled only by a flag does not show in the diagnostics of an editor.

### `[cmd.test]`

Add:

> - `test` takes the flags `--experimental-<name>`, as `[cmd.build]`.

### `[lsp.project]`

Add:

> - The language server applies the `[experimental]` of each Pome, as the commands do. It has no flags for experimental features.

### `[diag.list]`

> | W0206 | An experimental feature named in `[experimental]` or by a flag `--experimental-<name>` is unknown, stable or removed (`[intro.experimental]`) | ``unknown experimental feature `name` `` / ``experimental feature `name` is now stable and always on`` / ``experimental feature `name` was removed`` |
> | W0207 | A value of `[experimental]` is not a boolean | ``[experimental].name` must be `true` or `false` `` |

## Diagnostics

- [x] An unknown, stable or removed experimental feature, in `[experimental]` or as a flag: W0206
- [x] A value of `[experimental]` that is not a boolean: W0207
- [x] The note ``= note: experimental feature `name` `` on diagnostics produced by an experimental feature

## Compatibility

The change only adds. `[experimental]` is not read now, so no existing `Pome.toml` is expected to have it. A flag that starts with `--experimental-` is an unknown argument now.

## Implementation

- emela-pome: reading `[experimental]`, W0206 and W0207
- emela-cli: the flags `--experimental-<name>`, the list in `--help`, passing the setting of each Pome to the checker
- emela-lsp: passing the setting of each Pome to the checker
- emela-types and others: the rules of each feature

## Alternatives

- Only command flags: the setting differs between `check`, `build`, `test` and the language server, which does not know the flags of a command. The setting of a dependency cannot be expressed. `Pome.toml` is the source of the setting, and a flag adds to it for one run.
- One flag that takes a name (`--experimental <name>`): one argument is enough, but the names of the features do not show in `--help` or in shell completion. With a flag per feature, the name of the flag still follows mechanically from the name of the feature, so a command does not define anything per feature.
- An environment variable (`EMELA_EXPERIMENTAL`): it differs between tools for the same reason, and cannot express the setting of each Pome.
- An attribute in source (`@experimental(capture_checking)` on a module): the setting differs per module, and a feature that crosses module boundaries needs more rules. A setting per Pome is enough.
- No category, only a period of warnings written in the specification: every change to the defaults of the inference or to the text of a diagnostic would be recorded as an incompatible change.
- Editions (`[open.list]` #12): an edition is for a large switch such as new reserved words, and is too coarse for trying one feature.
- Telling the Pome that depends on a dependency that the dependency enables an experimental feature: not needed until a feature changes meaning across Pome boundaries. It can be added by such a feature.
- An upper bound of the compiler version for a Pome that enables a feature: experimental features are outside the compatibility guarantees, and the version that changes one is recorded in `spec/history.md`.

## Prior art

- Opt-in unstable features per crate or per compilation unit, outside stability guarantees: Rust (`#![feature(...)]`, nightly only), GHC language extensions (`{-# LANGUAGE ... #-}`), Scala 3 (`import language.experimental.captureChecking`).
- Experimental flags declared in a project manifest: Kotlin compiler options in the build script, TypeScript `compilerOptions`.

## Open questions

## Decision

Capture checking is not compatible with every program that compiles now, and its inference defaults and diagnostics need to be tried against real code before they become fixed rules. The specification had no place for rules that may still change, so every such change would have been recorded as an incompatible one. Experimental features give that place, outside the compatibility guarantees.

`Pome.toml` is the source of the setting, so the commands and the language server agree, and each Pome in the dependency graph is checked with its own setting. A flag per feature, `--experimental-<name>`, lets a run try a feature without editing `Pome.toml`, and is the only way in single-file mode. Naming the feature in the flag keeps it visible in `--help` and in shell completion.
