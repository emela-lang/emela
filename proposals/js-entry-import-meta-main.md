---
title: Telling the entry module of the JS host with import.meta.main
authors: [Allianaab2m]
status: draft
issue:
touches: [target.js.module]
requires: [js-direct-entry]
related: []
---

# Telling the entry module of the JS host with import.meta.main

## Summary

`main.mjs` decides whether it is the entry module of the JS host with `import.meta.main`. Only a JS host that does not define `import.meta.main` compares the real path of `process.argv[1]` with the path of `main.mjs`, as `js-direct-entry` (#677) specified for every host. With this, importing `main.mjs` from code that the JS host evaluates (`node -e`) does not launch `main`, on every host that defines `import.meta.main`.

## Motivation

`js-direct-entry` made `main.mjs` launch `main` when it is the entry module of the JS host, and not when it is loaded by an `import` (`[target.js.module]`). It decides the entry module with one rule on every host: the real path of `process.argv[1]` is the path of `main.mjs`.

The two do not agree. When the JS host evaluates code that imports `main.mjs`, and the path of `main.mjs` comes after the code, `process.argv[1]` is the path of `main.mjs`, and `main` is launched although `main.mjs` was loaded by an `import`:

```sh
node -e 'import("./main.mjs")' main.mjs   # launches main
```

A launcher that tries to recognize this case from the options of the JS host (`process.execArgv`) has to follow every form of them (`-e`, `--eval`, `-p`, `-pe`, `--eval=...`, `NODE_OPTIONS`), and was dropped in review of the implementation (#725).

The JS hosts that `[target]` supports, except older releases of Node 22, define `import.meta.main`, which is `true` exactly in the entry module. Checked on Node 22.23.4, 24.22.1 and 26.13.1 and on Bun 1.4.3 and 1.4.4: `true` under `node main.mjs`, `false` under `node -e 'import("./main.mjs")' main.mjs`. Node 22.14.0 does not define it.

## Proposal

### Meaning

- `main.mjs` is the entry module of the JS host when `import.meta.main` is `true` in it.
- On a JS host that does not define `import.meta.main`, `main.mjs` is taken to be the entry module when the real path of `process.argv[1]` is the path of `main.mjs`, as before. On such a host, and only there, `main.mjs` imported by evaluated code with the path of `main.mjs` as `process.argv[1]` launches `main`, and a JS host started through a symbolic link with `--preserve-symlinks-main` does not launch it.
- The rule stated in `[target.js.module]` ("Otherwise, including when `main.mjs` is loaded by an `import` ... does not launch `main`") names evaluated code explicitly.
- Everything else of `js-direct-entry` is unchanged.

### Typing rules

None.

## Specification changes

### `[target.js.module]`

The bullet on launching `main` starts with "When `main.mjs` is the entry module of the JS host (the module the JS host was started with)", and its sentence on `import` reads:

> Otherwise, including when `main.mjs` is loaded by an `import`, whether from another module or from code that the JS host evaluates (`node -e`), `main.mjs` does not launch `main`.

The bullet on the entry module is replaced with:

> - `main.mjs` is the entry module of the JS host when `import.meta.main` is `true` in it. On a JS host that does not define `import.meta.main`, `main.mjs` is taken to be the entry module when the real path of `process.argv[1]` (symbolic links resolved) is the path of `main.mjs`. On such a host, and only there, a JS host started with code to evaluate and the path of `main.mjs` as `process.argv[1]` (`node -e 'import("./main.mjs")' main.mjs`) launches `main`, and a JS host started through a symbolic link with `--preserve-symlinks-main` does not.

## Diagnostics

- [ ] None added, changed or removed.

## Compatibility

- `js-direct-entry` is not implemented yet (#725), so no released compiler behaves otherwise.
- Against `js-direct-entry` as specified: on a host that defines `import.meta.main`, `node -e 'import("./main.mjs")' main.mjs` no longer launches `main`, and `node --preserve-symlinks-main link.mjs` through a symbolic link to `main.mjs` launches it.

## Implementation

- `main.mjs` ends with `$launch(import.meta.main, import.meta.url, main)`. The runtime launches `main` when the first argument is `true`, does nothing when it is `false`, and compares paths only when it is `undefined`.
- The comparison treats a path that cannot be read as "not the entry module": under `node -e '...' word`, `process.argv[1]` is not a file, and importing `main.mjs` there must not throw.
- The comparison takes `node:fs` and `node:url` through `process.getBuiltinModule`, and on a Node that lacks it (before 22.3) through a dynamic `import`, only on Node and Bun, as `js-direct-entry` describes.
- Implemented in #725, with tests that starting the JS host with `main.mjs` launches `main`, and that importing it from another module or from evaluated code does not.

## Alternatives

- **The path comparison alone, on every host** (`js-direct-entry` as adopted). One way of deciding on every supported host, but "importing does not launch" does not hold for evaluated code.
- **Recognizing evaluated code from `process.execArgv`.** Every spelling of the options and `NODE_OPTIONS` would have to be followed, and a missed one launches `main`.
- **Only `import.meta.main`, raising the minimum Node version to a Node 22 release that defines it.** One rule with no exception, but drops the older Node 22 releases, which `[target]` supports.

## Prior art

- Entry-module detection through a property of the module itself, set by the host for the module it was started with, instead of reading the command line.

## Open questions
