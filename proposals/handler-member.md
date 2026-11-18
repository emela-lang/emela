---
title: Handler members
authors: [Alliana]
status: accepted
pr: 624
issue: 616
decided: 2026-11-18
touches: [effect.decl, effect.handler, effect.with, effect.use, type.rules, dyn.with, grammar.decl, grammar.expr, diag.list]
requires: [capability-type, handler-self-constructor]
related: []
---

# Handler members

## Summary

An effect can declare handlers that its providers supply (handler members). A handler of the effect implements each of them with an inner handler of the same name. A user calls its init and binds the capability it provides with `with x = Files.Open("a.txt") { ... }`. A handler member can also be opened inside the init of a handler, and then lives as long as that handler.

## Motivation

#616 asks for four things at once.

- (a) The state of each opened resource (a descriptor of the host and so on) is held by the handler of that resource.
- (b) The parent handler (a file system) chooses the implementation of the handler of a resource. Replacing the parent, say with one for tests, replaces the handlers of the resources too, and the callers are not rewritten.
- (c) Neither a value that points to a resource nor an operation that closes it appears among the operations of an effect.
- (d) The code that uses a resource is the body of an ordinary with.

Every way of writing it now lacks at least one of them (the table in #616).

- The user names the handler of the resource, `with HostFile(path: ...)`: lacks (b).
- The parent effect gets an operation that returns a handle and one that closes it, and forwards: lacks (a) and (c).
- An operation of the parent returns a set of closures that capture capabilities: lacks (c).
- An operation of the parent takes the body as a function: lacks (d).

Each operation of `[api.std.file-system]` takes a path and completes in one call, so something that means holding on between operations, such as a lock, cannot be expressed either (a comment in #616).

```emel
# what we want to write
with src = Files.Open("in.txt"), dst = Files.Create("out.txt") {
  copy(src, dst)
}
```

The forms rejected on the way:

- An operation returning an owned value (`suspend fn open(path) -> with ReadableFile`, a `release(f: T)` per type). The contents of the value differ per handler, so the effect needs a type member, and a type member opens a hole through which the value of another handler is passed, which needs a rule of owners per value. With the operations on the resource (a handler per resource), the contents are the `self` of the handler, and neither is needed.
- An operation returning a handler (`-> handler ReadFile`). It looks like returning a handler as a value, against the principle that a handler is not bound to a variable.
- Handlers as first-class values. Static resolution of handlers is lost.

## Proposal

### Syntax

```emel
pub effect ReadFile {
  suspend fn read(max: Int) -> Option[Bytes] fails FileSystemFailure
}

pub effect WriteFile {
  suspend fn write(data: Bytes) -> () fails FileSystemFailure
  suspend fn sync() -> () fails FileSystemFailure
}

pub effect Files {
  handler Open(path: String): ReadFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Create(path: String): WriteFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
}

pub handler SystemFiles: Files {
  handler Open(handle: Handle, path: String): ReadFile {
    init(path: String) {
      o = host_open(path, "r")
      check_open(o, path)
      Self(handle: outcome_handle(o), path:)
    }
    release { host_close(self.handle) }
    suspend fn read(max, resume) { ... }
  }

  handler Create(handle: Handle, path: String): WriteFile { ... }
}

fn copy(src: use ReadFile, dst: use WriteFile) -> () fails FileSystemFailure {
  src.read(65536) match {
    None        -> ()
    Some(chunk) -> {
      dst.write(chunk)
      copy(src, dst)
    }
  }
}

fn main() {
  with SystemFiles {
    with src = Files.Open("in.txt"), dst = Files.Create("out.txt") {
      copy(src, dst)
    }
  }
}
```

A provider that wraps the handlers of another provider opens the handler member of the outer provider in its init, and keeps the capability in a field.

```emel
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) {
      Self(inner: Files.Open(path)) # the outer Files; lives as long as this handler
    }

    suspend fn read(max, resume) {
      (use Console).print_line("read #{max}")
      resume(self.inner.read(max))
    }
  }

  handler Create(inner: use WriteFile): WriteFile { ... }
}
```

A handler member of a specific provider is opened through its capability.

```emel
fn open_in(fs: use Files, path: String) -> Int fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure {
  with f = fs.Open(path) {
    count(f)
  }
}
```

- `handler Name(params): Effect fails ...` inside an effect declares a handler member. It is not an operation, and has no return type.
- A providing handler implements it with an inner handler of the same name. The parentheses after the name of the inner handler declare its fields, and its `init` takes the params of the declaration.
- An item of a with can be written `x = Item`. When Item is a handler member (`Files.Open(...)`, `fs.Open(...)`), only this form can be written.
- Inside the init of a handler, a handler member can be written as an expression (`Files.Open(path)`, `fs.Open(path)`).

### Meaning

- The with item `x = Files.Open(args)` calls, with args, the init of `Open` of the innermost provider of Files at that position, and binds the capability it provides to x. As for an item `H(args)` (`handler-init-arguments`), args are evaluated outside the with, in the order written.
- The with item `x = c.Open(args)`, where c is a capability of Files (`use Files`, `capability-type`), calls the init of `Open` of the handler that provides c, instead of the innermost one.
- The effects that the init, the release and the operation bodies of an inner handler use are requirements of the outer handler (they are in its requires). They are resolved at the with of the outer handler, so the R of a with that writes `Files.Open` contains only Files. Replacing the provider of Files does not change the R of the caller.
- The value of an inner handler can be constructed only inside its init (E0418, as now). An inner handler cannot be referred to by name from outside its outer handler. It cannot refer to the `self` of its outer handler either: configuration of the outer handler reaches the inner handler through the requirements of the outer handler.
- The x of `with x = Item` refers to the handler of that item, even when the same with has two items of the same effect.
- The effect provided by a handler member item is not provided to `use E` by name. It is used only through the bound x. Resources are bound and passed; only services are looked up by name.
- An item that is not a handler member can also be bound: `with log = ConsoleLogger { ... }` provides Logger by name as usual, and binds its capability to log as well. The same holds for an item of a label (`with s = Source by P { ... }`).
- A bound item is released in the reverse order of initialization, as the other items are.
- A handler member is not an item of a layer: a layer has no binding.
- A handler member expression inside an init calls the init of that handler member on the spot and gives its capability. What it opens lives as long as the handler whose init opened it.
  - It is released after the release of that handler, in the reverse order of opening.
  - If the init fails, what it opened so far is released in reverse order.
  - A handler member of the handler's own effect (`Files.Open` inside TracedFiles) refers to the outer provider, as a requirement on its own effect is provided outside (`[effect.with]`).
- A provider for tests whose opened files have changing state (an in-memory table) keeps that state in an effect that the outer handler requires (State), since an inner handler cannot change its fields in its operation bodies.

### Typing rules

- The with item `x = E.M(a)` requires {E} ∪ (the R of a), and provides the effect of M. The type of x is `use F`, where F is the effect of M.
- The with item `x = c.M(a)` requires {c} ∪ (the R of a). The type of x is `use F`.
- Inside an init, the type of `E.M(a)` and of `c.M(a)` is `use F`. E (or c) is in the R of the init, and becomes a requirement of the outer handler. The E of the expression is the E of the init of M.
- On a capability `c: use E`, `c.M(a)` can be written only as a with item and inside an init. Elsewhere, only operations of E can be called on c (`[effect.use]`).

## Specification changes

### `[effect.decl]`

Add after the item on the `uses` clause of an operation:

> - An effect MAY declare handler members (`handler Open(path: String): ReadFile fails ...`). A handler member is not an operation: it names a handler that every handler of the effect provides. Its parameters are the parameters of that handler's init, and its `fails` is the E of that init. The effect after the colon is the effect that the handler implements.

### `[effect.handler]`

Add:

> - A handler of an effect with handler members implements each of them with an inner handler of the same name (`handler Open(handle: Handle, path: String): ReadFile { ... }`). The parentheses after the name declare its fields, and its init MUST take the parameters of the handler member, with the same names and types. A missing handler member is E0404, as for an operation.
> - The effects used in the init, the release and the operation bodies of an inner handler are requirements of the outer handler (requires(H), 9.4). They are provided where the outer handler is provided.
> - An inner handler cannot be referred to by name outside its outer handler, and cannot refer to the `self` of its outer handler.
> - Inside an init, a handler member can be written as an expression. `E.M(arguments)` runs the init of the handler member M of the innermost handler that provides E, and `c.M(arguments)`, where c is a capability of E, runs that of the handler that provides c. Its value is the capability of the handler opened. The handler opened lives as long as the handler whose init opened it: it is released after that handler's release, in the reverse order of opening. If the init fails, the handlers it opened so far are released in reverse order. A handler member of the handler's own effect refers to the outer handler.

### `[effect.with]`

Add:

> - An item can be written as `x = Item`. Within the body, x is the capability of the effect that Item provides (`use F`, 8.2), and refers to the handler of that item even when another item of the same with provides the same effect. An item that is not a handler member still provides its effect by name.
> - `E.M(arguments)`, where E is an effect and M one of its handler members, can be written as an item only in the form `x = E.M(arguments)`. It calls the init of M of the innermost handler that provides E at that position. `x = c.M(arguments)`, where c is a capability of E, calls the init of M of the handler that provides c. The arguments are treated as for `H(arguments)`. The item requires E (or c), not the requirements of the inner handler.
> - The effect provided by a handler member item is not provided to `use` by name. It is reached only through the bound capability.
> - A handler member cannot be an item of a layer.

### `[effect.use]`

Add:

> - On a capability c of an effect E that has handler members, `c.M(arguments)` opens the handler member M of the handler that provides c. It can be written only as a with item `x = c.M(arguments)` (8.3) and inside an init (8.4).

### `[type.rules]`

Add rows:

> | `with x = E.M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | Re ∪ { E } ∪ Ra |
> | `with x = c.M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | Re ∪ { c } ∪ Ra |
> | `E.M(a)`, `c.M(a)` inside an init | `use F` (F is the effect of M) | Ea ∪ (E of the init of M) | { E } or { c }, ∪ Ra |

### `[dyn.with]`

Add to step 1:

> A handler member item `x = E.M(arguments)` or `x = c.M(arguments)` runs the init of M of the handler that provides E (or c), and is ordered with the other items by its requirements (only E or c).

Add:

> - Handlers opened inside an init (8.4) are released after the release of the handler that opened them, in reverse order.

### `[grammar.decl]`

```ebnf
effect_decl  = "effect" type_name [ type_params ] "{" { effect_item NL } "}"
             | ...
effect_item  = op_sig | member_sig
member_sig   = "handler" type_name "(" [ params ] ")" ":" type_ref [ "[" type { "," type } "]" ]
               [ "fails" error_set ]

handler_item = "init" [ "(" [ params ] ")" ] block
             | "release" block
             | [ "suspend" ] "fn" lower_name "(" [ lower_name { "," lower_name } ] ")" block
             | handler_decl                                      (* inner handler; 8.4 *)
```

### `[grammar.expr]`

```ebnf
with_item  = [ lower_name "=" ] supply [ "by" by_right ]
supply     = type_ref [ "." type_name ] [ "(" [ args ] ")" ]
           | lower_name "." type_name "(" [ args ] ")"          (* a handler member of a capability; 8.3 *)
postfix    = ...
           | lower_name "." type_name "(" [ args ] ")"          (* inside an init only; 8.4 *)
```

## Diagnostics

- [ ] A handler member item is written without `x =`, or as an item of a layer: code to be assigned
- [ ] A handler member is written outside a with item and an init (`f = Files.Open(...)` in a fn): code to be assigned
- [ ] The init of an inner handler does not take the parameters of its handler member: code to be assigned
- [ ] A missing handler member in a handler: E0404 (extended)
- [ ] A lint: the same capability variable passed to two parameters of one call (`copy(tmp, tmp)`): code to be assigned (`[lint]`, draft `linter`)

## Compatibility

The change only adds. Adding a handler member to an existing effect breaks the handlers of that effect that users wrote (E0404), as adding an operation does.

## Implementation

- emela-syntax: member_sig, inner handlers, `x = Item`, `c.M(...)`
- emela-resolve: resolving `E.M` by the current rules of names (`[module.resolution]`: with `import Std.Files.{Files}`, `Files.Open` is the handler member; with `import Std.Files` alone, it is written `Files.Files.Open`), the visibility of the names of inner handlers
- emela-types: checking the implementations of handler members, moving the requirements of inner handlers to the outer handler, handler member expressions in an init, `c.M(...)` with c in R
- emela-core: lowering handler member items (calling the init of the inner handler of the provider), releasing what an init opened after the release of its handler
- std: Std.Files (tentative name)
- Std.Tcp (#563): its `Connection` is aligned to this form after this proposal, in a proposal of its own

## Alternatives

- An operation returning an owned value (`suspend fn open(path) -> with ReadableFile`, a `release(f: T)` per type): the contents of the value differ per handler, so the effect needs a type member, and the hole through which the value of another handler is passed has to be closed with a rule of owners.
- An operation returning a handler before its init (#616, `fn open(path: String) -> handler File`, an expression `handler H(args)`): it meets (a) to (d), and resolves the requirements of the returned handler where it is made, as this proposal does. The differences:
  - A value and a type of handlers before their init (`handler X`) are added, with rules on where they may be. This proposal has neither.
  - The E of the init counts in the E of the expression that makes it, which needs a special rule against `escape` on that expression. Here, the E of the init is the E of the with item, and `with x = Files.Open(p) { ... } escape { ... }` is written as it is.
  - The body of an operation can choose the returned handler by its arguments. Here, the implementation of a handler member is one declaration per provider, resolved statically; choosing at run time is a branch inside the inner handler.
  - A provider that wraps a returned handler is left open there. Here, the init opens the outer handler member and keeps it in a field (`capability-type`).
- An operation that puts a handler around its continuation (reading `-> () uses { ReadFile }` as "usable after the call"): it closes at the end of the with of Files, R changes from statement to statement, and the `use` of a caller sees a handler put there at run time, which breaks static resolution.
- First-class handlers: static resolution is lost. Keeping lifetimes lexical would need types that stop them from leaving.
- Looking resources up by name and telling them apart by labels (`with Source by Files.Open(...)`): a label has to be declared at the top level for each role. With both this and the bound form, libraries would mix `copy() uses { ReadFile }` and `copy(src: use ReadFile)`.
- A syntax for items initialized together with an inner handler (`init(path: String) with inner = Files.Open(path) { ... }`): a name bound at the head of init would be visible in the operation bodies, an exception to scoping. With a field of type `use E`, state stays in fields, as now.
- Letting an inner handler read the `self` of its outer handler: convenient for configuration such as the root of a file system, but it ties the state of the two together. It can be added later without breaking code.
- Releasing by freeing the value, or linear types that make the value be used up: both conflict with `[dyn.memory]`, which leaves cleanup to release alone and makes freeing unobservable.

## Prior art

- Scoped resource management: acquisition and release tied to a scope, with the scope as a requirement in the effect row. Effect-TS (`Scope`, `acquireRelease`, `@effect/platform` FileSystem's `open`) has it; resources are first-class values and escaping them is not checked.
- Named handlers and scoped effects: a handler instance as a value, with a rank-2 type variable that keeps it inside its scope. Koka has them (Xie, Cong, Ikemoto, Leijen, "First-class names for effect handlers", OOPSLA 2022).
- Abstract members implemented per instance: an interface that declares a nested component, which each implementation supplies.

## Open questions

## Decision

Adopted, on top of the type of a capability (#622) and Self(...) in init (#623), which this requires.

- A resource is a handler that its parent handler supplies, declared as a handler member of the effect. The parent chooses the implementation, the resource keeps its state in its own self, and the code that uses it is the body of an ordinary with, as #616 asks.
- A resource is bound (`with x = Files.Open(p)`) and passed as a capability; it is not provided by name. Only services are looked up by name.
- A handler member of a specific provider is opened through its capability (`fs.Open(p)`), since capabilities are values now.
- An inner handler does not read the self of its outer handler; that can be added later. Std.Tcp (#563) is aligned to this form after it.
