# 16.7 The C target `[target.c]`

The C target maps a program to C11 source and makes a native executable with a C compiler (cc). Passing `--target c` to `emela build`, `emela run` or `emela test` builds for this target.

- The meaning of numbers and strings follows `[target.number]` and `[target.string]`. `+`, `-`, `*` and unary minus on Int and Int64 wrap around in two's complement, and `-2147483648 / -1` also wraps around (the remainder is 0).
- Built-in functions that need the grapheme cluster tables are not available yet (`[target.c.unsupported]`).
- External functions are declared with `@external(c, ...)`, and host functions are written in the form of `[ext.c-host]`.

## 16.7.1 Output `[target.c.output]`

- The output directory is `target/emela/c` for `emela build` and `emela run`, and `target/emela/test-c` for `emela test` (both under the directory of the project or of the single file, `[module.name]`). The test output does not overwrite the output of a normal build.
- The following files are written to the output directory.

| File | Contents |
|---|---|
| `main.c` | The program |
| `emela_runtime.c`, `emela_runtime.h` | The runtime |
| `emela.h` | The public header for the host's C sources (`[ext.c-host]`) |
| `emela_core/<path>` | Bundled C sources that the external functions used by the program refer to. `<path>` is the path inside the bundled library (`std/x.c`). Only the ones used are written |
| `main` | The executable that cc makes |

- The project's C sources (the ones `@external(c, "./file.c", ...)` refers to) are not copied to the output directory, and are compiled by their absolute paths.
- If the entry point has no main, the executable does nothing and exits with exit code 0.

## 16.7.2 Invoking the C compiler `[target.c.cc]`

- The C compiler is the environment variable `EMELA_CC` if it is not only whitespace, and otherwise `cc` on PATH. `EMELA_CC` is split at whitespace: the first word is the program and the rest are flags (`clang -fsanitize=address`).
- The compiler is invoked with the output directory as the current directory, in the following form.

  ```
  <cc> <flags from EMELA_CC> -std=c11 -O2 -fno-strict-aliasing -pthread -I. -o main main.c emela_runtime.c <bundled .c> <project .c> -lm
  ```

  `-pthread` is for completions from other threads (`[ext.c-host]`), and `-I.` is so that the host's C sources can include `emela.h` in the output directory. The C compiler MUST accept C11.
- If the C compiler cannot be started, it is E0907 (``cannot start `cc`: <reason>``). When it is not found, the note ``install a C11 compiler and add it to PATH as `cc`, or set EMELA_CC to the compiler`` is attached.
- If the C compiler fails, it is E0908 (``` `cc` failed to compile `<output directory>/main.c` ```). The standard error of the C compiler is attached as a note. Anything beyond 4000 bytes is cut off, and a `...` line is added.
- On success, the output of the C compiler is not shown.

## 16.7.3 Running `[target.c.run]`

- `emela run --target c` starts `main` in the output directory. It passes the arguments after `--`, and the program inherits standard input. The exit code is as in `[cmd.build]`.
- An error, a defect or an interruption that reaches the entry point is printed to standard error as one line, `error: <value>`, `defect: <message>` or `interrupted` respectively, and the program exits with exit code 1, 101 or 130 (`[dyn.error]`, `[error.defect]`, `[dyn.suspend]`).
- `emela test --target c` runs the selected test functions in order, in one process of one executable. State (the values of evaluated consts, for example) carries over between tests. The output of results and the exit code are as in `[test.command]`, and the result of each test is one of success, error, defect or interruption.
- If the environment variable `EMELA_LEAK_CHECK` is not empty, the executable checks at exit whether any allocations have not been freed yet. If there are, it prints `emela: leak check: <count> allocations are still live` to standard error and exits with exit code 99.

## 16.7.4 Constructs not supported yet `[target.c.unsupported]`

Only the functions reachable from the entry point are checked. Unreachable ones are not a compile error even if they contain unsupported constructs.

