---
title: Bindings for a kind of host
authors: [somnicattus]
status: draft
pr: 727
issue:
decided:
touches: [ext.decl, grammar.decl, diag.list]
requires: []
related: [host-effects]
---

# Bindings for a kind of host

## Summary

An `@external` annotation MAY name the host effect of the host it is for: `@external(js, "./fs_process.mjs", "readText", host: ProcessHost)`. The build selects, for each external function, the annotation for the host that the entry provides, or the annotation without a host. One handler can then run on several kinds of host with a different host function on each, without a handler per host. What types show does not change: the host effects in the uses clause of the external function.

```emel
@external(js, "./store_process.mjs", "read", host: ProcessHost)
@external(js, "./store_page.mjs", "read", host: PageHost)
suspend fn store_read(key: String) -> String
```

## Motivation

### The same function, a different host function per host

Some host functions exist on every host of a language under different names or modules: reading a file in a process (`node:fs`) and in a page (the origin private file system), random bytes (`node:crypto` and Web Crypto in older hosts), a socket in a process and in a Cloudflare Worker. An `@external` names one host function per language (`[ext.decl]`), so an external function that should work on two hosts of the same language cannot have both.

The current way is a handler per host, chosen by the layer the entry lists:

```emel
pub handler ProcessStore: KeyValue { ... } # binds to store_process.mjs
pub handler PageStore: KeyValue { ... }    # binds to store_page.mjs
```

It works. It costs a copy of the handler for each host when only the lowest call differs, or a split into private helpers and thin handlers, and the program must name the right handler for its entry, although the entry already decides the host (`[effect.host]`).

### The host is known at build time

The entry decides the host effects of a launch: `main` and tests provide `ProcessHost`, an entry adapter the host effect it declares. The build already selects annotations by the language of the target (`[ext.decl]`), and checks that the reachable external functions have one (E0412). Selecting by the host as well uses what the build already knows.

### What it must not change

Types do not depend on the target, and must not depend on the annotation chosen. The host effects that an external function needs stay in its uses clause, and are what `emela check`, the LSP and the types of handlers show (`host-effects`, `host-effect-declaration`). An annotation for a host selects a host function; it does not add a requirement.

## Proposal

### Syntax

An `@external` annotation MAY end with `host:` and a host effect other than Async. The name after `host:` is looked up as a type name in the module of the declaration, so a host effect of another module is imported or written with its module. An annotation argument that names a declaration has a precedent: the argument of `@entry` names an entry adapter (`@entry(Http.serve)`, `[ext.entry]`). `@entry` looks up a fn; `host:` looks up a host effect.

```emel
@external(js, "./store_process.mjs", "read", host: ProcessHost)
@external(js, "./store_page.mjs", "read", host: PageHost)
@external(wasm, "wasi:filesystem/types", "read", host: ProcessHost)
suspend fn store_read(key: String) -> String
```

### Meaning

- For each language, an external function MAY have one annotation without a host and at most one annotation for each host effect.
- The host of a launch is given by its entry (`[effect.host]`): `main` and test functions `ProcessHost`; an entry adapter the host effect it declares, or none if it declares none; an `@export` fn that is not an entry adapter none.
- The build selects, for each reachable external function, the annotation of the target's language for the host of the launches that reach it; if there is none, the annotation without a host. If neither exists, it is a compile error (E0412, with the host named).
- When the launches that reach an external function have different hosts (a `main` and an `@export` fn in the same program), the annotation without a host is selected (see Open questions).
- If the uses clause of an external function has a host effect, an annotation for another host is a compile error (E04xx): the function runs only on the hosts its uses clause names.
- An external function whose uses clause has no host effect, and whose annotations of the language are all for hosts, type checks in every launch, and is E0412 at build time for a launch on another host, as a missing language is. To make the requirement visible in types, its uses clause names a host effect, and then its annotations are for that host or without one.
- The selection is made by the build, as for languages. `emela check` and the LSP do not select an annotation and do not report E0412.
- In a library-only project, which has no launch, the check of reachable external functions (`[ext.decl]`) accepts an annotation of the language for any host or without one; none is selected.

### Typing rules

No change. The uses clause of the external function is what types show.

## Specification changes

### `[ext.decl]`

Replace "The arguments of `@external` on a fn have the form `(language, "name")` or `(language, "file", "name")` only (E0138)." with:

```markdown
The arguments of `@external` on a fn have the form `(language, "name")` or `(language, "file", "name")`, optionally followed by `host:` and a host effect, only (E0138).
```

Add after the item "Each build target uses one language.":

