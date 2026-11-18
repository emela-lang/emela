---
title: Host effects, what the host provides to a launch
authors: [Alliana]
status: draft
pr: 617
issue:
decided:
touches: [effect.decl, effect.async, effect.host (new), effect.with, ext.call, ext.export, ext.entry, test.constraints, lib.layers, lib.http-service, api.std.platform, api.std.process, api.std.env, api.std.file-system, diag.list]
requires: []
related: []
---

# Host effects, what the host provides to a launch

## Summary

An effect whose handler exists only in the host is called a host effect, and `Async` is one of them. Two host effects are added: `ProcessHost` (the host is a process) and `PageHost` (the host is a page of a browser). An external function whose host function runs only in a process declares `uses { ProcessHost }`, and the requirement stays in R through handlers and layers, inferred as `Async` is. The entry decides which host effects are provided, so a handler for processes used in a page is the usual E0402. `Platform` becomes a layer of only the handlers that every host has, and `Std.Process` gains a layer `Process` that also includes the handlers for processes. Also, only the compiler can declare an effect without operations, and the `uses` of an external function can only be `{}` or host effects.

An image for explanation only (not for the specification): the entry decides which rootstock the program is grafted onto. The rootstock decides the environment it grows in, and the tree cannot replace it.

## Motivation

### A handler for processes breaks at run time on a host that is not a process

- `Platform` includes `SystemFileSystem` and `SystemArgs`. They work on Node, Bun and wasmtime, but not in a program that runs in a browser (`@entry(Browser.app)`, proposed in a following proposal).
- Now this is found only at run time (resolving `node:fs` fails, or a defect).

```emel
# a program in a browser that reaches a handler for processes without noticing
@entry(Browser.app)
pub fn app() -> Program[Model, Msg] {
  # compiles now, and fails at run time (Platform includes SystemFileSystem)
  with Platform {
    saved = (use FileSystem).read_text("model.json")
    Browser.program(init: decode_model(saved), update:, view:)
  }
}
```

- Splitting Platform per target (`NodePlatform`, `WasmPlatform`) lets the same program give different results per target, against `[target]`: "the same program MUST give the same observable results on every target". `[lib.layers]` also decided that "A platform handler does not expose target names".
- What should be told apart is not the target but what the host provides. That is a difference in available features, not in implementation, so it may appear in types.

### Effects without operations are not settled

- `Async` has no operations, and only the runtime provides it (`[effect.async]`). The specification writes this as a rule for `Async` alone, not as a general concept.
- The grammar lets a user declare `effect Marker {}`, but its meaning is not written.
- For the `uses` of an external function, the table of `[ext.call]` has only `uses {}` and leaving it out. The current implementation accepts `uses { Marker }` (an effect of the user), and the external function can then be called outside a handler (checked with `emela run` on 2026-11-16). Leaving `uses` out is E0407, but writing any one effect lifts that restriction.

```emel
effect Marker {}

handler GiveMarker: Marker {}

@external(js, "./x.mjs", "write")
fn write_host(text: String) -> () uses { Marker }

fn main() {
  # compiles, and a side effect of the host happens without going through a handler
  with GiveMarker { write_host("called outside a handler") }
}
```

## Proposal

### Syntax

No change. `ProcessHost` and `PageHost` are added to core as names.

### Meaning

- A host effect is an effect whose handler exists only in the host. There are three: `Async`, `ProcessHost` and `PageHost`.
  - `Async`: the host can resume later (it has an event loop).
  - `ProcessHost`: the host is a process. It has arguments, standard input and output, environment variables and a file system.
  - `PageHost`: the host is a page of a browser.
- A user cannot write a handler of a host effect (E0413), nor `use` it (E0414). It can be written in a uses clause.
- A host effect enters R only when a host function needs it.
  - Calling an `@external suspend fn` adds `Async` (as now).
  - Calling an external function that declares `uses { ProcessHost }` adds `ProcessHost`.
  - Calling them in the body of an operation of a handler puts them in requires(H) (`[effect.handler]`). So `ProcessHost` remains in the R of `with SystemFileSystem { ... }` and `with Process { ... }`.
- Only the entry provides host effects. A with cannot remove them.

| Entry | Host effects the host provides |
|---|---|
| `main` | `Async`, `ProcessHost` |
| `@test` | `Async`, `ProcessHost` |
| `@export` fn (not an entry adapter) | `Async` |
| An entry adapter | `Async`, and the host effect the adapter declares (at most one) |
| `Http.serve` (an entry adapter) | `Async`, `ProcessHost` |

