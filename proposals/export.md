---
title: Exported functions and entries
authors: [Alliana]
status: implemented
pr: 427
issue:
decided: 2026-10-24
touches: [glossary.terms, kw.annotation, grammar.decl, ext, target.js.module, target.js.start, target.wasm.external, target.c, effect.with, module.pome, cmd.build, diag.list]
requires: []
related: []
---

# Exported functions and entries

## Summary

`@export` declares a function that the host calls. It is the reverse of `@external`: it is written on a fn with a body and gives, for each target, the name the host sees. Its parameters and return value are boundary types.

An `@export` fn whose first parameter is a function is an entry adapter, and is not exported by itself. Writing `@entry(adapter)` on a fn of the entry module creates an export that passes that fn to the entry adapter as its first argument. Entries at the everyday level (HTTP services and others) are made by pointing `@entry` at an entry adapter written at the foundation level.

## Motivation

- The only entries today are `main` (a command) and `@test` (a test). We want more entries that the host calls: `fetch` and `scheduled` of Workers, `handle` of `wasi:http`, interrupt handlers on embedded systems.
- On JS, the pub fns of the entry are exported implicitly (`[target.js.module]`), but the program cannot choose the name the host expects (`fetch` of the default export, for example). C and WASM have no way in other than `main`.
- Most entries want types richer than boundary types (`Std.Http.Request` and others). If the person writing the application converts to and from boundary types, host names and boundary types appear at the everyday level, which `[intro.layers]` calls a design flaw. `x/platform-cloudflare` is written that way today.
- Building the conversion into the compiler for each kind of entry would mean changing the compiler for every new kind. The conversion should be written in Emela, and the compiler should only know how to make an export by passing a fn to an entry adapter.
- Entries could be expressed as effects (the host calls operations, and the program answers with a handler), but that makes people write handlers at the everyday level.

## Proposal

### `@export`

```emel
# C: an interrupt handler
@export(c, "TIM2_IRQHandler")
pub fn on_tick() -> () { ... }

# JS: a named export, and a property of the default export
@export(js, "add")
@export(js, "default", "add")
pub fn add(a: Int, b: Int) -> Int { a + b }
```

