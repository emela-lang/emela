# Appendix B. Commands `[cmd]`

This appendix defines the externally visible behavior of the `emela` command. `emela test` is in 13.4 and `[cmd.test]`, `emela pome` is in `[cmd.pome]`, and `emela flash` is in `[cmd.flash]`.

## emela check, build, run `[cmd.build]`

- Takes one path, which defaults to `.`. When a directory is given, the target is the whole project of the ancestor `Pome.toml` (4.1). The directory given can be the project directory or a directory inside the source root or the test root. A directory outside them (`scripts/`) is a compile error (E0208, the same as fmt). Even when a directory inside the source root or the test root is given, the whole project is checked, because type checking spans modules. `emela test` does the same.
- The command writes `main.mjs`, translated from the entry, and the runtime to the output directory. The fns with `@export(js, ...)` and the exports made by `@entry` are exported from `main.mjs` (`[target.js.module]`). `emela run` also writes to the default output directory (4.1) before running, and overwrites the previous output without deleting it. `--out-dir` changes the output directory.
- Arguments to the program are written after `--`. Diagnostics are reported before the program is launched.
- The node used is the environment variable `EMELA_NODE` if set, and otherwise `node` on PATH.
- For `c`, the command writes `main.c`, translated from the entry, and the C sources of the runtime to the output directory, and places the executable `main` compiled with cc. The cc used is the environment variable `EMELA_CC` if set, and otherwise `cc` on PATH (E0907, E0908).
- The exit code is as follows.
  - 1 if the diagnostics include an error. 0 if there are only warnings.
  - When `run` reaches the program, the exit code of the program (0, 1 for an error (14.3), 101 for a defect (7.5), 130 for an interruption (14.5)).
  - 128 + the signal number when stopped by a signal.

Build target and output directory:

- `build`, `run`, and `test` select the build target with `--target <js|c|c-embedded|wasm>`. When omitted, it is `[build].target` in `Pome.toml`, and if that is also absent, `c-embedded` for a `build` with a board selected (`[target.c-embedded.board]`) and `js` otherwise (`[module.pome]`). `run` and `test` are not available for `c-embedded`. `check` takes no build target.
- `build` and `check` take `--board <id>`, and `build` takes `--embedded-config <path>` (`[target.c-embedded]`). `check` with a board, or with `[embedded].board`, reads the module replacements of that board (`[target.c-embedded.project]`).
- The default output directory of `build` and `run` is `target/emela/<target>` (`target/emela/js`, `target/emela/c`, `target/emela/c-embedded`, `target/emela/wasm`; `target/emela/c-embedded/<board>` with a board) in the project (in single-file mode, in the directory that contains the file). The default output directory of `test` is as in `[cmd.test.out-dir]`. `--out-dir` changes the output directory of `build` and `run` for every target.
- Giving `build` or `run` a file inside the test root is a compile error (E0208, ``... is under the test root, which only check and test read``). `build` and `run` do not read the test root.

Entry and main:

- When a directory is given and neither `src/main.emel` nor `src/lib.emel` exists, `check` passes, and `build` and `run` report a compile error (E0209).
- When a directory is given and `src/main.emel` does not exist but `src/lib.emel` does, the project is a library-only project, and `lib.emel` is the entry for checking and testing. `build` only checks and outputs nothing. `run` is a compile error (E0230), whether or not `lib.emel` has a fn `main`. If `src/main.emel` exists, it is the entry. When a file is given, that file is the entry, including `lib.emel`.
- If the entry module has no fn named `main` and the program is not an HTTP service (`[lib.http-service]`), `run` reports a compile error (E0230, located at the entry file), and neither outputs anything nor launches the program. Something named `main` that is not a fn (a const and so on) does not count. This error is reported only when there are no other errors (syntax, names, types). `build` outputs even without main (8.3).

JS output:

- The command writes the entry `main.mjs` and, next to it, the runtime `emela_runtime.mjs` to the output directory. When host files of the bundled core are used, they are written under `emela_core/` in the output directory, in the same layout as the core sources.
- Files that `@external` points to are copied to the output directory and imported from `main.mjs` by relative path. Files of the project are placed under `emela_project/`, and files of dependencies under `emela_pomes/<source path>@<version>/` (for a dependency with `[replace]`, `emela_pomes/<source path>@replace/`), in the same layout as from the source root.
- What is copied is everything under the source root of the Pome that contains the pointed-to file, except `.emel` files and entries starting with `.`. This lets a host file import neighboring files by relative path. In single-file mode, only the pointed-to file is copied.
- The previous output is overwritten without being deleted (the same holds for `emela_core/` and the rest).

Starting the program:

- Before starting the program, `run` writes one line ``running `<command>` `` to standard error, for every build target. The line comes after the diagnostics and before any output of the program. It is not written when the program is not started (an error in the diagnostics, or wasmtime too old, E0909). When the command is written but cannot be started (node or the executable not found), the diagnostic about it follows the line.
- `<command>` is the program that is started followed by its arguments, separated by single spaces: node with its options for `js`, the executable `main` for `c`, and wasmtime with its options for `wasm` (`[target.wasm.output]`), each followed by the arguments after `--`.
- Each word is written as is when it is not empty and consists only of ASCII letters, digits and the characters `-_./=:@%+,`. Otherwise it is enclosed in single quotes, and each `'` inside it is written as `'\''`. Pasting the line into a POSIX shell starts the same command.
- For `js`, the module that node runs with `-e` to load the entry is written as `<launcher>` (``running `node --input-type=module -e <launcher> -- target/emela/js/main.mjs` ``). This word alone does not follow the quoting rule above.

Running the program (JS):

- For an HTTP service, `run` runs `node serve.mjs` (`[target.js.module]`) with the arguments after `--`, instead of what follows.
- `run` loads the entry module and calls the exported `main` with no arguments. The return value of `main` is discarded. Arguments to the program are passed after `--` as node arguments.
- A `main` that can suspend is awaited on the event loop until it finishes.
- The standard error output and exit code when execution stops are as follows. The exit code is determined after writing to standard error finishes (writes to a pipe are not cut off midway).

| How it stops | Standard error | Exit code |
|---|---|---|
| An error that reached `main` (14.3) | `error: <display of the error value>` | 1 |
| defect (7.5) | `defect: <message>` | 101 |
| Ended while suspended without being resumed (14.5) | `interrupted` | 130 |
| An exception that is not a defect | node's stack display | 1 |

- The display of the error value is the same as inspect (10.6) if the error type satisfies Show (for `error DbError(message: String) derive Show`, `error: DbError(message: "down")`), and only the name otherwise (`error: Retry`).
- "Ended while suspended without being resumed" means that nothing remains to resume the continuation of `main` and the event loop has nothing left to wait for. Also, when, inside a `main` that does not suspend, a continuation is discarded on the spot without being resumed, the program ends on the spot with `interrupted` and 130.
- A defect thrown inside a host callback also ends with `defect: <message>` and 101, like a defect of `main`. When stopping on a defect, remaining host work such as timers is not run. An exception that is not a defect thrown inside a host callback prints the stack and ends with 1.

Diagnostics and help:

- The form and destination of diagnostics are in Appendix A (`[diag]`).
- The text of `--help` is written in English.

## emela flash `[cmd.flash]`