- An `@export` fn gets neither `ProcessHost` nor `PageHost`. The types show that a fn called directly by the host runs on any host.
- `Http.serve` provides `ProcessHost`. An HTTP service runs as a process, never in a page, and `with Process` can be written inside it.
- A user cannot declare an effect without operations. A handler of such an effect can do nothing, and anyone can "provide" it with an empty handler (`handler H: Marker {}`). It appears in types and guarantees nothing, which is unsound. The only effects without operations are host effects.
- The `uses` of an external function can only be `{}` or host effects. An external function with `uses { ProcessHost }` can be called only inside a handler, as one with `uses` left out. `Async` is expressed by `suspend fn`, and is not written in `uses`.

### The form of std

```emel
@external(js, "./file_system.mjs", "readText")
suspend fn read_text_host(path: String) -> String uses { ProcessHost }

pub handler SystemFileSystem: FileSystem {
  suspend fn read_text(path, resume) { resume(read_text_host(path)) }
}
```

- The external functions of `SystemArgs`, `SystemStdin`, `SystemStdout`, `SystemStderr`, `SystemEnv` and `SystemFileSystem` need `ProcessHost`. `SystemConsole`, `SystemClock`, `SystemRandom` and `SystemHttp` do not.
- `Platform` includes only the handlers that every host has (`SystemConsole`, `SystemClock`, `SystemRandom`, `SystemHttp`). Its only host effect is `Async`, so it can be written in `main`, `@test`, `@export` and a page.
- `Std.Process` gains a layer `Process`. It includes `Platform` and the handlers for processes (`SystemArgs`, `SystemStdin`, `SystemStdout`, `SystemStderr`, `SystemEnv`, `SystemFileSystem`), and requires `ProcessHost`.

```emel
import Std.Process.{Process}
import Std.FileSystem.{FileSystem}

fn main() {
  with Process {
    (use FileSystem).write_text("out.txt", "hi")
  }
}
```

- A program that uses no arguments and no files keeps `with Platform`. A program that uses them is rewritten to `with Process`.

### The types users see

```emel
fn save(path: String, text: String) -> () uses { FileSystem } {
  (use FileSystem).write_text(path, text)
}
```

The type of `save` has no `ProcessHost`. A host effect stays in R only where a handler is chosen (`with`), so a library that uses `FileSystem` can be used in a page or in tests by replacing the handler.

### Typing rules

- Host effects are Immediate (`[type.effect-bound]`), except `Async` (as now).
- A fn with a uses clause has that set as its R (`[type.inference]`). A fn written `fn f() uses { ProcessHost }` can be called only in a process.

## Specification changes

### `[effect.host]` (new)

Insert before `[effect.async]`. `[effect.async]` keeps its number after it.

> ## 8.7 Host effects `[effect.host]`
>
> - A host effect is an effect whose only handler is the host. `Async` (`[effect.async]`), `ProcessHost` and `PageHost` are the host effects.
>   - `Async`: the host can resume a computation later.
>   - `ProcessHost`: the host is a process of the operating system, with arguments, standard input and output, environment variables and a file system.
>   - `PageHost`: the host is a page of a web browser.
> - Users cannot write a handler for a host effect. `handler H: ProcessHost` is a compile error (E0413), and that handler provides nothing even when written in a with. `use` of a host effect is a compile error (E0414). A host effect can be written in a uses clause (`fn f() uses { ProcessHost }`).
> - A host effect enters R only through what host functions need. Calling an `@external suspend fn` adds Async (`[effect.async]`). Calling an external function declared with `uses { ProcessHost }` or `uses { PageHost }` adds that host effect (`[ext.call]`). A handler whose init, release or operation bodies make such calls requires the host effect (requires(H) in `[effect.handler]`), so the host effect remains in the R of a with that lists the handler, or a layer that contains it.
> - A with does not remove a host effect. Only the entry of a launch provides host effects.
>
> | Entry | Host effects provided |
> |---|---|
> | `main` (`[effect.with]`) | Async, ProcessHost |
> | A test function (`[test.constraints]`) | Async, ProcessHost |
> | An `@export` fn that is not an entry adapter (`[ext.export]`) | Async |
> | An entry adapter (`[ext.entry]`) | Async, and the host effect the adapter declares |
>
> - `ProcessHost` and `PageHost` are Immediate (`[type.effect-bound]`).
> - Host effects do not name targets. A program whose entry type checks runs on every target that supports that entry (`[target]`).