- Written on a top-level fn with a body. Its arguments have the same forms as for `@external`: `(target, "name")` or `(target, "interface", "name")`.
- The parameters and the return value are boundary types (`[ext.boundary]`). E is empty, and R is empty except for Async.
- Each call by the host is one launch.
- Allowed only in the entry module (except for entry adapters, below).
- JS: `(js, "name")` is a named export of `main.mjs`, and `(js, "default", "name")` is a property of the default export, an object. As in `@external(js, "file", "name")`, the middle argument is the container and the last is the name. On JS the middle argument can only be `"default"`. A single string such as `"default.fetch"` is not used, because a JS export name can be any string, including one with `.`. A fn that may suspend is exported as a function that returns a Promise (the reverse of an `@external` suspend fn being a JS function that returns a Promise). A defect rejects the Promise (or throws, for a fn that does not suspend).
- C: emitted in `main.c` as a function with external linkage. Its prototype is given by the table of `[ext.boundary]`. Async in R is a compile error (the form of an export that suspends in C is not decided).
- C: calling an export from the same thread while a launch is running is a defect. This includes a call back from a C function that Emela called, and a call from an interrupt or signal handler. The runtime breaks if it is interrupted in the middle of an allocation or a fiber switch. The runtime keeps a mark that a launch is running and checks it on entry to the export.
- C: a call from another thread is unspecified (parallel execution is still open, `[open.list]` #13).
- C: when an export is called while `main` is not running, the first call initializes the runtime. No initialization function is exposed to the host.
- An interrupt is handled by setting a flag or pushing to a queue in a C interrupt handler, which Emela reads with `@external`. This is the same form as on JS, where a callback signals a fiber with a Deferred.
- WASM: an `@export(wasm, ...)` written by a user is not supported yet, and is an error like `@external(wasm, ...)` (to be decided together with #56).

### Entry adapters and `@entry`

```emel
# Foundation level (std or a Pome)
@export(js, "default", "fetch")
@export(wasm, "wasi:http/handler@0.3.0", "handle")
pub fn serve[E](app: fn(Request) -> Response fails E use { Async }, incoming: Incoming) -> Outgoing use { Async } {
  ...
}

# Everyday level (the entry module)
@entry(Http.serve)
pub fn app(request: Request) -> Response { ... }
```

- An `@export` fn whose first parameter has a function type is called an entry adapter. An entry adapter is not exported by itself and can be in any module. To point `@entry` at an entry adapter of a dependency, that Pome is listed in `[external] allow` (as for `@external`). The first parameter need not be a boundary type; the other parameters and the return value are boundary types.
- `@entry(path)` is written on a top-level fn of the entry module. The path names an entry adapter (`Http.serve`, `Cloudflare.Worker.scheduled`).
- For a fn `f` with `@entry(T)`, for each `@export` of `T`, the program behaves as if `fn(rest...) -> R { T(f, rest...) }` were exported under that name. `f` MUST be accepted as the first argument of `T`.
- `f` itself stays an ordinary fn, and tests can call it directly.
- Two or more exports with the same host name for the same target are a compile error.

## Specification changes

### `[glossary.terms]`

Add the term "entry adapter", with the Japanese term used in discussions. The row is added to `spec/glossary.md` directly when the specification changes are applied, because an adopted proposal is in English.

### `[kw.annotation]`

Add two rows to the table of attributes:

> | `@export(...)` | Top-level fn with a body | Exposes the fn to the host under a name, for each target (`[ext.export]`). An `@export` fn whose first parameter has a function type is an entry adapter and is not exposed by itself |
> | `@entry(...)` | Top-level fn of the entry module | Exposes the fn through an entry adapter (`[ext.entry]`) |

### `[grammar.decl]`

```ebnf
annot_arg    = lower_name | literal | lower_name ":" literal | qualified ;
qualified    = type_name { "." type_name } "." lower_name ;
```

`qualified` is allowed only as the argument of `@entry`.

### `[ext.export]` (new, after `[ext.c-host]`)

> ## 12.7 Exported functions `[ext.export]`
>
> ```emel
> @export(c, "TIM2_IRQHandler")
> pub fn on_tick() -> () { ... }
>
> @export(js, "default", "add")
> pub fn add(a: Int, b: Int) -> Int { a + b }
> ```
>
> - `@export` is written on a top-level fn with a body. Its arguments have the form `(target, "name")` or `(target, "interface", "name")`, as for `@external` (12.1). One `@export` is written per target name; a fn MAY have several for the same target. Malformed arguments are E0146.
> - An `@export` fn exposes itself to the host under each name. Each call by the host is one launch, as main is.
> - The parameters and the return value of an `@export` fn are boundary types (12.4) (E0436). Its E MUST be empty and its R MUST be empty except for Async (E0402).
> - An `@export` fn that is not an entry adapter is allowed only in the entry module (E0437).
> - Two exports with the same name for the same target, from `@export` or `@entry`, are a compile error (E0438).
> - How each target exposes the fn is in `[target.js.module]`, `[target.c.export]` and `[target.wasm.external]`.

### `[ext.entry]` (new)

> ## 12.8 Entry adapters `[ext.entry]`
>
> - An `@export` fn whose first parameter has a function type is an entry adapter. An entry adapter is not exposed by itself, and MAY be in any module. Its first parameter need not be a boundary type; the other parameters and the return value are boundary types (E0436).
> - `@entry(T)` is written on a top-level fn `f` of the entry module. `T` names an entry adapter (E0439). `f` MUST be accepted as the first argument of `T` (E0440).
> - For each `@export` of `T`, the program exposes under the same name a fn that takes the remaining parameters of `T` and returns `T(f, ...)`. Each call is one launch.
> - `f` remains an ordinary fn. It can be called, including from tests.

### `[target.js.module]`

Replace the items "The `pub` fns of the entry module and `main` are exported ...", "A `pub` fn with type parameters is exported ...", "A module that exports at least one suspending fn also exports `$start` ..." and "How to call an exported fn whose R has effects other than Async is unspecified" with:

> - Only the fns with `@export(js, ...)`, and the exports made by `@entry`, are exports of `main.mjs` for the host. A `pub` fn without `@export` is not exported for the host; `pub` means only that other modules can see it (`[module.pub]`).
> - `main.mjs` also has exports that the runtime's launcher and other files the compiler writes use (`main`, `$start`). Their names and forms are not specified for other uses.
> - An `@export(js, "name")` exposes the fn as the named export `name` of `main.mjs`. `@export(js, "default", "name")` makes it the property `name` of the default export of `main.mjs`, an object. On the `js` target, the middle argument of the three-argument form MUST be `"default"` (E0146).
> - An exported fn that may suspend is a function that returns a Promise. The Promise resolves to the return value, and rejects with the defect if the launch ends with a defect or an interruption. A fn that does not suspend returns its value and throws on a defect.

### `[target.js.start]`

Replace "When the host calls an exported suspending fn, it returns a generator" with "When the launcher calls a suspending `main`, it returns a generator". The section describes how the runtime's launcher starts `main`; an `@export` fn returns a Promise instead (`[target.js.module]`).

### `[cmd.build]`

Replace "The `pub` fns of the entry are exported from `main.mjs`. A fn with a type parameter that has a bound cannot be exported (E0327), because which impl to use is not determined. Make a fn that wraps it with concrete types `pub`." with "The fns with `@export(js, ...)` and the exports made by `@entry` are exported from `main.mjs` (`[target.js.module]`)."

### `[target.c]`

Add a section `[target.c.export]`:

> - An `@export(c, "name")` is emitted in `main.c` as a function with external linkage named `name`, with the prototype given by the table of 12.4.
> - An exported fn with Async in its R is a compile error on the `c` target (E0441).
> - Calling an exported fn while a launch is running on the same thread is a defect, with the message ``exported function `f` was called while Emela code is running``. This covers a call back from a host function that Emela called, and a call from an interrupt or signal handler. The runtime checks it on entry to the export.
> - Calling an exported fn from another thread is unspecified.
> - When an exported fn is called while no launch has started, the runtime is initialized by that call. The host does not call an initialization function.

### `[target.wasm.external]`

Add: "An `@export(wasm, ...)` written by a user is a compile error with no code: `` exported function `f` has a `wasm` export, which the WASI 0.3 component target does not support yet ``. The entry adapters of the bundled `Std` are exposed by the runtime."

### `[module.pome]`

In "Pomes that touch the host": `@export` counts as `@external` for `[external] allow`. An `@entry` that names an entry adapter of a Pome that is not allowed is E0233.

### `[effect.with]`

After the item on the R of main: "The same holds for an `@export` fn (`[ext.export]`)."

### `[diag.list]`

> | E0146 | Malformed arguments of `@export` (12.7) | ``expected `@export(target, "name")` `` |
> | E0436 | A parameter or the return value of an `@export` fn is not a boundary type (12.7, 12.8) | ``exported function `f` cannot take `List[Int]` `` |
> | E0437 | An `@export` fn that is not an entry adapter outside the entry module (12.7) | ```@export` is only allowed in the entry module`` |
> | E0438 | Two exports with the same name for the same target (12.7) | ```fetch` is exported twice for target `js` `` |
> | E0439 | The argument of `@entry` is not an entry adapter (12.8) | ```Http.get` is not an entry adapter`` |
> | E0440 | The fn cannot be passed as the first argument of the entry adapter (12.8) | ``app` does not fit `Http.serve` `` |
> | E0441 | An exported fn that may suspend on the `c` target (16.7) | ``exported function `f` may suspend, which target `c` does not support`` |

E0402: add "an `@export` fn" to the list of contexts.

E0327: mark as unused ("(Unused, because only `@export` fns are exported, and they take only boundary types)"), as E0428 is.

## Diagnostics

- [x] E0146: malformed arguments of `@export`
- [x] E0436: a parameter or return value that is not a boundary type
- [x] E0437: `@export` outside the entry module
- [x] E0438: two exports with the same name
- [x] E0439: the argument of `@entry` is not an entry adapter
- [x] E0440: the fn does not fit the first parameter of the entry adapter
- [x] E0441: an export that may suspend on C
- [x] E0402: the R of an `@export` fn
- [x] `@export(wasm, ...)` written by a user (no code)

## Compatibility

The implicit export on JS (exporting every pub fn of the entry) is removed. JS code that calls a pub fn of the entry breaks. Adding `@export(js, "name")` fixes it; if the parameters or the return value are not boundary types, a fn that wraps it with boundary types is needed.

`pub fn fetch` and `worker/entry.mjs` of `x/platform-cloudflare` depend on the implicit export. They move to `@entry(Http.serve)` (proposal http-service) and are rewritten.

E0327 (exporting a fn with a bounded type parameter) is no longer used. `@export` takes only boundary types, so a fn with type parameters is E0434.

## Implementation

- emela-syntax: `@export`, `@entry`, and the `qualified` argument
- emela-types: boundary types, E and R, matching `@entry` against the entry adapter
- emela-codegen-js: named exports and the default export object, and the wrapper that turns an export that suspends into a Promise
- emela-codegen-c: functions with external linkage, the running mark, and lazy initialization of the runtime
- emela-codegen-wasm: only the entry adapters of std (proposal http-service)

## Alternatives

- Keeping the implicit export on JS: there would be two ways to call an export (a generator driven by `$start`, and a Promise), and JS code would have to know which one each export uses. Values in their internal representation (List, records) would be visible to JS, so changing the representation would break JS code. `pub` would also mean "visible to the host" in the entry only. The rule that the R of an exported fn is only Async (a local draft, exported-fn-effects) is part of the rules of `@export`, so that draft is no longer needed.
- Writing interrupt handlers themselves in Emela: an interrupt in the middle of an allocation or a fiber switch breaks the runtime. It would need a separate sublanguage that neither allocates nor suspends, which is beyond this proposal.
- A C output that is a library without `main` (`.a` or `.o`): left to a separate proposal.
- Marking each entry by a name (`handle`): every new kind of entry reserves another name, and the form tends to allow only one entry of a kind per program.
- Choosing the kind by the parameter types of `main` (`main(request: Request)`): only one entry per program. `fetch` and `scheduled` of Workers could not sit side by side.
- Entries as values (`pub const FETCH = Http.service(app)`, with `@entry` on a type): what is exported is not a fn, so it needs a mechanism separate from `@export`.
- Using the path of the entry adapter as the attribute name (`@Http.serve`): users would be able to define attributes, giving up the rule of `[kw.annotation]` that only attributes built into the compiler exist.
- Entries as effects (the host calls operations and the program answers with a handler): people would write handlers and `resume` at the everyday level.

## Prior art

- `@external` (12.1). `@export` is its reverse.
- `@test`, an attribute known to the compiler that marks an entry.

## Open questions

## Decision

Entries other than `main` are needed for HTTP services, Workers, `wasi:http`, and embedded hosts, and they should not require a compiler change for each new kind.

- `@export` is the reverse of `@external`. The host boundary stays below the effect system, as it already does for `@external`, so entries do not have to be effects and nobody writes handlers at the everyday level.
- Entry adapters keep host names and boundary types inside std and Pomes. Application code only writes `@entry(Http.serve)` on an ordinary fn, which tests can still call directly.
- `@entry` is a single attribute built into the compiler, so attributes stay a closed list (`[kw.annotation]`). Users do not define attributes.
- Dropping the implicit JS export leaves one way to export (boundary types, Promises), and `pub` goes back to meaning only visibility between modules.
- On C, calling an export while Emela code is running is a defect. Interrupt handlers stay in C and signal Emela through `@external`, rather than adding a sublanguage that neither allocates nor suspends.