- Writes the firmware image of the last successful `emela build` with a board (`[target.c-embedded]`) to the device. It does not build.
- Takes one path like `build` (`[cmd.build]`). The board is `--board`, else `[embedded].board`; without either it is an error (``` `flash` requires `--board` or `[embedded].board` ```). `--out-dir` and `--embedded-config` MUST be the same as given to the build. `--port` is the serial port (ESP32-C3) or the serial number of the debug probe (STM32F4).
- A successful board build records the SHA-256 of its inputs and outputs in `board-build.json`: the sources and manifests of the project and of its dependencies, `Pome.lock`, the runtime configuration, the module replacements, the files of `[embedded].sources` and the headers next to them, the generated C, the board support files, and the firmware outputs. The selected project, board, entry, runtime configuration and output directory are recorded with them.
- `build` with a board removes the previous record once the target is decided and before it compiles anything, and writes a new one only when the build succeeds and no input changed while it ran. A failed build leaves no record.
- `flash` checks the current files and selection against the record before it starts any program from the output directory. Without a record, it is an error (``no successful firmware build exists; run `emela build` first``). When anything differs, it is an error and nothing is written to the device.
- After the check, the image is written with OpenOCD (STM32F4, the environment variable `OPENOCD`, else `openocd`) or esptool (ESP32-C3, run by the environment variable `EMELA_ESPTOOL_PYTHON`, else by the Python of the build). ESP32-C3 requires `--port` unless `--dry-run` is given. Paths and the port are passed as separate arguments, without a shell.
- `--dry-run` performs the same check, prints the command that would write the image, and does not open the device.
- The exit code is 0 on success and 1 otherwise.

## emela fmt `[cmd.fmt]`

- `emela fmt [path...]` formats and rewrites the `.emel` files in the given files and directories. The path defaults to `.`.
- A directory requires `Pome.toml` (E0207). For the project directory, the target is everything under the source root and the test root. For a directory inside either root, the target is everything under it. A directory outside them is a compile error (E0208).
- Entries starting with `.` are skipped. Files whose names are not valid module names are also formatted.
- A file with a syntax error is not rewritten. The command reports diagnostics and continues with the other files.
- `--check` does not rewrite, and prints the files that need formatting to standard output, one per line. `-` formats standard input and writes to standard output.
- The exit code is 1 if the diagnostics include an error or if `--check` finds files that need formatting, and 2 for an argument error.
- On successful formatting, nothing is printed to standard output.
- The paths `--check` prints are the same relative paths as in diagnostics. No diff is printed.
- With `-`, if the input is already formatted, it is written as is. On a syntax error, nothing is written to standard output, the diagnostic is reported to standard error with the path `<stdin>`, and the command ends with 1. `--check -` prints the single line `<stdin>` and ends with 1 if formatting is needed. Mixing `-` with other paths is an argument error (2). A file named `-` is written as `./-`.
- A given file is handled by the same rules as `[module.name]`. If `Pome.toml` exists and the file is outside the source root, it is E0208. If there is no `Pome.toml`, the file is in single-file mode. Even in single-file mode, only the given file is formatted, and imports are not followed.
- Formatting a file whose name is not a valid module name reports no name diagnostic.
- A file given more than once is formatted only once. Files are formatted in the order of their absolute paths.
- Diagnostics for syntax errors are reported with a code. A failed formatter verification (`[fmt.io]`) is reported as an `internal error: …` diagnostic without a code, and the file is not rewritten.
- A file whose formatted result is the same as the original is not written to (its modification time does not change).
- A file that cannot be read is E0902, and a file that cannot be written is E0903.

## emela pome `[cmd.pome]`

Operations on dependencies are placed under `emela pome` (4.7). Each of them rewrites `Pome.toml` and `Pome.lock` consistently. `<src>` can be a source path or the `github:` shorthand.

| Command | What it does |
|---|---|
| `emela pome add [--allow-external] [--dev] <src>[@<version>]` | Adds a dependency. When version is omitted, the latest tag. `--dev` adds it to `[dev-dependencies]` |
| `emela pome remove <src>` | Removes a dependency, whether it is in `[dependencies]` or `[dev-dependencies]` |
| `emela pome install` | Creates the lock to match `Pome.toml` and fetches the dependencies |
| `emela pome update [<src>]` | Raises the requirement to the latest tag of that Pome (all of them when omitted) |
| `emela pome list` | Prints the resolved dependency graph. Marks Pomes reachable only from the dependency graph of tests |
| `emela pome verify` | Checks the contents of cached dependencies against the hashes in the lock |
| `emela pome publish [<version>] [--dry-run]` | Compares the public API of a version of your own Pome with the previous version, has the proxy ingest it, and checks the sumdb record against the local contents |