### `[effect.decl]`

Add:

> - An effect declared in source MUST have at least one operation (E0xxx). The effects without operations are the host effects, which only the compiler declares (`[effect.host]`).

### `[effect.async]`

Replace the first two rules with:

> - Async is a host effect (`[effect.host]`) that represents "being resumed later". The rules of `[effect.host]` apply: only the host provides it, `handler H: Async` is a compile error (E0413), and `use Async` is a compile error (E0414).

The other rules of the section are unchanged.

### `[effect.with]`

Replace:

> The R of main MUST be empty except for Async.

with:

> The R of main MUST be empty except for the host effects that main is given (Async and ProcessHost, `[effect.host]`).

### `[ext.call]`

Replace the table with:

> | Declaration | Meaning | Where it can be called |
> |---|---|---|
> | `uses {}` written explicitly | Declares that the function is pure | Anywhere |
> | `uses` omitted | Has host side effects | Only inside the init, the release and the operation bodies of a handler |
> | `uses { ProcessHost }` or `uses { PageHost }` | Has host side effects, and works only on that host (`[effect.host]`) | Only inside the init, the release and the operation bodies of a handler |
> | `suspend fn` | Needs to wait for completion. Requires Async | Only inside a handler |

Add:

> - The uses clause of an external function contains only ProcessHost and PageHost, or nothing. Any other effect, including Async and a set type parameter, is a compile error (E0xxx). Suspension is declared with `suspend fn`, not with Async.
> - The host effects in the uses clause are added to the R of each call.

### `[ext.export]`

Replace "Its E MUST be empty and its R MUST be empty except for Async (E0402)" with:

> - Its E MUST be empty and its R MUST be empty except for Async (E0402). An exported fn is called by a host of a kind the program does not know, so it is given no other host effect (`[effect.host]`).

### `[ext.entry]`

Add:

> - The function type of the first parameter of an entry adapter MAY have one host effect other than Async in its uses (`[effect.host]`), and the R of the adapter MAY contain it. The host provides it for each launch through the adapter.

### `[test.constraints]`

Replace "The R of a test function MUST be empty except for Async. The test runner provides Async." with:

> - The R of a test function MUST be empty except for Async and ProcessHost. The test runner provides both, because tests run in a process (`[effect.host]`).

### `[lib.layers]`

Add:

> - The external functions of `SystemArgs`, `SystemStdin`, `SystemStdout`, `SystemStderr`, `SystemEnv` and `SystemFileSystem` are declared with `uses { ProcessHost }`, so these handlers require ProcessHost (`[effect.host]`). `SystemConsole`, `SystemClock`, `SystemRandom` and `SystemHttp` require no host effect other than Async.

Replace "The layer `Platform`, which bundles all System handlers, is placed in `Std.Platform`. main is usually written `with Platform { ... }`" with:

> - The layer `Platform`, placed in `Std.Platform`, bundles the System handlers that every host has: SystemConsole, SystemClock, SystemRandom and SystemHttp. It requires no host effect other than Async, so it can be written in main, a test, an exported fn and a page. The layer `Process`, placed in `Std.Process`, bundles Platform and the System handlers that need a process: SystemArgs, SystemStdin, SystemStdout, SystemStderr, SystemEnv and SystemFileSystem. It requires ProcessHost. main is usually written `with Platform { ... }`, or `with Process { ... }` when it uses arguments, the standard streams, environment variables or files.

### `[lib.http-service]`

Add:

> - `Http.serve` provides ProcessHost (`[effect.host]`). An HTTP service runs in a process, so `with Process { ... }` can be written in the fn.

### `[api.std.platform]`

Replace the row of `Platform` with:

> | `Platform` | `layer Platform { SystemConsole, SystemClock, SystemRandom, SystemHttp }` | The System handlers that every host has. Requires no host effect other than Async. To replace some of them, list the replacements after it, as in `with Platform, TestClock { ... }` |

Replace "SystemStdin, SystemClock, SystemHttp, and SystemFileSystem require Async" with "SystemClock and SystemHttp require Async".

### `[api.std.process]`

Add a row:

> | `Process` | `layer Process { Platform, SystemArgs, SystemStdin, SystemStdout, SystemStderr, SystemEnv, SystemFileSystem }` | Platform and the System handlers that need a process. Requires Async and ProcessHost (`[effect.host]`) |

