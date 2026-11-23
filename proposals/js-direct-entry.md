---
title: Running main.mjs directly
authors: [Satoru314]
status: implemented
pr: 677
issue:
decided: 2026-11-23
touches: [target.js.module, cmd.build]
requires: []
related: [js-launcher-names, http-service]
---

# Running main.mjs directly

## Summary

`main.mjs` written by `emela build` for the `js` target launches `main` when the JS host is started with it (`node target/emela/js/main.mjs`), with the same outcomes and exit codes as `emela run`. Importing `main.mjs` from another module does not launch `main`. An HTTP service is unchanged: it is started through `serve.mjs`, and its `main.mjs` does not launch anything. For a program that is not an HTTP service, `emela run` starts the JS host with `main.mjs` itself instead of a launcher passed with `-e`.

## Motivation

After `emela build`, the output cannot be run without `emela`:

```sh
emela build src/main.emel
node target/emela/js/main.mjs   # prints nothing, exits with 0
```

`main.mjs` only exports `main` (`[target.js.module]`). The code that calls it, maps errors, defects and interruptions to exit codes, and passes the arguments, exists only as a module that `emela run` passes to the JS host with `-e` (`[cmd.build]`, "Starting the program"). Its form is not specified, so a user cannot write it either: `[target.js.module]` says that the forms of `main` and `$start` "are not specified for other uses".

So a built program cannot be deployed to a machine or container that has a JS host but no `emela`, run from a process manager, or started from a shell script, without depending on unspecified internals.

An HTTP service does not have this problem: `serve.mjs` is written next to `main.mjs` and is started with `node serve.mjs` (`[target.js.module]`). Other JS hosts take the `fetch` of the default export of `main.mjs` themselves (a host that serves the default export of its entry module when it is started with it, or a host that imports it).

## Proposal

### Meaning

- When the JS host is started with `main.mjs` as its entry module, the entry has a `main`, and the program is not an HTTP service (`[lib.http-service]`), `main.mjs` launches `main`. The arguments of the program are the arguments of the JS host after the path of `main.mjs`. The program stops with the standard error output and the exit code given in `[cmd.build]` ("Running the program (JS)").
- When `main.mjs` is imported by another module (a host program, `serve.mjs`, the test runner), it does not launch `main`. Its exports are unchanged.
- The `main.mjs` of an HTTP service never launches `main`, including when the entry also has a `main`, so that `node main.mjs` does not run something other than what `emela run` runs. `serve.mjs` is written and started as before.
- When the entry has no `main`, starting the JS host with `main.mjs` only evaluates the module.
- For a program that is not an HTTP service, `emela run` starts the JS host with `main.mjs` and the arguments after `--`. The line it writes before starting becomes ``running `node target/emela/js/main.mjs` `` (plus the arguments), which can be pasted into a shell as it is; the `<launcher>` exception of the quoting rule goes away.
- `emela run` keeps checking the version of the JS host first (E0904). Starting `main.mjs` directly on a host older than the minimum of `[target]` is not specified.

### Typing rules

None.

## Specification changes

### `[target.js.module]`

Add after the bullet on the exports used by the launcher:

> - When the JS host is started with `main.mjs` as its entry module, the entry has a `main`, and the program is not an HTTP service (`[lib.http-service]`), `main.mjs` launches `main` with no arguments. The arguments of the program (`Args.list` of SystemArgs, `[api.std.process]`) are the arguments of the JS host after the path of `main.mjs`. Its standard error output and exit code are as in `[cmd.build]` ("Running the program (JS)"). Otherwise, including when `main.mjs` is loaded by an `import` from another module, `main.mjs` does not launch `main`. An HTTP service is started through `serve.mjs`.
> - Loading `main.mjs` does not require `node:fs` or any other module of the JS host.
> - `main.mjs` is the entry module of the JS host when the real path of `process.argv[1]` (symbolic links resolved) is the path of `main.mjs`. A JS host started with an option that keeps `process.argv[1]` from resolving to `main.mjs` (`--preserve-symlinks-main` through a symbolic link) does not launch `main`.

### `[cmd.build]`

In "Starting the program", replace

> - For `js`, the module that the JS host runs with `-e` to load the entry is written as `<launcher>` (``running `node --input-type=module -e <launcher> -- target/emela/js/main.mjs` ``). This word alone does not follow the quoting rule above.

with