```markdown
- `host:` names a host effect other than Async (`@external(js, "./store_page.mjs", "read", host: PageHost)`); naming anything else is a compile error (E04xx). The name is looked up as a type name in the module of the declaration (`[module.resolution]`): a host effect of another module is imported, or written with its module (`host: Cloudflare.CloudflareWorker`). For each language, a fn has at most one annotation without a host and at most one annotation for each host effect (E0139).
- The host of a launch is given by its entry (`[effect.host]`). The build selects, for each reachable external function, the annotation of the target's language for the host of the launches that reach it, and otherwise the annotation without a host. When the launches that reach it have different hosts, it selects the annotation without a host. If no annotation is selected, it is a compile error (E0412).
- In a library-only project, which has no launch, an external function reachable from public functions is checked to have an annotation of the target's language, for a host or without one; none is selected.
- An annotation for a host effect that the uses clause of the fn does not allow is a compile error (E04xx): a fn whose uses clause has a host effect runs only on that host.
```

### `[grammar.decl]`

```ebnf
annot_arg    = annot_value | lower_name ":" annot_value
annot_value  = lower_name | literal | qualified | type_ref
```

A named argument takes the same values as a positional one, and the restrictions say where each value is allowed. The grammar accepts the same annotations as before, plus `host:` with a type_ref.

Replace "A `qualified` annot_arg is allowed only as the argument of `@entry`." with:

```markdown
- A `qualified` annot_value is allowed only as the argument of `@entry`.
- A `type_ref` annot_value is allowed only as the value of `host:`, the last argument of `@external` (`[ext.decl]`). A host effect of another module MAY be written with its module (`host: Cloudflare.CloudflareWorker`).
- After `lower_name ":"`, an annot_value is a literal, or a type_ref as above.
```

### `[diag.list]`

Add:

| Code | Condition | Message |
|---|---|---|
| E04xx | An `@external` is for a host effect that the uses clause of the fn does not allow (`[ext.decl]`) | ``annotation for `PageHost` can never be used: `store_read` needs `ProcessHost` `` |
| E04xx | The `host:` of an `@external` names something that is not a host effect other than Async (`[ext.decl]`) | `` `host:` must name a host effect, but `Logger` is not one `` |

Change:

- E0138: the condition also covers a `host:` that is not the last argument, and a type name that is not the value of `host:`, and the message becomes ``expected `@external(target, "name")` or `@external(target, "name", host: PageHost)` ``.
- E0139: also two annotations of the same language for the same host, with the message ``duplicate `@external` for target `js` and host `PageHost` ``.
- E0412: when the annotations of the language are all for hosts, the message names the host of the launch: ``external function `store_read` has no `@external(js, ...)` for `PageHost` ``; for a launch without a host effect, ``... for a launch without a host effect``; and when the launches that reach it have different hosts, the note ``= note: `store_read` is reached from launches on `ProcessHost` and on `PageHost`; only an `@external` without `host:` serves both``.

## Diagnostics

- [ ] E04xx (new): an annotation for a host that the fn's uses clause does not allow
- [ ] E04xx (new): `host:` that does not name a host effect other than Async
- [ ] E0138: the form with `host:`
- [ ] E0139: the same host twice
- [ ] E0412: the host in the message

Codes to be assigned.

## Compatibility

No program that compiles now changes. An `@external` without `host:` is selected as now.

## Implementation

- `emela-syntax`, `emela-fmt`: the `host:` argument of `@external`.
- `emela-resolve`, `emela-types`: the name after `host:`; the two new diagnostics; E0138 and E0139 for hosts.
- `editors/tree-sitter-emela`: a named annotation argument whose value is a type name.
- The reachability of `[ext.decl]` (code generation): the host of the launches that reach each external function, the selection, E0412 with the host.
- `emela-driver`: tests for an external function with annotations for a process and a page, built from `main` and from an entry adapter for a page; an external function reached from launches of different hosts.

No prototype has been written.

## Alternatives

- **A handler per host, chosen by the layer the entry lists** (the current way). No new syntax; it costs a handler for each host when only the lowest call differs, and the program names a handler that the entry already implies.
- **Annotations named after runtimes** (`@external(nodejs, ...)`, `@external(browser, ...)`). They multiply with the runtimes, name targets in source against `[lib.layers]` ("A platform handler does not expose target names"), and do not match what an entry provides. A host effect is the kind of host the entry gives.
- **Infer the host effects of an external function from its annotations.** A function with annotations for a process and a page would need "a process or a page", which an effect set cannot say (R lists what is all required). And types would depend on annotations, which depend on the target.
- **Select at run time.** The launch would ask the host what it is and call the matching function. Every call pays for it, and a missing annotation is found only when the code runs.

## Prior art

- Conditional compilation by platform: an implementation selected per target platform at build time behind one declaration, with the selection checked for completeness.
- Package export conditions that select a module per environment (a server runtime or a browser) behind one import.

## Open questions

- Launches of different hosts. When an external function is reached from launches with different hosts, the annotation without a host is selected (proposed), or each launch gets its own selection, which needs the code reachable from both to be emitted once per host.
- Whether `host:` may name several hosts for one annotation (`host: PageHost | WebWorker`), which `host-effect-declaration` leaves open as the relations between kinds of host.

## Decision
