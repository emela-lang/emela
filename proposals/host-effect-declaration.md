---
title: Host effects declared by Pomes
authors: [somnicattus]
status: draft
issue:
touches: [effect.host, effect.decl, ext.call, ext.entry, module.pub, module.pome, module.resolution, grammar.decl, kw.contextual, diag.list]
requires: []
related: [host-effects, external-effect]
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

`host-effects` (#617) made "a handler for processes used in a page" a compile error, because what the host provides may appear in types. The kinds of host are fixed by the compiler: `ProcessHost` and `PageHost`. Only the compiler can declare an effect without operations (E4010), and the uses clause of an external function can contain only those two (E5002).

`[module.pome]` uses `platform-cloudflare` as its example of a Pome that touches the host: a `handler WorkerKv` with `@external`, for a Cloudflare Worker. The external functions of `WorkerKv` work only in a Worker, and there is nothing they can write:

- With `uses` omitted, a call of a suspend fn adds only Async and the External of the module (`[ext.call]`), and the bodies of `WorkerKv` remove External (`[effect.external]`). `WorkerKv` then requires only Async, so `with WorkerKv { ... }` type checks in `main` and in tests, and fails at run time (the host functions it binds do not exist in a process).
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
- `host` is a contextual keyword before `effect` at the start of an item.

### Meaning

- A host effect declared with `host effect` is a host effect (`[effect.host]`): only the host provides it, a handler for it is E4003, and `use` of it is E4402. It has no operations.
- External functions MAY write it in their uses clause. Calling one adds it to R, with the External of the module of the external function (`[ext.call]`), and it stays in R through handlers, withs and layers, as `ProcessHost` does.
- Only the entry provides it. An entry adapter (`[ext.entry]`) declares it in the uses of the function type of its first parameter, and the host provides it for each launch through the adapter.
- Any entry adapter that can name a declared host effect MAY provide it. A declaration can then be shared: a small Pome that declares only `pub host effect CloudflareWorker`, on which the Pomes of handlers and of adapters depend. A Pome that wants its own adapters to be the only ones that provide it declares it without `pub`, in the module that has the external functions that need it and the adapters: only that module can name it (`[module.pub]`), while it still appears in the requires of the pub handlers of that module. Other modules cannot name it, so a fn of another module whose R contains it leaves its uses clause to inference.
- Two Pomes that each declare `CloudflareWorker` declare different effects, and so do two major versions of one Pome (`/v2`), so an adapter of one does not provide what the handlers of the other require.
- `main` and test functions provide Async and `ProcessHost`, and an `@export` fn that is not an entry adapter provides Async, as now. They do not provide declared host effects. As for a handler that requires `PageHost` today, a test function cannot list a handler that requires a declared host effect (E4301); code that uses its effect is tested with another handler of that effect.
- `Async`, `ProcessHost` and `PageHost` stay as they are: host effects of the language, named without an import.
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
  with WorkerKv(binding: "KV") { run() } # E4301: CloudflareWorker is not provided in main
}
```

### Typing rules

- A declared host effect is an effect in R, and is the same effect only as itself: two declarations with the same name in different modules are different effects, as for other effects.
- It is not removed by a with (`[effect.host]`).

## Specification changes

The changes are applied to `spec/` in this pull request.

- `[effect.host]`: a module MAY declare host effects with `host effect`, besides `Async`, `ProcessHost` and `PageHost`: the form of the declaration, `pub`, the type namespace, that two declarations are different host effects, and that any entry adapter that can name one MAY provide it. Calling an external function with a host effect in its uses clause adds that host effect. An entry adapter provides the host effect it declares, of the language or declared; `main`, test functions and other `@export` fns do not provide a declared one (E4301). The host effects other than Async are Immediate.
- `[effect.decl]`: the effects without operations and handler members are the host effects and External; a module declares host effects with `host effect`, which is not a declaration with `effect`.
- `[ext.call]`: the uses clause of an external function contains only host effects other than Async, or nothing (E5002).
- `[ext.entry]`: the host effect in the function type of the first parameter of an entry adapter MAY be one declared with `host effect`.
- `[module.pub]`: `host effect` can be marked `pub`.
- `[module.pome]`: the example of the split into Pomes declares `CloudflareWorker` in `platform-cloudflare`, and the requirement of `WorkerKv` is stated.
- `[module.resolution]`: a host effect is in the type namespace.
- `[grammar.decl]`: `host_effect_decl` is an item, and `host` is a contextual keyword before `effect`.
- `[kw.contextual]`: `host` before `effect`.
- `[diag.list]`: the condition and the message of E5002, and the condition (an effect declared with `effect`) and the help of E4010.

## Diagnostics

- [ ] E5002: the condition and the message
- [ ] E4010: the condition and the help

No new code.

## Compatibility

No program that compiles now stops compiling or changes meaning. `host` is a contextual keyword only before `effect`.

## Implementation

- `emela-syntax`, `emela-fmt`: `host effect` declarations.
- `emela-resolve`: the name of a declared host effect in the type namespace.
- `emela-types`: declared host effects as host effects (E4003, E4402, not removed by with, Immediate); E5002 for any host effect; entry adapters with a declared host effect; the note of E4301 naming a declared host effect.
- `emela-driver`: tests for an external function with a declared host effect reached from `main` (E4301), from a test function (E4301), and from an adapter that provides it; a declared host effect imported from another Pome; a handler for it (E4003) and `use` of it (E4402); one declared without `pub` named from another module (E2104); two Pomes declaring host effects of the same name.
- Editors (`docs/syntax-highlight.md` lists them): `host` before `effect` as a contextual keyword.

No prototype has been written.

## Alternatives

- **Add kinds to the language's list** (`WorkerHost` and so on). Every new kind of host is a change of the language, and a third-party platform Pome waits for it.
- **The features of the host instead of its kind.** External functions would declare the features they need (a file system, environment variables, the standard streams, a DOM, a key-value store), and entries the features they provide; `ProcessHost` would be a set of features. More precise (a host with environment variables and no file system), at the cost of a vocabulary of features that the language or the Pomes must agree on. Left to a later proposal, which this one does not block: a kind could later be declared as a bundle of features (`ProcessHost` as a bundle of `FileSystemHost`, `EnvHost` and others). `uses { ProcessHost }` would keep its meaning, since it requires all of them, and external functions could be refined to the features they need one at a time, so the vocabulary would start from the kinds and grow instead of being agreed on in advance. Or the kinds of the language could keep their names, while the Pomes that use them give their meaning as features (which features a Pome takes `PageHost` to provide).
- **An entry that provides several host effects.** An external function that works in a page and in a Web Worker (the web APIs) needs one of: an entry that provides several host effects, relations between kinds (one kind includes another), or a union of host effects in a uses clause, which an entry satisfies by providing one of them (`uses { PageHost | WebWorker }`). Left to later proposals: this one keeps one host effect per entry, as now, which none of them needs to change. A union needs no vocabulary beyond the kinds, and changes only types: R gets a new kind of element, and inclusion of effect sets becomes implication. It names only the kinds known where it is written, so it does not reach a kind declared later.
- **The host in the binding instead of in R.** `@external(js, "./kv.mjs", "get")` would also name the kind of host, and the build would check, as for targets (E5001), that the external functions reached from the entry have a binding for its host. More precise per binding (a JS binding that works everywhere, a WASM binding that needs a process), but the requirement leaves the types: `emela check` and the LSP would not report it, and a library's handlers would not show it. What a user must arrange belongs in types. `host-bindings` (#727) names the host in a binding to select among bindings for several hosts; such an external function cannot name either host in its uses clause, so the requirement leaves the types in the same way.
- **Only the adapters of the declaring Pome provide a declared host effect**, so that the platform Pome is the authority for its host. Visibility is per module (`[module.pub]`), not per Pome, so the rule would be a new check on the Pome of each adapter, and it would rule out a declaration shared by the Pomes of handlers and of adapters. A declaration without `pub` already keeps the host effect to the adapters of one module.
- **Declare `ProcessHost` and `PageHost` in std** (`pub host effect ProcessHost` in `Std.Process`), so that the language knows no kind of host. They are named without an import now, as Async is; moving them would need an import wherever they are written, so programs that compile now would stop compiling. The purpose of this proposal does not need it.
- **Allow `effect Marker {}` declared by users with an empty handler.** #617 rejected it: anyone can provide it with `handler H: Marker {}`, so it guarantees nothing. A declared host effect cannot be provided by a handler (E4003).

## Prior art

- World and capability declarations of component models: a component states the interfaces it imports from its host, and a host provides a world of them (WebAssembly components and WASI worlds).
- Platform conditions in type systems, such as availability annotations that a compiler checks against the deployment target.

## Open questions

## Decision
