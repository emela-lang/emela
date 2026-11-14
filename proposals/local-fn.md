---
title: Local functions
authors: [Alliana]
status: accepted
pr: 502
issue:
decided: 2026-11-14
touches: [fn.decl, fn.local (new), fn.args, fn.block, fn.loop, lex.comment, module.resolution, type.inference, type.rules, effect.use, effect.with, effect.escape, grammar.expr, diag.list]
requires: []
related: [closure-syntax]
---

# Local functions

## Summary

A block MAY contain a named function, a local function, written in the same form as a top-level fn: `fn name(x: T) -> U { ... }`. Its name is visible after the declaration and inside its own body, so it can call itself, and a tail call to itself is guaranteed not to consume stack. It captures the variables visible at the declaration, as an anonymous function does. It is not generalized, but it can use the type parameters of the enclosing function.

## Motivation

### Helpers of one function sit in the module

A recursion that carries an accumulator or an index is written today as a pair: a `pub fn` that states the interface, and a private top-level fn that does the work and is called from nowhere else. The bundled core and std have many such pairs.

| Module | Public function | Helper used only by it |
|---|---|---|
| `core/list.emel` | `length` | `count_onto` |
| `core/list.emel` | `range` | `range_onto` |
| `core/list.emel` | `zip` | `zip_onto` |
| `core/list.emel` | `take` | `take_onto` |
| `core/list.emel` | `filter_map` | `filter_map_onto` |
| `core/list.emel` | `find_index` | `find_index_from` |
| `core/list.emel` | `sort_by` | `merge_all`, and under it `merge_pairs_onto`, `merge_onto` |
| `std/map.emel` | `to_list` | `to_list_onto` |
| `std/map.emel` | `union` | `union_onto` |
| `std/map.emel` | `from_sorted` (private) | `build` |
| `std/bytes.emel` | `decode_utf8` | `valid_utf8_from` |
| `std/bytes.emel` | `from_hex` | `from_hex_digits` |
| `std/bytes.emel` | `impl Bytes: Eq`, `impl Bytes: Ord` | `compare_from` |
| `std/instant.emel` | `pad` (private) | `pad_onto` |
| `std/file_system.emel` | `ancestors` (private) | `ancestors_onto` |
| `std/http/request.emel` | `percent_decode` (private) | `to_bytes` |

```emel
pub fn range(from: Int, to: Int) -> List[Int] {
  range_onto(from, to, [])
}

## Builds from the back, so `to - 1` cannot overflow.
fn range_onto(from: Int, to: Int, acc: List[Int]) -> List[Int] {
  if to <= from { acc } else { range_onto(from, to - 1, [to - 1, ..acc]) }
}
```

- The helper is visible to every function of the module, though only one calls it. A reader has to search the module to learn that.
- The helper needs a name that does not collide with anything in the module (`range_onto`, `zip_onto`, `take_onto`). With the helper inside the function, every one of them could be called `onto`.
- The helper cannot capture the arguments of the public function, so they are passed on every iteration. `range_onto` passes `from` unchanged at each step, and `filter_map_onto` and `find_index_from` pass `f`.
- The helper repeats the type parameters of the public function (`zip_onto[A, B]`) and the types already written in its signature.

### An anonymous function cannot call itself

An anonymous function has no name, and a binding is visible only after it (`[fn.block]`), so `go = |n| go(n - 1)` is E0211. A call inside the body of an anonymous function is never a tail call to the function itself (`[fn.loop]`). Recursion therefore always goes up to the top level.

The proposal `closure-syntax` made the syntax of anonymous functions `|x| e` and kept them unable to refer to themselves. With this proposal the two forms have separate roles: an anonymous function is a function value without a name, and a local function is a named function that can call itself.

## Proposal

```emel
pub fn range(from: Int, to: Int) -> List[Int] {
  ## Builds from the back, so `to - 1` cannot overflow.
  fn onto(to: Int, acc: List[Int]) -> List[Int] {
    if to <= from { acc } else { onto(to - 1, [to - 1, ..acc]) }
  }
  onto(to, [])
}

pub fn zip[A, B](xs: List[A], ys: List[B]) -> List[(A, B)] {
  fn onto(xs: List[A], ys: List[B], acc: List[(A, B)]) -> List[(A, B)] {
    (xs, ys) match {
      ([x, ..xs_rest], [y, ..ys_rest]) -> onto(xs_rest, ys_rest, [(x, y), ..acc])
      _                                -> reverse(acc)
    }
  }
  onto(xs, ys, [])
}
```

