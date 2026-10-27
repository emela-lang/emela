---
title: Method calls
authors: [Alliana]
status: implemented
pr: 454
issue:
decided: 2026-10-27
touches: [lex.name, data.type, type.rules, trait.call, module.resolution, api.list, api.string, api.std.http.request, api.std.http.response, api.std.http, lsp.completion, lsp.navigation, diag.list]
requires: []
related: [http-service]
---

# Method calls

## Summary

`x.f(a)` can be written for `M.f(x, a)`, where M is the module that defines the type of x and f is a function of that module whose first parameter has that type at its head. The functions of a trait that the type implements can be called the same way, when the trait is visible by name. Functions are not marked.

`String.join` moves to `List.join`, and the functions `Request.headers` and `Response.headers` are renamed to `header_values`.

## Motivation

- A chain of operations on the same type is read from the inside out. std, examples and x (about 5600 lines in the current syntax) have 30 places of the form `Mod.f(Mod.g(...))`.

  ```emel
  # examples/http
  Option.or(List.head(String.split(res.body, "\n")), "")
  ```

- `|>` reorders them, but the module name is written at every step. Of the 15 chains of `|>` with two or more steps, 8 stay within one module.
- There are 312 calls of a function whose first parameter has the type of its module (List 129, String 50, Option 33, `Number` of x/dynamic 32, Int 23, Map 18). All of them can be written as method calls.
- The other 166 calls are mostly functions that make values (`Map.from_list` 24, `Number.from_int` 18, `Map.empty` 14, `Response.new` 10, `List.range` 10). A rule based on the type of the first parameter leaves them out without marking anything.
- The only workaround in the current language is `|>`, and it cannot leave out the module name.

## Proposal

### Syntax

The grammar does not change. `postfix = primary { "(" [ args ] ")" | "." lower_name }` already reads `x.f(a)`. Today it is read as a call of an operation when x is a capability value, and otherwise as a call of a field of function type.

```emel
fn first_line(res: Response) -> String {
  res.body.split("\n").head().or("")
}

fn is_field_value(value: String) -> Bool {
  value.code_points().all(fn(c) { c == 0x09 || (c >= 0x20 && c <= 0x7E) })
}

fn labels(users: List[User]) -> String {
  users.map(fn(u) { label(u) }).join(", ")
}

fn sum_lengths(words: List[String]) -> Int {
  words.fold(from: 0, f: fn(total, w) { total + w.length() })
}
```

The current compiler stops these with E0313 (``type `String` has no fields``).

### Meaning

- When the type of x has a named type T at its head, `x.f(a1, ..., an)` looks up f in this order:
  1. The field f of T (the current rule; a field of function type is called).
  2. The fns f of the module that defines T whose first parameter has T at the head of its type. Outside that module, only pub fns.
  3. The functions f that take `self` of the traits that T implements, among the traits that are visible by name in the calling module: the traits of the Prelude (Eq, Ord, Show), the traits imported by name (`import L.{Seq}`), and the traits defined in the calling module. A trait that can only be written qualified (`L.Seq` after `import L`) is not looked up.
- A function found is called as `M.f(x, a1, ..., an)` (or `Trait.f(x, a1, ..., an)`), with the same named arguments, order of evaluation (x first), E and R.
- Step 2 does not look at the scope. `user.rename("a")` can be called without importing the module of User, as a field access can. Only the owner of T can add functions to that module, so a new dependency cannot add candidates to step 2.
- Step 3 looks only at visible traits because an impl can be written in the module of the trait (`[trait.impl]`). If every trait in the program were looked up, a dependency that declares `trait Seq { fn map(self, ...) }` with `impl List: Seq` would make every `xs.map(f)` in the program ambiguous, also in modules that never import it.
- When the type of x is a type parameter A, only the functions of the bounds of A are looked up (step 3). A bound names the trait, so it is visible.
- Only the head of the type needs to be known, not its type arguments. `List.join` is found for `[1, 2].join(", ")` as `List.join(xs: List[String], ...)`, and the call is an ordinary mismatch of the argument type.
- A capability value (the value of `use X`) is not a named type, so `log.info(...)` stays a call of an operation.
- `x.f` written without a call, where f is not a field but a function found by step 2 or 3, is E0312 with ``= help: call it as `x.f(...)` ``. It is not a partially applied function value. A function value that fixes x is written as a lambda (`fn(f) { users.map(f) }`).
- `|>` does not change. `xs |> List.map(f)` can still be written. `emela fmt` does not rewrite one form into the other.

