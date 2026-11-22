---
title: "Effect aliases: `x <- Effect` resolves at the call"
authors: [Alliana]
status: accepted
pr: 731
issue: 719
decided: 2026-11-23
touches: [grammar.expr, effect.use, effect.with, effect.handler, effect.escape, fiber.scope, diag.list]
requires: []
related: [capability-type, capture-checking, scope-handle-values, handler-member]
---

# Effect aliases: `x <- Effect` resolves at the call

## Summary

- A statement `x <- Effect` is added. x is called an effect alias. x is not a value; its name is scoped lexically, and `x.op(...)` reaches the innermost handler of Effect at the time of the call and adds Effect to R.
- Inside a closure or a local fn, the withs that enclose the function do not resolve it. Effect stays in the R of the closure, and the side that calls the closure provides it.
- `(use E).op(...)` becomes shorthand for `{ h <- E; h.op(...) }`. A call written in place makes no root, and is checked only through R.
- `use E` written as a value (`x = use E`, an argument, an element of a data structure) still works as a fixed value, and capture checking still checks it.
- `use E` written as an expression is deprecated in both forms (W04xx). Making a value with `use E` and binding it to a name is replaced by `x <- E` or by a named item (`with x = H`).

This proposal is the first step. The second step, `capability-identity` (#732), defines a capability as an instance of a handler, tells capabilities apart in types and in R, and makes `use E` written as an expression a compile error.

Each construct is checked by exactly one mechanism.

| Construct | What it is | Checked by |
|---|---|---|
| `x <- E`, `(use E).op(...)` | The effect, resolved at the call | R (types) |
| `x = use E`, `with x = H`, `use E` written as a value | A fixed value | Capture sets |

## Motivation

### A call is fixed even without a name

- A value of `use E` is fixed to the handler of the innermost with that provides E at the position where it is written (`[effect.with]`).
- `use E` inside a closure also looks first at the withs that enclose the closure (`[effect.with]`). An effect resolved by an enclosing with does not stay in the R of the closure.
- So `|s| (use Greeter).greet(s)`, which never takes a value out, leaves the with while still fixed to `Plain`.

### This is reproductions 2 to 5 of #719

- A fixed value, or a closure that captures one, leaves the with through `State.set` of an outer state, `Scope.fork` of an outer scope, an operation of a handler initialized earlier, or a field of an error.
- The R of the closure that leaves does not contain Greeter, or it is widened to `uses { Greeter }` (`[effect.use]`). The types stop nothing, and the call reaches a handler whose with has ended.
- Only capture checking stops this, and today it looks only at the value of the body (#719). Closing each path one by one (`capture-checking-exits`) brings the function boundary ("Decision 1" of #719) and false positives ("Decision 2").

### There is no way to name an effect without fixing it

- Even when a program only wants to call operations, the only name it can make today is a fixed value. A fixed value is subject to capture checking, and every path out of the with has to be closed.
- With a name that is not fixed, R protects the program in the types even when a closure leaves the with. Capture checking is left with only the values that really need to be fixed.

Counts in this repository (excluding `.snap` files):

| Form | Count |
|---|---|
| `use E` written as an expression | about 1,110 |
| `(use E).op(...)` (called in place) | 803 |
| `x = use E` | 278 |
| A value taken outside used inside an inner with that provides the same E | 18 |
| A value of `use E` passed, stored or returned | about 59 lines |

Nothing stops working with this proposal. `use E` written as an expression gets a warning. The meaning changes only where `(use E).op(...)` inside a closure was resolved by a with that encloses the closure (Compatibility).

### Out of scope

- #701 (calling an operation of a capability of a handler member received as a closure parameter or a return value leaves the effect in R, E0402) is a problem of fixed values. This proposal does not solve it; `capability-identity` does.

## Proposal

### Terms

| Term | Meaning |
|---|---|
| effect alias | The x of `x <- E`. Not a value, but a name for calling the operations of E. What goes into R is E itself |
| declaration of an effect alias | The statement `x <- E` |
| resolved at the call / fixed | When the handler that an operation reaches is decided: at each call, or once where the value is made |

An effect alias is not a label. A label (`[effect.label]`) counts in R as an effect different from its underlying effect; an effect alias puts E itself into R.

### Syntax

```ebnf
stmt         = binding | effect_alias | local_fn | expr
effect_alias = lower_name "<-" type_ref [ "[" type { "," type } "]" ]
```

```emel
fn greet(id: Int) {
  log <- Logger
  users <- Users

  user = users.find(id) |> Option.or_fail(NotFound(id:))
  log.info("hello #{user.name}")
  user
}
```

- An effect alias names one effect. A type annotation cannot be written on it, and one alias cannot name several effects; the right side already names the effect.
- `<-` is not used in with items. An effect alias is not fixed, while an item of a with provides one handler.

### Meaning

- `x <- E` does not make a value. x becomes a name for calling the operations of E in the rest of the block.
- `x.op(arguments)` calls the operation op of the handler of E that is innermost at the time the call is evaluated.
- `x.op` without a call gives a function value. E is resolved each time the function value is called.
- `x.op(...)` inside a closure or a local fn is not resolved by the withs that enclose the function. The side that calls the closure provides E.
- `(use E).op(arguments)` means `{ h <- E; h.op(arguments) }`, where h is a fresh name. `(use E).op` without a call is expanded in the same way.
- `use E` written anywhere else is a fixed value, as today.
- `s <- Scope` is allowed. `s.fork(f)` forks into the innermost scope at the time of the call, as `Fiber.fork(f)` does. Forking into an outer scope still uses a value of `use Scope` taken in the outer scope until `capability-identity`.

```emel
with PrimaryDb {
  primary = use Db # a fixed value (deprecated)
  with ReplicaDb {
    db <- Db
    db.query("...") # ReplicaDb
    primary.execute("...") # PrimaryDb
  }
}
```

```emel
with Plain {
  st.set(|s| (use Greeter).greet(s)) # Greeter stays in the R of the closure
}
```

### Typing rules

- Writing x anywhere other than as the receiver of `x.op(...)` or `x.op` is a compile error: as an argument, an element of a data structure, a return value, or the right side of a binding.
- A handler member cannot be opened through x (`with f = x.Open(...)` is a compile error). x is not a capability; the item is written `f = E.Open(...)`.
- `x.op(...)` adds E to the R of the innermost function that contains the call, including a closure or a local fn.
- A with removes effects only from the R of its body. The R of a closure inside the body stays in the type of the closure.

### Capture checking

- x is not a value, so it is not in any capture set. A closure that captures x holds nothing for it. `(use E).op(...)` makes no root either.
- `use E` written as a value still holds the root of the innermost with at that position.
- The checks of A and B of #719 (arguments of operations and errors) remain for fixed values and for the handles of scopes.

### Deprecation

- `use E` written as an expression, in either form, reports W04xx. The help names the replacement: `x <- E` for the shorthand and for a value used only as a receiver, and a named item `with x = H` otherwise.
- The diagnostic carries the rewrite where it can be done mechanically, and the language server offers it as a code action. `emela fmt` does not rewrite it, since formatting does not change meaning.
- The warning lasts one minor version. `capability-identity` makes it a compile error in the minor version after the one that adds this warning.
- No shorter form for calling in place is added. If one is wanted, it is a separate proposal.

## Specification changes

### `[grammar.expr]`

```ebnf
stmt         = binding | effect_alias | local_fn | expr
effect_alias = lower_name "<-" type_ref [ "[" type { "," type } "]" ]
```

- An effect_alias is a statement. Whether a statement is an effect_alias is decided by whether a lower_name is immediately followed by `<-`.

### `[effect.use]`

(Added.)

- `x <- E` is a statement that declares x as an effect alias of E for the rest of the block. It does not make a value. E is an effect, with its type arguments, or a label. A type annotation cannot be written on it.
- `x.op(arguments)` calls the operation op of the handler of E that is innermost at the time the call is evaluated, and adds E to the R of the innermost function that contains the call. Inside an anonymous function or a local fn, the withs that enclose the function do not provide E to the call; the side that calls the function provides it.
- `x.op` without a call gives a function value whose R contains E. E is resolved each time the function value is called.
- x can be written only as the receiver of `x.op(...)` or `x.op`. Writing x anywhere else is a compile error (E04xx). A handler member cannot be opened through x.
- `(use E).op(arguments)` means `{ h <- E; h.op(arguments) }`, where h is a fresh name. `(use E).op` without a call is expanded in the same way.
- `use E` written anywhere else is a capability value. It refers to the handler that provides E at that position, and keeps referring to it.
- `use E` written as an expression, in either form, is deprecated (W04xx). It is read for one more minor version.

### `[effect.with]`

- "`use X` inside an anonymous function or a local fn first looks at the withs that enclose the function ..." applies only to `use X` written as a capability value. A call through an effect alias or `(use X).op(...)` inside the function is provided by the side that calls the function.

### `[effect.handler]`

- "The `use` expressions inside `init` are the requirements of the handler" also covers the effect aliases declared inside `init`.

### `[effect.escape]`

- An effect alias is not a root and holds nothing. `(use E).op(...)` does not refer to a root.

### `[fiber.scope]`

- `s <- Scope` declares an effect alias of the innermost scope at each call. `s.fork(f)` is the same as `Fiber.fork(f)`.
- The exception "a closure created inside a with in the scope body ... is passed to a child" no longer applies to a closure that calls operations through effect aliases or `(use E).op(...)`: the child resolves the effect at its own call.

### `[diag.list]`

- E04xx: an effect alias written as a value, or a handler member opened through an effect alias.
- W04xx: `use E` written as an expression is deprecated. The help is `` = help: write `x <- E`, or a named item `with x = H` ``.

## Diagnostics

- [ ] E04xx: an effect alias written as a value, or a handler member opened through it (help: for a fixed value, use a named item `with x = H`)
- [ ] W04xx: `use E` written as an expression (deprecated), with a machine-applicable rewrite where it can be done mechanically

## Compatibility

- The meaning changes where `(use E).op(...)` inside a closure was resolved by a with that encloses the closure.
  - If the closure is called inside that with, the result is the same (`List.map(names, |n| (use Logger).info(n))`).
  - If the closure is called outside that with, the call reaches the handler at the place of the call, or the types stop it. It no longer reaches a handler whose with has ended.
  - Passing such a closure to a function type written with `uses {}` (a callback for the host, for example) becomes E0402 or E0403.
- `x = use E` and `use E` written as a value keep working, with a warning.
- Warnings in this repository: 803 for the shorthand, 278 for `x = use E`, and about 59 lines for values passed, stored or returned. The shorthand, and `x = use E` used only as a receiver, can be rewritten to `x <- E` mechanically.

## Implementation

- A prototype is on the branch `proto/effect-binding-arrow`.
- Syntax: add `x <- E`. Expand `(use E).op(...)` into an effect alias, before name resolution or at the entry of type checking.
- Type checking: a call through an effect alias adds E to R even inside a closure, and does not take the path that resolves it by an enclosing with.
- Lowering: a call inside a closure uses the evidence passed by the caller, which is the path taken today when no with encloses the function.
- Capture checking: an expanded call holds no root.

## Alternatives

- Remove `use E` written as an expression in this proposal without a deprecation period. The 278 `x = use E` and the about 59 lines of values would have to be rewritten at once.
- Keep the shorthand `(use E).op(...)` without deprecating it. `use E` would keep changing meaning by position (resolved at the call as a receiver, fixed elsewhere), and making a value with `use E` and binding it to a name would stay.
- `capture-checking-exits` (count arguments of operations and errors as places where a value leaves the with, as #719 expects). It can be combined with this proposal as the check for fixed values. On its own, the function boundary stays a problem even for closures that only call operations.
- Do not widen roots in R to effects (candidate 3 of #719). Function values are stopped by types, but existing code that relies on widening becomes E0403.
- Decide the handler of `x <- E` where it is declared. x would be the same as a fixed value, and a closure that captures it would reach an ended handler after leaving the with. It needs capture checking, and `<-` would add nothing.

## Prior art

## Open questions

## Decision

A capability is fixed to a handler even when it is only called in place, so a closure that calls `(use E).op(...)` leaves its with still fixed to that handler, and only capture checking can stop it. Effect aliases give a name that is resolved at the call: what leaves a with keeps E in its R, and the types protect it. Each construct is now checked by one mechanism: effect aliases and calls in place by R, fixed values by capture sets.

Binding a value made by `use E` to a name is what makes capabilities hard to follow, so `use E` written as an expression is deprecated in both forms. Nothing stops working in this step; the warning names the rewrite. `capability-identity` (#732) is the second step, which tells capabilities apart in types and in R and removes `use E` written as an expression.
