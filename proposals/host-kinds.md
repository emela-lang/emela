---
title: Host kinds, apart from host effects
authors: [Allianaab2m]
status: draft
issue:
touches: [glossary.terms, effect.decl, effect.with, effect.async, effect.host, effect.host-kind, effect.external, type.rules, type.effect-bound, ext.call, ext.boundary, ext.export, ext.entry, test.constraints, lib.layers, lib.http-service, api.std.process, api.std.env, api.std.tcp, api.std.file-system, api.std.platform, diag.list]
requires: []
related: [host-effects]
---

# Host kinds, apart from host effects

## Summary

`ProcessHost` and `PageHost` are no longer called host effects. They are host kinds: requirements in R that the host of a launch is of a kind. A host kind is not an effect; it has no operations, no handler and no meaning at run time. `Async` stays the one host effect. The rules that hold for both (only the entry provides them, a with does not remove them, no handler and no `use`) are stated for both by name. No program changes whether it compiles; the messages of four diagnostics change their words.

## Motivation

### Two different things under one name

`host-effects` (#617) put three things under "host effect": `Async`, `ProcessHost` and `PageHost`. They share how they are provided, and nothing else.

- `Async` is something a computation does: it is suspended and resumed later. It has a meaning at run time, and it decides Immediate (`[type.effect-bound]`). Its handler is in the host, but it is an effect.
- `ProcessHost` and `PageHost` say nothing about what a computation does. They have no operations and no meaning at run time; they only decide which entries type check. They are requirements on the environment of a launch, not effects.

The specification already needs exceptions to treat the second group as effects: E4010 allows an effect without operations only for "host effects", and `[effect.host]` gives them rules no other effect has. The cost grows with the proposals that build on them:

- `host-effect-declaration` (#726) lets Pomes declare `pub host effect CloudflareWorker`. What a Pome declares there is a kind of host, not an effect, and the word `effect` in the declaration invites operations, handlers and `use`, which are all errors.
- A union of kinds (`uses { PageHost | WorkerHost }`, satisfied by an entry that provides one of them) is a natural requirement on an environment, and has no meaning for effects: a body that may get either of two effects with operations can call the operations of neither. If both are called host effects, the rule "only host kinds can be members of a union" has no term to name it, and `Async | PageHost` would read as meaningful.
- Diagnostics call `PageHost` an effect (``effects `PageHost`, `Main.External` are not provided in `main` ``, ``only host effects have none``), so users read a kind of host as something that a handler could provide.

### Why R still holds them

A host kind travels as an effect does: a call of an external function adds it to R, the requires of a handler carry it, a with keeps it, and the entry provides it. A clause of its own (`needs { ... }`) was rejected in `host-effects` because it would double inference, set type parameters and diagnostics. This proposal keeps host kinds in R and changes what they are called, and what R is said to hold.

## Proposal

### Syntax

No change. `ProcessHost` and `PageHost` keep their names and are written where they are now.

### Meaning

- A host effect is an effect whose only handler is the host. `Async` is the host effect (`[effect.async]`).
- A host kind is a requirement that the host of a launch is of a kind. `ProcessHost` and `PageHost` are the host kinds.
  - `ProcessHost`: the host is a process of the operating system, with arguments, standard input and output, environment variables and a file system.
  - `PageHost`: the host is a page of a web browser.
- A host kind is not an effect. It has no operations and no handler. It is an element of R, as effects, labels and capabilities are (`[type.rules]`), and it is written in a uses clause (`fn f() uses { ProcessHost }`). Its name is resolved as the name of an effect is.
- What holds for both, by name:
  - Users cannot write a handler for a host effect or a host kind (E4003). `use` of either is a compile error (E4402).
  - Each enters R only through what host functions need: Async by calling an `@external suspend fn`, a host kind by calling an external function that writes it in its uses clause (`[ext.call]`). It stays in the requires of a handler whose bodies make such calls, and in the R of a with that lists the handler or a layer that contains it.
  - A with does not remove either. Only the entry of a launch provides them.

| Entry | Host effects provided | Host kinds provided |
|---|---|---|
| `main` (`[effect.with]`) | Async | ProcessHost |
| A test function (`[test.constraints]`) | Async | ProcessHost |
| An `@export` fn that is not an entry adapter (`[ext.export]`) | Async | none |
| An entry adapter (`[ext.entry]`) | Async | the host kind the adapter declares, if any |

- A host kind is Immediate (`[type.effect-bound]`): an effect set is Immediate when all its effects are, and host kinds do not affect it.
- Host kinds do not name targets. A program whose entry type checks runs on every target that supports that entry (`[target]`). A target and a kind of host are independent: the js target runs in a process and in a page, and code that requires `ProcessHost` runs on every target.
- The effects without operations and handler members are `Async` and External, which only the compiler declares (`[effect.decl]`). Host kinds are not effects, so E4010 no longer needs them as an exception.

### Typing rules

No change to which programs type check. `[type.rules]` lists host kinds among the elements of R: effects, labels, capabilities and host kinds.

### Sections reworded

- `[glossary.terms]`: adds host effect and host kind. A host kind is not a kind of a type parameter (`[type.fn]`), and not related to a host type (`[ext.host-type]`): a host type is the type of a value of the host, and a host kind is a requirement in R. The term is the compound only.

- `[effect.host]`: Async and the rules shared with host kinds. The entry table gets the two columns above.
- `[effect.host-kind]` (new, 8.10.1 under `[effect.host]`, so that the numbers of later sections do not shift): the host kinds, what each requires, and that they do not name targets.
- `[effect.external]`: the functions an external function returns have the host kinds in its uses clause.
- `[type.rules]`: host kinds are among the elements of R.
- `[effect.decl]`: the exception of E4010 names `Async` and External.
- `[effect.with]`, `[test.constraints]`: "the host effects that main is given (Async and ProcessHost)" becomes "the host effect Async and the host kind ProcessHost".
- `[effect.async]`: Async is the host effect.
- `[ext.call]`: the row "host effects in `uses`" becomes "host kinds in `uses`"; a `suspend fn` adds Async and the host kinds in its uses; the uses clause of an external function contains only host kinds (E5002).
- `[ext.boundary]`: the R of a callback MAY contain Async and host kinds.
- `[ext.export]`, `[ext.entry]`: an entry adapter MAY declare one host kind.
- `[lib.layers]`, `[lib.http-service]`, `[api.std.platform]`, `[api.std.process]`, `[api.std.env]`, `[api.std.tcp]` and `[api.std.file-system]`: "host effect" becomes "host kind" where it means `ProcessHost`; "requires no host effect other than Async" becomes "requires no host kind".

## Diagnostics

- [ ] E4010: the help ``only host effects have none`` becomes ``= help: an effect needs at least one operation``.
- [ ] E4402: `` `ProcessHost` is a host kind and cannot be used with `use` ``. The message for `Async` is unchanged.
- [ ] E4301: a missing host kind is named as one: ``host kind `PageHost` is not provided in `main` ``. When host kinds and effects are missing together, the message lists them apart: ``host kind `PageHost` and effect `Main.External` are not provided in `main` ``. The note naming the entry and what it provides lists host effects and host kinds apart (``= note: `main` provides `Async` and the host kind `ProcessHost`, not `PageHost` ``).
- [ ] E5003: "an effect other than a host effect" becomes "an effect other than Async, or a host kind".
- E4003 keeps its messages (``handler `H` cannot implement `ProcessHost`: only the host provides it``), and E5002 keeps its message. Their descriptions name the host effect Async and host kinds.

## Compatibility

- No program changes whether it compiles or what it does.
- The messages of E4010, E4402, E4301 and E5003 change their words. Their codes do not change.
- `host-effect-declaration` (#726) is open and uses "host effect" for what it declares. It is revised on top of this proposal, in its own pull request, with the declaration `pub host WorkerHost`: not `pub host effect`, which says effect, and not `pub host kind`, which reads as syntax for the kinds of type parameters (`[type.fn]`). The form leaves room for a body listing other host kinds and for `: ProcessHost`, if kinds become bundles of features or are related to each other.

## Implementation

- The resolver keeps one predicate for "only the host provides it" (Async, the host kinds and External), and gains one for host kinds. Names in the code follow the terms (`is_host_kind`).
- The diagnostics listed above.
- No change to type checking or to the build.

## Alternatives

- **Keep one term, host effect, for all three.** It needs no change now, and every later proposal on kinds of host (#726, a union of kinds, `host:`) has to write around it, as the Motivation shows.
- **Host kinds as a subset of host effects** (a host kind is a host effect that names a kind of host). It keeps the word effect for something that is not one, and "host effect" then means Async alone in some sentences and all of them in others.
- **A clause of its own for host kinds** (`needs { ... }` or `runs on { ... }`). Rejected in `host-effects`: it doubles inference, set type parameters and diagnostics, while host kinds travel exactly as host effects do.
- **Host kinds named after targets** (`JsHost`, `WasmHost`, `NativeHost`). A target and a kind of host are independent; see Meaning.

## Prior art

- Requirements on the environment of a computation, tracked beside its effects in the same row: a coeffect is what a computation needs from its context, and an effect is what it does.

## Open questions

## Decision