- `add`, `install`, and `update` check whether the `.emel` files of the fetched versions contain `@external`. If a Pome not allowed by `[external]` (including transitive dependencies) contains it, the command lists the files containing `@external`, writes nothing, and ends with 1. `add --allow-external` adds to `allow` in `[external]` the Pomes that use `@external` among the Pome being added and the Pomes that this `add` newly brings into the lock.
- `remove` also checks `@external` like `add`, `install`, and `update`, and if there is a Pome that is not allowed, it writes nothing and ends with 1. How files containing `@external` are counted is in `[pome.lock]`.
- A Pome that was already in the lock is not allowed by `add --allow-external`, even if this `add` changes its version, and the error remains: add it to `allow` by hand. Agreeing to give the host to the Pome being added does not extend to a Pome the project already depended on.
- `add <src>@<version>` writes the version even if it is lower than the current requirement (the requirement can be lowered). `@latest` and an omitted version use the `@latest` version of `[pome.fetch]`.
- `add --dev` writes nothing and ends with 1 if it points to a Pome in `[dependencies]` (E0240). When `add` without `--dev` points to a Pome in `[dev-dependencies]`, it moves it from `[dev-dependencies]` to `[dependencies]`. When moving, it moves the entry in its written form and then writes the version.
- `update` only raises requirements and never lowers them. It raises the requirements in both `[dependencies]` and `[dev-dependencies]`. It also raises the requirements of dependencies with `[replace]`, and leaves `[replace]` as it is.
- `remove` leaves `[replace]` in place.
- `install` fetches the versions in the lock, verifying them with the hashes in the lock, and does not query the sumdb. Only versions not in the lock are checked against the sumdb. The same holds for the `Pome.toml` files read while walking the dependency graph.
- `list` prints one entry per line: the source path and the requirement. If the selected version is greater than the requirement, it appends ` -> <version>`, and for a replaced dependency it appends ` => <directory>`. Lines for Pomes in `[dev-dependencies]` and Pomes marked `dev` in the lock get `(dev)`. Cycles are not followed.
- `update` prints the dependencies whose list of files containing `@external` in the lock changed, together with the version changes. Because the permission is given per source path, it does not stop even if the list grows.
- `verify` prints mismatched dependencies one per line and ends with 1. The cache is read-only, but the command exists to detect that another tool has rewritten it.
- `verify` checks only the dependencies in the cache, and does not treat dependencies missing from the cache as a problem. It prints mismatched dependencies to standard output and `error: N dependencies differ from Pome.lock` to standard error. If everything matches, it prints `the cached dependencies match Pome.lock` and ends with 0.
- `publish` does not rewrite `Pome.toml` or `Pome.lock`. When version is omitted, it uses `[pome].version`. It proceeds in the following order, and if it stops at any step, it reports an error.
  1. A local tag `v<version>` exists, and its `Pome.toml` satisfies the conditions of 4.7 (matching `name` and `version`, restrictions on the contents).
  2. `git ls-remote` shows that the same tag in the repository points to the same commit as the local one. If it does not, the command suggests pushing the tag. `publish` does not push.
  3. Computes `h1:` locally from the tree of the tag.
  4. Compares the public API with the previous version, and warns if the version bump is not large enough (W0901, "Comparing the public API" below). It does not stop even if there are warnings.
  5. Requests `<p>/@v/<version>.info` from the first proxy in `EMELA_POMEPROXY` to have it ingest the version.
  6. Looks up the sumdb record and verifies it as in 4.7. If `h1:` matches the local one, it prints the record and finishes. If it differs, different contents were recorded first (for example because the tag was moved), so the command suggests a tag for a new version.
