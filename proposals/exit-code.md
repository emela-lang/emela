---
title: Choosing the exit code with the return value of main
authors: [Alliana]
status: draft
pr: 567
issue:
decided:
touches: [dyn.error, effect.with, diag.list, cmd.build, target.native.run, target.wasm.output, ext.export, lib.layers, api.std.process]
requires: [std-process]
related: [std-exit, stdout-stderr]
---

# Choosing the exit code with the return value of main

## Summary

Add to `Std.Process` (the module that std-process created) a value type `ExitCode` and the constants `EXIT_SUCCESS` (0) and `EXIT_FAILURE` (1). When the return type of `main` is `ExitCode` and `main` ends with a value, the program exits with that exit code. Every value from 0 to 255 can be returned. The return type of `main` is `()` or `ExitCode`; a `main` that returns any other type gets the warning W0304 (the value is thrown away and the program exits with 0, as now). An error still exits with 1, a defect with 101 and an interruption with 130. No function or effect operation that stops the process on the spot is added.

```emel
import Std.Platform.{Platform}
import Std.Console.{Console}
import Std.Process.{EXIT_SUCCESS, ExitCode}

error UsageError(message: String)

fn main() -> ExitCode {
  with Platform {
    run() escape {
      UsageError(message:) -> {
        (use Console).print_error_line("usage: tool <file>: #{message}")
        Process.exit_code(2)
      }
    }
  }
}

fn run() -> ExitCode fails UsageError uses { Console } {
  (use Console).print_line("ok")
  EXIT_SUCCESS
}
```

## Motivation

- An Emela program can now exit with only four exit codes: 0, 1, 101 and 130 (`[dyn.error]`, `[dyn.suspend]`, `[error.defect]`). The return value of `main` is thrown away (`fn main() -> Int { 2 }` was checked to exit with 0).
- A CLI tool reports its result through its exit code. Conventions such as 2 for a usage error, or 1 for "not found" and 2 or more for "failed", are read by the shell scripts and CI that call it. Emela has no way to write them.
- The only workaround is to call `process.exit` or C `exit` through `@external`. That skips the releases of withs, cuts output not yet written, and has to be written for each target.
- Raising an error up to `main` fixes the exit code at 1 and always writes the line `error: <value>`. It does not fit when the program wants to write its own usage message.

## Proposal

### Syntax

No change.

### Meaning

- `Std.Process` declares an opaque type `ExitCode`. Its contents are an Int from 0 to 255. `Std.Process` is the module that std-process created, and this proposal adds a type and functions to it (`requires: [std-process]`).
- `Process.exit_code(code)` returns the `ExitCode` of `code`. A `code` outside 0 to 255 is a defect (an assumption of the caller is broken, `[lib.api-rules]`). `Process.exit_code_to_int` returns the Int (`code.exit_code_to_int()` as a method call).
- The function names contain `exit_code` because `Std.Process` also holds `Args` and `Stdin`, so `Process.from_int` and `Process.to_int` would not say what is converted.
- Layer: `ExitCode` itself is a value independent of the host, but it means something only as the way the process ends, so it is placed in `Std.Process` of the platform layer. The row of the platform layer in `[lib.layers]` says that value types that belong to the process are placed there too. Importing `Std.Process` needs no handler (making an `ExitCode` uses no effect).
- When the return type of `main` is `ExitCode` and `main` ends with a value, the program exits with the exit code of that value. When it ends with an error, a defect or an interruption, the program exits as now (1, 101, 130).
- A `main` whose return type is `()` exits with 0 when it ends with a value, as now.
- A `main` whose return type is none of `()`, `ExitCode` and Never (such as `fn main() -> Int { 2 }`) gets the warning W0304. Its behavior is as now: the value is thrown away and the program exits with 0. Now it exits with 0 silently, so someone who wrote it expecting the `Int` to become the exit code cannot notice. Never (a body that always fails, for example) returns no value and has nothing to throw away, so it is not warned about.
- Every value from 0 to 255 can be returned, including 1, 101 and 130. These three are only the numbers the runtime uses for abnormal ends (error, defect, interruption), and are no reason to restrict the values a user returns. From the exit code alone, the caller cannot tell whether it came from an error or from `main`; it tells them apart by whether there is a line on standard error (`error: ...` and the others).
- The upper bound 255 comes not from Emela but from the 8-bit exit status of POSIX and the u8 that `exit-with-code` of WASI takes. A value outside the range means an assumption of the caller is broken, so it is a defect (`[lib.api-rules]`). It is not taken modulo 256, because then `exit_code(256)` would silently succeed.
- The constants `EXIT_SUCCESS` (`exit_code(0)`) and `EXIT_FAILURE` (`exit_code(1)`) are declared with `pub const`. A const cannot be written with a qualified name and is used through an import (next item), so its bare name enters the user's code. `SUCCESS` alone does not say what succeeded and easily clashes with the user's names, so the prefix `EXIT_` says what the constant is. Consts of std have uppercase names (`[data.const]`). This is the first `pub const` in std, but `pub` applies to `const` (`[module.pub]`), and a value of an opaque type can be made by a function call on the right-hand side (`[data.const]`).
- A const of another module cannot be written `Process.EXIT_SUCCESS`. Qualified names in expressions are only `Module.lower_name` (`[grammar.expr]`), and a const is used through an import (`import Std.Process.{EXIT_SUCCESS}`). In a prototype, `pub const SUCCESS: ExitCode = exit_code(0)` in another module was imported and returned from `main`, and `emela check` and `emela run` passed (emela 0.27.0, 2026-11-14). `Process.SUCCESS` is E0110 (the rule is the same with the name `EXIT_SUCCESS`).
- `main` returns its value after every release of the withs has run and every child of `Fiber.scope` has ended, so choosing an exit code never skips cleanup. The exit code is set after the writes to standard output and standard error have finished (the existing guarantee of `[cmd.build]`).
- `main` is an ordinary fn, so calling it from a test only returns an `ExitCode` value; the process does not end.
- `ExitCode` is not a boundary type (12.4), so an `@export` fn or an entry adapter cannot return it (E0436). HTTP services do not change.

