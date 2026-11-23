---
title: "Capabilities are told apart in types and in R"
authors: [Alliana]
status: accepted
pr: 732
issue: 719
decided: 2026-11-23
touches: [intro.experimental, lsp.hover, ext.boundary, glossary.terms, effect.use, effect.with, effect.handler, effect.escape, fiber.scope, fiber.escape, fiber.deferred, fiber.semaphore, fiber.queue, api.std.fiber, type.rules, trait.builtin, open.next, diag.list]
requires: [effect-alias]
related: [closure-r-by-caller, capability-type, capability-trait, handler-member, handler-init-arguments, capture-checking, scope-handle-values]
---

# Capabilities are told apart in types and in R

## Summary

This proposal follows `effect-alias` (#731) and `closure-r-by-caller` (#736), which #791 implemented for 0.29.

- A capability is a handler after its init has run and before its release: an instance of a handler. Each item of a with makes one.
- The x of `with x = Item` is the capability itself. Type checking tells capabilities apart, in types and in R.
- Calling an operation of a capability adds that capability to R (x for `x.read()`). A capability type records a set of capabilities, so capabilities of different withs can be put in the same list. The elements of R are effects, declared labels and capabilities.
- One principle: a capability cannot leave the with that made it. A value whose type or R contains the capability cannot leave the with, and type checking sees it. Whatever path a value takes, the capability stays in its type, so there are no paths to enumerate.
- A capability type is still written `use E` in source. A parameter `x: use E` receives a capability that the caller passes.
- A capability is not widened to its effect.
- A capability is still an ordinary value. Inside the same with, it can be passed, stored, returned, put in a data structure and captured by a closure, as any other value.
- Capture checking (capture sets and roots) is no longer needed. The experimental feature `capture-checking` is removed without being made stable.
- Everything lands in 0.29, with #791. `use E` written as an expression (W0403 since #791) becomes a compile error in 0.30.

## Motivation

### What remains after #791

After #791, calling operations without naming a handler is protected by the types (R) alone.

- `x <- E` and `(use E).op(...)` make no root; E stays in the R of a closure and the caller provides it (#731).
- The R of a closure or a local fn is provided only by its caller; enclosing withs do not pass handlers (#736). Reproductions 2 to 5 of #719, and a closure passed to a child of a scope (reproduction 3), now reach the outer handler.
- std, examples and tests/laws no longer contain `use E` written as an expression.

Fixed values are still protected only by capture checking.

- The capability of a named item (x of `with x = H`) and `use E` written as a value (W0403). When a closure captures one, the R in the type of the closure contains the name of the effect (widening), and the types stop nothing.
- The paths out of a with are enumerated by capture sets. Arguments of operations and errors (A and B of #719) are still not seen. #736 keeps effects out of the R of host callbacks, but a callback that captures a capability (`set_timeout(10, || g.greet("later"))`) can still outlive the with.
- #701 (calling an operation of a handler member's capability received as a closure parameter or a return value leaves the effect in R, E0402) remains.
- Taking the handles of fibers, Deferreds and Queues out (reproductions 1 and 6 of #719) is also left to capture sets and checks at run time.
- #666 planned to make capture checking the default in 0.29 and remove W0402 and the checks at run time. This proposal replaces it in 0.29.

### Types do not say which capability

- Today a capability type is `use ReadFile`; which with's handler it is does not appear in the type (`capability-type`). An operation of a capability adds the name of the effect to R (`[effect.use]`).
- Which handler it is, is followed by capture sets, separately from types (`[effect.escape]`). So every path out of a with has to be counted.
  - Today only the value of the body is seen (#719). Arguments of operations, errors and function boundaries leak.
  - Host callbacks and Deferred / Queue are undecided (#666).
- Since R contains the name of the effect, an operation of a handler member's capability cannot be removed by the member's with (#701). Putting opened resources in a list and calling `map` on it gives E0402.
- Since a capability in R is widened to the name of its effect, the types alone stop nothing (this is why reproductions 2 to 5 of #719 pass as `uses { Greeter }`).

### Telling them apart lets the types keep the principle

- A capability lives only inside the body of its with. Types and R that contain the capability cannot be written outside the body.
- Types fixed outside (the type argument of an outer `State`, the parameter types of an outer handler's operations, error types, type arguments of functions) cannot contain the capability. Every path out is a type error.
- R contains capabilities too, so their with can remove them (#701 is fixed).

### The principle is unchanged

- "A capability cannot leak" is the principle of `[effect.escape]` today. Only how it is kept changes: types keep it instead of capture sets.
- The motivation of `capability-type` (passing, storing and returning capabilities) stays. A capability is an ordinary value.

## Proposal

### Terms

| Term | What it is | In source |
|---|---|---|
| handler | A declaration (`handler Plain: Greeter`), as today | `Plain` |
| capability | A handler after its init has run and before its release. Each item of a with makes one, and each `Fiber.scope` makes one capability of Scope. It is a value | x of `with x = Item`, a parameter x |
| capability type | The type of a capability. Written `use E`; type checking tells which capabilities | `use ReadFile` |
| capability parameter | A parameter whose type is `use E`. Which capabilities is decided at each call | `fn copy(from: use Db)` |
| capability-bound type | A type whose values live only while a capability of E does. Declared with `uses E`. Its values are not capabilities | `pub type Fiber[A] uses Scope` |
| borrowed callback | A callback passed to a `@borrowed` parameter of an external function. The host holds it only during the call | `@borrowed compare: fn(Int, Int) -> Int` |
| named item | `with x = Item`. Names the capability of the item x | `with f = Files.Open(p)` |
| provide by name | A with removing the name of an effect from R. The capability of an item without a name does only this | `with Plain { ... }` |
| leak | A value whose type or R contains a capability leaves the with that made it. A compile error | |
| widen | Treating a capability in R as the name of its effect. A compile error with this proposal | |
| effect alias | x of `x <- E` (#731). Not a capability; it puts E itself in R | `log <- Logger` |

- In Japanese text, capability, capability type, capability parameter, capability-bound type and borrowed callback are written in English.
- The elements of R are effects, declared labels and capabilities.
- Not capabilities: an effect alias (a name for calling the E provided by name), a handler declaration, and L of `L by P` (a declared label).
- No longer used: the roots and holding of capture checking, for capabilities and for handles.

### Capabilities

- An item of a with runs its init and makes a capability. Leaving the with releases it, and the capability ends.
- `with x = Item` names that capability x. The same holds for a handler member (`with f = Files.Open(path)`).
- The capability of an item without a name (`with Plain { ... }`) only provides by name. `x <- Greeter` and `(use Greeter).op()` in the body put Greeter in R, and the with removes it.
- A capability can be written on the right of `L by x` (plugging x into L).

```emel
with primary = PrimaryDb {
  with replica = ReplicaDb {
    copy(replica, primary)
    primary.execute("...")
  }
}

with f = Files.Open("a.txt") {
  lines = read_all(f)
  [f].map(|x| x.read(10)) # the R of the closure is f; this with removes it
}
```

### R and types

- Calling an operation of a capability x adds x to R. The with that made x removes x from R.
- A value of type `use E` records, inside type checking, which capabilities it is. The element type of `[f]` is the capability f.
- A capability is not widened to the name of its effect. A closure whose R contains f does not fit a function type written `uses { ReadFile }`.
- If the type of a value that a closure captures contains capabilities (including through capability-bound types), those capabilities are in the R of the closure. This is what capture sets did, folded into R: nothing is recorded on values, and it appears only in the type of the closure.

```emel
Fiber.scope(|s| {
  f = s.fork(|| 1)
  st.set(|| Fiber.join(f)) # the R of the closure is s; it does not fit the type of the outer State
})
```

### Sets of capabilities

- A capability type records a set of capabilities, not one. In source it is still `use E` (and `use E1 + ... + En`).
- Capabilities made by different withs can be put in the same list. The element type of `[x, y]` is "a capability of E that is x or y". Unification takes the union of sets, as for R; there is no general subtyping.
- Calling an operation of a value with a set adds the whole set to R (x and y for an element of `[x, y]`). Every capability is required, so that whichever comes is provided.
- The with of x and the with of y each remove theirs. `[x, y]` can be used only where both are alive, and the principle holds as is.
- Only precision is lost: the type does not say which capability each element is, which does not affect safety.

```emel
with x = Counters.Open(1), y = Counters.Open(10) {
  ns = [x, y].map(|c| c.next()) # the R of the closure is x and y; this with removes both
}
```

### Capability parameters

- A parameter of type `use E` gives the function a type parameter that ranges over sets of capabilities of E. `x.read()` inside the function adds that type parameter to R.
- At a call, the type parameter is decided by the argument. `read_all(f)` adds f to the caller's R; passing an element of `[x, y]` adds x and y.
- Two parameters of the same effect give two type parameters (`fn copy(from: use Db, to: use Db)`).
- The result of a function can be a capability type. The set of the result of `fn pick(primary: use Db, replica: use Db, write: Bool) -> use Db` is the union of those of the two parameters.
- Only capabilities of named items, and values that contain them, can be passed to a capability parameter. An effect alias cannot be an argument (E0453, as today).
- A function that wants the innermost E does not take a capability; it has E in its R (`uses { Db }`) and writes `db <- Db` inside. What is decided at the call is R; a fixed handler is a named item.
- During 0.29, a value of `use E` written as an expression (W0403) is a capability of the name of the effect E in types.

### Capability parameters in function types

- `use E` written as a parameter of a function type (`fn(use ReadFile) -> String`) is quantified at that function type: it is the type of a function that works for whatever set of capabilities of E it is given.
- A closure written where such a function type is expected is checked as receiving an unknown set C. C cannot be in its result. C may be in its R, and the with that owns the set removes it on the side that calls the closure.
- A closure without an expected type (`k = |fs| ...`) is not quantified. The set of its parameter is an ordinary set variable decided by its uses. A quantified type is never inferred; it is only checked against a written type.
- There is no syntax for naming the type parameter of a capability parameter (`[C: use Db]`). When two arguments should share a set, a closure that captures the capability is passed instead of the capability.

```emel
fn retry(attempts: Int, f: fn() -> ()) { ... }

with primary = PrimaryDb {
  retry(3, || primary.execute("...")) # the R of the closure is primary; this with removes it
}
```

### Resources opened a number of times decided at run time

- An item of a with makes one capability, so `paths.map(|p| Files.Open(p))` cannot be written. The withs are nested by recursion.
- A capability parameter is a type parameter over sets, so the type holds as the set grows at each recursive call (the signature has the type, so no polymorphic recursion has to be inferred).
- Lifetimes nest, and resources are closed in the reverse order of opening. Uses with one common lifetime are written this way.

```emel
fn concat_all(paths: List[String], acc: List[use ReadFile]) -> String {
  match paths {
    [] -> acc.map(|f| f.read_all()).join("")
    [p, ..rest] -> with f = Files.Open(p) { concat_all(rest, acc ++ [f]) }
  }
}

fn with_all[T](paths: List[String], acc: List[use ReadFile], k: fn(List[use ReadFile]) -> T) -> T {
  match paths {
    [] -> k(acc)
    [p, ..rest] -> with f = Files.Open(p) { with_all(rest, acc ++ [f], k) }
  }
}
```

- This proposal does not depend on regions (#651). Whether regions are needed for opening and closing at any time is decided in #651. Under this proposal, a pool can also return ids of a capability-bound type, so that the types stop them from leaving the pool's with.

### Capability-bound types

- A type declaration can say that its values live only while a capability of E does: `pub type Fiber[A] uses Scope`. `uses` is the same contextual keyword as in signatures. This is different from `use E`, the type of a capability itself.
- When an operation of a capability c of E returns a value of such a type, the type of the value records c. A value returned by an operation called through the name of E (`x <- E`, `Fiber.fork`) records the name E.
- Such a value is treated as a value that contains the capability, so the principle applies as is.
- User types can use it too. With `pub type EnemyId uses Engine`, an id returned by a pool cannot leave the with of the pool.

### Withs that provide by name, and names of effects in types

- When an item without a name (`with Plain { ... }`) or `Fiber.scope` provides E by name, the leak check counts the name E in the types of values as its own capability.
- The Fiber returned by `Fiber.fork(f)` records the name Scope in its type. It cannot leave the scope that provided Scope by name.
- A value of `use E` written as an expression (0.29 only) leaves no with that provides E by name, for the same reason.

### `Fiber.scope`

- The Scope that `Fiber.scope` makes is a capability. The body receives it as a parameter. The parameter type is a quantified `use Scope[E, R]`, so the body cannot return a value that contains s. The body still has Scope in its R, so `Fiber.fork` works in it as today.

```emel
@intrinsic
pub fn scope[A, E, R](body: fn(use Scope[E, R]) -> A fails E uses { Scope[E, R], Async, R }) -> A
  fails E
  uses { Async, R }

@external
pub type Fiber[A] uses Scope
```

- `Fiber`, `Deferred`, `Semaphore` and `Queue` are capability-bound types of Scope. The Fiber of `s.fork(f)` records s.
- The operations of Scope keep their signatures. A child still has Scope in its R, provided by the scope it is forked into, as the signature of `fork` says.
- `Fiber.scope` has no special typing rules left. Running the children, interrupting them when the body returns, and propagating failures are done by the runtime (`@intrinsic`).
- Removed: E0425 (looking for `Fiber` and the like by type name), following handles as roots of capture sets, and the checks at run time for handles of a finished scope.
- Reproduction 1 of #719 (a Fiber in an outer State) is a type mismatch; reproduction 6 (a Fiber in an error) is a leak (E04xx).
- To fork into an outer scope, the parameter s of the outer scope's body is used (instead of `outer = use Scope` today).

### Fields and operations of handlers

- The set of a field of type `use E` (or of a capability-bound type) is decided when an item of a with runs the init. The users of `with users = PostgresUsers(db: replica)` is a handler whose field has the set {replica}.
- The type of users does not mention replica. The arguments of init are evaluated outside the with, so the with of replica encloses the with of users, and replica is alive while users is. The principle for users alone protects replica.
- A parameter of type `use E` of an operation is decided at each call, as for a function, and is an unknown set inside the implementation of the handler. The implementation cannot store a received capability in a field or in State (a type error). A of #719 (leaving through the arguments of an operation of a longer-lived handler) is closed by the types for capabilities and capability-bound types.
- The result type of an operation MUST NOT contain a capability type or a capability-bound type (E04xx). Returning a capability leads straight to a leak, and for an operation called by name it is not even decided which handler's capability it is. A capability is made and handed out by a handler member (`with c = Pool.Conn()`).

```emel
handler PostgresUsers: Users {
  init(db: use Db) { Self(db:) }
  fn find(id: Int) -> User { self.db.query("...") }
}

with replica = ReplicaDb {
  with users = PostgresUsers(db: replica) {
    users.find(1)
  }
}
```

### `self`

- `self` in an operation body and in release is counted as the capability of that handler. `self` is a capability with the added power of reading the fields.
- A closure that captures `self` has that capability in its R, by the rule for captured values. A closure returned by an operation that reads a field cannot leave the with (E04xx).
- `self` can be passed to a parameter of type `use E`. The receiver can call operations through it, as through any capability, but cannot read fields.

```emel
handler SystemFiles: Files {
  handler Open(handle: Handle): ReadFile {
    init(path: String) { Self(handle: host_open(path)) }
    release { host_close(self.handle) }
    fn reader(resume) { resume(|| host_read(self.handle)) } # the R of the closure is this handler's capability
  }
}
```

### The trait `Capability`

- `[C: Capability]` (`capability-trait`) works as it is. C is a type parameter of a top-level function, decided at each call to the capability type passed, with its set (passing f to `close[C: Capability](item: C)` makes C the capability f).
- The rule that quantifies `use E` in function types applies only where `use E` is written, not to `[C: Capability]`.
- A capability-bound type does not satisfy Capability. Its values are not capabilities, and no operations can be called on them.
- An ordinary type parameter (`[A]`) accepts a capability type too (`List[use ReadFile]`). Capability limits it to capabilities.

### Host callbacks

- A callback in the parameters or the result of an external function is, by default, one that the host may keep. The host may call it after the external function returns, so the R of the callback MUST NOT contain a capability either (E0454 extended to capabilities).
- A callback that is called only before the external function returns (a comparison for sorting, for example) is marked with the attribute `@borrowed` on the parameter. The host borrows the callback for the duration of the call and lets it go when it returns. Only a callback passed to a `@borrowed` parameter can have capabilities in its R. The writer of the declaration is responsible for the mark, as for the types of `@external`.
- `@borrowed` can be written only on a parameter of function type of an external function. Anywhere else (a result, a parameter of a function that is not external, a parameter that is not a function type) it is a compile error (E04xx).
- The types alone cannot tell the two apart: the set of a callback in the type of an external function is decided at each call, and no capability remains in the result, so the leak check cannot see it.
- The example of `closure-r-by-caller`, `with g = Plain { set_timeout(10, || g.greet("later")) }`, is an error. To use a handler from work that the host calls later, the work is completed on the host side, or handed to an Emela fiber through a Queue.

```emel
@external("js", "./array.js", "sortBy")
fn sort_by(items: Array[Int], @borrowed compare: fn(Int, Int) -> Int) -> Array[Int]

@external("js", "./timer.js", "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> () # the host may keep f; f cannot have capabilities in R
```

### Leaks

- When the type of a with is decided, if the type of the value of the body, the error type that leaves the body, or the R of the with contains a capability made by that with, it is a compile error (E04xx).
- A type fixed outside (the type argument of an outer `State`, the parameter types of the operations of a handler provided outside) cannot contain the capability at all. The assignment or the call does not type check (E0403 or a type mismatch).
- The same holds at function boundaries. A type parameter contains a capability only when the function is called inside its with.

### The language server

- Types are shown as in source (`use Counter`, `Fiber[Int]`). A line `capabilities: ...` below names which capabilities. The line `holds:` of capture checking is gone.
- A named item's capability is shown by its name, an item without a name by the name of its handler (`Plain`), the set of a parameter by the name of the parameter, and a parameter of a quantified function type as `any`.
- The `uses { ... }` of a closure or a function lists capabilities by the same names (`uses { f, Console }`).
- The messages of leaks use the same names.

```
ns: List[use Counter]
capabilities: x, y
```

### `use E` written as an expression

- The x of `x = use E` does not say which capability it is (it is fixed to the innermost capability of E where it is written, but that is not in the type).
- `use E` written as an expression, in either form, is W0403 since #791. With this proposal it is a capability of the name E in types during 0.29, and a compile error in 0.30. It is replaced by:
  - `x <- E`, when x is used only as a receiver in the same function (the quick fix of W0403 does this, #791);
  - a named item (`with x = H`), when it keeps referring to an outer handler, or is passed or stored. This cannot be done mechanically.

### Versions

This proposal lands in 0.29, with #791. 0.29 is released after this proposal is implemented.

- 0.29:
  - Capabilities are told apart in types and in R. Capability-bound types, capability parameters in function types, `self`, the fields and operations of handlers, host callbacks and `Fiber.scope(|s| ...)` come in.
  - The newly stopped forms (leaks, widening, capabilities in the results of operations, callbacks that capture capabilities) are compile errors from the start. There is no warning stage.
  - Capture checking (W0402, E0444, capture sets), E0425, and the checks at run time of `[effect.escape]` and `[fiber.escape]` are removed.
  - `use E` written as an expression is still read with W0403 (#731 keeps it for one minor version). In types it is a capability of the name E. A with that provides E by name counts the name E in types as its own capability, so a value of `x = use E` cannot leak either.
  - The rewrites are helped by the quick fix of W0403, and by a quick fix that turns `Fiber.scope(|| ...)` into `|s|`.
- 0.30: `use E` written as an expression is a compile error.
- #666 is closed, replaced by this proposal.
- Capture checking is no longer needed: capture sets followed only capabilities (including Scope) and handles, and types and R carry both now. The experimental feature `capture-checking` is not made stable; the checks are removed in 0.29. As `[intro.experimental]` says for a removed feature, its name is accepted with W0206 (no effect) for at least one minor version.

## Specification changes

### `[glossary.terms]`

- capability: a handler after its init has run and before its release. Each item of a with makes one, and each `Fiber.scope` makes one capability of Scope.
- The Japanese term of capability and capability value becomes "capability" (written in English in Japanese text). Remove hold.
- Add, with the Japanese term in English: capability type, capability parameter, capability-bound type (a type declared with `uses E`, whose values live only while a capability of E does), borrowed callback (a callback passed to a `@borrowed` parameter of an external function).
- Add named item, provide by name and widen, with their Japanese terms. Redefine leak: a value whose type or effect set contains a capability leaves the with that made the capability.

### `[effect.use]`

- Replace "Calling an operation of a capability value obtained by `use E` adds E to R. Calling an operation of a capability captured by a closure or a local fn adds that capability to R (`uses { primary }`). A capability in R can be widened to its effect ..." by:
  - Calling an operation of a capability adds that capability to R. The with that made the capability removes it from R.
  - A capability is not widened to its effect. An effect set that contains a capability is not included in an effect set that contains only the effect of the capability.
  - If the type of a value that a closure or a local fn captures contains capabilities, including through capability-bound types, those capabilities are in the R of the function.
- Replace "The type of a capability value of E is written `use E`. ..." by: The type of a capability of E is written `use E` (or `use E1 + ... + En`). Type checking records in it a set of capabilities. Unification takes the union of sets. Calling an operation of a value whose type has a set adds every capability of the set to R.
- A parameter of type `use E` gives the function a type parameter that ranges over sets of capabilities of E, decided at each call by the argument. An effect alias cannot be an argument (E0453). The result of a function can be a capability type; its set comes from the parameters.
- `use E` written as a parameter of a function type is quantified at that function type. A closure checked against such a type receives an unknown set, which cannot be in its result. A closure without an expected type is not quantified. A quantified type is never inferred.
- In 0.29, `use E` written as an expression is a capability of the name E in types. From 0.30 it is a compile error.
- Remove "A closure or a local fn that captures a capability value can be passed where its effect is expected (the `copy` example below)." Rewrite the `copy` example with named items (`with primary = PrimaryDb { with replica = ReplicaDb { copy(replica, primary) } }`).

### `[effect.with]`

- Each item of a with runs its init and makes a capability. Leaving the with releases it, and the capability ends.
- A named item `x = Item` binds x to the capability of the item. An item without a name provides its effect by name only.
- Replace "A capability value taken out on the outer side keeps referring to the outer handler even if an inner with overrides the same effect." by: "The capability of a named item keeps referring to its handler even if an inner with overrides the same effect."

### `[effect.handler]`

- The set of a field of a capability type or a capability-bound type is decided when an item of a with runs the init.
- A parameter of type `use E` of an operation is decided at each call and is an unknown set in the implementation; it cannot be stored in a field or in State.
- The result type of an operation MUST NOT contain a capability type or a capability-bound type (E04xx).
- `self` in an operation body and in release is the capability of the handler, with the added power of reading the fields. It can be passed to a parameter of type `use E`. A closure that captures `self` has the capability in its R.

### `[effect.escape]`

- Replace the section by: A capability cannot leak. If the type of the value of a with body, the error type that leaves the body, or the R of the with contains a capability made by that with, it is a compile error (E04xx). A with that provides E by name counts the name E in types as its own capability. A type fixed outside the with cannot contain the capability, so other paths out are type errors.
- Remove capture sets, roots, W0402, E0444 and the check at run time ("calling an operation of a handler that has already been released is a defect").

### `[type.rules]`

- Add the rules of sets of capabilities in capability types and in R, the quantification of `use E` in function types, and capability-bound types, as in `[effect.use]`.
- A type declaration may end with `uses E`: its values live only while a capability of E does. When an operation of a capability c of E returns a value of such a type, the type of the value records c; an operation called through the name of E records the name E. A value of such a type is treated as containing the capability.

### `[trait.builtin]`

- Capability is satisfied by capability types, not by capability-bound types. `[C: Capability]` is decided at each call with the set of the capability passed.

### `[fiber.scope]`

- `Fiber.scope` has the signature `pub fn scope[A, E, R](body: fn(use Scope[E, R]) -> A fails E uses { Scope[E, R], Async, R }) -> A fails E uses { Async, R }`. The body receives the capability of the scope, and still has Scope in its R.
- `Fiber`, `Deferred`, `Semaphore` and `Queue` are capability-bound types of Scope (`@external pub type Fiber[A] uses Scope`). The handles that `s.fork`, `s.deferred`, `s.semaphore` and `s.queue` return record s; those that `Fiber.fork` and the like return record the name Scope.
- To fork into an outer scope, the parameter of the outer scope's body is used. Remove the text that says `s = use Scope` is written to refer to an outer scope, and that helper functions cannot take the scope as a parameter.
- Rewrite the examples with `Fiber.scope(|s| ...)`.

### `[fiber.escape]`

- Replace the section by: The capability of a scope, and the handles that record it, cannot leave the scope (`[effect.escape]`).
- Remove E0425, the roots of scopes, the table of what is checked at run time, and the defects of using a handle of a finished scope.

### `[fiber.deferred]`, `[fiber.semaphore]`, `[fiber.queue]`

- A Deferred, Semaphore or Queue is a capability-bound type of the Scope that made it. Its element type is fixed when it is made, so a capability made later in an inner with cannot be put in it.

### `[api.std.fiber]`

- Change the declarations of `scope` and of the handle types as in `[fiber.scope]`.

### `[ext.boundary]`

- Replace "The R may contain capabilities; a callback reaches a handler only through a capability it captures (8.2), since the host calls it without passing handlers." by: "The R MUST NOT contain a capability either, since the host may keep the callback and call it after the with of the capability has ended (E0454). A parameter of an external function marked `@borrowed` (the host holds the callback only during the call) may take a callback whose R contains capabilities. `@borrowed` can be written only on a parameter of function type of an external function (E04xx)."

### `[intro.experimental]`

- Remove the row of `capture-checking`. The feature is removed, not made stable; its name is accepted with W0206 as the rules for a removed feature say.

### `[lsp.hover]`

- Replace the line `holds: ...` by a line `capabilities: ...` that names which capabilities the type of the value records: the variable of a named item, the handler of an item without a name, the parameter that received it, or `any` for a parameter of a quantified function type. The type itself is shown as written in source.
- The `uses { ... }` of a closure or a function lists capabilities by the same names.

### `[open.next]`

- Replace the 0.29 item ("make E0444 the default ... remove the run-time check of 8.6") by this proposal: in 0.29, capture checking and the checks at run time are removed, and leaks of capabilities and handles are compile errors from the types.
- `use E` written as an expression becomes a compile error in 0.30.

### `[diag.list]`

- E04xx: a capability leaks out of the with or scope that made it.
- E04xx: the result type of an operation contains a capability type or a capability-bound type.
- E04xx: `@borrowed` on something other than a parameter of function type of an external function.
- E0454: also a capability in the R of a callback passed to a parameter without `@borrowed`.
- Remove W0402, E0425 and E0444.

## Diagnostics

- [ ] E04xx: a capability leaks (a value whose type or R contains a capability leaves the with). The message names the capability as the language server does.
- [ ] E04xx: a capability type or a capability-bound type in the result of an operation
- [ ] E04xx: `@borrowed` in a wrong position
- [ ] E0454 for capabilities in the R of a callback without `@borrowed`
- [ ] Widening is reported where an effect set does not fit (E0403), with the help: take the capability as a parameter `use E`, or capture it in a closure
- [ ] Remove W0402, E0444 and E0425 (0.29). Remove the experimental feature `capture-checking` (its name gives W0206)
- [ ] 0.30: W0403 becomes an error

## Compatibility

Counts in main after #791, excluding `.snap` files. std, examples and tests/laws contain no `use E` written as an expression; the rest is in the programs of the compiler's tests (Rust strings), `spec/` and `proposals/`.

| Form | Count | Replaced by |
|---|---|---|
| `(use E).op()` | 817 | `<-`, by the quick fix of W0403 |
| `x = use E` | 278 | `x <- E` by the quick fix when x is only a receiver in the same function; otherwise `with x = H` |
| `x <- E` | 160 | Unchanged |
| `: use E` in parameters and types | 171 | Written the same; they become capability parameters |
| An outer value used in an inner with that overrides the same E | 18 | `with x = H` |
| `Fiber.scope(|| ...)` | 244 | `Fiber.scope(|s| ...)` or `|_|`, mechanically |
| `= use Scope` | 34 | The parameter of the body of `Fiber.scope` |
| `with x = H`, `with x = E.M(...)` (`.emel`) | 14 | Unchanged; the type of x records which capability it is |
| Widening a capability to the name of its effect | Not counted | A compile error in 0.29; the help shows a capability parameter or a closure that captures it |

- Capability parameters are written the same (`fn copy(from: use Db, to: use Db)`, `copy(replica, primary)`).
- Capabilities in collections, `State[use Name]` (#708) and fields of type `use E` keep working inside the same with.
- `capture-checking` in `[experimental]` of `Pome.toml` and `--experimental-capture-checking` give W0206.

## Implementation

- Type checking: give capability types and effect sets the capabilities they record. At the exit of a with, check that the type of the value of the body, the errors and R do not contain the with's capabilities. Sets of capability types and the type parameters of capability parameters are inferred by the same machinery as R (unions and set variables).
- Quantified function types: check a closure against a written type with a fresh set, and check that the set is not in its result.
- Capability-bound types: parse `uses E` at the end of a type declaration, and record the capability when an operation returns such a value.
- Lowering: the evidence of a capability is the handler of its item. A type parameter decided as the name of an effect uses the evidence at the time of the call.
- Capture checking: remove the checks (`capture.rs` in `emela-types`, the switch between W0402 and E0444 in the driver, `holds:` in hover). Values of `use E` are checked by the leak check through the names of effects in types.
- std: change `Std.Fiber` as in `[fiber.scope]`, and rewrite `Fiber.scope(|| ...)` in the repository.

## Alternatives

- Keep values and keep closing paths with capture checking (`capture-checking-exits`). Decisions 1 and 2 of #719, and the host and Deferred / Queue questions, remain. #701 is not fixed.
- Make capture checking stable, then remove it. E0444 would become the default and be removed in the next version. Removing it as an experimental feature stays outside the compatibility guarantees.
- Make capabilities not values (the first version of this pull request). Capabilities in collections and `State[use Name]` could not be written. Telling them apart in types gives the same safety while keeping them values.
- Make a local label and let x be a capability of that label (the second version of this pull request). Users would learn both capabilities and labels. Putting the capability itself in R needs no label.
- Widen capabilities to the name of their effect so that capabilities of different withs fit in one list. A widened value is not followed by R, which is the hole of #719. Sets let them fit without widening.
- Name the type parameter of a capability parameter (`fn retry[C: use Db](db: C, f: fn(C) -> ())`). It lets two arguments share a set, but a closure that captures the capability writes the same thing, and few places need a name. Adding it later does not change the meaning of what is written now.
- Give a forked child its scope as a parameter (`fork[A](f: fn(use Scope[E, R]) -> A ...)`). Every `Fiber.fork(|| ...)` would change, and a child reaches its own scope by name already.
- Write named items as `with f <- Files.Open(path)` or `with Files.Open(path) as f`. `<-` is the symbol of effect aliases, and `as` would require rewriting every `with x = ...`.
- Keep `use E` without a name as "a value that reaches the innermost E at the call". In types it is only the name of an effect, so while it can be passed and stored, the type does not say which handler it refers to (the source of the hole of #719). The 18 places that use an outer value inside an inner with overriding the same E would silently change meaning.
- Release 0.29 with #791 alone and this proposal in 0.30. 0.30 would break again right after W0403 in 0.29, and the handling of capture checking would change twice.
- Default host callbacks to "called only during the call" and mark the ones the host keeps. A missing mark would let a capability outlive its with silently.
- Use regions (#651) for resources opened a number of times decided at run time. Recursion writes them, and capability-bound types let a pool stop its ids from leaving.

## Prior art

## Open questions

## Decision

Tell capabilities apart in types and in R, so that the one principle "a capability cannot leave the with that made it" is kept by type checking instead of capture sets. Every path out (operation arguments, errors, host callbacks, Deferred and Queue, fiber handles) then becomes a type error without enumerating paths, and #701 is fixed because the with can remove its capability from R. Capabilities stay ordinary values written `use E`, with sets so that they fit in one list without widening. Fiber handles become capability-bound types, which removes the special typing of `Fiber.scope`. Capture checking is no longer needed and is removed as an experimental feature. This lands in 0.29 together with #791, and `use E` written as an expression becomes an error in 0.30.