- An external function that takes or returns a callback is E0427. The message is ``external function `f` takes a callback, which target `c` does not support yet`` (`returns a callback` when it returns one).
- The following built-in functions, which need the grapheme cluster tables, are a compile error with no code. The message has the form `` `String.length` needs the Unicode grapheme cluster tables, which target `c` does not support yet ``.
  - `String.length`
  - `String.chars`
  - `String.split` whose separator is a constant empty string (`` `String.split` with an empty separator ``)
- These errors have no location.
- A `String.split` whose separator turns out to be the empty string at run time splits correctly at each grapheme cluster if the string is ASCII only (in ASCII, the sequence CR LF is one grapheme cluster, and every other character is one each). If the string contains a non-ASCII character, it is a defect (`` `String.split` with an empty separator on non-ASCII text needs the Unicode grapheme cluster tables, which target `c` does not support yet ``). This defect can be caught with `Fiber.attempt`, and the releases of the withs on the way also run.
- An external function without `@external(c, ...)` is E0412 if it is reachable from the entry point. One that is emitted without being reachable is a defect (``external function `f` is not available on this target``) when called (`[ext.decl]`).
- HTTP services (`[lib.http-service]`). `@entry(Http.serve)` is a compile error when the target is `c` (E0442).

## 16.7.5 Event loop `[target.c.event-loop]`

The completions of external functions declared as `suspend fn` (`[ext.c-host]`) are delivered by the runtime's event loop.

- The event loop runs when no fiber can run. While fibers keep yielding, the runtime also inserts non-blocking iterations from time to time, and calls the callbacks that are ready. How often is implementation-defined.
- One iteration waits for one of: a file descriptor registered with `em_on_readable` or `em_on_writable` becoming ready, the earliest timer deadline, or a completion from another thread.
- Time is in milliseconds of a monotonic clock. It is fixed to the value read at the start of each iteration of the event loop, and the timer checks and the deadlines of `em_after` (the fixed time + `ms`) within the same iteration all use this value.
- Timers are called in order of deadline, and in order of registration for the same deadline. A timer registered inside a callback is called in the next iteration, even if its deadline has passed.
- Completions from other threads are delivered to fibers in the order they arrive.
- If there is no fiber that can run, no work waiting for a non-blocking iteration, no registration (timers and file descriptors), and no completion not yet delivered, the launch has stalled. In this case, if there are fibers sleeping on TestClock, the one with the earliest time is woken, as in `[effect.state]`. Otherwise, as in `[dyn.suspend]`, all fibers are interrupted, the waiting continuations are unwound, and the waiting tasks are canceled (`[ext.c-host]`). If the unwinding adds no new work and no registration, the launch ends.
- Tasks held by other threads are not by themselves a reason for the launch not to stall. To keep it waiting, the host holds its own registration (`[ext.c-host]`).

## 16.7.6 Exported functions `[target.c.export]`

- An `@export(c, "name")` is emitted in `main.c` as a function with external linkage named `name`, with the prototype given by the table of 12.4. The three-argument form is E0146 on the `c` target.
- A parameter or return value of a function type is not supported yet on the `c` target. It is a compile error with no code: ``exported function `f` takes a callback, which target `c` does not support yet`` (the form of callbacks is open, 18.1 #38).
- A String argument is copied into an Emela string, and sequences that are not valid UTF-8 are replaced as for the return value of an external function (12.4). A NULL argument is a defect of the launch, with the message ``exported function `f` was given NULL``. A String return value is passed to the host as NUL-terminated UTF-8 allocated with `malloc`, which the host frees. A return value that contains U+0000 is a defect of the launch, with the message `a string passed to C contains U+0000`.
- When the launch of an exported fn ends with a defect or an interruption, the process ends as it does when main ends with one (`defect: <message>` and exit code 101, or `interrupted` and exit code 130, 14.5). The exported function does not return to the host.
- An exported fn with Async in its R is a compile error on the `c` target (E0441).
- Calling an exported fn while a launch is running on the same thread is a defect, with the message ``exported function `f` was called while Emela code is running``. The process ends as for a defect of an exported fn above. This covers a call back from a host function that Emela called, and a call from an interrupt or signal handler. The runtime checks it on entry to the export.
- Calling an exported fn from another thread is unspecified.
- When an exported fn is called while no launch has started, the runtime is initialized by that call. The host does not call an initialization function.
