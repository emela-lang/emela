---
title: Host effects declared by Pomes
authors: [somnicattus]
status: draft
pr: 726
issue:
decided:
touches: [effect.host, effect.decl, ext.call, ext.entry, module.pome, grammar.decl, diag.list]
requires: []
related: [host-bindings, host-effects]
---

# Host effects declared by Pomes

## Summary

A Pome can declare a host effect: an effect without operations that only the entry of a launch provides (`pub host effect CloudflareWorker`). External functions that work only on that kind of host write it in their uses clause, as `ProcessHost` and `PageHost` are written now, and the requirement stays in R through handlers and layers up to the entry. An entry adapter declares which host effect it provides. The kinds of host are then not limited to the two that the compiler knows.

```emel
# github.com/emela-lang/x/platform-cloudflare
pub host effect CloudflareWorker

@external(js, "./kv.mjs", "get")
suspend fn kv_get(binding: String, key: String) -> String uses { CloudflareWorker }
```

## Motivation

### The specification's own platform Pome cannot say where it runs

`host-effects` (#617) made "a handler for processes used in a page" a compile error, because what the host provides may appear in types. The kinds of host are fixed by the compiler: `ProcessHost` and `PageHost`. Only the compiler can declare an effect without operations (E0446), and the uses clause of an external function can contain only those two (E0445).

`[module.pome]` uses `platform-cloudflare` as its example of a Pome that touches the host: a `handler WorkerKv` with `@external`, for a Cloudflare Worker. The external functions of `WorkerKv` work only in a Worker, and there is nothing they can write:

- With `uses` omitted, `WorkerKv` requires only Async, so `with WorkerKv { ... }` type checks in `main` and in tests, and fails at run time (the host functions it binds do not exist in a process).
- `uses { ProcessHost }` or `uses { PageHost }` would be false: a Worker is neither.

This is the problem #617 solved for processes and pages, left open for every host that the compiler does not list: a Web Worker (no page), the `wasi:http/proxy` world (no arguments, no standard streams), a serverless runtime, an embedding. A platform Pome written by a third party cannot add its host without a change of the language.

### What should stay in types

The kinds of host are what a user of a library must arrange: which entry the program is launched from. They belong in types, as #617 decided, so that `emela check` and the LSP report them. What a Pome declares is a name for a kind of host; the language does not need to know its features.

## Proposal

### Syntax

```emel
pub host effect CloudflareWorker
```

- A top-level declaration with a name and nothing else. It MAY be `pub`. Its name is in the type namespace, as the name of an effect is (`[module.resolution]`), and it is imported as one (`import Cloudflare.{CloudflareWorker}`).

### Meaning

- A host effect declared with `host effect` is a host effect (`[effect.host]`): only the host provides it, a handler for it is E0413, and `use` of it is E0414. It has no operations.
- External functions MAY write it in their uses clause. Calling one adds it to R, and it stays in R through handlers, withs and layers, as `ProcessHost` does.
- Only the entry provides it. An entry adapter (`[ext.entry]`) declares it in the uses of the function type of its first parameter, and the host provides it for each launch through the adapter.
- `main` and test functions provide Async and `ProcessHost`, and an `@export` fn that is not an entry adapter provides Async, as now. They do not provide declared host effects. As for a handler that requires `PageHost` today, a test function cannot list a handler that requires a declared host effect (E0402); code that uses its effect is tested with another handler of that effect.
- `Async`, `ProcessHost` and `PageHost` stay as they are.
- A declared host effect is Immediate (`[type.effect-bound]`).

```emel
# platform-cloudflare
pub host effect CloudflareWorker

@external(js, "./kv.mjs", "get")
suspend fn kv_get(binding: String, key: String) -> String uses { CloudflareWorker }

pub handler WorkerKv(binding: String): Kv {
  init(binding: String) { WorkerKv(binding:) }

  suspend fn get(key, resume) { resume(kv_get(self.binding, key)) }
}

@export(js, "default", "fetch")
pub fn serve(
  app: fn(Request) -> Response uses { Async, CloudflareWorker },
  request: JsRequest,
) -> JsResponse
  uses { Async, CloudflareWorker } {
  ...
}
```

```emel
# the app: the worker entry
@entry(Cloudflare.serve)
fn app(request: Request) -> Response {
  with WorkerKv(binding: "KV") { handle(request) } # CloudflareWorker is provided by the adapter
}

fn main() {
  with WorkerKv(binding: "KV") { run() } # E0402: CloudflareWorker is not provided in main
}
```

### Typing rules

- A declared host effect is an effect in R, and is the same effect only as itself: two declarations with the same name in different modules are different effects, as for other effects.
- It is not removed by a with (`[effect.host]`).

## Specification changes

### `[effect.host]`

Replace "A host effect is an effect whose only handler is the host. `Async` (`[effect.async]`), `ProcessHost` and `PageHost` are the host effects." (the text of the first item, before its sub-items) with:

```markdown
A host effect is an effect whose only handler is the host. `Async` (`[effect.async]`), `ProcessHost` and `PageHost` are the host effects of the language, and a Pome MAY declare more with `host effect` (`pub host effect CloudflareWorker`).
```

Add to the sub-items of the first item:

```markdown
  - A declared host effect: the host is of a kind on which the external functions that write it in their uses clause work. The language does not know what that kind of host has. Its name is in the type namespace and is imported as the name of an effect is.
```

Replace "Calling an external function declared with `uses { ProcessHost }` or `uses { PageHost }` adds that host effect (`[ext.call]`)." with:

```markdown
Calling an external function with a host effect in its uses clause adds that host effect (`[ext.call]`).
```

Replace the row of the entry adapter in the table with:

```markdown
| An entry adapter (`[ext.entry]`) | Async, and the host effect the adapter declares (`ProcessHost`, `PageHost` or a declared one) |
```

Replace "`ProcessHost` and `PageHost` are Immediate (`[type.effect-bound]`)." with:

```markdown
- The host effects other than Async are Immediate (`[type.effect-bound]`).
```

### `[effect.decl]`

Replace the last item with:

```markdown
- An effect declared with `effect` MUST have at least one operation or handler member (E0446). The effects without operations and handler members are the host effects: those of the language, and those declared with `host effect` (`[effect.host]`).
```

### `[ext.call]`

Replace the row `uses { ProcessHost }` or `uses { PageHost }` of the table with:

```markdown
| host effects in `uses` (`uses { ProcessHost }`) | Has host side effects, and works only on a host that provides them (`[effect.host]`) | Only inside the init, the release and the operation bodies of a handler |
```

Replace "The uses clause of an external function contains only ProcessHost and PageHost, or nothing. Any other effect, including Async and a set type parameter, is a compile error (E0445)." with:

```markdown
- The uses clause of an external function contains only host effects other than Async, or nothing. Any other effect, including Async and a set type parameter, is a compile error (E0445).
```

### `[ext.entry]`

Replace "The function type of the first parameter of an entry adapter MAY have one host effect other than Async in its uses (`[effect.host]`)" with:

```markdown
The function type of the first parameter of an entry adapter MAY have one host effect other than Async in its uses: `ProcessHost`, `PageHost`, or one declared with `host effect` (`[effect.host]`)
```

(The rest of the item, ", and the R of the adapter MAY contain it. The host provides it for each launch through the adapter.", stays.)

### `[module.pome]`

Replace the example of the split into Pomes with:

````markdown
```
github.com/emela-lang/x/kv                    only effect Kv. no targets
github.com/emela-lang/x/platform-cloudflare   host effect CloudflareWorker, handler WorkerKv, entry adapter serve. targets = ["js"]
github.com/me/app                             logic. depends only on x/kv and is written with uses { Kv }
github.com/me/app/worker                      JS entry. depends on app and platform-cloudflare, allows it in [external]
```
````

Add after the example:

```markdown
- The external functions of `WorkerKv` work only in a Cloudflare Worker, so they have `uses { CloudflareWorker }` (`[effect.host]`), and so does requires(WorkerKv). The entry adapter `serve` of `platform-cloudflare` provides it: `with WorkerKv(...)` type checks in the worker entry, and is E0402 in `main` and in test functions. The tests of `github.com/me/app` use a handler of `Kv` that does not need the host.
```

### `[grammar.decl]`

```ebnf
item         = fn_decl | type_decl | enum_decl | error_decl | const_decl
             | effect_decl | host_effect_decl | handler_decl | layer_decl | trait_decl

host_effect_decl = "host" "effect" type_name
```

Add to the restrictions:

```markdown
- `host` is a contextual keyword before `effect` at the start of an item.
```

### `[diag.list]`

Change:

- E0445: the condition becomes "The uses clause of an external function has an effect that is not a host effect other than Async", and the message ``the uses of external function `write_host` can contain only host effects``.
- E0446: the help becomes ``= help: an effect needs at least one operation; to name a kind of host, declare `host effect Marker` ``.

## Diagnostics

- [ ] E0445: the condition and the message
- [ ] E0446: the help

No new code.

## Compatibility

No program that compiles now stops compiling or changes meaning. `host` is a contextual keyword only before `effect`.

## Implementation

- `emela-syntax`, `emela-fmt`: `host effect` declarations.
- `emela-resolve`: the name of a declared host effect in the type namespace.
- `emela-types`: declared host effects as host effects (E0413, E0414, not removed by with, Immediate); E0445 for any host effect; entry adapters with a declared host effect; the note of E0402 naming a declared host effect.
- `emela-driver`: tests for an external function with a declared host effect reached from `main` (E0402), from a test function (E0402), and from an adapter that provides it; a handler for it (E0413); two Pomes declaring host effects of the same name.
- Editors (`docs/syntax-highlight.md` lists them): `host` before `effect` as a contextual keyword in `editors/emela.sublime-syntax`, `editors/vscode/syntaxes/emela.tmLanguage.json`, `editors/nvim/syntax/emela.vim` and `editors/tree-sitter-emela`.

No prototype has been written.

## Alternatives

- **Add kinds to the language's list** (`WorkerHost` and so on). Every new kind of host is a change of the language, and a third-party platform Pome waits for it.
- **The features of the host instead of its kind.** External functions would declare the features they need (a file system, environment variables, the standard streams, a DOM, a key-value store), and entries the features they provide; `ProcessHost` would be a set of features. More precise (a host with environment variables and no file system), at the cost of a vocabulary of features that the language or the Pomes must agree on. Kept as an open question; declared kinds can be refined later.
- **The host in the binding instead of in R.** `@external(js, "./kv.mjs", "get")` would also name the kind of host, and the build would check, as for targets (E0412), that the external functions reached from the entry have a binding for its host. More precise per binding (a JS binding that works everywhere, a WASM binding that needs a process), but the requirement leaves the types: `emela check` and the LSP would not report it, and a library's handlers would not show it. What a user must arrange belongs in types. (`host-bindings` names the host in a binding only to select among bindings, beside the requirement in R.)
- **Allow `effect Marker {}` declared by users with an empty handler.** #617 rejected it: anyone can provide it with `handler H: Marker {}`, so it guarantees nothing. A declared host effect cannot be provided by a handler (E0413).

## Prior art

- World and capability declarations of component models: a component states the interfaces it imports from its host, and a host provides a world of them (WebAssembly components and WASI worlds).
- Platform conditions in type systems, such as availability annotations that a compiler checks against the deployment target.

## Open questions

- Who may provide a declared host effect. Any entry adapter that can name it (proposed), or only the adapters of the Pome that declares it, so that the platform Pome is the authority for its host. Under the proposed rule a platform Pome that wants to be the authority can declare it without `pub`: only its own adapters can name it, while it still appears in requires of its pub handlers. The proposed rule also lets a declaration be shared: a small Pome that declares only `pub host effect CloudflareWorker`, on which the Pomes of handlers and of adapters depend. Two Pomes that each declare `CloudflareWorker` declare different effects, and so do two major versions of one Pome (`/v2`), so an adapter of one does not provide what the handlers of the other require. With the other rule, only the declaring Pome can have the adapter.
- How many host effects an entry provides. An adapter provides one now. An external function that works in a page and in a Web Worker (the web APIs) needs either an entry that provides several, or relations between kinds (one kind includes another).
- The features of the host instead of its kind (Alternatives).
- Whether `ProcessHost` and `PageHost` become declarations in std (`pub host effect ProcessHost` in `Std.Process`) or stay in the language.

## Decision
