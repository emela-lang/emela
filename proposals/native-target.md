---
title: The C target becomes the native target
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [target, target.c, target.c.output, target.c.cc, target.c.run, target.c.unsupported, target.c.event-loop, target.c.export, ext.decl, ext.host-type, ext.boundary, ext.c-host, ext.export, module.name, module.pome, lib.layers, api.std.file-system, cmd.build, cmd.doctor, cmd.test.out-dir, lsp.completion, open.list, diag.list]
requires: []
related: [js-host-variable, doctor-command]
---

# The C target becomes the native target

## Summary

The build target `c` is renamed `native`. It still emits C, but C becomes an intermediate form: the specification promises a native executable made by the C compiler and linker of the machine that builds, and the header `emela.h` for host sources, not the emitted C files. The program links against the C library of that machine. Differences between the C libraries and headers of operating systems are resolved by compiling the C on that machine with its own headers; the compiler still does not read C headers, and Emela source has no construct that depends on the OS.

The first argument of `@external` and `@export` is redefined as the language of the code that is linked with, not the build target. It stays `c` for the native target. Each target has one such language: `native` uses `c`, `js` uses `js`, and `wasm` uses `wasm`.

The C compiler is named by `EMELA_CC` alone, and its flags are given by a new variable `EMELA_CC_ARGS`. `emela doctor` checks that the C compiler can compile and link a program against the C library. The build target `c` is deprecated in 0.28 and removed in 0.29.

## Motivation

- `c` names how the target is built, not what it produces. What a user asks for with `--target c` is an executable for the machine in front of them. `js` and `wasm` name what is produced.
- The emitted C files (`main.c`, `emela_runtime.c`) are specified in `[target.c.output]` and `[cmd.build]`, so splitting the runtime differently, or later producing machine code without C, is a change of the specification. Only the executable and `emela.h` are used by anyone outside the compiler.
- The specification does not say how the C target relates to the C library of each OS. The bundled sources already depend on it (`file_system.c` and `random.c` branch on `__APPLE__` and `__linux__`), and `@external(c, "name")` resolves `name` in the C library at link time. Whether a user may call any C library function this way, and what happens with structures and macros whose definitions differ between operating systems, is not written down.
- `EMELA_CC` is split at whitespace to take flags, so it cannot name a compiler whose path contains a space. There is no way to pass a flag that has to follow the sources, such as `-l` for a library of the OS.
- `emela doctor` reports a C compiler that starts as `ok`, even when it cannot compile against the C library (a Linux system without the C headers, or a macOS whose Command Line Tools do not match the OS). The failure appears only at the next build, as E0908 with the full standard error of the compiler.

## Proposal

### Meaning

The native target:

- `--target native`, `[build].target = "native"` and `[pome].targets` that contains `"native"` select it.
- The compiler emits C and invokes the C compiler of the machine it runs on. The C compiler compiles and links with the headers and the C library of that machine. The result is an executable for that machine.
- The C library of the OS is a dependency of every native executable. The compiler does not read C headers and does not carry definitions of C library types. The C compiler resolves them.
- The emitted C and the runtime include only headers of C11 and of POSIX.1-2008. The bundled C sources of std MAY use headers specific to one OS, guarded so that each source compiles on every supported platform.
- Differences between operating systems are confined to C sources. A value that crosses the boundary between Emela and C has a boundary type (12.4). A structure, a macro or a constant of the C library never crosses the boundary: a C source converts it to boundary types.
- Emela source has no construct that depends on the OS. The same program has the same meaning on every OS that the native target supports.

Supported platforms:

- The native target builds for the machine that runs the compiler. It does not build for another OS or CPU.
- The supported platforms are macOS on arm64, and Linux on x86_64 with glibc. These are the platforms for which the compiler is released and on which the native target is tested.
- On another POSIX system (Linux on aarch64, Linux with musl, macOS on x86_64, a BSD), and with a C compiler chosen with `EMELA_CC`, the native target MAY work. The specification does not forbid it, and it is not tested.
- Windows is not supported. #39 of `[open.list]` stays open. A platform is added together with a release of the compiler for it and tests of the native target on it.