`range` no longer passes `from`, and `zip` writes `List[A]` with the type parameters of the enclosing function.

### Syntax

- A local function is a statement of a block, written `fn name(params) [-> T [fails E]] [uses { R }] { body }`. It can appear in any block: the body of a fn, an anonymous function, a handler operation, init or release, and a block inside an expression.
- A statement that starts with `fn` followed by a name is a local function. `fn` followed by `(` in an expression stays E0148 (`[fn.decl]`).
- A local function cannot have type parameters, attributes, `pub` or `suspend` (E0150).
- A local function cannot be the last statement of its block (E0151).
- A doc comment `##` right before a local function attaches to it. The language server shows it on hover. Generated documentation does not include it, because a local function is not part of the module's API.

### Meaning

- The parameters, the return type and the `fails` and `uses` clauses follow the rules of a top-level fn. The parameter types are written (E0316). The meaning of an omitted return type, `fails` or `uses` is that of `[type.inference]` and `[error.fails]`: with a return type and no `fails`, the function has no errors.
- Parameters are names only, as for a top-level fn (`[fn.decl]`): a parameter name is the label of a named argument.
- The declaration binds the name in the value namespace. The name is visible from the declaration to the end of the block, and inside the function's own body. It is not visible before the declaration (there is no hoisting), so two local functions cannot call each other.
- The body can refer to every variable visible at the declaration: parameters and bindings of the enclosing blocks, and local functions declared before it. Values are immutable, so a captured variable keeps the value it had at the declaration. A later binding of the same name does not change it.
- A local function is a binding, so the same rules of shadowing apply (`[fn.block]`). A later binding or local function with the same name hides it from then on. Inside its body, its name refers to itself, unless a parameter with the same name hides it. A local function MAY have the name of a top-level fn or an import, and hides it inside the block. None of these hidings is warned about, as for other bindings.
- Called by its name, a local function takes named arguments (`[fn.args]`). Written without a call, its name is a function value, which has no parameter names.
- A local function is not found by a method call (`x.f(...)`), which looks only at the fns of the module that defines the type of x (`[type.rules]`).
- A local function as the last statement of a block is a compile error (E0151). Its name is visible only inside its block, and nothing follows it in the block, so it can never be called. This follows E0149, which rejects an anonymous function whose value is always discarded. The usual cause is a forgotten call after the declaration.
- `use X` inside a local function is resolved as inside an anonymous function (`[effect.with]`): the withs that enclose the local function in the same top-level function first, then the handler in effect where the local function is called.
- A capability value captured by a local function is tracked as for a closure. The local function, used as a value, cannot leave the with that provides the capability (`[effect.escape]`).
- A tail call to the local function itself, inside its own body, is guaranteed not to consume stack (`[fn.loop]`).
- A local function cannot be called from a test, because it is not visible outside its block. A helper that needs its own tests stays a top-level fn.

### Typing rules

- The type of a local function is the function type made from its parameters and its return type, with the E and R of its body enclosed in it, as for an anonymous function (`[type.rules]`). The declaration itself has E {} and R {}. The E and R appear where the local function is called.
- A local function is not generalized (`[fn.block]`). The unknown types of its signature, and the set variables of its E and R and of the function types of its parameters, are shared by every use in the enclosing function. Two calls that pass functions with different errors give the union of the two.
- The type names in the signature and the body are resolved in the scope of the enclosing function, so the type parameters of the enclosing fn can be written.
- A local function is checked at its position in the block, before the statements after it. Inside its body, a reference to itself has the type being inferred (a monomorphic recursive binding).

### Local functions and top-level private fns

`[fn.decl]` gives anonymous functions one form, so that the same thing is not written in two ways. A local function overlaps with neither of the existing forms in what it can do.

| | Top-level private fn | Local function | Anonymous function |
|---|---|---|---|
| Visible | the whole module | after the declaration, in the block | not named |
| Calls itself | yes | yes | no |
| Captures variables | no | yes | yes |
| Generalized | yes | no | no |
| Named arguments | yes | yes | no |
| Parameter types | required | required | optional |
| Parameter patterns | no | no | yes |
| Callable from a test | yes | no | no |