> - For `js`, the command is the JS host followed by the path of `main.mjs`, or of `serve.mjs` for an HTTP service (``running `node target/emela/js/main.mjs` ``).

In "Running the program (JS)", replace

> - `run` loads the entry module and calls the exported `main` with no arguments. The return value of `main` is discarded, except that when the return type of `main` is `ExitCode` the program exits with its exit code. Arguments to the program are passed after `--` as arguments of the JS host, and the launcher passes them to the runtime, where `Args.list` of SystemArgs returns them (`[api.std.process]`).

with

> - `run` starts the JS host with `main.mjs` followed by the arguments after `--`, and `main.mjs` launches `main` (`[target.js.module]`). The return value of `main` is discarded, except that when the return type of `main` is `ExitCode` the program exits with its exit code. The rest of this part applies equally when the JS host is started with `main.mjs` without `emela`.

## Diagnostics

- [ ] None added, changed or removed.

## Compatibility

- Programs compile and behave as before under `emela run`, except for the text of the `running` line.
- Starting the JS host with a built `main.mjs` of a program that is not an HTTP service, which used to evaluate the module and exit with 0 without calling `main`, now runs the program.
- HTTP services, their `serve.mjs`, and importing `main.mjs` from JS are unchanged.

## Implementation

- `emela-codegen-js`: move the launcher (now the `LAUNCHER` constant in `emela-driver/src/run.rs`) into `emela_runtime.mjs` as a function, and, when the entry has a `main` and the program is not an HTTP service, end `main.mjs` with a call to it guarded by the entry-module check.
- The check takes `node:fs` through `process.getBuiltinModule`, and on a Node that lacks it (before 22.3) through a dynamic `import("node:fs")`, only when the host is Node or Bun. This is how `std/file_system.mjs` of the bundled core already loads `node:fs/promises`, so that importing `main.mjs` on a host without `node:fs` still works and Node 22.0 to 22.2, which `[target]` accepts (E0904 compares with 22.0.0), are covered.
- `emela-driver`: start the JS host with `main.mjs` and the arguments, and write the `running` line without `<launcher>`. The version check keeps using `--input-type=module -e`.
- Prototype (Node 24.22.2): appending the guard `realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)` to a built `main.mjs` runs `main` under `node main.mjs`, from the output directory as `node main.mjs`, and through a symbolic link to `main.mjs`. `import()` of the same file does not run it, and neither does `node --preserve-symlinks-main` through a symbolic link. Not yet tried on Node 22 or Bun.
- Observed with the current output: `bun main.mjs` of an HTTP service starts Bun's own server from the default export, and `bun main.mjs` of a program with `main` prints nothing.

## Alternatives

- **A separate launcher file** (`run.mjs` next to `main.mjs`, started as `node run.mjs`). It needs no entry-module check. Not chosen because the file a user expects to run is the one named `main`.
- **Moving the server of `serve.mjs` into `main.mjs`, and no longer writing `serve.mjs`.** One file to start for every program. Not chosen: `main.mjs` is loaded by several JS hosts, and some of them serve its default export themselves when started with it, so a server of its own would need a branch on the host inside `main.mjs`. It would also break deployments that start `node serve.mjs`.
- **Keeping the `-e` launcher for `emela run`, and adding direct running beside it.** Two launchers that must give the same outcomes and exit codes, one of them not exercised by `emela run`. Starting `main.mjs` directly makes `emela run` test the path that deployments use.
- **Calling `main` unconditionally at the end of `main.mjs`.** Importing `main.mjs` (from `serve.mjs`, a host program or the test runner) would run the program.
- **Only `import.meta.main`, raising the minimum Node version to 24.2.** Drops Node 22, which `[target]` supports.
- **`import.meta.main` where the host defines it (Node 24.2 and later, Bun), and the path comparison otherwise.** Handles `--preserve-symlinks-main`, but the rule would differ between hosts. The path comparison alone gives one rule on every supported host.
- **Specifying the forms of `main` and `$start`** so that users can write their own launcher. Every user would copy the same code, and the forms could no longer change.

## Prior art

- Entry-module detection: a module that runs its program only when it is the entry point of the process, and otherwise only defines its exports.

## Open questions

## Decision

The `.mjs` files that Emela builds for JS could not be run directly. `emela run` called the `main` function exported from `main.mjs`, but when the program was started directly, as with `node main.mjs`, nothing called `main`, and running it gave nothing back.

This proposal makes the files written by the build give the same outcome as `emela run` when a JavaScript runtime starts them directly.
