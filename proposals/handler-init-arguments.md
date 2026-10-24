---
title: Arguments for handler init
authors: [Alliana]
status: implemented
pr: 389
issue: 374
decided: 2026-10-22
touches: [effect.handler, effect.with, effect.layer, effect.label, grammar.decl, grammar.expr, type.rules, dyn.with]
requires: []
related: []
---

# Arguments for handler init

## Summary

The `init` of a handler can take parameters, and a value is passed to it by writing `H(x: ...)` as an item of a with or a layer, or on the right side of `by` for a label. This revises "The handler itself takes no arguments." in `[effect.handler]`.

## Motivation

Today the only way for a handler to receive a value from outside is `use` inside `init` (`[effect.handler]`). Passing one value takes a dedicated effect that returns it.

- `WorkerKv` in x/platform-cloudflare hard-codes its KV binding name as `KV`, because there is no way to pass the name.
- In a prototype of a router for an HTTP server, passing the path parameters of each route (the `id` of `/users/:id`) to a handler through a `PathParams` effect went through two steps, `State.run` and `init`.

```emel
handler StateParams(p: Params): PathParams {
  init { StateParams(p: (use State[Params]).get()) }
  fn get(name) { ... }
}

# for each route
(res, _) = State.run(params, fn() { with StateParams { f(req) } })
```

- Every value decided per request and distributed through an effect, such as the result of authentication or a correlation ID, takes the same shape.
- Settings such as the URL to connect to can already be passed by using a `Config` effect in `init`, but this needs an operation per setting or a type that bundles them.

The current specification has no way to build a handler from a value and supply it only to a body (a local binding).

## Proposal

### Syntax

```emel
pub handler WorkerKv(ns: Namespace): Kv {
  init(binding: String) { WorkerKv(ns: kv_namespace(binding)) }
  ...
}

with WorkerKv(binding: "KV") { ... }

layer AppLive {
  SystemConsole
  WorkerKv(binding: "KV")
}

effect AuditLog of Logger

with SystemConsole, AuditLog by FileLog(path: "audit.log") { ... }
```

- `init` can have a list of parameters. They are written as the parameters of an fn, and their types cannot be omitted.
- Writing `H(...)` as an item of a with or a layer, or on the right side of `by` for a label (`L by H(...)`, `[effect.label]`), calls init with those arguments. Arguments are passed as in a call of an fn (`[fn.args]`; by position or by label).
- A handler whose init takes no parameters is written `with H`, as before. `with H()` cannot be written.

### Meaning

- The arguments are evaluated outside the with. The handlers of the items of the same with are not yet provided when the arguments are evaluated. The rule that the requirements of init can be satisfied by an item of the same with (`[effect.with]`) does not apply to the arguments.
- The arguments are evaluated in the order the items are written, before any init runs. The E and R of the arguments of a handler that is not overridden are added to the E and R of the with, in the same way as the E of init.
- The arguments of a handler overridden by a later item are not evaluated, and their E and R are not added to the E and R of the with. This matches init not running for such a handler. The arguments are still resolved and type-checked.
  - Most overriding replaces part of a layer (`with AppLive, FakeKv`), and the arguments of a layer item are pure (below), so no side effect is lost by not evaluating them. A side effect can be lost only when handlers for the same effect are listed directly in one with, where it is visible in the code.
- A capability value cannot be passed as an argument (`[effect.use]`). A closure that captures a capability can be.
- The rule that a handler value can be constructed only inside init (E0418) does not change. `H(...)` at a with is a call of init, not a construction of the value.
- Handlers stay non-generic (`[effect.decl]`). The types of the parameters of init are concrete types.
- The arguments of a layer item are limited to pure expressions (E and R are empty), as for the right-hand side of a const (`[data.const]`). `use` and `fail` cannot be written, and function calls can.
  - The order of the items of a layer has no meaning, and the initialization order is decided from requires(H) (`[effect.layer]`). Arguments with E or R would have no defined order of evaluation. Pure arguments make the order unobservable, and give the same value wherever the layer is expanded.
  - A layer is a top-level declaration, so its arguments can only refer to consts and top-level fns in any case. To pass a value read from the environment, list the handler after the layer to override it (`with AppLive, WorkerKv(binding: name) { ... }`).

### Typing rules

- `with H(args)` checks args against the types of the parameters of the init of `H`. Errors in the number, labels and types of the arguments get the same diagnostics as a call of an fn.
- provides(H) and requires(H) (`[type.rules]`) do not change. The R of the arguments is not a requirement of the handler; it is added to the R of the with directly.
- The same holds for `L by H(args)`. Provision and requirements stay those of `L by H` in `[effect.label]`, and the arguments are treated as for an item of a with.

## Specification changes

### `[effect.handler]`

Replace "To pass configuration from outside, use an effect such as Config in init. The handler itself takes no arguments." with:

> `init` MAY take parameters (`init(url: String) { ... }`). Their types MUST be written. Arguments are passed by writing `H(url: ...)` as an item of a with or a layer, or on the right side of `by` (`[effect.with]`, `[effect.layer]`, `[effect.label]`). A handler whose init takes no parameters cannot be given arguments, and `H()` cannot be written for it.
>
> `H(...)` at a with is a call of init, not a construction of the handler value. As before, the handler value can be constructed only inside init (E0418).

