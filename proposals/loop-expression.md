---
title: The loop expression
authors: [somnicattus]
status: implemented
issue:
touches: [glossary.terms, kw, kw.reserved, kw.future, module.resolution, fn.decl, fn.args, fn.loop, effect.with, type.inference, type.rules, grammar.expr, diag.list]
requires: []
related: [local-fn]
pr: 909
decided: 2026-11-29
---

# The loop expression

## Summary

A new expression `loop name(p1: e1, p2:, ...) [signature] { body }` defines a function that only calls itself in tail position, and calls it in place with the initial values `e1, ...`. The parameter types are inferred from the initial values and the body. The name is visible only inside the body, and can be written only as the callee of a call in tail position of the body; every other use, including a call inside the target of `escape` or the body of `with`, is a compile error. The expression adds nothing that a local fn cannot express, and generates the same code as the local fn; it turns the writer's intent that a recursion is a loop into something the compiler checks.

## Motivation

### A recursion meant as a loop is not checked

A tail call to the function itself is guaranteed not to consume stack (`[fn.loop]`). Nothing checks that a call the writer meant as such a tail call is one. A self call that is not in tail position compiles without a diagnostic, and the program fails only at run time, when the input is deep enough:

```emel
## Adds the elements. Not a tail call: `x +` runs after the call returns.
fn sum(xs: List[Int]) -> Int {
  xs match {
    [] -> 0
    [x, ..rest] -> x + sum(rest)
  }
}
```

`sum(List.range(0, 1000000))` exhausts the call stack on JS (`RangeError`) and on WASM (`call stack exhausted`), with no warning from `emela check` (emela 0.28.2+596b603).

A self call that looks like a tail call is not one in the target of `escape` or the body of `with` (`[fn.loop]`). The specification says that the linter warns there, but the warning has no code in `[diag.list]`, there is no lint command, and the compiler emits nothing for this function, where `fetch` has the type `fn() -> Int fails Timeout`:

```emel
fn retry(n: Int) -> Int {
  fetch() escape { Timeout -> if n > 0 { retry(n - 1) } else { 0 } }
}
```

The bodies of escape arms are not tail positions either (they are not in the list of `[fn.loop]`), and `retry(1000000)` exhausts the stack on both targets.

