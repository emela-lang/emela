# 12. External functions `[ext]`

A host function is declared as a fn with `@external` and no body. One with side effects can be called only inside a handler. This way, every side effect goes through a named effect.

`@external` is a last resort. The way out to the host is the job of the effects in the platform layer of std (15.2). Libraries and applications are written on top of them in pure Emela, so that they run with the same meaning on both JS and WASM. The `@external` functions of a Pome used as a dependency can be used only when the Pome being built allows them in `[external]`, and the dependency shows them to the outside as effects ("Pomes that touch the host" in 4.7). Host features that most programs use are added to std as platform effects or as value types (Socket, Bytes, and so on). A feature that only a particular host has (Cloudflare Workers, for example) goes in a target-specific Pome.

## 12.1 Declaration `[ext.decl]`

```emel
@external(js, "console.log")
fn console_log(msg: String) -> ()

@external(js, "Math.sin")
@external(wasm, "env", "sin")
@external(c, "sin")
fn sin(x: Float) -> Float use {}

@external(js, "./http_ffi.mjs", "fetchText")
@external(wasm, "wasi:http/client", "fetch")
suspend fn fetch_text(url: String) -> String
```

- The parameter types and the return type are always written. The compiler cannot see inside the function, so it does not infer them.
- One `@external` is written per target. A fn MAY have annotations for several targets. Two annotations for the same target on the same fn is a compile error (E0139).
- The arguments of `@external` on a fn have the form `(target, "name")` or `(target, "file", "name")` only (E0138). The target is `js`, `wasm` or `c` (E0140). The strings cannot contain interpolation.
- If the first argument of `@external` on a fn is not a name, the remaining arguments of that annotation are not checked. An annotation whose arguments could not be parsed (one with a syntax error) is not checked for the form of `@external`.
- An unknown target name gives only E0140 and is not counted as a duplicate of the same target (E0139). For a duplicate, E0139 is reported on the target name of the second and later annotations.
- The strings of `@external` MAY be multiline strings.
- A fn with no annotation for the build target is a compile error only when it is reachable from the entry point (main for `emela build` and `emela run`, the test functions for `emela test`). When a handler is written in a with, the fns called from the bodies of its init and release become reachable. A fn called from an operation body becomes reachable only when that operation is called or used as a value somewhere in reachable code (effects with different type arguments are different effects, as in `[effect.decl]`: calling `put` of `Store[String]` does not make the `put` of a handler for `Store[Int]` reachable). Unreachable fns are not checked, so even though the bundled `SystemConsole` has only a JS `@external`, reading it in a WASM build is not a compile error as long as it is not used. Writing `with Platform` still builds for a target that has no annotation for Http, as long as Http is not used.
- In a library-only project (4.1), the external functions reachable from public functions are checked instead of those reachable from an entry point.
- This check runs in build, run and test. `emela check` and the LSP do not choose a target, so they do not run this check.
- An external function that is emitted without being reachable (copied out of a public fn) and has no annotation for the target is a defect when called.
- A file (`"./http_ffi.mjs"`) is referred to by a path relative to the directory of the source that contains the declaration (starting with `./` or `../`). The JS output copies the non-`.emel` files under the source root of the Pome that holds the referred file into the output directory, and reads them there by relative path (in single-file mode, only the referred files are copied). They are placed under `emela_project/` for the project, `emela_tests/` for the test root during testing (13.4), `emela_pomes/<source path>@<version>/` for dependencies, and `emela_core/` for the bundled core, in the same layout as under the source root. The output still works when the whole output directory is moved elsewhere. A file MAY read other files of the same Pome by relative path.
- For `c`, `(target, "name")` refers to a C function resolved at link time (one from libc or an OS library). `(target, "file", "name")` refers to a C source (`.c`) by a path relative to the directory of the source that contains the declaration, and the compiler compiles that file with cc together with the output and links it. The `.c` files that the bundled std refers to are bundled with the compiler together with std.
- The prototype of a C function is determined from the types of the Emela declaration by the table in 12.4. The compiler does not read C headers. The behavior when the types of the declaration and the host function do not match is unspecified.
- A call is written the same way as a call to an ordinary function.

## 12.2 Where they can be called `[ext.call]`

| Declaration | Meaning | Where it can be called |
|---|---|---|
| `use {}` written explicitly | Declares that the function is pure | Anywhere |
| `use` omitted | Has host side effects | Only inside the init, the release and the operation bodies of a handler |
| `suspend fn` | Needs to wait for completion. Requires Async | Only inside a handler |