### Typing rules

- No new typing rules. The return type of `main` may be inferred or annotated. Whether it is `ExitCode`, and whether W0304 is reported, is decided by the return type fixed after type checking.
- W0304 is a warning, so type checking passes, and `build` and `run` work. It is independent of the rule that the R of `main` is empty except for Async (`[effect.with]`): that rule looks at R, and this one only at the return type.

## Specification changes

### `[api.std.process]`

The section is created by std-process. Add these rows to its table, after the rows of that proposal, and add the paragraph and the impl line after the table:

```markdown
| `ExitCode` | `opaque type ExitCode` | The exit code of a program. Its contents are an Int from 0 to 255 |
| `exit_code` | `exit_code(code: Int) -> ExitCode` | The exit code `code`. A `code` outside 0 to 255 is a defect. Implemented in Emela |
| `exit_code_to_int` | `exit_code_to_int(code: ExitCode) -> Int` | The number of the exit code. Implemented in Emela |
| `EXIT_SUCCESS` | `const EXIT_SUCCESS: ExitCode` | `exit_code(0)` |
| `EXIT_FAILURE` | `const EXIT_FAILURE: ExitCode` | `exit_code(1)` |
```

```markdown
- A `main` whose return type is `ExitCode` chooses the exit code of the program with its value (`[dyn.error]`). Every value from 0 to 255 can be chosen, including 1, 101 and 130. `ExitCode` is a value: making one uses no effect, and no handler is needed.
- The constants are named with the prefix `EXIT_` because a const is used through an import, not as a qualified name (`[grammar.expr]`), so its name appears in user code on its own.
- The upper bound 255 comes from the exit status of POSIX processes and the argument of `exit-with-code` of WASI, which are 8 bits wide.
- impl of `ExitCode` (`[trait.derive]`, `[trait.builtin]`): `derive Eq, Show`. The Show impl gives `ExitCode(2)`.
```

### `[lib.layers]`

In the table of layers, the platform layer becomes (the list of modules follows std-process and stdout-stderr, whichever are adopted):

```markdown
| platform | Effect declarations and the host handlers, and the value types used only with them | Console, Clock, Random, Env, FileSystem, Http, Process |
```

After the table, add:

```markdown
- A module of the platform layer MAY declare value types that are independent of the host but have meaning only together with its effects (`Request` and `Response` of Http, `ExitCode` of Process). They are not listed in the value layer, which holds values that are useful on their own.
```

### `[dyn.error]`

Replace the third item with:

```markdown
- When main ends normally, the program exits with exit code 0, and the return value is discarded without being shown. The exception is a main whose return type is `ExitCode` (`[api.std.process]`): the program exits with the exit code of the value. To see a value, use `dbg`.
- The return type of main SHOULD be `()` or `ExitCode`. A main whose return type is none of `()`, `ExitCode` and Never is W0304; it still runs, and its value is discarded as above.
- The exit code chosen by an `ExitCode` MAY be 1, 101 or 130. The exit code alone then does not tell it apart from an error, a defect or an interruption; only the line written to standard error does.
```

### `[effect.with]`

Add after the item on the R of main:

```markdown
- The return type of main is `()` or `ExitCode` (`[api.std.process]`); another type, other than Never, is W0304 (`[dyn.error]`). This is separate from the rule on R above.
```

### `[diag.list]`

Add after W0303:

```markdown
| W0304 | The return type of main is none of `()`, `ExitCode` and Never (`[dyn.error]`). Points at the return type annotation of main, or at its name when the type is inferred. The help is ``= help: return `()`, or return an `ExitCode` to choose the exit code`` | ``the value of type `Int` returned by `main` is discarded`` |
```

### `[cmd.build]`

In "The exit code is as follows", replace the second item with:

```markdown
  - When `run` reaches the program, the exit code of the program: 0, or the value of the `ExitCode` that `main` returned (`[dyn.error]`), when main ends with a value; 1 for an error (14.3); 101 for a defect (7.5); 130 for an interruption (14.5).
```

In "Running the program (JS)", replace "The return value of `main` is discarded." with:

```markdown
The return value of `main` is discarded, except that when the return type of `main` is `ExitCode` the program exits with its exit code.
```

and add a row at the top of the table:

```markdown
| `main` ended with a value of `ExitCode` (`[dyn.error]`) | nothing | the exit code of the value |
```

### `[target.native.run]`

Add after the second item:

```markdown
- When `main` returns an `ExitCode` (`[dyn.error]`), the executable exits with its exit code, after standard output and standard error are flushed.
```

### `[target.wasm.output]`

Replace "an exit code other than 0 is passed with `exit-with-code`" with:

```markdown
an exit code other than 0, including one that `main` returns as an `ExitCode` (`[dyn.error]`), is passed with `exit-with-code`
```

### `[ext.export]`

No text change. `ExitCode` is not a boundary type (12.4), so an `@export` fn or an entry adapter that returns it is E0436 as before.

## Diagnostics

- [ ] W0304: a `main` whose return type is none of `()`, `ExitCode` and Never. The message is ``the value of type `Int` returned by `main` is discarded``, and the help is ``= help: return `()`, or return an `ExitCode` to choose the exit code``. It points at the annotation of the return type, or at the name `main` when the type is inferred. It is the first free number of W03xx (warnings about types).
- A value outside the range of `exit_code` is a defect at run time, and no diagnostic is added. The message is `exit code <n> is out of range 0 to 255`.

## Compatibility

- No existing program changes behavior. `ExitCode` is a new type, so no current `main` has `ExitCode` as its return type.
- An existing `main` that returns something other than `()` newly gets W0304. It is a warning, so build and run still work. No `main` in the examples or in the code examples of the specification is affected (checked with `git grep`).
- The code examples use `Std.Console`, which is in the specification. This proposal does not depend on stdout-stderr. Now that stdout-stderr has been adopted, the imports of the code examples may be rewritten to `Std.Process.{Stderr, Stdout}` when they are pasted into the specification; the meaning does not change.

## Implementation

- After type checking, the compiler looks at whether the return type of `main` of the entry is `Std.Process.ExitCode`, and changes what it passes to the launcher.
- JS: the launcher puts the value of `main` (an Int) into `process.exitCode`. As with the existing 101 and 130, the program ends after the writes have finished (the launcher in `crates/emela-driver/src/run.rs`). A `main` that suspends gives its value when `$start` completes.
- C: the generated C `main` returns the Int of the `ExitCode`. stdout and stderr are flushed before `exit`. The 99 of `EMELA_LEAK_CHECK` takes precedence, as now.
- WASM: `run` of `wasi:cli/run` can return only a `result`, so a code other than 0 is returned with `exit-with-code` of `wasi:cli/exit@0.3.0`, which is already imported.
- `ExitCode` and the two functions can be written in plain Emela in `std/process.emel` (`opaque type ExitCode = Int` and a range check). No host file is needed.

## Alternatives

### Put it in a separate module `Std.ExitCode`