### Declarations

- Declaring, in the module of T, an fn whose first parameter has T at its head and whose name is the name of a field of T, is a compile error (E0334). So steps 1 and 2 are never both found at a call.
- When steps 1 and 3, steps 2 and 3, or two or more traits in step 3 are found at the same time, it is a compile error at the call (E0335). Writing `T.f(x)` or `Trait.f(x)` avoids it.

### Library

- Add `List.join(xs: List[String], sep: String) -> String`. `String.join` becomes `@deprecated` and remains for one minor version.
- `String.concat(parts: List[String])` stays in String and is not moved. It makes a String from parts, like the functions that make values, and is not a method. At the end of a chain, `.join("")` does the same.
- Rename `Request.headers(req, name)` and `Response.headers(res, name)` to `header_values`. They collide with the field `headers` (E0334), so the old names cannot remain. This is a breaking change without a deprecation period.
- `Std.Http.headers` (`@deprecated`) is in the module `Std.Http`, so E0334 does not apply. Only the name it points to changes to `Response.header_values`.
- Inside std, the private field `raise` of `Cause` collides with the function `Cause.raise`. The field is renamed (it is not part of the API).

### Typing rules

- The type of `x.f(a)` is the type of the call `M.f(x, a)` of the function found.
- If the head of the type of x is not determined when checking reaches the call, it is E0314, under the same condition as a field access.

## Specification changes

### `[lex.name]`

Replace the second bullet after "From these rules, the meaning of `.` is clear from its left-hand side." with:

> - Lowercase name or uppercase name on the left: access to a value (a field, an operation of a capability obtained with `use`, or a method call, `[type.rules]`). Examples: `user.name`, `log.info`, `DEFAULT_CONFIG.port`, `xs.map(f)`

### `[data.type]`

After "Construction is written `User(id: 1, name: "a")`, and field access is written `user.name`.", add:

> - A function of the module that defines a type, whose first parameter has that type, can also be called as `user.f(...)` (`[type.rules]`). Such a function MUST NOT have the name of a field of that type (E0334).

### `[type.rules]`

Add a row after `f(a)`:

> | Expression | T | E | R |
> |---|---|---|---|
> | `x.f(a)` (method call) | The return type of the function found, called as `M.f(x, a)` | Ex ∪ Ef ∪ Ea | Rx ∪ Rf ∪ Ra |

After the bullet on field access, add:

> - A method call `x.f(a1, ..., an)` applies when the type of x has a named type T at its head and x is not a capability value. f is looked up among: (1) the fields of T; (2) the fns of the module that defines T whose first parameter has T at the head of its type (only pub fns outside that module), without looking at the scope; and (3) the functions that take `self` of the traits that T implements, among the traits visible by name in the calling module (the traits of the Prelude, the traits imported by name in braces, and the traits defined in that module; a trait reached only as `L.Trait` is not looked up). When x has a type parameter A, only the functions of the bounds of A are looked up. A field is called as a value of function type. A function is called as `M.f(x, a1, ..., an)`, with the same named arguments, order of evaluation, E and R. Only the head of the type of x is needed: the type arguments are checked as for the first argument of that call.
> - When more than one candidate is found, it is a compile error (E0335). A field and a function of the module cannot both be found, because declaring them is E0334. When none is found, it is E0312 (E0313 for a type without fields).
> - `x.f` without a call, where f is a function found by (2) or (3) and not a field, is E0312 with ``= help: call it as `x.f(...)` ``. It does not make a function value.
> - The head of the type of x MUST already be determined when checking reaches the method call (E0314), as for a field access.