- The compiler cannot verify that a `use {}` declaration is correct. The person who declares it is responsible for it.
- The restriction on where a function can be called applies to references, not only to calls. An external function with `use` omitted cannot be carried out of a handler as a function value either. The inside of an anonymous function inside an operation body of a handler counts as inside.
- An external function declared as `suspend fn` can be used only inside a handler, even when `use {}` is written (E0407).
- The host side effects of an external function are not put in R. The R of an external function with `use` omitted is empty, and its side effects are handled by the restriction on where it can be called.
- An external function declared as `suspend fn` corresponds to a function that returns a Promise in JS, and to an async import in WASI. The calling fiber suspends until completion.
- In JS, the runtime adds an `AbortSignal` as the last argument of an external function declared as `suspend fn`. It aborts when the waiting fiber is interrupted. When an implementation of a suspend fn operation is waiting on an external function and the caller is interrupted before resume, the runtime also unwinds the implementation and aborts. This is for stopping network requests and timers (without it, even if the result is discarded, the requests and timers stay in the host event loop and the process does not exit). A function that does not use the signal MAY ignore it. Interruption stops only waits of this form. A handler that resumes later from a callback is not stopped (discarded continuations in 14.5). The bundled handlers (including `sleep` of SystemClock) are written in this form.
- In JS, a returned value that is not a Promise is treated as a resolved value. A rejected Promise is a defect, the same as an exception thrown by the call (12.5).
- In C, an external function declared as `suspend fn` is written so that it reports completion later (12.6).

## 12.3 Host types `[ext.host-type]`

```emel
@external
type Connection
```

An `@external type` with no fields is an opaque type that represents a host object. The annotation takes no arguments. Emela can only pass its values around.

- Writing an argument list on the annotation of an `@external type` is E0141, even an empty `()`. Two `@external` without arguments on the same type give no diagnostic.
- On the `c` target, a value of an `@external type` is a host pointer (`void *`). Emela neither frees nor inspects this value. Resources are freed from the release of a handler, by calling an external function that frees them (14.6).

## 12.4 Boundary types `[ext.boundary]`

Only the following types can be used for the parameters and the return value of an external function.

- Int, Int64, Float, Bool, String, Bytes (`Std.Bytes`), `()`
- Types declared with `@external type`
- Function types made only of the types above (callbacks)

List and user-defined types cannot be passed directly. They are converted inside a handler.

- The parameters and return value of a function passed as a callback are treated the same as the parameters and return value of an external function (a String received from the host is normalized as in 12.5).
- An external function that returns `()` discards the host's return value and returns `()`.
- A Bytes received from the host is copied when it crosses the boundary, so a later change to the host's buffer does not change the Bytes, and the Bytes does not keep the rest of a larger host buffer alive. A Bytes passed to the host MUST NOT be modified by the host; if it is, the behavior is unspecified. Whether the runtime passes its own buffer or a copy is implementation-defined.

On the `c` target, the types map to C types by the following table.

| Emela type | C parameter | C return value |
|---|---|---|
| Int | `int32_t` | `int32_t` |
| Int64 | `int64_t` | `int64_t` |
| Float | `double` | `double` |
| Bool | `bool` (C99 `_Bool`) | `bool` |
| String | `const char *` (NUL-terminated UTF-8) | `char *` (NUL-terminated, allocated with `malloc`) |
| `()` | Not a parameter | `void` |
| `@external type` | `void *` | `void *` |
| Function type | None (E0427) | None (E0427) |