- Make `Std.ExitCode` a module of the value layer, so that `ExitCode.from_int(2)` can be written. The `from_*` and `to_*` names of `[lib.api-rules]` would apply as they are.
- Rejected: like `Args`, `Stdin`, `Stdout` and `Stderr`, it belongs to the process, and gathering them in `Std.Process` leaves one place to look (the author's decision). It would also add a module only for a type used only for how the process ends.

### Add it to `Std.Exit`

- Add `ExitCode` to `Std.Exit` (`[api.std.exit]`) and use it with `import Std.Exit.{ExitCode}`.
- Rejected: `Std.Exit` is about how a computation ends (`Exit[A, E]`, `attempt`) and is used inside fibers too. The exit code of the process is a different concept, and `Exit` and `ExitCode` side by side in one module would be confusing.

### Put it in a submodule `Std.Process.ExitCode`

- It would have the form of `Std.Http.Request`, and `ExitCode.from_int` and `ExitCode.to_int` would be natural names.
- Not taken: it breaks the premise of placing it in `Std.Process` next to `Args` and `Stdin` with `exit_code` in the names. The layer is handled by one rule in `[lib.layers]`.

### List `ExitCode` in the row of the value layer

- Not taken: the value layer holds values that are useful on their own (Instant, Bytes, Map and others), and it does not list `Request` and `Response` either, which are also independent of the host. An exit code means something only as how the process ends.

### Make the return value of `main` an `Int`

- The value of `fn main() -> Int` would be the exit code, with no new type.
- Rejected: the exit code of a `main` that now returns an `Int` (its last expression happens to be an `Int`) would change silently. The specification says the return value of `main` is thrown away (`[dyn.error]`), so such programs follow it. How to handle values outside the range would also be open (JS takes them modulo 256, and `exit-with-code` of WASM takes a u8).

### An effect operation `exit(code)` that does not resume

- Add a platform effect `Process { suspend fn exit(code: Int) -> Never }` and `SystemProcess`, so that the program can end from deep inside. Tests replace the handler.
- Rejected:
  - An operation that does not resume its continuation means, now, "unwind as interrupted" (`[dyn.suspend]`). A fourth way to end, carrying an exit code, would have to be added to `[fiber.cause]`, and how `Exit.attempt`, `Fiber.await` and `Fiber.scope` treat it would all have to be decided again.
  - Called from a child of `Fiber.scope`, it is not settled whether it interrupts the siblings and ends the process or becomes the result of the scope.
  - A library could require `uses { Process }` and end the process. The library would decide what `main` should decide.
  - Ending from deep inside can be written by raising an error up to `main` and turning it into an `ExitCode` with `escape` (the example in Summary). The releases run correctly.

### Choose the exit code from an error that reaches `main`

- Implementing a trait (`trait ExitStatus { fn exit_code(self) -> Int }`) for the type of an error would make the program exit with that exit code when the error reaches `main`. It could be decided at the same place where the display now depends on whether the error satisfies `Show` (`[dyn.error]`).
- Rejected:
  - The exit code would be decided in two places, the return value of `main` and the type of the error, and the reader could not trace which one decided it. The same error type would give different exit codes depending on whether it reaches `main` (the author's decision).
  - Results other than 0 that mean success (1 for "not found" of `grep`, for example) would be written as errors.
  - The line `error: <value>` is always written, so the program could not choose its own usage message.

### Reserve 1, 101 and 130

- Make `exit_code(1)` and the others a defect, so that the exit code alone tells an error, a defect and an interruption apart.
- Rejected: the three are only numbers the runtime uses for abnormal ends. Reserving them would make conventions such as 1 for "not found" of `grep` impossible, and `EXIT_FAILURE` would have to be something other than 1. The line on standard error tells them apart.

### A function that stops the process on the spot

- Make `Process.exit(code) -> Never` an intrinsic of the runtime.
- Rejected: neither the releases of withs nor the unwinding of the children of `Fiber.scope` would run. Called inside `emela test`, it would stop the test runner and fail with "the test runner exited with code ..." (`[cmd.test.outcome]`). std would be recommending the workaround that `@external` already allows.

## Prior art

- Rust: `fn main() -> ExitCode` (`std::process::ExitCode`) and the `Termination` trait. `std::process::exit` does not run destructors.
- Go: `os.Exit` does not run deferred functions.
- Haskell: `exitWith` throws an `ExitCode` exception, and the cleanup of `bracket` runs.
- WASI 0.3: `wasi:cli/run` returns `result<_, _>`, and any other code goes through `exit-with-code` (u8) of `wasi:cli/exit`.

## Open questions

## Decision
