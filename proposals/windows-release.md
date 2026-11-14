---
title: Release the compiler for Windows
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [target, target.native, open.list, lib.layers, api.std.path, module.name, module.pome, pome.fetch, cmd.build, cmd.test.outcome]
requires: []
related: [native-target]
---

# Release the compiler for Windows

## Summary

From 0.28, the compiler is also released for Windows on x86_64. On Windows, every command and the JS and WASM targets are supported; the native target is not (18.1 #39 stays open). `[target]` stops using "the platforms for which the compiler is released" as the definition of the platforms that the native target supports, and states the two lists separately.

## Motivation

- `[target]` says: "The supported platforms are macOS on arm64 and Linux on x86_64 with glibc, the platforms for which the compiler is released and on which the native target is tested." One sentence fixes two lists: the platforms of the compiler and the platforms of the native target. Releasing the compiler for Windows without the native target cannot be written in it.
- The JS and WASM targets do not depend on the C toolchain of the machine. Node, Bun and wasmtime run on Windows. Nothing in the language or in the JS and WASM targets requires a POSIX system, so a user on Windows can use them if the compiler runs there.
- Today a user on Windows has no compiler to install. Running it under WSL is a workaround, but the editor (the language server) and the programs then run inside Linux, not on the user's system.

## Proposal

- The compiler is released for macOS on arm64, Linux on x86_64 with glibc, and Windows on x86_64. Windows is added in 0.28.
- On each platform of the compiler, every command of Appendix B, the language server, the JS target and the WASM target are supported and tested.
- The native target supports macOS on arm64 and Linux on x86_64 with glibc. It is not supported on Windows. Adding Windows to the native target stays #39 of `[open.list]`. `--target native` on Windows gets no special treatment: the compiler looks for the C compiler as on any machine, and reports E0907 or E0908 when it cannot use one.
- The specification is written for macOS and Linux, and does not hide behavior that is specific to Windows. Where Windows differs from them in what it hands to a program (environment variable names compared without case, paths with `\`, no signals), the program sees what Windows does, and the specification does not add rules to make it look like the other platforms.
- The meaning of a program does not depend on the platform of the compiler. `Std.Path` keeps `/` as its only separator on every platform.
- The paths that the commands write (diagnostics, test locations, the `running` line) use `/` as the separator on every platform, so the output is the same text on every platform.
- The `running` line keeps quoting for a POSIX shell on every platform.
- The Pome cache stays at `~/.emela/pomes`, where `~` is the home directory of the user (`%USERPROFILE%` on Windows). The layout is the same on every platform, next to the directory into which the compiler is installed.
- The exit code of `run` on Windows is the exit code that Windows reports for the program, passed through. A process on Windows is not stopped by a signal, so the rule "128 + the signal number" does not arise there.

Out of scope: a source file with CRLF line breaks (the default checkout of Git for Windows) gives a multi-line string whose value contains `\r\n`, so the same repository gives different string values when checked out on Windows and on Linux. `[lex.literal]` does not say which line break the value contains. This holds on every platform, and needs its own proposal.

## Specification changes

### `[target]`

Replace the third paragraph with:

> The compiler is released for macOS on arm64, Linux on x86_64 with glibc, and Windows on x86_64 (from 0.28). On each of them, every command (Appendix B), the JS target and the WASM target are supported and tested.
>
> The native target makes an executable for the machine that builds it, by emitting C and compiling it with the C compiler and the C library of that machine (`[target.native]`). The native target supports macOS on arm64 and Linux on x86_64 with glibc, on which it is tested. On another POSIX system the native target MAY work, and it is not tested. The native target is not supported on Windows (18.1 #39). The constructs that the native target does not support yet are in `[target.native.unsupported]`, and using them is a compile error. The observable behavior of each target is in `[target.js]`, `[target.wasm]` and `[target.native]`.

### `[target.native]`

In the fifth bullet, replace "on every supported platform (`[target]`)" with:

> on every platform that the native target supports (`[target]`)

### `[open.list]`

Replace the question of #39 with:

> More platforms for the native target (Windows, other CPUs, other C libraries). A platform is added together with tests of the native target on it, and with a release of the compiler for it if the compiler is not released for it yet

### `[lib.layers]`

Replace the `Std.Path` bullet with:

> - `Std.Path` has pure functions that build path strings: `join(a, b)`, `parent(path)`, `file_name(path)`, and `extension(path)`. Only `/` is handled as a separator, on every platform: `\` and a drive letter (`C:`) are ordinary characters of a segment, so the results do not depend on the platform. The hosts of the JS target accept `/` as a separator on Windows as well. `parent`, `file_name`, and `extension` return an Option (`Path.parent("a.txt")` is None).

### `[api.std.path]`

Replace the paragraph under the heading with:

> The value layer (`[lib.layers]`). Pure functions that build path strings. Only `/` is a separator, on every platform; `\` is an ordinary character (`parent("a\\b.txt")` is `None`). No normalization. A trailing `/` is ignored.

### `[module.name]`

Replace "Paths in diagnostics are relative to the project (for a single file, to the file's directory)." with:

> - Paths in diagnostics are relative to the project (for a single file, to the file's directory). They are written with `/` as the separator on every platform, including absolute paths (`C:/Users/me/.emela/pomes/...` on Windows).

### `[module.pome]`

Replace "Fetched versions are extracted to `~/.emela/pomes/<source path>@<version>/` and kept read-only." with:

> - Fetched versions are extracted to `~/.emela/pomes/<source path>@<version>/` and kept read-only. `~` is the home directory of the user (`%USERPROFILE%` on Windows).

The rest of the bullet (the escaping, `EMELA_POME_CACHE`) is unchanged.

### `[pome.fetch]`

In the table, replace "Read-only, with files 0444 and directories 0555" with:

> Read-only, with files 0444 and directories 0555. On Windows, files have the read-only attribute and directories are not marked

### `[cmd.build]`

Under "Starting the program", after the bullet on quoting, add:

> - The rule above is the same on every platform. Paths that the command builds (the entry module, `main.wasm`, the executable) are written with `/` as the separator.

In the exit codes, replace "128 + the signal number when stopped by a signal." with:

> - 128 + the signal number when stopped by a signal. On Windows, where a process is not stopped by a signal, the exit code that Windows reports for the program.

### `[cmd.test.outcome]`

Replace "The path is relative to the project directory, and in single-file mode to the directory that contains the file." with:

> The path is relative to the project directory, and in single-file mode to the directory that contains the file, and is written with `/` as the separator on every platform.

## Diagnostics

- [ ] None.

## Compatibility

- No program changes meaning, and no program that compiles now stops compiling. The change adds a platform.
- On macOS and Linux nothing changes. The new wording of the paths in output and of the exit code only states what these platforms already do.

## Implementation

Done in another workspace, not part of this proposal:

- Fixes so that the workspace compiles for `x86_64-pc-windows-msvc`.
- CI that builds and tests on Windows (the commands, the JS target and the WASM target).

Also needed for the release:

- A Windows job in `.github/workflows/release-plz.yml`. The archive is a `.zip` with `emela.exe`, `LICENSE` and `LICENSE-UNICODE`.
- The MSVC build links the C runtime dynamically by default, so `emela.exe` needs the Visual C++ runtime on the machine. Linking it statically (`-C target-feature=+crt-static`) removes that dependency.
- `install.sh` is a POSIX shell script. Windows needs another way to install (a PowerShell script, or only the archive).
- The JS host and wasmtime are looked up "on PATH" (`[cmd.build]`, `[target.wasm.run]`). On Windows the lookup also tries the extensions in `PATHEXT` (`node` finds `node.exe`).
- Paths in output are converted from `\` to `/` when they are written (`[module.name]`, `[cmd.test.outcome]`, `[cmd.build]`).
- The Pome cache on Windows is created under `%USERPROFILE%`, and its files get the read-only attribute.

## Alternatives

- Keep Windows out until the native target supports it. #39 has no plan yet; the JS and WASM targets do not need it, so users on Windows would wait for a target they may not use.
- Release for Windows, and leave `[target]` unchanged. The sentence would then say that the native target supports Windows, because it is a platform for which the compiler is released.
- Release for `x86_64-pc-windows-gnu` (MinGW) instead of MSVC. MSVC is the ABI of the Windows toolchain itself and needs no MinGW runtime. The choice does not change what the specification says; it says only "Windows on x86_64".
- A separator of `Std.Path` chosen by the platform (`\` on Windows). The result of a pure function would then depend on the machine, against the first paragraph of `[target]`.
- A compile error for `--target native` on Windows, before the C compiler is started. It needs a new diagnostic, and stops experiments with a C compiler given through `EMELA_CC`. E0907 and E0908 already report a machine on which no C compiler can be used.
- Paths in output with `\` on Windows. The output would differ between platforms, and tests of the output would need one expectation per platform.
- Quoting the `running` line for PowerShell on Windows. The line would differ between platforms; the specification is written for macOS and Linux.
- Making environment variable names case-sensitive on every target, or case-insensitive on Windows on every target. Either adds a rule for Windows; `SystemEnv` reads what the host gives it.
- The Pome cache in `%LOCALAPPDATA%\emela\pomes` on Windows, the directory for data of the user that is local to the machine. The layout would then differ between platforms, and the cache would be apart from the directory into which the compiler is installed (`~/.emela/bin`).

## Prior art

- Separating the platforms on which a toolchain runs (host platforms) from the platforms for which it generates code (target platforms), each with its own tier of support.

## Open questions

## Decision