- A host function MUST be defined with external linkage. The generated `main.c` declares host functions with the prototypes this table determines. Parameters are in the order of the declaration, and a function with no parameters takes `(void)` (`void host(void)`).
- The generated `main.c` includes only the runtime header, so a host function whose name is the same as a declaration in a standard C header does not cause conflicting declarations.
- A String is passed to C as a pointer to its NUL-terminated UTF-8 bytes. The memory is owned by the runtime: the host MUST NOT modify or free it, and the pointer is valid only until the host function returns, even for an external function declared as `suspend fn` (not until the completion of the task, 12.6). If the C side uses the string later, it copies the string. Whether the runtime passes its own bytes or a copy is implementation-defined.
- When the string contains U+0000, it is a defect before the call (a C string cannot represent it). In this case the host function is not called, and the message is `a string passed to C contains U+0000`.
- A String received from C is copied into an Emela string, and freed with `free` right after the copy. Sequences that are not valid UTF-8 are replaced with the replacement character (U+FFFD) (the same idea as the normalization in 12.5). The replacement is one replacement character per maximal subpart of an invalid sequence. A returned NULL is a defect. The message is ``external function `f` returned NULL`` (`X` is the name of the external function).
- A C type not in the table, such as `size_t` or `int`, is converted to a type in the table by a small function written on the C side. The compiler does not guess the width of integers.
- The first version of the C target (0.21) does not support parameters and return values of function types. When the build target is `c` and an external function reachable from the entry point takes or returns a callback, it is a compile error (E0427). The form of callbacks (a pair of a function pointer and a context) and how long the C side can keep them are decided in 18.1 #38.
- The C target does not support Bytes yet. When the build target is `c` and an external function reachable from the entry, or an exported fn, takes or returns Bytes, it is a compile error (E0443). The C form (a pointer and a length) is decided together with callbacks (18.1 #38).

## 12.5 Exceptions and failures `[ext.exception]`

- An exception thrown by an external function is a defect. The message is the exception prefixed with its name (`defect: TypeError: bad`), and a value that is not an Error gives `uncaught exception: ...`.
- An exception thrown by host code outside a call of an external function (for example in a timer callback after the call returned) is also a defect, with the same message. It ends the launch as a defect (`emela run` exits with 101, and a test ends as a defect).
- `fails` cannot be written on the declaration of an external function. A result to be treated as a failure is returned as a value by the host side and turned into a fail inside a handler.
- When a string received from JS contains an unpaired surrogate, it is normalized to the replacement character at the boundary.
- A defect raised inside a callback propagates as the original defect, without being wrapped as an exception of the external function.
- C has no exceptions. A failure of a C function is returned as a value by the host side and turned into a fail inside a handler (`errno` is not mapped to a failure automatically).
- When a C function calls `abort` or is stopped by a signal, the process ends instead of a defect. The exit code is 128 + the signal number, as in Appendix B.

## 12.6 Asynchronous host functions in C `[ext.c-host]`

A C function cannot return a value that completes later, so an external function declared as `suspend fn` on the `c` target is written so that it reports completion to the runtime later. The runtime provides the following types and functions in the header `emela.h`, which it writes to the output directory. The `.c` file of an external function includes it.

```c
#include "emela.h"

static void on_timer(em_task *task, void *ctx) {
  em_complete_unit(task);
}

// @external(c, "./clock.c", "host_sleep")
// suspend fn host_sleep(ms: Int) -> ()
void host_sleep(int32_t ms, em_task *task) {
  em_after(ms, task, on_timer, NULL);
}
```

- The C function of an external function declared as `suspend fn` takes `em_task *task` after the parameters from the table in 12.4, and returns `void`. `task` represents the fiber waiting for completion.
- `em_task` is an incomplete type, and the host handles only `em_task *`. `em_callback` is the type of a function that the runtime calls back, `void (*)(em_task *task, void *ctx)`.
- Completion is reported with the function that matches the return type: `em_complete_int`, `em_complete_int64`, `em_complete_float`, `em_complete_bool`, `em_complete_string` (takes NUL-terminated UTF-8 allocated with `malloc`; the runtime copies it and then calls `free`), `em_complete_unit` and `em_complete_handle` (`@external type`).
- Invalid sequences and NULL in the value passed to `em_complete_string` are treated the same as the return value of an external function that returns String (12.4).
- When a task is completed with a completion function that does not match the return type (completing a function that returns String with `em_complete_int`, for example), the waiting fiber receives a defect (``external function `f` was completed with `em_complete_int`, which does not match its return type``). The function that matches a function returning `()` is `em_complete_unit`.
- A mistake on the host side is reported with `em_complete_defect(task, message)`. It is a defect in the waiting fiber (12.5). A result to be treated as a failure is returned as a value and turned into a fail inside a handler.
- The `message` of `em_complete_defect` is copied and used as the defect message as it is, with no prefix. A NULL `message` gives an empty message.
- A completion function is called exactly once for each `task`. It MAY be called right away inside the C function (the task completes on the spot and the fiber does not suspend), and it MAY be called later. A second call is a defect.
- When the task completes on the spot (inside the C function), the fiber does not suspend and continues with that value. Otherwise, the fiber suspends until completion and resumes on completion (12.2).
- The message of the defect for a second completion is ``the task of external function `f` was completed twice``. If the second completion happens on the spot (inside the same C function), it is a defect of that call. If it happens later (in a callback or on another thread), the launch (main or a test) ends with that defect. For a second completion after the runtime has reused the record of the task for another task, the name of the external function is not known, so the message is `the task of an external function was completed twice`.
- `em_task *` is a handle, not the address of storage the host can read. The host MUST NOT read what it points to. From the call of the external function until completion, the handle MAY be passed to any function in `emela.h`. After completion, it MAY be passed only to a completion function, and that is the defect for a second completion. The runtime recognizes handles even after it frees the task, so a second completion does not touch freed storage.
- The same handle is given to another task again only after the same record has been used for the next 2^32 tasks (2^12 tasks in an environment with 32-bit pointers). A completion that late can complete another task.
- Passing a pointer not received from the runtime to a function in `emela.h` is a host mistake. The runtime prints `emela: <function> was given <pointer>, which is not a task` to standard error and aborts.
- The completion functions can be called from any thread. The runtime wakes the event loop and resumes the waiting fiber. The other `em_` functions can be called only from the runtime's thread (inside an external function and inside a callback the runtime calls).
- Completions from other threads reach the fibers on the runtime's thread in the order they arrive. Copying a String and receiving an `@external type` value happen on the runtime's thread.
- When an `em_` function that is not a completion function is called outside the runtime's thread, the runtime prints `emela: <function> was called outside the runtime's thread` to standard error and aborts.
- As functions that register waits in the event loop, the runtime provides `em_after(ms, task, callback, ctx)` (after a number of milliseconds), and `em_on_readable(fd, task, callback, ctx)` and `em_on_writable(fd, task, callback, ctx)` (when a file descriptor is ready). The callback is called on the runtime's thread and usually calls a completion function inside it.
- A callback is called with `(task, ctx)` as arguments. A negative `ms` for `em_after` counts as 0. The deadlines and order of timers are in `[target.c.event-loop]`.
- A file descriptor registration fires only once. When the descriptor is ready, the runtime removes the registration and then calls the callback. To wait again, register again.
- Completing a task removes the timers and file descriptors registered for that task, and their callbacks are no longer called. Passing a completed task to `em_after`, `em_on_readable`, `em_on_writable` or `em_on_cancel` does nothing.
- When the waiting fiber is interrupted (11.4), the callback registered with `em_on_cancel(task, callback, ctx)` is called (notification of cancellation, 12.2). Even after the interruption, the C side calls a completion function exactly once (the value is discarded). This frees the storage of `task`.
- Besides when the waiting fiber is interrupted, the runtime cancels a task and calls its `em_on_cancel` callback when the launch has stalled and discarded continuations are unwound (`[target.c.event-loop]`), when the launch ends, and when an operation that was not resumed on the spot in a non-suspending context is unwound (`[effect.async]`, `[dyn.suspend]`).
- Calling `em_on_cancel` again on the same task replaces the previous callback. Calling `em_on_cancel` before completion on a task that has already been canceled calls the callback on the spot.
- The bundled `sleep` completes right away inside its cancellation callback. This also removes the timer.
- Registrations left after the launch ends are discarded. Tasks that have not completed are not freed (another thread can complete them later, and that completion discards its value).
- When the event loop has no registered waits, only uncompleted `task`s remain, and every fiber has stopped, the continuations are treated as continuations discarded while suspended (14.5).
- A host function that hands a task to another thread MUST keep the event loop waiting with its own registration (an `em_after` with a long deadline, for example). Tasks held by other threads alone do not keep the launch from being considered stalled (`[target.c.event-loop]`). The registration is removed on completion.
- The types and the function signatures of `emela.h` can change between versions, like any other API.

## 12.7 Exported functions `[ext.export]`

```emel
@export(c, "TIM2_IRQHandler")
pub fn on_tick() -> () { ... }

@export(js, "default", "add")
pub fn add(a: Int, b: Int) -> Int { a + b }
```

- `@export` is written on a top-level fn with a body. Writing it on any other declaration, or on a fn without a body, is E0146. Its arguments have the form `(target, "name")` or `(target, "interface", "name")`, as for `@external` (12.1). One `@export` is written per target name; a fn MAY have several for the same target. Malformed arguments are E0146, and an unknown target is E0140.
- An `@export` fn exposes itself to the host under each name. Each call by the host is one launch, as main is.
- The parameters and the return value of an `@export` fn are boundary types (12.4) (E0436). Its E MUST be empty and its R MUST be empty except for Async (E0402).
- A hidden set variable of a parameter's function type is part of the fn's E and R (9.4), so a callback parameter of an `@export` fn is written with `fails {} use {}` (`f: fn(Int) -> String fails {} use {}`). Otherwise its E and R are not empty (E0402).
- An `@export` fn that is not an entry adapter is allowed only in the entry module (E0437).
- Two exports with the same name for the same target, from `@export` or `@entry`, are a compile error (E0438).
- How each target exposes the fn is in `[target.js.module]`, `[target.c.export]` and `[target.wasm.external]`.

## 12.8 Entry adapters `[ext.entry]`

- An `@export` fn whose first parameter has a function type is an entry adapter. An entry adapter is not exposed by itself, and MAY be in any module. Its first parameter need not be a boundary type; the other parameters and the return value are boundary types (E0436).
- `@entry(T)` is written on a top-level fn `f` of the entry module. Writing it outside the entry module is E0437. Its one argument is a lower name (`serve`, a fn of the same module) or a `qualified` path (`Http.serve`) that names an entry adapter; anything else is E0439. `f` MUST be accepted as the first argument of `T` (E0440).
- For each `@export` of `T`, the program exposes under the same name a fn that takes the remaining parameters of `T` and returns `T(f, ...)`. Each call is one launch.
- `f` remains an ordinary fn. It can be called, including from tests.