- `--dry-run` skips 2, 5, and 6. It is used to run only the checks and the public API comparison after tagging locally and before pushing. Because an ingested version cannot be withdrawn, this lets you push after seeing the warnings.
- `publish` is not a required step for publishing. It exists so that the author, rather than the dependents, finds ingestion errors (a mismatched `Pome.toml`, symbolic links, and so on) and version bump errors first. A Pome matched by `EMELA_POMEPRIVATE`, and the case where `EMELA_POMEPROXY` has no proxy, are errors (except with `--dry-run`).

Common:

- The commands look for `Pome.toml` from the current directory up through its ancestors. They take no argument that passes a path.
- Every command builds the new `Pome.toml` and lock first, and writes the two only after all dependencies are resolved. If it fails midway, neither file changes.
- When rewriting `Pome.toml`, comments, order, and the form of tables (`{ version, module }`) are preserved.
- When `[dev-dependencies]` becomes empty, the whole table is removed. `[dependencies]` is kept even when empty.
- An error is printed as one line starting with `error: ` to standard error, and the command ends with exit code 1.

Output:

- `add`, `remove`, `install`, and `update` print what changed to standard output. When requirements in `Pome.toml` change, they are printed under the heading `Pome.toml:`, and when the lock changes, under the heading `Pome.lock:`, one per line. If the lock does not change, the command prints `Pome.lock is up to date`.
- The form of a line is `+ <source path> <version>` for an addition, `- <source path> <version>` for a removal, and `  <source path> <old version> -> <new version>` for a change. Test dependencies get ` (dev)`. A Pome added to `allow` by `add --allow-external` is printed under `Pome.toml:` as `+ [external] <source path>`.
- For a dependency whose `external` list in the lock changed, added files are printed as `+ <path>` and removed files as `- <path>` under the heading `@external of <source path>:`.

Comparing the public API:

- The public API is the pub definitions (functions, type, enum, error, effect, trait, const, handler, layer) of all modules in the source root except `src/main.emel`, together with their types. Public functions with omitted type annotations (4.5) are compared by their inferred types.
- The version compared against (the previous version) is the largest version in the proxy's `@v/list` that has the same major and is smaller than the current version. Prerelease versions are included. If there is no previous version, no comparison is made. The previous version is fetched by the same path as dependencies and verified with the sumdb.
- The following changes are treated as incompatible.
  - Removing a pub definition or module, making it non-pub, or renaming it.
  - Changing the number, types, or names (6.2) of a function's parameters, or its return type. Adding an error to fails. Adding an effect to use. Adding a bound to a type parameter.
  - Adding, removing, or changing fields of a type or error. Making it opaque (4.4).
  - Adding, removing, or changing variants of an enum (this breaks exhaustive matches in dependents).
  - Adding, removing, or changing operations of an effect or methods of a trait (this breaks implementers).
  - Changing the type of a const.
- Adding pub definitions or modules, and making something no longer opaque, are treated as compatible additions. Removing errors from fails and effects from use are treated as compatible (18.1 #41).
- The required bump is major if there are incompatible changes, minor if there are only compatible additions, and patch if the public API does not change. While at v0, each is shifted down by one: incompatible changes require minor, and compatible additions require patch. `v0.0.z` and prerelease versions are not compared.
- Behavior changes that do not change types are not examined.

Other commands:

- A new Pome is created with `emela new <name>`. With `--lib`, it is a library (`src/lib.emel`).

## emela lsp `[cmd.lsp]`

- Takes no arguments and runs as a language server over standard input and output.
- On receiving the exit notification, it ends with 0. When it stops on a protocol error, it prints one line to standard error and ends with 1.
- Reads the test root and `[dev-dependencies]` (`[module.pome]`). Dependencies are read only from the cache, and the server does not go to the network (`[pome.lock]`).