The C compiler:

- The machine that builds needs a C compiler that accepts C11 or a later version of C, and the headers and the C library of POSIX.1-2008, with threads.
- The C compiler accepts the command-line options `-std=`, `-I`, `-o`, `-l` and `-pthread` in the form that GCC and Clang accept them. GCC and Clang meet this requirement, and so does the `cc` of macOS.
- The emitted C and the runtime compile as C11 and as every later version of C. Which `-std=` the compiler passes is implementation-defined, and is C11 or later.
- The compiler is the value of `EMELA_CC` if it is not only whitespace, and otherwise `cc` on PATH. The whole value names one program; it is not split at whitespace.
- `EMELA_CC_ARGS` holds flags for the C compiler. It is split at whitespace, and the words are passed at the end of the command line, after every source file. So a flag given there takes precedence over a flag of the same kind given earlier (`EMELA_CC_ARGS="-std=c23"`), and a library given with `-l` is linked after the sources that use it.
- The C sources of the project and of Pomes are compiled by the same command as the emitted C. The command guarantees them the following: they are compiled as C11 or later, they can use threads, `#include "emela.h"` finds the header in the output directory, and the math library is linked. The rest of the command line (the optimization level, other flags, the order of the files) is implementation-defined.
- When the C compiler cannot be started and the value of `EMELA_CC` contains whitespace, E0907 has the help ``= help: EMELA_CC names only the compiler; pass flags in EMELA_CC_ARGS``.

The output of the native target:

- The output directory contains the executable `main` and the header `emela.h` for the host's C sources (`[ext.c-host]`).
- The output directory also contains implementation-defined files, such as the emitted C. Their names and contents are not specified, and a later version MAY change them.
- No option is added to keep or show the emitted C. The executable keeps the name `main`.
- E0908 does not name an emitted file: ``` `cc` failed to compile the output of target `native` ```. The standard error of the C compiler is attached as a note, as today.

`emela doctor`:

- The row of the native target checks the C compiler in two steps. First it starts the compiler with `--version`, as today. If that passes, it compiles and links a small C program in a temporary directory, which it removes afterwards. The program includes the headers that the runtime uses, and the command has the same flags as a build, including the flags of `EMELA_CC_ARGS` at the same position. The executable is not started.
- When the program cannot be compiled or linked, the status is `failed`. The message contains the first line of the standard error of the compiler that reports an error, and the help is ``install the C headers and the C library of the system, or set EMELA_CC to another compiler``.
- When `EMELA_CC_ARGS` is set, the report shows its value.

The first argument of `@external` and `@export`:

- It names the language of the code that is linked with. The languages are `c`, `js` and `wasm`.
- Each build target uses one language: `native` uses `c`, `js` uses `js`, and `wasm` uses `wasm`. An `@external` or `@export` takes effect when its language is the language of the build target.
- The rules that refer to "the build target" in 12.1 and 12.7 refer to the language of the build target instead. For example, E0412 is reported when an external function reachable from the entry has no `@external` for the language of the build target.
- `@external(c, "name")` with no file is for a C function whose prototype is made of the C types of 12.4 only. A C function whose use needs a structure, a macro or a constant of the C library is called through a C source of the project or of the Pome.

The old name `c` as a build target:

- `c` is deprecated as a build target. `--target c`, `[build].target = "c"` and `[pome].targets` that contains `"c"` select the native target, and W0903 is reported.
- `c` is read as a build target until the minor version after the one that deprecates it: 0.28 deprecates it, and 0.29 removes it. After that, it is an unknown target.
- The language `c` in `@external` and `@export` is not deprecated.
- When the implementation is complete, an issue is opened for removing the build target `c` in 0.29.

Section names:

- The sections `[target.c]` and `[target.c.*]` are renamed `[target.native]` and `[target.native.*]`, and the file `spec/targets/c.md` becomes `spec/targets/native.md`. The old names are kept in `spec/history.md` as headings that point to the new names, so that the references from adopted proposals still resolve. They MUST NOT be reused.
- `[ext.c-host]` keeps its name: it is about host functions written in the language C.

