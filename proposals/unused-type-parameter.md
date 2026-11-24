---
title: Warn about type parameters of a fn that its signature does not use
authors: [Alliana]
status: implemented
issue:
touches: [fn.decl, trait.decl, ext.decl, diag.list, lsp.code-action]
requires: []
related: []
pr: 806
decided: 2026-11-23
---

# Warn about type parameters of a fn that its signature does not use

## Summary

When a type parameter of a top-level fn, a trait function or an `@external` fn does not appear in the type of the fn (its parameters, return type, fails and uses), the compiler warns (W0305), and the language server offers a quick fix that removes it. A type parameter that appears only in type annotations inside the body is warned about too, because no call can determine it. Type parameters of types, enums and handlers are out of scope.

## Motivation

In 0.28.1 all of the following pass with no error and no warning.

```emel
fn f[A]() -> Int { 1 }

fn g[A](x: Int) -> Int {
  y: Option[A] = None
  x
}

fn h[A: Show]() -> Int { 1 }

fn e[E, R](x: Int) -> Int { x }

fn main() {
  dbg(f())
  dbg(g(1))
  dbg(h())
  dbg(e(1))
  ()
}
```

- A caller has no syntax for writing type arguments. Type parameters are determined only from the types of the arguments and the result, so a type parameter that does not appear in the type of the fn stays undetermined at every call, and the call is accepted.
- The writer may believe that A distinguishes something, but it distinguishes nothing, and nothing points out the mistake.
- A bound does not change this, as in `h`: a bound does not determine A.

### What the current specification already allows

- Removing the type parameter is enough. The warning only points it out; it does not make anything expressible that was not.

## Proposal

### Meaning

- When a type parameter A of a fn that declares type parameters (a top-level fn, a trait function, an `@external` fn) does not appear anywhere in the type of the fn, the compiler warns (W0305).
- The type of the fn is the parameter types, the return type, the fails clause and the uses clause. For a fn that omits the return type, fails or uses, the inferred one is used (in `fn f[A](x: Int) { none: Option[A] = None; none }` the return type is inferred as `Option[A]`, so A is used). Appearing inside a function type of a parameter (`f: fn() -> A`) and in a type argument of a named type (`List[A]`, `Handler[E, R]`) counts.
- The following do not count as appearing:
  - type annotations in the body (`y: Option[A] = None`)
  - bounds of type parameters (`[A: Show]`, `[R: Immediate]`)
- Ordinary type parameters and set type parameters (E, R) are treated alike.
- The warning points at A in the list of type parameters.

### Scope

| Declaration | Treatment |
|---|---|
| A top-level fn | In scope |
| Type parameters that a trait function declares itself (`fn describe[A](self) -> String`) | In scope. The warning is reported once, at the trait declaration, not at the impls |
| An `@external` fn | In scope. Type parameters do not exist at run time, so they distinguish nothing on the host side either |
| Type parameters of an impl head (`impl Box[A]: Show`) | Out of scope. They always appear in the head type `Box[A]` (and `[trait.impl]` does not allow `impl Pair[A, A]`) |
| A fn in an impl | Cannot declare type parameters (`[trait.impl]`) |
| A local fn | Cannot declare type parameters (E0150) |
| Operations of built-in effects (`fork[A]`) | Out of scope. Only bundled declarations can write them (E0426) |
| A type or an enum | Out of scope (see Alternatives) |
| A handler | Out of scope. The draft `generic-handler` handles it with E0462 |

### Example

```emel
fn f[A]() -> Int { 1 }
```

```
warning[W0305]: type parameter `A` of `f` does not appear in its signature
   ╭─[ src/main.emel:1:6 ]
   │
 1 │ fn f[A]() -> Int { 1 }
   │      ┬
   │      ╰──
───╯
   = help: a call cannot determine `A`; remove it, or use it in a parameter, the return type, `fails` or `uses`
```

### Language server

- W0305 has a quick fix that removes the type parameter, together with its bound. When the list becomes empty, the brackets are removed too.
- It is not offered for a type parameter that also appears in a type annotation in the body, because removing it would make the body E0211.

## Diagnostics

- [x] W0305: new. A type warning, so it is in W03xx (`[diag.bands]`). W0305 is not used in `[diag.list]`, `proposals/`, the local drafts or `crates/`.

## Compatibility

- Only a warning is added, so every program that compiles now still compiles, with the same meaning.
- No fn in the bundled core and std, the examples or the tests is affected (checked by scanning the `.emel` files). The only matches are two in `crates/emela-fmt/test_data/` (B in `generics[A: Ord + Show, B, R: Immediate]` and in `deferred[A,B]`), which are formatter test data and are not type checked.
- Removing an unused type parameter from a pub fn does not break its callers, because callers have no syntax for writing type arguments. The version rules of Appendix B are unchanged.

## Implementation

- `emela-types`: after the type of a fn is determined (after inference, for the omitted parts), check each declared type parameter for an occurrence in that type. Trait functions and `@external` fns go through the same path. The diagnostic is in `crates/emela-types/src/diagnostic.rs`, and the code in `crates/emela-driver/src/code.rs`.
- `emela-lsp`: the quick fix for W0305. Whether the type parameter appears in the body is decided from the result of type checking.
- There is no prototype.

## Alternatives

### Warn about type parameters of types and enums too

- Not chosen. A type parameter of a type or an enum appears in the name of the type (`Id[User]`), and unification compares type arguments, so it distinguishes values even when no field uses it (a phantom type). `type Tagged[A](id: Int)` makes `Tagged[User]` and `Tagged[Order]` different types.
- For the same reason the bundled `Array[A]`, `Deferred[A]`, `Queue[A]` and `Fiber[A]` (`@external` or `@intrinsic` types without fields) are out of scope.

### Make a call that does not determine a type parameter a compile error

- Report at the call that `f()` does not determine A, as E0333 does for Self.
- Not chosen. A is used by nothing, so leaving it undetermined does not change what the program does. The problem is in the declaration, and pointing at the declaration shows how to fix it.

### Make it a rule of the linter

- The draft `linter` moves warnings about things that are plainly pointless from the compiler to rules of the linter, and W0305 is of that kind.
- W0305 is added as a compiler warning without waiting for the linter. If the linter is adopted, it is a candidate to move together with the other warnings.

### Make the declaration a compile error

- Not chosen. It breaks programs that compile now. The declaration is pointless, but its types and behavior are correct.

## Prior art

- Detecting type parameters that do not appear in a type (ambiguous type variables, unused type parameters). In languages with explicit type application, a type parameter that does not appear in the type can still be given at the call and has a meaning. Emela has no explicit type application, so such a type parameter is always pointless.
- Phantom types: a type parameter of a nominal type distinguishes types even when no field uses it.

## Open questions

## Decision

A type parameter of a fn that does not appear in its type is never determined by any call, because callers have no syntax for writing type arguments. It distinguishes nothing, and nothing tells the writer so. A warning at the declaration points at the place to fix.

Type parameters of types and enums stay out of scope: they appear in the name of the type and unification compares them, so a phantom type parameter is legitimate. Handlers are left to `generic-handler`.

Trait functions and `@external` fns are included for the same reason, and an inferred return type, fails or uses counts as part of the type. The quick fix makes the warning cheap to act on. It goes in as a compiler warning now rather than waiting for the linter; if the linter is adopted, it can move with the other warnings.