### `[trait.call]`

Replace "A trait function is called in the form `Trait.function(value)`. There is no method call syntax such as `user.show()`." with:

> - A trait function is called in the form `Trait.function(value)`. A function that takes `self` can also be called as `value.function()` when the trait is visible by name in the calling module (`[type.rules]`).

### `[module.resolution]`

After "On the left of `.`, only modules and Traits are looked up.", add:

> - A method call (`x.f(...)`, `[type.rules]`) looks up the functions of the module that defines the type of x without looking at the scope, so that module does not need to be imported. The functions of traits are looked up only in the traits that can be written unqualified in the calling module.

### `[api.list]`

Add:

> | Function | Signature | Implementation | Description |
> |---|---|---|---|
> | `join` | `join(xs: List[String], sep: String) -> String` | intrinsic | Joins with `sep` in between. Formerly `String.join` |

### `[api.string]`

Replace the row of `join` with:

> | Function | Signature | Implementation | Description |
> |---|---|---|---|
> | `join` | `join(parts: List[String], sep: String) -> String` | Emela | `@deprecated`, remains for one minor version. `List.join(parts, sep)` |

### `[api.std.http.request]`

Replace the row of `headers` with:

> | Declaration | Contents | Description |
> |---|---|---|
> | `header_values` | `header_values(req: Request, name: String) -> List[String]` | All values whose name matches. Formerly `headers`, renamed because the field `headers` has the same name (E0334) |

### `[api.std.http.response]`

Replace the row of `headers` with:

> | Declaration | Contents | Description |
> |---|---|---|
> | `header_values` | `header_values(res: Response, name: String) -> List[String]` | All values whose name matches. Moved from `Std.Http`. Formerly `headers`, renamed because the field `headers` has the same name (E0334) |

### `[api.std.http]`

Replace `Response.headers` in the list of new places of the deprecated functions with `Response.header_values`.

### `[lsp.completion]`

Replace "After `.`, otherwise: ..." with:

> - After `.`, otherwise: if the type of the left expression is a capability value, the operations of the effect. If its head is a named type, its fields (the description is the text of the written type), then the functions that can be called as methods (`[type.rules]`; kind Method, the first parameter left out of the signature). For other types, empty.

### `[lsp.navigation]`

Add:

> - A method call is a reference of the function it calls. Go to definition, references and rename treat `x.f(...)` as `M.f(x, ...)`.

### `[diag.list]`

Replace the rows of E0312, E0313 and E0314 with:

> | E0312 | No field or function with that name for a method call or a field access, or a function found by a method lookup written without a call (`[type.rules]`). For the latter, the help is ``= help: call it as `x.f(...)` `` | ``no field or function `nmae` for type `User` `` |
> | E0313 | A field access or a method call on a type without fields, where no function is found | ``type `Int` has no field or function `nmae` `` |
> | E0314 | A field access or a method call on a value whose type is not determined. The help is ``= help: add a type annotation`` | ``the type of this value must be known to access field `name` `` / ``the type of this value must be known to call `f` `` |

Add after E0333:

> | E0334 | In the module of a type, an fn whose first parameter has that type at its head has the name of a field of that type (`[data.type]`). Points at the name of the fn | ``function `headers` has the same name as a field of `Request` `` |
> | E0335 | More than one candidate for a method call (`[type.rules]`). The help names the qualified forms | ``ambiguous method `show` for `User`: `User.show` and `Show.show` `` with ``= help: write `User.show(x)` or `Show.show(x)` `` |

## Diagnostics