### Syntax

No change. `@external(c, ...)` and `@export(c, ...)` are written as today.

## Specification changes

### `[target]`

Replace the second and third paragraphs with:

> There are three targets: JS, WASM and native. JS and WASM are Tier 1, and native is Tier 2. A Tier 2 target has the same meaning as Tier 1, meets every rule in this chapter, and passes the same test suite. However, new features MAY arrive later than on Tier 1.
>
> The native target makes an executable for the machine that builds it, by emitting C and compiling it with the C compiler and the C library of that machine (`[target.native]`). The supported platforms are macOS on arm64 and Linux on x86_64 with glibc, the platforms for which the compiler is released and on which the native target is tested. On another POSIX system the native target MAY work, and it is not tested. Windows is not supported (18.1 #39). The constructs that the native target does not support yet are in `[target.native.unsupported]`, and using them is a compile error. The observable behavior of each target is in `[target.js]`, `[target.wasm]` and `[target.native]`.

### `[target.c]` → `[target.native]`

Rename the file to `spec/targets/native.md`. Replace the heading and the introduction with:

> # 16.7 The native target `[target.native]`
>
> The native target makes an executable for the machine that runs the compiler. The compiler emits C and invokes the C compiler of that machine, which compiles and links it with the headers and the C library of that machine. The native target does not build for another OS or CPU. Passing `--target native` to `emela build`, `emela run` or `emela test` builds for this target.
>
> - The meaning of numbers and strings follows `[target.number]` and `[target.string]`. `+`, `-`, `*` and unary minus on Int and Int64 wrap around in two's complement, and `-2147483648 / -1` also wraps around (the remainder is 0).
> - Built-in functions that need the grapheme cluster tables are not available yet (`[target.native.unsupported]`).
> - External functions are declared with `@external(c, ...)`, and host functions are written in the form of `[ext.c-host]`. The language of the native target is `c` (`[ext.decl]`).
> - The C library of the OS is a dependency of every executable. The compiler does not read C headers and does not carry definitions of the types of the C library; the C compiler resolves them.
> - The emitted C and the runtime include only headers of C11 and of POSIX.1-2008. The bundled C sources of std MAY use headers specific to one OS, guarded so that each source compiles on every supported platform (`[target]`).
> - Differences between operating systems are confined to C sources. Only boundary types (12.4) cross the boundary between Emela and C, so a structure, a macro or a constant of the C library never reaches Emela code. Emela source has no construct that depends on the OS, and a program has the same meaning on every supported platform.
> - The build target `c` is a deprecated name of the native target. `--target c`, `[build].target = "c"` and a `[pome].targets` that contains `"c"` select the native target and report W0903. It is read until 0.29, after which it is an unknown target.

### `[target.c.output]` → `[target.native.output]`

Replace the section with:

> ## 16.7.1 Output `[target.native.output]`
>
> - The output directory is `target/emela/native` for `emela build` and `emela run`, and `target/emela/test-native` for `emela test` (both under the directory of the project or of the single file, `[module.name]`). The test output does not overwrite the output of a normal build.
> - The output directory contains the following files.
>
>   | File | Contents |
>   |---|---|
>   | `main` | The executable |
>   | `emela.h` | The public header for the host's C sources (`[ext.c-host]`) |
>
> - The output directory also contains implementation-defined files, such as the emitted C, the runtime, and the bundled C sources that the external functions used by the program refer to. Their names and contents are not specified.
> - The C sources of the project and of Pomes (the ones `@external(c, "./file.c", ...)` refers to) are not copied to the output directory, and are compiled by their absolute paths.
> - If the entry point has no main, the executable does nothing and exits with exit code 0.

### `[target.c.cc]` → `[target.native.cc]`

Replace the section with:

> ## 16.7.2 Invoking the C compiler `[target.native.cc]`
>
> - The machine that builds needs a C compiler that accepts C11 or a later version of C, and the headers and the C library of POSIX.1-2008, with threads.
> - The C compiler MUST accept the options `-std=`, `-I`, `-o`, `-l` and `-pthread` in the form that GCC and Clang accept them. GCC and Clang meet this requirement.
> - The C compiler is the value of the environment variable `EMELA_CC` if it is not only whitespace, and otherwise `cc` on PATH. The whole value names one program, and is not split at whitespace.
> - The environment variable `EMELA_CC_ARGS` holds flags for the C compiler. It is split at whitespace, and the words are passed at the end of the command line, after every source file. A flag given there takes precedence over a flag of the same kind given earlier (`-std=c23`), and a library given with `-l` is linked after the sources that use it.
> - The emitted C, the runtime, the bundled C sources and the C sources of the project and of Pomes are compiled and linked by one invocation of the C compiler, with the output directory as the current directory. The invocation guarantees the following to the C sources of the project and of Pomes: they are compiled as C11 or a later version (which one is implementation-defined), they can use threads (`[ext.c-host]`), `#include "emela.h"` finds the header in the output directory, and the math library is linked. The rest of the command line is implementation-defined.
> - The emitted C and the runtime compile as C11 and as every later version of C.
> - If the C compiler cannot be started, it is E0907 (``cannot start `cc`: <reason>``). When it is not found, the help ``install a C11 compiler and add it to PATH as `cc`, or set EMELA_CC to the compiler`` is attached. When the value of `EMELA_CC` contains whitespace, the help ``EMELA_CC names only the compiler; pass flags in EMELA_CC_ARGS`` is attached instead.
> - If the C compiler fails, it is E0908 (``` `cc` failed to compile the output of target `native` ```). The standard error of the C compiler is attached as a note. Anything beyond 4000 bytes is cut off, and a `...` line is added.
> - On success, the output of the C compiler is not shown.

### `[target.c.run]` → `[target.native.run]`

Rename the heading to `## 16.7.3 Running` with the name `[target.native.run]`. Replace `--target c` with `--target native` in the first and third items.

### `[target.c.unsupported]` → `[target.native.unsupported]`

Rename the heading to `## 16.7.4 Constructs not supported yet` with the name `[target.native.unsupported]`. In every message, replace ``which target `c` does not support yet`` with ``which target `native` does not support yet``. Replace "An external function without `@external(c, ...)`" with "An external function without `@external(c, ...)` (the language of the native target, `[ext.decl]`)". Replace "when the target is `c` (E0442)" with "when the target is `native` (E0442)".

### `[target.c.event-loop]` → `[target.native.event-loop]`

Rename the heading to `## 16.7.5 Event loop` with the name `[target.native.event-loop]`. The text does not change.

### `[target.c.export]` → `[target.native.export]`

Rename the heading to `## 16.7.6 Exported functions` with the name `[target.native.export]`. Replace "is emitted in `main.c` as a function with external linkage" with "is emitted as a function with external linkage in the executable". Replace "on the `c` target" with "on the native target" throughout, and ``which target `c` does not support yet`` with ``which target `native` does not support yet``.

### `[ext.decl]`

Replace the item "The arguments of `@external` on a fn have the form ..." with:

> - The arguments of `@external` on a fn have the form `(language, "name")` or `(language, "file", "name")` only (E0138). The language names the language of the code that is linked with: `js`, `wasm` or `c` (E0140). The strings cannot contain interpolation.
> - Each build target uses one language. An annotation takes effect when its language is the language of the build target.
>
>   | Build target | Language |
>   |---|---|
>   | `js` | `js` |
>   | `wasm` | `wasm` |
>   | `native` | `c` |

Replace "One `@external` is written per target. ... Two annotations for the same target on the same fn is a compile error (E0139)." with "One `@external` is written per language. A fn MAY have annotations for several languages. Two annotations for the same language on the same fn is a compile error (E0139)."

Replace "An unknown target name gives only E0140 and is not counted as a duplicate of the same target (E0139). For a duplicate, E0139 is reported on the target name of the second and later annotations." with "An unknown language name gives only E0140 and is not counted as a duplicate of the same language (E0139). For a duplicate, E0139 is reported on the language name of the second and later annotations."

Replace "A fn with no annotation for the build target is a compile error only when it is reachable from the entry point" with "A fn with no annotation for the language of the build target is a compile error (E0412) only when it is reachable from the entry point". Replace "has no annotation for the target is a defect when called" with "has no annotation for the language of the build target is a defect when called".

Replace the item "For `c`, `(target, "name")` refers to ..." with:

> - For `c`, `(c, "name")` refers to a C function resolved at link time (one from the C library or another library of the OS). It is for C functions whose prototype is made of the C types of 12.4 only. A C function whose use needs a structure, a macro or a constant of the C library is called through a C source that converts them to boundary types. `(c, "file", "name")` refers to a C source (`.c`) by a path relative to the directory of the source that contains the declaration, and the compiler compiles that file together with the output and links it (`[target.native.cc]`). The `.c` files that the bundled std refers to are bundled with the compiler together with std.

### `[ext.host-type]`

Replace "On the `c` target" with "On the native target".

### `[ext.boundary]`

- Replace "On the `c` target, the types map to C types" with "For the language `c`, the types map to C types".
- Replace "The generated `main.c` declares host functions" with "The emitted C declares host functions", and "The generated `main.c` includes only the runtime header" with "The emitted C includes only the runtime header".
- Replace "The first version of the C target (0.21)" with "The native target"; replace "When the build target is `c`" with "When the build target is `native`" (twice); replace "The C target does not support Bytes and Array" with "The native target does not support Bytes and Array".

### `[ext.c-host]`

Replace "an external function declared as `suspend fn` on the `c` target" with "an external function declared as `suspend fn` on the native target". Replace references to `[target.c.event-loop]` with `[target.native.event-loop]`.

### `[ext.export]`

Replace "Its arguments have the form `(target, "name")` or `(target, "interface", "name")`, as for `@external` (12.1). One `@export` is written per target name; a fn MAY have several for the same target. Malformed arguments are E0146, and an unknown target is E0140." with "Its arguments have the form `(language, "name")` or `(language, "interface", "name")`, as for `@external` (12.1). A fn MAY have several `@export` for the same language. Malformed arguments are E0146, and an unknown language is E0140. An `@export` takes effect when its language is the language of the build target."

Replace "Two exports with the same name for the same target" with "Two exports with the same name for the same language". Replace `[target.c.export]` with `[target.native.export]`.

### `[module.name]`

Replace "(`js`, `c`, `wasm`)" with "(`js`, `native`, `wasm`)".

### `[module.pome]`

- In the example of `Pome.toml`, replace `target = "c"` with `target = "native"`.
- Replace "(`js`, `c`, `wasm`, chapter 16)" with "(`js`, `native`, `wasm`, chapter 16)". Add after "An unknown target is a compile error (E0232).": "`c` is a deprecated name of `native` and reports W0903 (`[target.native]`)."
- Replace "declares the targets the Pome runs on (`js`, `wasm`, `c`)" with "declares the targets the Pome runs on (`js`, `wasm`, `native`; `c` is a deprecated name of `native` and reports W0903)".

### `[lib.layers]`

Replace "A platform handler also lists `@external` for the `c` target." with "A platform handler also lists `@external(c, ...)` for the native target."

### `[api.std.file-system]`

Replace "On C, it calls the POSIX functions synchronously" with "On native, it calls the POSIX functions synchronously". Replace the item "On the C target, SystemFileSystem fails with FileSystemFailure ..." with:

> - On the native target, SystemFileSystem fails with FileSystemFailure when it reads a file that contains U+0000 (it cannot be passed as a C string). The message of FileSystemFailure has the form `EIO: Input/output error, open 'a.txt'`. The descriptive text is that of the C library of the OS, and MAY differ between operating systems.

### `[cmd.build]`

- Replace the item "For `c`, the command writes `main.c` ..." with: "For `native`, the command writes the executable `main` and the header `emela.h` to the output directory, with other implementation-defined files (`[target.native.output]`). The C compiler is chosen as in `[target.native.cc]` (E0907, E0908)."
- Replace "the executable `main` for `c`" with "the executable `main` for `native`".
- Replace "`--target <js|c|wasm>`" with "`--target <js|native|wasm>`", and add: "`--target c` selects `native` and reports W0903."
- Replace "(`target/emela/js`, `target/emela/c`, `target/emela/wasm`)" with "(`target/emela/js`, `target/emela/native`, `target/emela/wasm`)".

### `[cmd.doctor]`

- Replace "`--target <js|c|wasm>`" with "`--target <js|native|wasm>`".
- Replace the row of `c` in the table with:

  | Target | Program | Rule | Passes when |
  |---|---|---|---|
  | `native` | the C compiler | `[target.native.cc]` | it starts, and it compiles and links a program against the C library; the first line of its `--version` is reported as its version |

- Add after the table:

  > - For `native`, after the C compiler starts, the command compiles and links a small C program in a temporary directory, which it removes afterwards. The program includes the headers that the runtime uses, and the command line has the same flags as a build, including those of `EMELA_CC_ARGS` at the same position. The executable is not started. When the program cannot be compiled or linked, the status is `failed`, the message contains the first line of the standard error of the C compiler that reports an error, and the help is ``install the C headers and the C library of the system, or set EMELA_CC to another compiler``.
  > - When `EMELA_CC_ARGS` is set, the line of the C compiler is followed by a line with its value.

- Replace "in the order `js`, `c`, `wasm`" with "in the order `js`, `native`, `wasm`".

### `[cmd.test.out-dir]`

Replace "If the target is `c`, the directory is `test-c`" with "If the target is `native`, the directory is `test-native`".

### `[lsp.completion]`

Replace "the target names `js`, `wasm` and `c`" with "the language names `js`, `wasm` and `c`".

### `[open.list]`

- #39: replace with "More platforms for the native target (Windows, other CPUs, other C libraries). A platform is added together with a release of the compiler for it and tests of the native target on it".
- #53: append "On the native target, the descriptive text MAY also differ between operating systems".
- #56: append "Whether the language name of WASM in `@external` becomes `wit` is decided with this".

### `[diag.list]`

- E0140: "An unknown language name in `@external` or `@export` (currently js, wasm, c) (12.1)". Message ``unknown language `node` ``.
- E0146: replace "on the `c` target" with "on the native target".
- E0232: the note is ``= note: expected one of `js`, `native`, `wasm` ``.
- E0412: "An external function reachable from the entry has no `@external` for the language of the build target (12.1)". The help is ``= help: add `@external(<language>, ...)`, or avoid calling it from the entry``. Message ``external function `wasm_now` has no `@external(js, ...)` for target `js` ``.
- E0427, E0441, E0442, E0443: replace "target `c`" with "target `native`" in the descriptions and the messages.
- E0907: add "When `EMELA_CC` contains whitespace, the help points to `EMELA_CC_ARGS` (`[target.native.cc]`)".
- E0908: message ``` `cc` failed to compile the output of target `native` ```.
- Add W0903: "The build target `c` in `--target`, `[build].target` or `[pome].targets` (`[target.native]`)". Message ``target `c` is deprecated; use `native` ``, with the help ``= help: `c` is removed in 0.29``.

### `spec/history.md`

Besides the entry for this proposal, add headings for the renamed sections, in the form used for the retired sections:

> - The section names below were renamed when the C target became the native target. They are kept for references from earlier history and proposals, and MUST NOT be reused.
>
> ### The C target (renamed) `[target.c]`
>
> Renamed to `[target.native]`.

and the same for `[target.c.output]`, `[target.c.cc]`, `[target.c.run]`, `[target.c.unsupported]`, `[target.c.event-loop]` and `[target.c.export]`.

## Diagnostics

- [ ] E0140: the description and the message name a language instead of a target
- [ ] E0146: names the native target
- [ ] E0232: the note lists `js`, `native`, `wasm`
- [ ] E0412: names the language and the target
- [ ] E0427, E0441, E0442, E0443: messages name target `native`
- [ ] E0907: a help when `EMELA_CC` contains whitespace
- [ ] E0908: does not name a file
- [ ] W0903 (new): the build target `c` is deprecated

## Compatibility

- `--target c`, `[build].target = "c"` and `[pome].targets = ["c"]` keep working with W0903 in 0.28, and stop working in 0.29.
- `@external(c, ...)` and `@export(c, ...)` do not change.
- `EMELA_CC` is no longer split at whitespace, with no period of deprecation. A value such as `clang -fsanitize=address` becomes E0907 with a help that points to `EMELA_CC_ARGS`. The change comes in the same version as the rename, so a user revisits the settings of the native target once.
- Libraries that a Pome needs (`-lsqlite3`) are passed with `EMELA_CC_ARGS` for now. Declaring them in `Pome.toml` is left to another proposal.
- The output directories move from `target/emela/c` and `target/emela/test-c` to `target/emela/native` and `target/emela/test-native`. Scripts that read `target/emela/c/main` change. Scripts that read the emitted C keep working for now, and are not covered by the specification.

## Implementation

- The CLI accepts `native` and the deprecated `c` for `--target` of `build`, `run`, `test` and `doctor`, and reports W0903 for `c`. `Pome.toml` does the same for `[build].target` and `[pome].targets`.
- The output directories, and the target names in diagnostics.
- The check of reachable external functions and of `@export` maps the build target to its language.
- `EMELA_CC` is no longer split; `EMELA_CC_ARGS` is appended to the command line.
- `emela doctor`: the compile and link check, and the report of `EMELA_CC_ARGS`.
- The section names in comments of the code.
- Tests, CI (`tests/laws` runs `--target c`) and documentation.
- When complete, an issue for removing the build target `c` in 0.29.

## Alternatives

- `@external(native, ...)`: the first argument stays the build target. One vocabulary instead of two, and no table of targets and languages. Rejected because the argument says what is linked with: `@external(native, "./x.c", "f")` does not say that `x.c` is C, and if the native target later produces machine code without C, the linked code is still C. A C declaration could also be shared with another target that links C code.
- Reading C headers (by embedding a C front end) so that Emela declarations can use C library types. Rejected because it makes the compiler depend on a C front end, and because the boundary types (12.4) already keep C library types out of Emela.
- Carrying the definitions of C library types for each OS and CPU in the compiler. Rejected because every supported OS and CPU adds definitions to keep in step with the C library, while compiling on the machine that builds gets them from its headers.
- Calling the OS without the C library. Rejected because macOS supports calls into the OS only through libSystem, linked dynamically.
- A construct in Emela that selects code by OS. Rejected because the same program would have different meanings on different operating systems, against `[target]`.
- Raising the minimum version of C to C23. Rejected for now: GCC accepts `-std=c23` from 14 and Clang from 18, which are newer than the compilers of several current Linux distributions, and nothing in the runtime needs C23. A user's C sources can use a later version with `EMELA_CC_ARGS`.
- Supporting more platforms (macOS on x86_64, Linux on aarch64, musl). Rejected for now because the compiler is not released for them and the native target is not tested on them, and the native target cannot build for a platform other than the one that runs the compiler.
- An error for `--target c` at once, with a help that shows `native`. Rejected in favor of one minor version with a warning.
- Keeping `EMELA_CC` split at whitespace, or deprecating the split over a minor version. Rejected because a path with a space cannot be named while the value is split, and the rename already asks users to revisit the settings of this target.
- Deciding #53 (the message of FileSystemFailure) here. Rejected because the difference is mostly between targets, not between operating systems, and it is a separate decision.
- Keeping the section names `[target.c.*]`. Rejected because the sections would describe the native target under the name of a language.

## Prior art

- Nim and Koka emit C and leave the C compiler to resolve the headers of the platform. Nim's `importc` with `header` emits only the name and the `#include`.
- Go calls the kernel directly on Linux, and through libSystem on macOS since 1.11. It generates the definitions of OS constants and structures from the headers of each OS and commits them.
- Rust's `libc` crate writes the definitions of each target by hand, selected by `cfg`.
- Zig bundles the headers of many C libraries, which allows compiling for another OS. Swift reads C headers through Clang.

## Open questions

## Decision