A helper used by one function, and not tested on its own, goes inside that function. A helper shared by several functions, or tested on its own, stays at the top level. For example, `compare_from` in `std/bytes.emel` serves both `impl Bytes: Eq` and `impl Bytes: Ord`. It moves into `compare`, and `eq` calls `Ord.compare(self, other)`, or it stays at the top level. The self tail call is inside the helper in both cases.

A non-recursive local function overlaps with an anonymous function bound to a name (`f = |x| ...`). The difference is in named arguments, the doc comment, and required parameter types. Both are allowed, and the linter does not suggest one over the other: the two are written differently, and the choice follows from whether the function needs named arguments, a doc comment or to call itself.

## Specification changes

### `[fn.decl]`

Replace the first item with:

- In a top-level fn and a local fn (6.9), the parameter types are written (omitting them is a compile error (E0316)). In an anonymous function they can be omitted. The return type, `fails`, and `uses` can be omitted. The meaning when they are omitted is defined in 7.4 and 9.3.

Replace "`fn` is written only in declarations and in function types." with:

- `fn` is written only in declarations, local fns (6.9) and function types. `fn(...)` in an expression is a compile error (E0148).

Replace "The parameters of top-level fns, fns in impls and handlers, and operations of traits and effects are limited to names." with:

- The parameters of top-level fns, local fns, fns in impls and handlers, and operations of traits and effects are limited to names.

Replace the last item with:

- A top-level fn, a local fn, and a constructor with fields become function values when their names are written without calling them (`List.map(xs, Some)`).

### `[fn.local]` (new, after `[fn.loop]`)

A new section, 6.9 Local functions.

```emel
pub fn range(from: Int, to: Int) -> List[Int] {
  ## Builds from the back, so `to - 1` cannot overflow.
  fn onto(to: Int, acc: List[Int]) -> List[Int] {
    if to <= from { acc } else { onto(to - 1, [to - 1, ..acc]) }
  }
  onto(to, [])
}
```

- A block can contain a local fn, a statement written `fn name(params) [signature] block`. Its parameters, return type, `fails` and `uses` follow the rules of a top-level fn.
- A local fn cannot have type parameters, attributes, `pub` or `suspend` (E0150).
- A local fn cannot be the last statement of its block (E0151). Nothing after it can call it.
- A doc comment (`##`) can be written before a local fn (2.2). The language server shows it on hover. Generated documentation does not include it.
- The declaration binds the name as a value. The name is visible from the declaration to the end of the block, and inside the local fn's own body. It is not visible before the declaration, so local fns cannot be mutually recursive.
- The body can refer to every variable visible at the declaration (it captures them, as an anonymous function does). A captured variable keeps the value it had at the declaration.
- A local fn is a binding, and follows the shadowing rules of 6.3. A later binding or local fn of the same name hides it. Inside its body, its name refers to itself unless a parameter of the same name hides it. It can hide a top-level fn or an imported name of the same name inside the block. None of these is warned about.
- A local fn is not generalized (6.3). The type parameters of the enclosing fn can be written in its signature and body.
- A local fn is not found by a method call (9.4), and cannot be called from a test (13.1), because its name is visible only inside its block.

### `[fn.args]`

Replace "Named arguments can be used only when calling an fn or a constructor referred to by name" with:

- Named arguments can be used only when calling an fn (a top-level fn or a local fn) or a constructor referred to by name

### `[fn.block]`

Replace the first item with:

- A binding is written `pattern = expr`. There is no declaration keyword. A local fn (6.9) is also a binding of its name.

Replace the last item with:

- Bindings inside a block are not generalized, and neither are local fns. Only top-level functions and consts are polymorphic.

### `[fn.loop]`

Replace the second item with:

- A tail call to the function itself is guaranteed not to consume stack. For a local fn, a call to its own name inside its body, where the name refers to that local fn, is a call to itself. The same holds for functions that contain suspend operations.

Replace the first item of "The following are not tail positions" with:

- The body of an anonymous function and the body of a local fn, as positions of the enclosing function (a call there to the enclosing function is not a call to the function itself)

### `[lex.comment]`

Replace the second item with:

- `##` is a doc comment. It attaches to the declaration, local fn, variant, field, effect operation, or handler item that immediately follows it. A doc comment written between a declaration and its `derive` does not attach to the declaration.

### `[module.resolution]`

Add after "Binding the same name twice in one pattern or one parameter list is a compile error (E0220).":