- [x] E0312: widened to method calls and to a function written without a call. ``no field or function `nmae` for type `User` ``
- [x] E0313: widened to method calls. ``type `Int` has no field or function `nmae` ``
- [x] E0314: widened to method calls. ``the type of this value must be known to call `f` ``
- [x] E0334 (new): a fn with the name of a field of the type of its first parameter, in the module of that type.
- [x] E0335 (new): more than one candidate for a method call.

## Compatibility

- Every current form (`M.f(x)`, `x |> M.f`) still compiles with the same meaning.
- Some programs that are E0312 or E0313 now compile.
- Programs with an fn, in the module of a type, whose name is a field of that type and whose first parameter has that type, stop compiling (E0334). In std, examples and x, these are `Request.headers`, `Response.headers` and the private field `raise` of `Cause`.
- Renaming `Request.headers` and `Response.headers` is a breaking change with no deprecation period.

## Implementation

- emela-types: add the method lookup to the check of field access. It reads the table of functions of the module that defines T, the table of impls of T, and the traits visible in the calling module.
- emela-resolve: report E0334 in the check of declarations.
- emela-core: add `List.join` and deprecate `String.join`. Rename to `header_values`. Rename the field of `Cause`.
- emela-lsp: completion after `.`, go to definition, references and rename.
- A method call is lowered to the same form as `M.f(x, a)`, so the code generators do not change.

## Alternatives

- Only functions that write `self` are methods. The name `self` of a parameter would take a special meaning. Deciding by the type of the first parameter already leaves out the functions that make values, so no mark is needed.
- Requiring the caller to import the module of the type. It would not match field access, and which functions can be called would not be decided by the type alone.
- Looking up every trait that T implements, regardless of the scope. A dependency that declares a trait and implements it for List or String in its own module would make method calls ambiguous in modules that never import it.
- Importing or hiding impls (`pub impl`). For one pair of a type and a trait there is one impl in the program (E0330), so importing the trait already decides which impl is meant. Hiding an impl in some modules would make bounds, `==` and interpolation depend on the module, while values flow between modules.
- Telling fields and methods apart by the form of the call (`x.f` for a field, `x.f(...)` for a function). A field of function type is already called as `x.f(...)`, so the distinction does not hold.
- `x.f` without a call as a partially applied function value. The same form would take a field out or make a closure depending on the declaration of f, and named arguments, E and R of the closure would need rules of their own. It is E0312 now, so it can be added later without breaking programs.
- Moving `String.concat` to `List.concat`, or deprecating it. `List.concat` reads as joining lists, which `List.flatten` does. `String.concat` makes a String and is not wanted at the end of a chain.
- Keeping `Request.headers` and `Response.headers` deprecated for one minor version, by lowering E0334 to a warning for `@deprecated` functions and preferring the field in method calls. It adds a rule for a short period before 1.0.
- Other names for `header_values`: `all_headers` reads as all the pairs, the same as the field `headers`; `headers_named` has no counterpart in std. `header` and `header_values` read as the first value and all the values.
- Only `|>`. The module name is written at every step, and nested calls stay hard to read.
- A lint that prefers `|>` or method calls. There is no linter in the specification yet, and which form fits which code is known only after method calls are used. It is left to a later decision.

## Prior art

- Uniform function call syntax, which puts the first argument on the left of `.` (D, Nim). These look up the functions in the scope.
- Looking up functions from the module that defines the type (inherent impls in Rust, method sets in Go).
- Extension functions that the caller imports (Kotlin), and trait methods that need the trait in scope (Rust).

## Open questions

## Decision

Chains of calls on one type were read from the inside out, and `|>` still repeats the module name at every step. Looking up functions by the type of the first argument needs no mark on functions and leaves out the ones that make values. The functions of the type's module are found without imports, as fields are, because only the owner of the type can add them. Trait functions are found only from traits visible by name, so a new dependency cannot make existing method calls ambiguous. `x.f` without a call stays an error, which keeps partial application open for later. The rename to `header_values` breaks code, but before 1.0 that is better than a rule kept only for a deprecation period.
