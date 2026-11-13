---
title: JS hosts are Node and Bun, chosen with EMELA_JS_HOST
authors: [Alliana]
status: accepted
pr: 504
issue:
decided: 2026-11-14
touches: [target, target.js.host, cmd.build, cmd.doctor, cmd.test.out-dir, diag.list]
requires: []
related: []
---

# JS hosts are Node and Bun, chosen with EMELA_JS_HOST

## Summary

`emela run` and `emela test` run the JS output with the host named by the environment variable `EMELA_JS_HOST`. Its value is a command, started in the same way as `EMELA_NODE` today. `EMELA_NODE` is deprecated and is read, with a warning, for one more minor version. The hosts that the JS target assumes become Node and Bun; Deno is no longer one of them. The version check before a run reads the version from inside the host, so that it knows Bun from Node and compares each with its own minimum.

## Motivation

- The JS target assumes hosts other than Node (`[target]`), but the variable that chooses the host is named `EMELA_NODE`. When it names another host, the name and the value disagree.
- Pomes that call built-in features of Bun have appeared, such as x/platform-bun (`bun:sqlite`). Their tests run with `EMELA_NODE=bun emela test`, which reads as if it named Node.
- The version check of `[cmd.build]` reads the major version from `<executable> --version` and compares it with the minimum version of Node. Bun prints its own version (`1.4.3`), which is read as major version 1, so Bun is refused with E0904 although `[target]` lists it as a host.

Deno is removed from the assumed hosts:

- The specification lists Deno as a host and says its results were checked to be the same as Node's (`[target.js.host]`). emela cannot start Deno: `deno` does not accept `--input-type=module -e`, the form in which `emela run` and `emela test` start the host.
- There is no reason now to start Deno in a different way, to choose its permission flags, or to keep checking it in CI. Pomes that call built-in features of Bun exist; none exist for Deno.
- Removing Deno means that emela does not promise to run on Deno. Importing `main.mjs` from Deno may still work, but the specification does not cover it.

The variables of other tools (`EMELA_CC`, `EMELA_WASMTIME`) are named after the tool, which works because only one tool can fill each role. The JS host can be chosen, so the variable is named after the role. The word is host because the specification calls what runs the JS output the host throughout (`[target.js.host]`, host functions). `RUNTIME` would be confused with `emela_runtime.mjs`, which emela writes.

## Proposal

### Meaning

- The command that runs the JS output is the value of `EMELA_JS_HOST` if it is not empty, otherwise the value of `EMELA_NODE` if it is not empty, otherwise `node`.
- The host is started as today: main with `<cmd> --input-type=module -e <launcher> -- <entry> <args>`, an HTTP service with `<cmd> serve.mjs <args>`, and the tests with `<cmd> emela_test.mjs`. Node and Bun run in this form.
- When `EMELA_NODE` is read, W0902 is reported. When `EMELA_JS_HOST` is also set, `EMELA_JS_HOST` is used and the same warning is reported.
- `EMELA_NODE` is read until the minor version after the one that deprecates it, and is no longer read from the minor version after that.
- Before a run, the version of the host is read by starting it with `--input-type=module -e` and a short module that writes `process.versions.bun` when it is defined and `process.versions.node` otherwise. A host that wrote a Bun version is compared with the minimum version of Bun, and any other with the minimum version of Node. The minimums are Node 22 and Bun 1.3, the oldest versions checked in `[target.js.host]`.

## Specification changes

### `[target]`

Replace "The JS target assumes a host that runs on a server (Node 22 or later, and Deno and Bun with `node:` modules and `fetch`)." with "The JS target assumes a host that runs on a server: Node 22 or later, or Bun 1.3 or later." The rest of the paragraph stays.

### `[target.js.host]`

Replace the informative item with: "Informative: suspension and resumption, yielding, AbortSignal, and the order of virtual time (`[target.js.event-loop]`, `[target.js.abort]`) have been checked to give the same results on Node 22.14 and Bun 1.3.8 (2026-10-13)."

### `[cmd.build]`

Replace the item "The node used is the environment variable `EMELA_NODE` if set, and otherwise `node` on PATH." with:

- The JS host is the command in the environment variable `EMELA_JS_HOST` if it is set and not empty, otherwise the command in `EMELA_NODE` if it is set and not empty, otherwise `node` on PATH. It MUST accept the options `--input-type=module -e` as node does; Node and Bun do.
- `EMELA_NODE` is deprecated. When it is read, W0902 is reported, also when `EMELA_JS_HOST` is set and takes precedence. It is read until the minor version after the one that deprecates it.

Replace the item on the version check ("For `js`, before starting the program, the command reads the version from `<executable> --version`. ...") with:

- For `js`, before starting the program, the command reads the version of the JS host by starting it with `--input-type=module -e` and a module that writes `bun <process.versions.bun>` when `process.versions.bun` is defined, and `node <process.versions.node>` otherwise. A host that wrote `bun` is Bun, and is compared with the minimum version of Bun that `[target]` requires; a host that wrote `node` is compared with the minimum version of Node. When the version is older than the minimum, it is E0904 and the program is not started: the message is ``` `<executable>` is <Host> <version>, but target `js` needs <Host> <minimum> or later ```, with the note `install Node.js 22 or later or Bun 1.3 or later, or set EMELA_JS_HOST to a newer JS host`. `<executable>` is the command of the JS host, `<Host>` is `Node` or `Bun`, `<version>` is the version written by the host, and `<minimum>` is the minimum that `[target]` gives for that host. If the version cannot be read, including when the host cannot be started, the program is started. The same check is made by `emela test` (`[cmd.test]`).