Replace `with Platform` in the example of the section with `with Process`, and its import with `import Std.Process.{Process, Stdout}`.

### `[api.std.process]`, `[api.std.env]`, `[api.std.file-system]`

Each section adds one line: "The System handler requires `ProcessHost` (`[effect.host]`)."

### `[api.std.console]`

Add:

> - On a page host, SystemConsole writes with `console.log` and `console.error`, as on any JS host without standard output.

### `[diag.list]`

- E0413: ``handler `H` cannot implement `ProcessHost`: only the host provides it`` (the message for Async becomes the same form).
- E0414: `` `ProcessHost` is a host effect and cannot be used with `use` `` (the message for Async becomes the same form).
- E0402: when the missing effect is a host effect, add a note that names the entry and what it provides (``= note: `Browser.app` provides `Async` and `PageHost`, not `ProcessHost` ``). When the missing effect is one that `Process` provides and the with lists `Platform`, add ``= help: `Platform` has only the handlers every host has; use `Process` for the ones that need a process``.
- E0xxx (new): ``the uses of external function `f` can contain only `ProcessHost` and `PageHost` ``, at the effect in the uses clause.
- E0xxx (new): ``effect `E` has no operations``, at the name of the effect. Help: ``an effect needs at least one operation; only host effects have none``.

## Diagnostics

- [ ] The texts of E0413 and E0414 in the general form for host effects
- [ ] A note for host effects on E0402
- [ ] An effect other than a host effect in the uses of an external function: code to be assigned
- [ ] An effect without operations declared: code to be assigned

## Compatibility

- `with Platform { ... }` in `main` and `@test` still compiles for a program that uses only `Console`, `Clock`, `Random` and `Http`.
- A program that uses `Args`, `Stdin`, `Stdout`, `Stderr`, `Env` or `FileSystem` through `Platform` becomes E0402, and is rewritten to `with Process`. 21 files in the repository write `with Platform` (2026-11-17). The help of E0402 shows `with Process`.
- A program that uses `with Platform` or a handler for processes inside an `@export` fn no longer compiles.
- A program that writes `uses { an effect of the user }` on an external function no longer compiles. That form is not in the table of the specification now.
- `with Platform` inside the fn of `@entry(Http.serve)` still compiles. To use a handler for processes, it is rewritten to `with Process`.
- A program that declares an effect without operations no longer compiles. None was found in the repository (2026-11-16).

## Implementation

- `emela-types`: host effects (generalizing the checks for `Async` alone), the host effects each entry provides, the check of the uses of external functions
- `emela-core`: `uses { ProcessHost }` on the external functions for processes; `Platform` rebuilt and `Process` added to `Std.Process`; the examples and tests that use handlers for processes through `with Platform` rewritten to `with Process`
- `emela-lsp`: the note of E0402

## Alternatives

- A Platform per target (`NodePlatform`, `BunPlatform`, `WasmPlatform`): the same program could give different results per target, against `[target]`.
- Browsers as a separate target, stopped by E0412: one more target, with two nearly identical outputs and runtimes. E0412 does not run in `emela check` and the language server (`[ext.decl]`), so editors would not show it.
- The host a Pome runs on, declared per Pome: too coarse. One `with Platform` in `main` would make the whole package process-only.
- Another clause on function types, such as `needs { }`: closure types and inference would be doubled. R is already the set of what the caller provides, and what the host provides is one of them.
- Allowing `use ProcessHost` and writing it in the init of a handler: the same can be done now with an effect of the user. But `use` of an effect without operations is an unnatural use of effects, and disagrees with E0414 for `Async`. Which host function needs a process is a property of the external function, so it is written there.
- `ProcessPlatform`, `System` or `Os` as the name of the layer for processes: `ProcessPlatform` is long and looks like a variant of `Platform`; `System` is confusable with the prefix of handlers; `Os` does not tell what it includes.
- Keeping `Platform` with everything and adding another layer of the handlers every host has: existing programs do not break, but `Platform` becomes process-only, a name that cannot be used in `@export` or a page.
- Runtime effect, marker effect or context as the name: runtime is confusable with the runtime library; marker describes only the form; context is used by `Context[T]` (the draft `context-and-given`) and by "non-suspending context" in the glossary.

## Prior art

- Abilities that the environment provides, expressed as built-in effects in the effect row: built-in effects without user handlers in Koka (`div`, `ndet` and others).
- Coeffects, which track in types what a computation requires of its context.

## Open questions

## Decision