### `[effect.with]`

Add after the item that says what H can be:

> - An item can be written as `H(arguments)`. The arguments are evaluated outside the with, in the order the items are written, before any init runs. The effects provided by the items of the same with cannot be used in the arguments. The E and R of the arguments of a handler that is not overridden are added to the E and R of the with. The arguments of a handler overridden by a later item are not evaluated, and their E and R are not included in the E and R of the with; they are still resolved and type-checked.

### `[effect.layer]`

> - An item of a layer can be written as `H(arguments)`. The arguments are limited to pure expressions (E and R are empty), as for the right-hand side of a const (`[data.const]`): `use` and `fail` cannot be written, and function calls can. The order of the items has no meaning, so arguments with E or R would have no defined order of evaluation. To pass a value that is not pure, list the handler after the layer in the with (`with AppLive, WorkerKv(binding: name) { ... }`).

### `[effect.label]`

> - The right side of `by` can be written as `H(arguments)`. The arguments are treated as for an item of a with.

### `[grammar.decl]`

```
handler_item = "init" [ "(" [ params ] ")" ] block
             | "release" block
             | [ "suspend" ] "fn" lower_name "(" [ lower_name { "," lower_name } ] ")" block

layer_decl   = "layer" type_name "{" supply { sep supply } "}"
supply       = type_ref [ "(" [ args ] ")" ]
```

### `[grammar.expr]`

```
with_item  = supply [ "by" by_right ]
by_right   = type_ref [ "[" type { "," type } "]" | "(" [ args ] ")" ]
```

The type arguments on the right of `by` (`Hits by State[Int]`) can be written only when it names an effect, and arguments only when it names a handler.

### `[type.rules]`

In the row of `with H { e }`, add the E of the arguments of init to E, and the R of the arguments of init to R:

> | `with H { e }` | The type of e | Ee ∪ (E of init) ∪ (E of the arguments of init) | (Re ∖ provides(H)) ∪ requires(H) ∪ (R of the arguments of init) |

### `[dyn.with]`

Replace step 1 with:

> 1. Evaluate the arguments of the items (`H(arguments)`, 8.3) in the order the items are written. Then sort the handlers by their dependencies, and run their inits in that order.

Add before the item about an init that fails:

> - If evaluating an argument fails, no init has run yet, so nothing is released, and the with ends with that error.

## Diagnostics

- [x] A handler whose init takes parameters is written without arguments: E0434
- [x] Arguments are written for a handler whose init takes no parameters, including `with H()`: E0435
- [x] The arguments of a layer item have E or R: E0318 (failure) and E0402 (R), as for the right-hand side of a const. The help shows how to list the handler after the layer to override it
- [x] Errors in the number, labels and types of the arguments use the diagnostics of a call, such as E0306 and E0307

## Compatibility

The change only adds. Programs that compile now compile with the same meaning.

## Implementation

- emela-syntax: the parameters of init, and the arguments of the items of a with, a layer and `by`
- emela-resolve: name resolution of the arguments of the items
- emela-types: checking against the parameters of init, and adding the E and R of the arguments to the with
- emela-core: lowering that evaluates the arguments and passes them to init
- x/platform-cloudflare: change `WorkerKv` to `WorkerKv(binding:)`

## Alternatives

- `with H.init(url: ...)`: presents init as a function value, but init cannot be taken out as a value.
- `with H { url: ... }`: hard to tell apart from the block of the body.
- Writing the parameters in the head of the handler (`handler H(url: String)`): the parentheses in the head already declare the fields.
- Parameters on a layer (`layer AppLive(binding: String) { WorkerKv(binding: binding) }`): this needs new rules for the scope of the parameters and the time they are evaluated. A handler listed after the layer overrides it, so this is not needed now. It can be proposed separately if a need appears.
- Evaluating the arguments of an overridden handler too: the E and R of arguments that are never used would be added to the with, in conflict with init not running.
- Passing handlers as first-class values: breaks E0418 and the rule that capability values do not escape (`[effect.escape]`).

## Prior art

- Local injection of a dependency: building a service from a value and supplying it to only part of a computation. Effect-TS (`Effect.provideService`) and ZIO (`ZLayer.succeed`) have it.
- Parameterised handlers: a handler receives arguments and uses them to interpret its operations. Koka, Eff and Effekt have them.

## Open questions

## Decision

A handler can receive a value from outside only through `use` in its init, so passing one value costs a dedicated effect: `WorkerKv` hard-codes its KV binding name, and the Router prototype had to route path parameters through `State.run` and init. Parameters on init give a direct way to build a handler from a value and supply it only to the body of a with.

The change is additive. Existing programs compile with the same meaning, and `with H` is unchanged for handlers whose init takes no parameters.

It keeps the existing invariants. `H(...)` at a with is a call of init, so the handler value is still constructed only inside init (E0418). Capability values cannot be passed, so capabilities still do not escape. Handlers stay non-generic, and supply and dependency of handlers do not change. Arguments of a layer item are pure, as the right-hand side of a const, so a layer gives the same value wherever it is expanded.