- A local fn binds its name in the value namespace from the declaration to the end of its block, and inside its own body. Inside the block it hides a top-level fn, a const or an import of the same name. A type name in a local fn is resolved in the scope of the enclosing declaration, including its type parameters.

### `[type.inference]`

Add after the table:

A local fn (6.9) follows the same table. Its E and R are inferred as part of the enclosing body, and its type is not generalized.

### `[type.rules]`

Add after the row of `|x| e`:

| `fn f(x: A) -> T { e }` (local fn, a statement) | Binds f to `fn(A) -> T fails Ee uses { Re }` | {} | {} |

Replace "An anonymous function encloses the E and R of its body in its function type. They surface when the function is called." with:

- An anonymous function and a local fn enclose the E and R of their bodies in their function types. They surface when the function is called. A reference to a local fn inside its own body has the type being inferred, and is not generalized.

### `[effect.use]`

Replace "The type of a closure that captures a capability value also contains that effect (the `copy` example below)." with:

- The type of a closure or a local fn that captures a capability value also contains that effect (the `copy` example below).

### `[effect.with]`

Replace the item on `use X` inside an anonymous function with:

- `use X` inside an anonymous function or a local fn first looks at the withs that enclose the function (in the same top-level function). If there is none, it refers to the one passed from the side that calls the function. This is because the R of these functions is enclosed in their types (9.4) and does not appear in the R of the enclosing function.

### `[effect.escape]`

Replace the first sentence with:

A capability value, and closures and local fns that capture it, cannot leave the with that provides it.

### `[grammar.expr]`

```ebnf
block      = "{" { stmt NL } "}"
stmt       = binding | local_fn | expr
binding    = pattern [ ":" type ] "=" expr
local_fn   = "fn" lower_name "(" [ params ] ")" [ signature ] block
```

Add to the list of restrictions:

- A statement that starts with `fn` followed by a lower_name is a local_fn. `fn` followed by `(` in an expression is a compile error (E0148).
- Type parameters, attributes, `pub` and `suspend` on a local_fn are a compile error (E0150).
- A local_fn as the last statement of a block is a compile error (E0151).
- A doc comment can come before a local_fn on its own lines (2.2).

Replace "When the last statement is a binding, the value of the block is `()`." with:

- When the last statement is a binding, the value of the block is `()`. The last statement cannot be a local_fn (E0151).

### `[diag.list]`

- E0316: "A parameter of a top-level fn, a local fn, or of the init of a handler, has no type (6.1, 6.9, 8.4)". For a local fn, ``parameter `x` of a local function needs a type``.
- E0150 (new): "A local fn with type parameters, an attribute, `pub` or `suspend` (6.9)". Messages ``a local function cannot have type parameters``, ``a local function cannot have attributes``, ``a local function cannot be `pub` ``, ``a local function cannot be `suspend` ``, each with ``= help: remove it``.
- E0151 (new): "A local fn written as the last statement of its block (6.9)". Message ``local function `go` is never called`` with ``= help: call it after the declaration``.
- "E0143 and up" becomes "E0152 and up".

## Diagnostics

- [ ] E0316 extended to local fns, with its own message
- [ ] E0150: type parameters, attributes, `pub` and `suspend` on a local fn, one code with a message for each
- [ ] E0151: a local fn as the last statement of its block
- [ ] The existing self tail call lint (`[fn.loop]`, a self call in the target of escape or the body of with) also applies to local fns

## Compatibility

Additive. `fn name(` inside a block is a syntax error today (E0110 `expected }`), so no program that compiles now changes meaning. A doc comment inside a block attached to nothing before, and now attaches to a local fn that follows it.

## Implementation

Nothing has been prototyped. The outlook by crate:

- emela-syntax: `stmt` gains `local_fn`, decided by the token after `fn`. The parser reuses the parameter list and signature of `fn_decl`. It reports type parameters, attributes, `pub` and `suspend` with E0150 and a local fn at the end of a block with E0151, and attaches a preceding `##` to the local fn.
- emela-resolve: the declaration adds the name to the value scope of the block before its body is resolved (so that the body sees it) and keeps it until the end of the block. The body is resolved with the scope visible at the declaration, as for a lambda. Type names resolve in the scope of the enclosing declaration.
- emela-types: check the local fn at its position, binding its name to a monomorphic function type before checking the body (the parameter types are known; the return type, E and R are fresh variables unless written). Enforce the written return type, `fails` and `uses` as for a top-level fn (E0318, E0402). Enclose E and R in the type as for a lambda. The capture set (`[effect.escape]`) is computed as for a lambda.
- emela-core: lower each local fn to a module-level Core function by lambda lifting. The captured variables become leading parameters, and the type parameters of the enclosing fn become its type parameters. Every captured variable is bound before the declaration and immutable, so each call inside the scope can pass them. A call by name becomes a direct call with the captures prepended, a self tail call becomes a direct self tail call, and the existing loopification (`crates/emela-core/src/tail.rs`) gives the guarantee of `[fn.loop]` with no change. A use of the name as a value becomes a closure that passes the captures. Capabilities captured by the local fn are passed as Core parameters (an internal representation; E0408 is a rule of the source language and does not apply).
- Code generation: after lifting, a local fn is an ordinary Core function, and a local fn used as a value is an ordinary closure. Every backend consumes Core or LIR made from Core, so no backend needs new constructs. To check: the handler passing of `use X` inside a lifted local fn follows the path of lambdas, and a lifted local fn that reaches a suspend operation goes through the suspend state machine as a top-level function does.
- emela-fmt: format a local fn with the layout of a top-level fn, indented in the block, with its doc comment. Blank lines around it follow the rules for other statements.
- emela-lsp: hover shows the type of the local fn and its doc comment. Go to definition, references and rename are scoped to the block. Document symbols list local fns as children of the enclosing function. Completion offers the name only after the declaration and inside the body.
- Bundled core and std: the helpers listed in Motivation can move inside their functions in a separate change after implementation. That change is not part of this proposal.

## Alternatives

- Let an anonymous function refer to itself (a name for the lambda, or a `self`-like keyword). Rejected: `closure-syntax` keeps anonymous functions without names. A name, named arguments and the doc comment belong to the form that has a name.
- A recursive binding of a lambda (`rec go = |n| ...`). Rejected: it adds a keyword to write what `fn go(n: Int)` already says, and leaves the parameters without names for named arguments.
- Local fns that do not capture. Rejected: the arguments passed unchanged on every iteration (`from` of `range`, `f` of `filter_map`) are half of the motivation, and values are immutable, so capturing is safe.
- Hoist local fns, or let consecutive local fns see each other, for mutual recursion. Rejected: bindings are visible only after them (6.3), and tail calls between mutually recursive functions are not guaranteed (6.8). Mutual recursion stays at the top level.
- Generalize local fns. Rejected: only top-level functions and consts are polymorphic (6.3). The type parameters of the enclosing fn cover the cases in Motivation.
- A clause after the body of a fn that holds its helpers (a `where` block). Rejected: it adds a second place for declarations with its own scope rules, while a statement of the block reuses the rules of bindings.
- Keep private top-level fns. Rejected for the reasons in Motivation.

## Prior art

- Local recursive function bindings that capture their environment: nested functions in Kotlin, Swift, Scala and Python, `let rec` in an expression in OCaml and Standard ML, local definitions in Haskell (`where`, `let`).
- Nested functions that do not capture their environment: nested `fn` items in Rust.
- Lambda lifting (Johnsson, 1985) turns captured variables into parameters, so a local function becomes a top-level one.
- Monomorphic local bindings without let-generalization: "Let Should Not Be Generalised" (Vytiniotis, Peyton Jones, Schrijvers, 2010).

## Open questions

## Decision

A recursion with an accumulator or an index is written today as a public fn and a private helper that only it calls, and the bundled core and std have 16 such pairs. The helper is visible to the whole module, needs a name of its own, cannot capture the arguments of its caller, and repeats its type parameters. A local fn puts the helper where it is used: it captures the variables visible at its declaration, can use the type parameters of the enclosing fn, and its self tail call keeps the guarantee of `[fn.loop]`. It is a binding, so it is visible only after it, is not generalized, and follows the shadowing rules of bindings without warnings; mutual recursion and generalization stay at the top level. Anonymous functions stay without names, as `closure-syntax` decided, and a local fn is the form that has a name: named arguments, a doc comment shown on hover, and calls to itself. Both forms stay, and no lint prefers one. A local fn as the last statement of its block can never be called, so it is an error (E0151), as an unused anonymous function statement is (E0149). Type parameters, attributes, `pub` and `suspend` on a local fn share one code (E0150). It lowers by lambda lifting to an ordinary Core function, so no backend needs new constructs.
