---
title: "The R of a closure is provided only by its caller"
authors: [Alliana]
status: implemented
pr: 736
issue: 719
decided: 2026-11-23
touches: [effect.use, effect.with, type.rules, fiber.scope, grammar.lex, ext.boundary, diag.list]
requires: [effect-alias]
related: [capture-checking, scope-handle-values]
---

# The R of a closure is provided only by its caller

## Summary

This proposal completes `effect-alias` (#731).

- The R of an anonymous function or a local fn is provided only by the side that calls it. The withs that enclose the place where the function is made do not pass handlers to the calls in its body.
- The types already say so: the R of a closure stays in the type of the closure and appears where it is called (`[type.rules]`). The rule of execution is made to match the types.
- The handlers that a forked child uses are the ones at the time `Fiber.scope` is called, without exception. The exception in `[fiber.scope]` is removed.
- A callback passed to the host is called without handlers, so a function whose R contains an effect cannot be passed to the host. The types stop it.
- Three points found in the prototype of #731 are decided: when a function value `x.op` resolves, that `(use E).M(...)` is not expanded, and that `<-` is one token.

## Motivation

### #731 alone moves the fixing one level out

Found in the prototype of #731 (branch `proto/effect-binding-arrow`):

- Lowering today gives a closure, when it is made, the handlers of the withs that enclose it. In the specification this is the sentence of `[effect.with]` "`use X` inside an anonymous function or a local fn first looks at the withs that enclose the function", and the exception in `[fiber.scope]`.
- #731 stops enclosing withs from resolving only `x.op(...)` and `(use E).op(...)`. A call `h()` of another closure inside a closure is still resolved by the enclosing withs.
- As a result, implementing the text of #731 alone:
  - does not fix reproduction 3 of #719 (a closure passed to a child of an outer scope);
  - makes a closure that calls a closure worse. Today it is stopped by W0402 / E0444; written with `<-`, it reaches an ended handler without any diagnostic.

### Providing R only from the caller fixes them

Results of the prototype with the rule "the R of a closure is provided only by its caller" (`EMELA_PROTO_CALLER_TABLE=1`):

| Form | Today | #731 alone | This proposal |
|---|---|---|---|
| Reproductions 2, 4 and 5 of #719, and Decision 1 (written with `<-` or `(use E).op`) | Reaches an ended handler | Reaches the outer handler | Reaches the outer handler |
| Reproduction 3 of #719 (a child of an outer scope) | Reaches an ended handler | Reaches an ended handler | Reaches the outer handler |
| A closure that calls a closure | W0402 / E0444 | Reaches an ended handler silently | Reaches the outer handler |
| A closure made in a with inside the scope body and passed to a child | The inner handler | The outer handler (when written with `<-`) | The outer handler (at the time `Fiber.scope` is called) |

- 8 cargo tests change, all as expected from the change of meaning.
  - `children_get_the_handlers_where_scope_is_called` and `fiber_scope` (3 tests for JS, native and WASM): the output of the child changes from inner to outer, as in the last row of the table.
  - 5 snapshots of lowering and mono: the closure no longer captures handlers, and `(use Clock)` is looked up in the table of the caller. Each closure is called inside the same with, so the results are the same.

### Types and execution disagree

- The R of a closure stays in its type (`fn(String) -> String uses { Greeter }`). In the types, the side that calls the closure provides Greeter.
- In execution, the with where the closure is made provides it. The same closure uses the handler of the caller in the types and the handler of the maker in execution.
- This disagreement is what lets a call silently reach an ended handler (#719).

### Callbacks to the host already have a hole

Checked in the prototype by passing closures to a JS `@external` (WASM cannot declare user `@external` functions, and on native a callback is E0427).

| Form | Today | Provided only by the caller |
|---|---|---|
| The callback type says `uses { Greeter, Console }`, and the closure calls `(use Greeter).greet` | Works (the handler where it is made) | Type checks, and fails at run time with a defect (`TypeError`) |
| The callback type says `uses {}` | E0403 | E0403 |
| `uses` is omitted on the parameter of the external function, and the closure is made inside an operation of a handler | Defect at run time (`TypeError`) | The same |
| The closure captures a value of `g = use Greeter` | Works | Works (the value stays fixed) |

- The host calls a closure without passing handlers. The first row works today only because the closure holds the handlers of the place where it is made.
- `[ext.boundary]` says nothing about the R of a callback. The third row already breaks at run time today.

## Proposal

### Rule

- The handlers that the calls in the body of an anonymous function or a local fn need are passed by the side that calls the function. The withs that enclose the place where the function is made do not pass them.
- When a closure captures the capability of a named item (x of `with x = H`) or a value of `use E` (deprecated), calls through it reach the handler of that value, as today. Only values are fixed.
- A closure written in the body of a with and called inside that with (`List.map(names, |n| log.info(n))`) gives the same result, because the caller is inside the with.

### Forked children

- A child uses the handlers at the time `Fiber.scope` is called (the current rule of `[fiber.scope]`). A with inside the scope body does not pass its handlers to closures passed to children either.
- The exception in `[fiber.scope]` (a closure made in a with inside the scope body and passed to a child uses the inner handler) and "capture sets in 0.22 stop it statically" are removed.
- To let a child use the handler of a with inside the scope body, the child captures the capability of a named item.

### Callbacks to the host

- The R of a function type in the parameters or the result of an external function (a callback) MUST NOT contain an effect. A hidden set variable of an omitted `uses` is taken as empty. Passing a function whose R contains an effect is a compile error (E04xx).
- The R of a callback may contain capabilities (roots of named items and of captured capability values). A call through a captured capability reaches its handler without the host passing anything. How capabilities are counted in R is decided by `capability-identity`; this rule only excludes effects.
- To use a handler inside a callback, the closure captures the capability of a named item.

```emel
with g = Plain {
  set_timeout(10, || g.greet("later")) # captures g; no effect in the R of the callback
}
```

### Three points carried over from #731

1. A function value written without a call, `x.op` where x is an effect alias or `(use E).op`, resolves E when it is called. `c.op` where c is a capability value (`log.info` of `log = use Logger`) resolves at the position where it is written, as today. The only place in this repository affected is `List.map([1], (use Clock).sleep)` in `crates/emela-driver/tests/diagnostics.rs:1515`, which is called inside the same with, so its result is the same.
2. `(use E).M(...)`, which opens a handler member, is not expanded. It opens a member of the innermost handler of E at that position, as today (it is `use E` written as an expression, so it is deprecated). As decided in #731, a handler member cannot be opened through an effect alias.
3. `<-` is one token.
   - The prototype read `lower_name < - TYPE_NAME` at the head of a statement as `x <- E`, and misread valid programs such as `x < -Int.abs(3)` and `{ x < -Money(cents: 1).cents }` (as the last expression of a block).
   - Today `<` and `-` are separate tokens, so both `a<-1` and `a <- 1` are read as `a < -1` (a comparison). As one token, both become compile errors (E01xx). No form silently changes meaning.
   - `<-` after an operand in an expression reports E01xx, and the diagnostic shows `a < -b` for a comparison.
   - In a statement `x <- R` where R is not an effect name (`x <- 1`, `x <- LIMIT`, `x <- Int.abs(3)`), E01xx is reported too, and the help shows the comparison built from what was written (`` write `x < -Int.abs(3)` ``).
   - `a < -1` (a comparison written with a space) stays as it is. The right side of `x <- E` is an effect name and the `b` of `a < -b` is a value, and no name is both. In the prototype, every form with a misplaced space was a compile error (`x <- 1`, `x <- LIMIT`, `x <- Int.abs(3)`, `x <- Plain`, `x < -Greeter`, `x < -Plain`). No combination passes silently with another meaning.
   - `emela fmt` prints a comparison as `a < -1`, so formatted code is not affected. There are no such forms in std, the examples or the tests.

### Applying and implementing

- The "Specification changes" of `effect-alias` and of this proposal are applied to `spec/` together, in one pull request.
- #735 (the implementation of `effect-alias`) implements both proposals together. Implementing `effect-alias` alone would ship the regressions in Motivation.

## Specification changes

### `[effect.with]`

- Replace "`use X` inside an anonymous function or a local fn first looks at the withs that enclose the function (in the same top-level function). If there is none, it refers to the one passed from the side that calls the function." by: "The handlers that the calls inside an anonymous function or a local fn need are passed by the side that calls the function. The withs that enclose the function do not provide them. A capability value that the function captures keeps referring to its handler."

### `[type.rules]`

- Add to the rule that an anonymous function and a local fn enclose the E and R of their bodies: "The R of the function is provided where the function is called, not where it is made."

### `[fiber.scope]`

- Remove the exception "The exception is when the same effect is also provided outside the scope, and a closure created inside a with in the scope body (or a capability value taken out there) is passed to a child ... (capture sets in 0.22 stop it statically)." except for a capability value that the child captures.
- Add: "To let a child use a handler of a with inside the scope body, the child captures the capability of a named item."

### `[effect.use]`

- `x.op` without a call, where x is an effect alias, and `(use E).op` without a call, give a function value that resolves E each time it is called. `c.op` where c is a capability value resolves at the position where it is written.
- `(use E).M(arguments)` that opens a handler member is not expanded into an effect alias.

### `[grammar.lex]`

- `<-` is one token. A comparison with a negated operand is written with a space between `<` and `-` (`a < -1`).
- `<-` after an operand in an expression (`a<-1`, `a <- 1`) is a compile error (E01xx). `<-` is written only in a statement `x <- E` (`[grammar.expr]`).
- In a statement `x <- R`, R that is not an effect name (`x <- 1`, `x <- LIMIT`, `x <- Int.abs(3)`) is a compile error (E01xx). The diagnostic shows the comparison written with a space, `x < -R`, as a help.

### `[ext.boundary]`

- The R of a function type in a boundary type (a callback) MUST NOT contain an effect. A hidden set variable is taken as empty. Passing a function whose R contains an effect is a compile error (E04xx). The R may contain capabilities; a callback reaches a handler only through a capability it captures.

### `[diag.list]`

- E04xx: a callback passed to or returned from an external function has an effect in its R.
- E01xx: `<-` after an operand in an expression, or the right side of `x <- R` that is not an effect name.

## Diagnostics

- [x] E0454: a callback has an effect in its R (help: capture the capability of a named item)
- [x] E0152: `<-` in an expression, or R of `x <- R` that is not an effect name. Messages in the prototype (E0199):
  - In an expression: `` `<-` can only bind an effect at the start of a statement (`x <- E`); to compare with a negative value, write `a < -b` ``
  - At the head of a statement: `` the right side of `<-` must be an effect ``, with the help `` `x <- E` binds an effect; to compare with a negative value, write `x < -Int.abs(3)` `` (built from what was written)

## Compatibility

- The meaning changes for code that relied on the handlers of the withs where a closure is made.
  - A closure made inside a with and called outside it. Today it reaches an ended handler or gets W0402 / E0444. With this proposal it reaches the handler of the caller, or the types stop it.
  - A closure made in a with inside a scope body and passed to a child (the exception in `[fiber.scope]`). The child uses the outer handler.
- A closure made inside a with and called inside that with does not change.
- A callback whose type has an effect in R and whose body uses a handler becomes E04xx. It is rewritten to capture the capability of a named item.
- `a<-1` and `a <- 1`, read today as `a < -1` (a comparison), become compile errors (E01xx). No form silently changes meaning. They do not appear in code formatted by `emela fmt`.

## Implementation

- Lowering: when a closure is made, do not add the evidence of enclosing withs to the table of the closure. The table of a closure is only what it receives from its caller (`caller_ev` in the prototype).
- Capture checking: the roots inside a closure are decided only from the withs inside the closure and the captured values.
- Type checking: check the R of callback types.
- Lexing: make `<-` one token (LEFT_ARROW). The parser reports E01xx for `<-` in an expression (E0199 in the prototype).
- The parser reads R of `x <- R` broadly (to the end of the line, even when it is not an effect name), and the syntax check (`validation.rs`) reports E01xx with the help. Name resolution does not report E0217 on top of it.
- fmt: format a statement `x <- E` as `c <- Console`, `st <- State[Int]` (add EFFECT_BINDING to the printer of `emela-fmt`).
- Left over from the prototype: it allows opening a member through `fs <- Files` in a with item (`t = fs.Temporary()`). As decided in #731, this is a compile error.

## Alternatives

- Resolve only `x <- E` and `(use E).op` at the caller (#731 alone). Fixing remains for a closure that calls a closure, and some forms get worse.
- Give only callbacks the handlers of the place where they are made. The rule would differ only for callbacks called by the host, and types (where R stays) and execution would still disagree. Capturing a capability expresses the same thing.
- Use another symbol instead of `<-`. `<-` was adopted in #731, and making it one token removes the misreading.

## Prior art

## Open questions

## Decision

Make the rule of execution match the types: the R of a closure stays in its type and is provided where the closure is called, so the withs that enclose the place where it is made no longer provide it. This closes the paths of #719 that #731 alone leaves open (reproduction 3 and a closure that calls a closure), and makes the handlers of forked children uniform. Callbacks to the host cannot carry effects in R, since the host passes no handlers; capturing a capability covers that use. `<-` becomes one token to avoid misreading `x < -y`. The spec changes of #731 and this proposal are applied together, and #735 implements both.