In "Starting the program", replace "node too old (E0904)" with "the JS host too old (E0904)", "(node or the executable not found)" with "(the JS host or the executable not found)", and "node with its options for `js`" with "the JS host with its options for `js`". In the item on `<launcher>`, replace "the module that node runs with `-e`" with "the module that the JS host runs with `-e`". The example ``running `node --input-type=module -e <launcher> -- target/emela/js/main.mjs` `` stays, as the line for `node`.

In "Running the program (JS)", replace "`run` runs `node serve.mjs`" with "`run` runs `serve.mjs` with the JS host", "passed after `--` as node arguments" with "passed after `--` as arguments of the JS host", and "node's stack display" with "the JS host's stack display".

### `[cmd.doctor]`

Replace the `js` row of the table with:

| Target | Program | Rule | Passes when |
|---|---|---|---|
| `js` | the JS host | `[cmd.build]` | it starts and the version read as in `[cmd.build]` is one that `[target]` supports for that host |

The report names the host and its version as the host wrote them (`node 24.11.0`, `bun 1.4.4`).

### `[cmd.test.out-dir]`

Replace "`emela test` runs the launcher module with node." with "`emela test` runs the launcher module with the JS host (`[cmd.build]`)."

### `[diag.list]`

- Change E0904 to "The JS host is not found, cannot be started, or is older than the version `[target]` requires for it with `--target js` (`[cmd.build]`)", with the example ``cannot start `node` `` kept.
- Add W0902: "`EMELA_NODE` is set (Appendix B)". Message: ``EMELA_NODE is deprecated; use EMELA_JS_HOST``.

## Diagnostics

- [ ] W0902 (new): `EMELA_NODE` was read
- [ ] E0904: its meaning, message and note follow the JS host and its minimum version

## Compatibility

- A program that ran `main.mjs` on Deno is no longer covered by the specification. The output of emela does not change, so it does not stop working at once.
- An environment that sets only `EMELA_NODE` keeps working for one minor version, with a warning.
- A JS host that reports Bun is no longer refused with E0904; it is compared with the minimum version of Bun.
- The message of E0904 changes from ``` `node` is v20.11.1, but target `js` needs node 22 or later ``` to ``` `node` is Node 20.11.1, but target `js` needs Node 22 or later ```, and the `js` line of `emela doctor` names the host.
- The tests of the compiler (`crates/emela-driver/tests/wasm_*.rs`, `crates/emela-codegen-wasm/tests/unicode.rs`, `crates/emela-cli/tests/`) read `EMELA_NODE`. They move to `EMELA_JS_HOST`.

## Implementation

- emela-driver: replace `NODE_ENV` with the two variables `EMELA_JS_HOST` and `EMELA_NODE`. The way the host is started (`node_command`, `node_script_command`) does not change. Replace `node_version`, which parses `--version`, with a probe that starts the host with `--input-type=module -e` and parses `node <version>` or `bun <version>`; the version check and `emela doctor` use it.
- emela-cli: report W0902. Change the message and note of E0904.
- CI: add a job that runs the JS tests of the compiler (`emela run` and `emela test`) with `EMELA_JS_HOST=bun`, next to the jobs on Node. Without it, a change that breaks Bun goes unnoticed.
- README: correct the description of the hosts of the JS target (Node.js, Deno, Bun) and the statement that the output of an HTTP service runs with `deno serve`.

## Alternatives

- `EMELA_JS_RUNTIME`. It departs from the term of the specification (host) and is confused with `emela_runtime.mjs`.
- Limit the value to the name of a host (`node`, `bun`) and pass the command in another variable. That makes two variables, and is inconvenient for pointing at a wrapper or a path.
- Keep `EMELA_NODE` and add `EMELA_JS_HOST`. Two variables that do the same thing would remain.
- Write the host in `Pome.toml` (`[build] js-host = "bun"`). The tests of a Pome that runs only on Bun could then never be run on the wrong host. It is left to a separate proposal.
- Tell Bun from Node by the form of `--version` (Node writes a leading `v`, Bun does not). It depends on the output format of each host.
- Skip the version check for Bun. An old Bun would then go undetected.
- Make Bun the default host when nothing is set. The default, the setup of CI and `setup-emela` change together with it; it is left to a separate proposal.

## Prior art

- `EMELA_CC`, `EMELA_WASMTIME`: the naming used when only one tool can fill the role.

## Open questions

## Decision

Adopted, keeping the name `EMELA_JS_HOST`: the specification calls what runs the JS output the host, and `EMELA_JS_RUNTIME` would be confused with `emela_runtime.mjs`. Node and Bun are both supported for now, so CI runs the JS tests on Bun as well, and the version check reads the version from inside the host so that Bun is compared with its own minimum. Making Bun the default is left for later.