A lint that warns on every self call not in tail position cannot replace a check. Non-tail recursion is correct code in many programs (a traversal of a tree, a recursion whose depth is bounded by the input's structure), and such a lint would warn there forever. The check has to apply only where the writer chose it. Where it applies, it also catches a later edit that moves the call out of tail position (adding `+ 1` to it, wrapping it in `escape`), at the edit.

### A loop written with a local fn takes two statements and leaks its name

```emel
pub fn range(from: Int, to: Int) -> List[Int] {
  fn onto(to: Int, acc: List[Int]) -> List[Int] {
    if to <= from { acc } else { onto(to - 1, [to - 1, ..acc]) }
  }
  onto(to, [])
}
```

- A local fn cannot be the last statement of its block (E1214), so the call that starts the loop is a separate statement after it, and the initial values sit below the body.
- The name `onto` stays visible for the rest of the block, though nothing else calls it.
- The parameter types are written, though the initial values and the body determine them.
- Nothing in the form tells a reader that `onto` is a loop rather than a helper.

## Proposal

```emel
pub fn range(from: Int, to: Int) -> List[Int] {
  loop onto(to:, acc: []) {
    if to <= from { acc } else { onto(to - 1, [to - 1, ..acc]) }
  }
}
```

### Syntax

```ebnf
primary    = ... | block | if_expr | with_expr | loop_expr
loop_expr  = "loop" lower_name "(" [ loop_param { "," loop_param } ] ")" [ signature ] block
loop_param = lower_name ":" [ expr ]
```

- `loop` becomes a reserved word of the category Expressions (`[kw.reserved]`), and leaves `[kw.future]`.
- The head `(...)` is a sequence of named arguments. `p: e` makes the parameter `p` and gives it the initial value `e`. `p:` is short for `p: p`, as in a call (`[fn.args]`).
- Every element of the head starts with a name followed by `:`. Any other element is a syntax error (E1101): `loop go(0)` is ``expected parameter name``, and `loop go(i)` and `loop go(x + 1)` are ``expected `:` ``.
- The head can be empty (`loop go() { ... }`).
- `signature` is the one of a top-level fn: `-> T`, `fails` and `uses` can each be written or left out.
- `fn` is not written. `fn` is written only in declarations, local fns and function types (E1405).
- A loop expression has no place for type parameters, attributes, `pub` or `suspend`. Whether it suspends is decided by its R, as for any function: the body can call operations that suspend.

### Meaning

- The initial values are evaluated left to right, in the scope outside the loop expression, before the body runs. The parameters are not yet visible there: in `loop go(a: 1, b: a)`, the `a` of `b: a` is the `a` outside.
- The body then runs with each parameter bound to its initial value. The value of the loop expression is the value of the body.
- A call to the name in the body runs the body again with the parameters bound to the arguments of the call. It is a tail call of the function to itself, so it does not consume stack (`[fn.loop]`), including when the body contains operations that suspend.
- The body can refer to the variables visible at the loop expression.
- The name is visible only inside the body. It is not visible in the head or after the loop expression. The name and the parameters are in the value namespace, and inside the body they hide a top-level fn, a const or an import of the same name (`[module.resolution]`).
- A loop expression is not a function value. It runs where it is written, so its E and R are part of those of the enclosing body, and the withs that enclose it provide its handlers (`[effect.with]`, `[type.rules]`). The handlers that a local fn or an anonymous function needs are passed by its caller instead.

The name can be written only as the callee of a call in tail position of the body (`[fn.loop]`, with the body as the function body). Every other use is a compile error:

- the name as a value (`List.map(xs, onto)`)
- a call not in tail position (`onto(..) + 1`, the inner call of `onto(onto(1, []), [])`, the right-hand side of a binding, a statement that is not the last of its block)
- a call inside the body of an anonymous function, a local fn or another loop expression in the body (these are not tail positions of the enclosing function)
- a call inside the target of an `escape`, an escape arm, or the body of a `with`, where the `escape` or `with` is in the body. An `escape` or `with` around the whole loop expression does not count: `loop go(..) { .. } escape { .. }` is allowed

Whether the bodies of escape arms become tail positions is not part of this proposal. If a later change makes them tail positions, a call there becomes allowed, and no program that compiles stops compiling.

A parameter, binding or local fn of the same name inside the body hides the name (`[fn.local]`), and the rule does not apply to what the hiding name refers to.

A call to the name follows the rules of calls (`[fn.args]`): the arguments can be positional, in the order of the head, or named by the parameter names. A pipe into the name (`acc |> onto(n)`, `n |> onto`) is a call to it (`[fn.pipe]`). The name is not found by a method call.

### Typing rules

- The type of each parameter is a monomorphic type that the type of its initial value and the types of the arguments of the calls to the name in the body determine. It is not generalized, as the type of a binding is not (`[fn.block]`).
- A type that they leave open (the element type of `acc: []` when the body only passes it on), or that is needed before they determine it (a field access, `[type.rules]`), is written in the return type when the body returns the parameter (`-> List[Point]`), or in a binding with a type before the loop expression (`empty: List[Point] = []`, then `acc: empty`). A type left open is not an error by itself, as for a binding (`xs = []`).
- The return type, `fails` and `uses` follow the table of `[type.inference]`, as for a local fn. The E and R of the loop expression are part of the E and R of the enclosing body.
- The type of the loop expression is its return type.

### The premises

`[intro.premises]` ("There is no loop syntax") is unchanged. A loop expression is a recursive function, as a local fn is, and has no `continue` or `break`.

## Diagnostics

Two kinds of mistake, fixed in different ways, so two codes (`[diag]`). They are the next free codes of E31xx (calls and patterns).

- [x] E3111 (new): a call to the name of a loop expression that is not in tail position of its body. It is fixed by moving the call to a tail position, or by writing a local fn when the recursion is not meant as a loop. The label says why the position is not a tail position (something runs after the call: `+` applied to the result, another call taking it as an argument, a binding, a later statement; or the call is in the target of `escape`, an escape arm, the body of `with`, inside an anonymous function, a local fn or another loop expression). Example message: ``call to `onto` is not in tail position``
- [x] E3112 (new): the name of a loop expression used as a value (`List.map(xs, onto)`, `f = onto`). It is fixed by calling the name, or by writing a local fn when a function value is needed. Example message: ``loop `onto` can only be called``
- [x] E1101 (changed): in the head of a loop expression, a help.
  - a name without `:` (`loop go(i)`): ``= help: write `i:` to start `i` from the value of `i` ``
  - any other element (`loop go(0)`, `loop go(x + 1)`): ``= help: give it a name: `name: 0` ``, with the element as written after `name: `. `name` is a placeholder, not a guess from the element
  - a type name followed by `[` (`loop go(xs: List[Int])`, which stops at `[` with ``expected `)` ``): the help of E2106 below
- [x] E2106 (changed): in the head of a loop expression, when the initial value is a type name (`loop go(i: Int)`, ``expected constructor, found type `Int` ``), ``= help: the parentheses of `loop` take initial values; the type of `i` is inferred from its initial value``
- [x] E3104 (changed): also the same parameter given twice in the head (`loop go(a: 1, a: 2)`), ``argument `a` given more than once``. The head is written as arguments, so this is E3104 and not E2109 (a name bound twice in one parameter list).

## Compatibility

`loop` is already reserved for the future (`[kw.future]`), so no program uses it as an identifier. No program that compiles now stops compiling or changes meaning. The change is additive and can land in a minor release without a period as an experimental feature.

## Implementation

No prototype yet.

- emela-syntax: parse `loop_expr` as a primary expression, the head as named arguments, and the E1101 helps; emela-fmt formats the head as an argument sequence (`[fmt.wrap]`) and the signature as one of a local fn (`[fmt.signature]`).
- emela-resolve: the scope of the name (the body only) and of the parameters; the check on the uses of the name (E3111, E3112), with the reason a position is not a tail position; the help of E2106 in the head.
- emela-types: the parameter types as inference variables unified with the initial values and the arguments of the calls. A local fn requires its parameter types (E3006), so the loop expression is not lowered to a local fn before type checking.
- emela-core: lower to a local fn and a call to it, which lambda lifting turns into a module-level function (`proposals/local-fn.md`, "Implementation"). The existing loopification of self tail calls (`crates/emela-core/src/tail.rs`) gives the guarantee of `[fn.loop]`.
- emela-lsp: hover and go to definition on the name and the parameters.

The example of `[fn.local]` and the helpers of the bundled core and std that are loops (`proposals/local-fn.md`, "Motivation") are not rewritten with loop expressions by this proposal.

## Alternatives

| Alternative | Why not |
|---|---|
| A declaration `loop fn name(..) { .. }` that binds the name, also at the top level | Not now. A function body wrapped in a loop expression gets the check. It can be added later; the details then were: the name is visible in the rest of the block, the declaration does not call it in place, and it can be written at the top level |
| A clause for the exit condition and the value (`while cond else value`) | It writes only loops with one exit, and duplicates if. It saves one level of nesting. It can be added later |
| The condition at the end (do while) | It does not fit a body that ends by calling itself; it is a different model, updating a state |
| `continue` and `break` | Not added |
| No name; `loop(...)` calls the loop itself | It is a continue with arguments, and the loop is no longer a function; a nested loop cannot name the outer one |
| Initial values as default values of the parameters (`p: T = e`) | It adds syntax. The named arguments `p: e` cover it |
| Typed parameters and a call after the body (`loop go(i: Int) { .. }(0)`) | The initial values sit below the body, `}(` needs a formatting rule and a diagnostic for a loop never called, and the types are written. The named arguments remove all of these |
| Types in the head (`p: T`, `p: T = e`) | Not now. `p: T` and `p: e` look alike and have to be read apart. A binding with a type before the loop or the return type determines the type. It can be added later |
| Calling an ordinary fn in place | It is a block. Calling in place matters only for a function that calls itself again |
| A lint warning on self calls not in tail position | It warns on correct non-tail recursion (Motivation) |
| A warning, not an error, for a call to the name not in tail position | The program still compiles and exhausts the stack at run time, which is what the loop expression is written to rule out. A recursion that is meant not to be a tail call is written as a local fn |
| `loop` as a contextual keyword (`[kw.contextual]`), an identifier elsewhere | The contextual keywords are words inside declarations and signatures; none starts an expression. `loop` is already reserved for the future, so no identifier is lost by reserving it |
| The head written as bindings (`loop go(i = 0)`) | A mistaken type (`i = Int`) looks wrong at once, but the head no longer has the form of the calls to the name (`go(i: 1)`), and the shorthand `p:` has no counterpart. If parameters ever get default values, `p = e` also reads as a default value with its type left out, while an initial value is the value of the first call |
| The name as a bound capability of an effect (`Loop`), so that capture checking keeps it from leaving the body | It would keep the name inside the body even where it is used as a value. Not needed now: the name cannot be used as a value at all, which a check of the source alone enforces. It can be added later |

## Prior art

- A named let: a local recursive procedure bound and applied to its initial values in one form, with the name visible only in its body (Scheme `let loop`).
- A self call checked to be in tail position at compile time: `recur` in Clojure's `loop`, which is an error outside tail position; Scala's `@tailrec`, an annotation that makes a non-tail self call an error. Kotlin's `tailrec` and OCaml's `[@tailcall]` attribute check the same thing with a warning.

## Open questions

## Decision

A tail call to the function itself is guaranteed not to consume stack, but nothing checked that a call meant as such a tail call is one: a self call out of tail position, or inside the target of `escape` or the body of `with`, compiled without a diagnostic and exhausted the stack only at run time on deep input. A lint on every non-tail self call cannot replace a check, since non-tail recursion is correct in many programs, so the check applies only where the writer asks for it. `loop name(p: e, ...) { body }` defines a function that may call itself only in tail position, and calls it in place with the initial values; any other use of the name is a compile error rather than a warning, because a program that still compiles would fail as before. It adds nothing that a local fn cannot express and generates the same code, and it also removes the costs of writing a loop as a local fn: the separate call after it, a name that stays visible in the rest of the block, and parameter types that the initial values determine. `loop` is already reserved, so no program changes. A declaration form, an exit clause and types in the head can be added later without breaking anything.
