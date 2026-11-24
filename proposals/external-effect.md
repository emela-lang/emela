---
title: External functions as an effect of their module
authors: [somnicattus]
status: draft
issue:
touches: [effect.external (new), effect.decl, effect.handler, ext, ext.decl, ext.call, ext.boundary, module.pome, diag.list]
requires: []
related: [host-effects, capture-checking, capability-identity, closure-r-by-caller]
---

# External functions as an effect of their module

## Summary

Calling an external function with host side effects adds `External`, a built-in effect of the module that declares the function, to R, and only the bodies of the handlers of that module remove it: a value that calls an external function can be carried anywhere, but called only inside those handlers, so rule 2 of `[ext.call]` becomes a type rule. E0407 is retired, so private fns can call external functions, and E0237 is retired, so an `@external` fn MAY be pub in every Pome: in another module it keeps the External of the module that declares it. External is never written in the declaration of an external function, and one declared with `uses {}` that is not a `suspend fn` stays pure.

## Motivation

### Rule 2 of `[ext.call]` is not checked

`[ext.call]` has three rules for an external function with `uses` omitted (and for one with host effects in its uses, and for a `suspend fn`):

1. The restriction on where it can be called applies to references, not only to calls.
2. It cannot be carried out of a handler as a function value either.
3. The inside of an anonymous function inside an operation body of a handler counts as inside.

Rules 1 and 3 are rules on where the name is written, and the compiler checks them (E0407). Rule 2 is a rule on where a value flows, and no section says how it is checked. The R of these functions is empty ("The host side effects of an external function are not put in R"), so a closure that calls one has a type such as `fn(String) -> () uses {}`, and it goes wherever a pure function can go.

No check finds these ways out. At `ea45cbfb`, each of the four programs below passes `emela check` without a diagnostic, and `emela run` performs the host side effect from `main`, outside any handler. Before 0.29, the fourth, returned from an operation, got W0402, because the result of an operation held its handler under capture checking; the first three passed at `c4a7768`, with and without `--experimental-capture-checking`.

`capability-identity` (#732), adopted for 0.29 and implemented in #813, replaces capture checking with capabilities told apart in types and in R. A closure that captures `self` has the capability of its handler in R and cannot leave the with. A closure that calls an external function captures nothing, so its R stays empty, and the ways below stay open.

Through State:

```emel
import Std.State.{State}

@external(js, "./log.mjs", "hostLog")
fn host_log(s: String) -> ()

effect Stash {
  fn stash() -> ()
}

handler Stasher: Stash {
  fn stash() {
    st <- State[fn(String) -> ()]
    st.set(|s| host_log(s))
  }
}

fn main() {
  (_, f) = State.run(|_s| (), || {
    with Stasher {
      s <- Stash
      s.stash()
    }
  })
  f("called from main via State")
}
```

Through a function that stores what it receives, called from the operation body as `keep(|s| host_log(s))`:

```emel
fn keep(f: fn(String) -> () fails {} uses {}) {
  st <- State[fn(String) -> ()]
  st.set(f)
}
```

Through an error:

```emel
error Carry(f: fn(String) -> ())

effect Leak {
  fn get() -> () fails Carry
}

handler Leaker: Leak {
  fn get() { fail Carry(f: |s| host_log(s)) }
}
```

Through the host itself: an external function can return a function type, and the function it returns (a host function, or a callback the host received earlier) has an empty R.

```emel
@external(js, "./h.mjs", "takeBack")
fn take_back() -> fn(String) -> () fails {} uses {}

effect Hand {
  fn hand() -> fn(String) -> () fails {} uses {}
}

handler Returner: Hand {
  fn hand() { take_back() } # fn(String) -> () uses {}: it looks pure
}
```

### Why it matters

- A function whose type says `uses {}` performs a host side effect outside any handler, after its handler has ended. Replacing the handler (a test handler) does not stop it.
- `[module.pome]` relies on rule 2: an `@external` fn in a dependency cannot be pub (E0237), because "the only path to the host visible from the depending side is effects". A dependency can hand out such a closure through State or an error, and the host is then reachable from the depending side without an effect.

### Where the value may be called

Inside the handlers of the module that declares the external function, and nowhere else. `[ext.call]` keeps host side effects in the bodies of handlers so that callers see an effect: from outside the module, every host side effect is part of an operation, an init or a release of one of its handlers (or of a callback one of them gave the host, below), and providing other handlers (test handlers) stops all of them. Another module, and another Pome, must not reach the host without an effect of the module.

Which handler of the module calls the value, and which instance, does not matter for this. A value that only calls external functions does, in another handler body of the module, what that handler could call directly. A value that uses the state of an instance (a field, a resource held in one) reads it through `self`, and `capability-identity` keeps such a value inside the with of that instance.

Not changed here: the lifetime of a resource taken out of an instance as a plain value (`h = self.handle`, or a handle opened in the init and captured by a closure). A closure that captures it, or the handle itself returned by an operation, can reach another handler body of the module after the resource is closed. This happens without carrying an external function, and is a question of the lifetime of the handle (`capability-identity` gives capability-bound types for values that live only while a capability does).

The host keeping a callback past the lifetime of a handler is a question of lifetime too. The host may call Emela code (`[ext.boundary]`), and `capability-identity` answers for capabilities: a callback that the host may keep cannot have a capability in its R (E0454), and a parameter marked `@borrowed` takes a callback used only during the call. A callback that calls an external function holds no capability, and the host side effects it performs are the ones a handler of the module gave the host. This proposal lets such a callback have External in its R, as an exception to "only while a handler body of the module runs": the host calls it after the operation that passed it has returned. What the exception keeps:

- Only a handler of the module can give the callback to the host. No Emela code outside the handlers of the module can call it.
- From outside, the host side effect was set up through an operation of a handler of the module, so it is visible as an effect (`[module.pome]`). Providing another handler (a test handler) keeps the callback from being given to the host at all.

What it does not keep is that the side effect happens while the handler lives; a callback that uses a resource closed by the release fails on the host. That is the lifetime of the resource, as above.

Programs do this today. At `c4a7768`, an operation body that calls `set_timeout(100, || host_log("tick"))` (an external function that keeps its callback) passes `emela check`, and the host calls `host_log` after the with has ended.

### Helpers cannot call external functions

E0407 restricts references to external functions with host side effects to the bodies of handlers. Code shared by the operations of a handler, or by the handlers of a module, cannot be a private fn that calls them: it takes the external functions as closures, through parameters whose uses clauses must be written to match (`args_onto`, `next_line` and `rest_text` of `Std.Process`). Once the requirement is in R, a private fn can call them, and R says where it may be called.

## Proposal

External is not written in the declaration of an external function. Every external function, except one declared with `uses {}` that is not a `suspend fn`, has in its type the External of the module that declares it, also where another module refers to it: an `@external` fn MAY be pub in every Pome, and E0237 is retired. The host effects in its uses clause are added as now. An external function declared with `uses {}` stays pure and can be called anywhere, as now; a `suspend fn` adds External even with `uses {}`, as it is restricted to handlers today.

External says where host side effects run. It does not say which handler instance or which resource they use: a closure that reads a field captures `self`, and `capability-identity` keeps it inside the with of its handler.

```emel
@external(js, "./write.mjs", "hostWrite")
fn host_write(s: String) -> ()

fn tagged(tag: String, msg: String) { host_write("[#{tag}] #{msg}") } # R: { External }

pub handler FileLog: Log {
  fn info(msg) { tagged("info", msg) } # External is removed here
}
```

### Syntax

No new syntax. `External` is a built-in name, as `Async`, `ProcessHost` and `PageHost` are, and it can be written in a uses clause:

```emel
fn read_all(step: fn() -> Int uses { Async, External, ProcessHost }) -> String
  uses { Async, External, ProcessHost } {
  n = step()
  "#{n}"
}
```

In another module, the External of M is written with the module, `M.External`, as for the other names of M (`[module.resolution]`). `External` alone names the External of the module where it is written.

The name `External` is treated as a name of the Prelude (`[module.resolution]`): a definition or an import named `External` in a module shadows it, and the compiler warns (W0202). In that module, `External` names the definition or the import, and the built-in External of that module has no name: it is not written, and is only inferred. In another module, `M.External` names that definition of M, and the built-in External of M has no name there either.

The declarations of external functions do not change. External is never written in them; it is added to their type:

```emel
@external(js, "./write.mjs", "hostWrite")
fn host_write(s: String) -> () # adds External

@external(js, "./fs.mjs", "writeText")
suspend fn write_text(path: String, text: String) -> ()
  uses { ProcessHost } # adds External, ProcessHost and Async

@external(js, "./text.mjs", "fold")
fn fold_case(text: String) -> String uses {} # adds nothing: pure, callable anywhere
```

### Meaning

- Every module M has one effect `External`. Written in M, `External` names the External of M. The Externals of two modules are different effects.
- Every external function, except one declared with `uses {}` that is not a `suspend fn`, has in its type the External of the module that declares it. Its uses clause still contains only host effects, or nothing (E0445); External is not written there.
  - `uses` omitted: a call adds External.
  - Host effects in `uses` (`uses { ProcessHost }`): a call adds External and those host effects, as the function has host side effects and works only on that host.
  - `uses {}` written explicitly: a call adds nothing. The function is declared pure and can be called anywhere, as now. The compiler cannot verify the declaration, and the person who declares it is responsible (`[ext.call]`). This is the one exception to External, and it is where an unverified promise stays; a mark on the declaration can make it visible later (Alternatives). Other modules can call it when it is pub, or call a pub fn that calls it, whose R is empty. Today a dependency can hand it out only through such a pub fn, as E0237 keeps the external function itself from being pub there; with E0237 retired, the two are the same. An external function that adds External cannot be handed out this way: made pub, or wrapped in a pub fn, it keeps the External of its module, which no other module can remove.
  - A `suspend fn` adds Async besides (`[effect.async]`), and adds External even when `uses {}` is written, as it is restricted to handlers today (`[ext.call]`). This proposal does not change where such a function can be called (Alternatives).
- A reference to an external function that adds External, as a value, and a closure that calls one, have that External in the uses of their function type, and add it to R where they are called.
- The functions that such an external function returns have that External, and the host effects in its uses clause, in their uses: the host may return a host function, or a callback it received earlier. This follows every function the host hands back: the return type when it is a function type, and the return types of those function types in turn (`fn() -> fn(String) -> ()`, if `[ext.boundary]` admits function types inside function types). Under `[ext.boundary]` a return type holds functions in no other way: a boundary type has no List, and an Array of functions is not a boundary type. The parameters of those function types are callbacks that Emela passes to the host, and follow `[ext.boundary]` instead.
- Only the host provides External, and only in the bodies of the handlers of M: the init, the release and the operation bodies of a handler declared in M, inner handlers included. The External of M is removed from the R of those bodies. It is not removed from the types of the anonymous functions and local fns written inside them; when one of them is called in a body, its External enters the R of the body and is removed there.
- requires(H) (`[effect.handler]`) therefore never has the External of the module of H as an element. A with that lists H, and a layer that contains it, do not need to provide it.
- A handler for External is E0413, `use External` is E0414, and External cannot be listed in a with or a layer. External is not a host effect: an entry does not provide it, and a with does not remove it.
- External has no operations, so it is Immediate (`[type.effect-bound]`).
- Any fn of M MAY call an external function. A private fn that does has External in its R (inferred, or written in its uses clause), and calling it in a handler body of M removes it. E0407 is retired.
- An `@external` fn MAY be pub, in the Pome being built as in a dependency (`[ext.decl]`); E0237 is retired. A call, or a reference as a value, adds the External of the module that declares the function, in that module and in any other. In another module, a pub `@external` fn that adds External is therefore the same as a pub fn of M that calls it: the other module can refer to it and pass it, and only the handlers of M call it. Another module reaches the host through the handlers of that module, or through an effect and a handler that the module declares for it:

```emel
@external(js, "./write.mjs", "hostWrite")
fn host_write(s: String) -> ()

pub effect Raw {
  fn write(s: String) -> ()
}

pub handler RawHost: Raw {
  fn write(s) { host_write(s) }
}
```

The handlers of other modules use `Raw`, and the with that provides them provides `RawHost` too. The path to the host is then an effect in the types, and a test can replace `RawHost` alone. Today a module of the Pome being built can instead make its `@external` fns pub, and the handlers of other modules call them directly. Under this proposal such a call is E0402, at the call; no module of std or the examples does this (Compatibility).
- A value keeps the External it got where its text is written, not where it is run later: a closure written in M has the External of M in its type wherever it is carried, and code in another module, in this Pome or in another, cannot call it.

Where External may appear:

- External MAY appear in any type, in M and in other modules (as `M.External`): uses clauses, parameters and results, the fields of types, enums, errors and handlers, the declarations of effects, the parameters of an init, requires(H), and type arguments. A value with External in its type may be passed wherever these types allow. Wherever it goes, calling it requires the External of M, which only the handlers of M remove.
- So a module can hand out a value that other modules can hold and pass, but cannot run. A pub fn that returns a closure calling an external function (a factory of callbacks) has an empty R itself, since it only refers to the external function. Another module can pass the closure to a handler of M (an argument of its init or of an operation), whose body calls it.
- A pub fn with the External of M in its R can be called only inside the handlers of M. It is allowed, and the compiler does not report it: another module can refer to it as a value without calling it, and pass it to a handler of M, which calls it or passes it to an external function of M as a callback. A fn that wraps an external function is handed out this way, as the closures of a factory are.
- A function value with External in its type MAY be passed to an external function as a callback, when the External is the one of the module of the external function. The host may call it after the operation that passed it has ended. E0454 (an effect in the R of a callback, `[ext.boundary]`) does not count that External, as it does not count host effects. The callback types in the declaration of an external function are written without External: their sets are hidden set variables, decided at each call and checked there by E0454. The External of another module is counted: a module that holds a value of M, received from a factory, cannot give it to the host and have the host run it outside the handlers of M.

The ways out of the Motivation are then closed:

| Way out | Result |
|---|---|
| The result of an operation, the argument of `resume`, the error of an operation | The types are declared by the effect. If they do not have the External of the module: E0403 where the value is returned or passed. If they do, the value is called only in a handler body of the module, elsewhere E0402 |
| A State, Deferred or Queue provided outside the handler | A pure function type (`use State[fn(String) -> ()]`): E0403 at `st.set`. A type argument instantiated with External (the `State.run` of `main`): the function that calls the value needs External, so E0402 outside the handlers of the module |
| A task started in a group or a background whose with is outside the handler | The R of the task is in the R of the group or the background, which is added to the R of its with (`[task.group]`), so the function that contains that with needs External: E0402 unless it is a handler body of the module |
| The field of an error | If the type of the field does not have the External of the module: E0403. If it does, the code that catches the error and calls the value needs External: E0402 outside the handlers of the module |
| A function with a pure parameter (`keep`) | E0403 |
| The init of another handler | The same: E0403, or the value is still called only in handler bodies of the module |
| A function returned by an external function (`take_back()`) | Its type has External: called only in handler bodies of the module |
| A pub fn that returns such a closure | Other modules can hold and pass it, and only the handlers of the module call it |
| Another handler of the module, or another instance of the same handler | Allowed: the value runs inside a handler body of the module |
| A callback passed to an external function, called by the host later | Allowed for an external function of the module: a handler of the module gave it to the host. A value of M passed to the host by code of another module (received from a factory): E0454 |

A value that calls an external function is therefore never called by Emela code outside the handlers of its module, in particular by code of another module or of another Pome (`[module.pome]`), as long as the declarations of external functions keep to `[ext.boundary]` (#815). Outside those handlers, only the host calls such a value, as a callback that a handler of the module gave it.

### Typing rules

- The External of a module is an effect in R. Containment of R (`[type.rules]`) and set variables treat it as other effects.
- At the end of each body of a handler declared in M, the External of M is removed from the R of the body, before requires(H) is computed.
- The type of an external function, other than one declared with `uses {}` that is not a `suspend fn`, has the External of the module that declares it, in what a call adds and in the functions it returns.

## Diagnostics

- [ ] E0445: the help for External
- [ ] E0407: retired
- [ ] E0237: retired
- [ ] E0402, E0403: the help for External
- [ ] E0413, E0414: External
- [ ] W0202: a definition or an import named `External`
- [ ] E0454: the External of the module of the call is not counted; that of another module is

## Compatibility

Programs that compile now and stop compiling:

- A program that calls a closure that calls an external function with host side effects outside the handlers of its module, or puts it where a function type written without External is expected: the type argument of a State, a field, a parameter or the result of an operation, a parameter of a fn. This is the hole being closed; it was already against rule 2 of `[ext.call]`.
- A program that starts, inside a handler, a task that calls an external function with host side effects in a group or a background whose with is outside the handlers of the module.
- A pub `@external` fn of the Pome being built that adds External, called from a handler of another module: E0402, as the handlers of that module do not remove the External of the declaring module. Call it from a handler of its module, or declare an effect and a handler for it in its module, as in the Meaning. No module of std or the examples declares a pub `@external` fn. One test of the compiler does: the core module `HostTest` that `crates/emela-core/tests/lower.rs` adds to the bundled core declares `now` and `gone` as pub `@external` fns with `uses {}`, and its main module calls `HostTest.now()`. They add nothing, so the test keeps compiling (surveyed at `ea45cbfb`).
- A closure that calls an external function, passed inside a handler where a pure function is expected (the function given to `State.update`): E0403. Call the external function outside the closure and pass its result (`v = host_read()` and then `st.update(|s| merge(s, v))`). No such call is in std.
- A program that calls a function returned by an external function with host side effects outside the handlers of the module, or stores it where a pure function type is expected. No external function of std returns a function type.
- In std, three private fns of `Std.Process` receive closures that call external functions, through parameters with written uses clauses: `args_onto` (`at: fn(Int) -> String uses { ProcessHost }`, called from `SystemArgs` with `|i| arg_at(i)`), and `next_line` and `rest_text` (`uses { Async, ProcessHost }`, called from `SystemStdin`). External is added to their uses clauses, or they call the external functions directly. No other fn of std has a parameter of a function type with a written uses clause that is not a set type parameter, except the `app` of the entry adapter `Std.Http.serve` and of its helper `launch`, which receive the user's fn, not a closure that calls an external function; no field of std holds such a closure; no handler of std starts a task; and no external function of std returns a function type (surveyed at `6f95cca` and `c4a7768`, and again at `ea45cbfb`, after Task replaced Fiber).

No declaration of an external function changes, and none can be called in more places than today.

Programs that compile now because they call external functions only inside handlers keep compiling: the External of the module is removed in the same bodies where E0407 allowed the references. Retiring E0407 and E0237 makes more programs compile (private fns that call external functions, and pub `@external` fns in a dependency). `External` becomes a name treated as a name of the Prelude, so a module that declares or imports something named `External` keeps compiling, with W0202. No module of std, the examples or the tests does (surveyed at `ea45cbfb`). External has no run-time representation.

The meaning of rule 2 of `[ext.call]` changes: a value that calls an external function may be carried out of a handler, to another handler of the module or through the body of a with, as long as it is called only inside the handlers of the module. No program changes behavior because of this; rule 2 was not checked.

## Implementation

- `emela-resolve`: `External` and `M.External` as names of effects, shadowed by a definition or an import as a name of the Prelude is (W0202).
- `emela-types`: the External of each module as an effect in R, added by calls of and references to external functions according to their uses clauses, in any module with the External of the declaring module, and to the functions they return (return types of function type, recursively through their return types); its removal at the bodies of the handlers of the module, before requires(H); the retirement of E0407 (`check_external` in `body.rs`); E0454 without External; the diagnostics.
- `emela-core`: `args_onto`, `next_line` and `rest_text`.
- `emela-driver`: the retirement of E0237 (`check_external_in_dependencies` in `internal.rs`, and its test in `tests/dependencies.rs`).
- `emela-driver`: tests for the four programs of the Motivation (E0403, or E0402 where `main` calls the value), a generic helper that stores the closure in a State, a task started in a group whose with is outside the handler, a pub fn with External in its R called from another module, a pub `@external` fn that adds External, called from a handler of another module, in the Pome being built and in a dependency (E0402), a closure of M received from a factory and passed by another module to its own external function as a callback (E0454), and the forms that stay silent (a private fn that calls an external function, called from a handler; a pub `@external` fn declared with `uses {}`, called from another module and from a dependent Pome; a pub `@external` fn that adds External, referred to as a value by another module and passed to a handler of M; a pub fn of M that returns such a closure, whose result another module passes to an init of a handler of M; an effect and a handler of M that call external functions, used by the handlers of another module; a closure passed to `List.map` inside a handler; a closure kept in a field of the handler; a closure passed to another handler of the same module through a private effect and called in its body; a task started in a group whose with is inside the body; a callback passed to an external function inside a handler).
- `emela-lsp`: External in hovers, with its module.

Following the functions an external function returns relies on `[ext.boundary]`, which allows functions in a return type only as the return type itself and the return types of function types. At `c4a7768`, the parameters and the return type of an external function are not checked against `[ext.boundary]` (issue #815); that is a defect to be fixed on its own, not a limit of this proposal. Type parameters of external functions are part of the same defect: a function put in a type argument passes the boundary without a function type in the declaration, out of reach of both the rule on returned functions and E0454 (#815, comment of 2026-11-23). The fix discussed on #815 requires a trait that the compiler decides, which a function type does not satisfy, on the type parameters of external functions. With it, a function passes between Emela and the host only where the declaration writes a function type, and the rule on returned functions and E0454 have no exception.

No prototype has been written. The four programs in the Motivation, and a task started in the background of `main` by a handler through `Spawn` (Compatibility), were checked at `ea45cbfb` on Windows (`emela check`, and `emela run` on the JS target). The first three were checked before at `c4a7768`, with and without `--experimental-capture-checking`, and with 0.28.1 in the form with `use`. The fourth is checked with `fails {} uses {}` written on the returned function type; with them omitted, the call of `take_back` is rejected with "not supported yet: reference to this kind of definition" (issue #815).

## Alternatives

- **Keep the value inside the handler instance that made it.** External would also be forbidden in the declarations of all effects, the fields of all types, the parameters of all inits, and inside the effects of requires(H), so that no value with External passes from one instance to another. Not chosen: a value that only calls external functions does in another handler body of the module what that handler could call directly, and the state of an instance is reached through `self`, which `capability-identity` keeps inside the with. The stricter boundary is stricter than the one the language keeps for values that read fields, and it needs a list of places to forbid.
- **Forbid External in declarations visible from other modules** (a pub fn, a pub effect, the fields of pub types). Soundness does not need it, since only the handlers of M remove the External of M. It would forbid a module from handing out values that only its handlers run, such as a factory of callbacks whose results are passed back to a handler of the module, or a pub fn that wraps an external function and is passed back to be given to the host as a callback. A warning on a pub fn with the External of its module in R is not given either, for the same reason.
- **Keep E0237, and widen it to every Pome**, so that only the module that declares an external function refers to it. Not chosen: the External of the declaring module already keeps a pub `@external` fn that adds External inside the handlers of that module, so E0237 would forbid only a way of writing. A pub fn that calls the external function does the same (with an empty R for one declared with `uses {}`, and with the External of its module otherwise), and this proposal allows it without a diagnostic; E0237 would not lead other modules to an effect and a handler either. Before this proposal, E0237 was what kept the handlers of a dependent Pome from calling the host functions of a dependency directly, as E0407 allowed a reference in the handlers of any module.
- **Give a pub `@external` fn the External of the module that refers to it**, so that a module that holds only bindings to the host can be used by the handlers of other modules. Not chosen: the type of an imported name would depend on the module that imports it, and the host of a dependency would be reachable from the handlers of the depending side without an effect of the dependency (`[module.pome]`). An effect and a handler in the module of the bindings write the same thing with the path to the host visible in the types.
- **Allow External in a callback only for a `@borrowed` parameter**, as `capability-identity` does for capabilities. External would then mean "only while a handler body of the module runs" with no exception. Not chosen: what it adds is that the side effect happens while the handler lives, which is the lifetime of resources, not where host side effects run. It would also reject programs that pass a callback calling an external function to a host that keeps it (a timer, an event listener), which compile today; they would have to finish the work on the host, or hand it to an Emela fiber through a Queue.
- **Write External in the uses clause of an external function** (`uses { External, ProcessHost }`, with `uses` omitted meaning `uses { External }`), so that the clause lists what a call adds. Not chosen: the External a call adds is always the one of the module that declares the function, so writing it says nothing. Host effects without External would also change meaning (a pure function that works only on one host), which every existing declaration with host effects would have to be rewritten against, for a kind of function with few uses.
- **Add External to an external function declared with `uses {}` too**, so that no external function is pure. Not chosen: code that is not a handler body could no longer call one. In std, the entry adapter `Std.Http.serve` reads the request and builds the response with external functions declared `uses {}`, and `exit_with`, called by the compiler when `main` returns an `ExitCode`, calls one; programs that use such functions for values (strings, numbers) outside handlers would have to move them into handlers. `uses {}` is an unverified promise either way, and this proposal leaves it as it is.
- **Mark the exception** with an attribute on the declaration (`@external(js, "./m.mjs", "sin", unchecked)`), instead of `uses {}` alone. External is implicit in every other external function, and `uses {}` on an external function is a promise the compiler does not check, written the same as a pure function type. A mark would make that promise visible and easy to find. It can be added later without changing anything else here.
- **Let the exception cover a `suspend fn` declared with `uses {}`**, which would add only Async and could be called wherever Async is provided. One rule ("`uses {}` adds no External") would then hold for every external function, and whether a function has host side effects does not depend on whether it waits: a wait declared as `suspend fn sleep(ms: Int) -> () uses {}` would bypass Clock and its test handler, but so does `fn now_ms() -> Int uses {}`, which is allowed today. Not chosen: it relaxes a restriction of today (`[ext.call]`: a `suspend fn` can be used only inside a handler even when `uses {}` is written), and nothing calls for relaxing it now, except that the type checker would have one rule fewer; this proposal closes the hole without changing where external functions can be called. So the exception to External is only an external function declared with `uses {}` that is not a `suspend fn`. The restriction may be relaxed, or made stricter, later.
- **A reference to an external function counts as a use of `self`.** Under `capability-identity`, `self` in an operation body and in release is the capability of its handler, and a closure that captures it cannot leave the with. Counting a reference to an external function there as a use of `self` would stop the ways of the Motivation at the end of the with. Not chosen:
  - It says which instance, not where. The value could be called in the body of the with, outside the handlers.
  - The init has no `self` (the handler value is made at its end). A closure made in the init and kept in a field would leave through an operation that returns the field.
  - A private fn could call an external function only by receiving `self` as a parameter of type `use E`, which cannot be shared among handlers of different effects.
  - For an operation called through the name of its effect (`x <- E`), the capability in the R of the closure it returns is the one of the with that provides E at the call. In R the name E means the handler at the time the closure is called, which may be another, so the type has to keep the two apart. `capability-identity` does not yet write how.
  - A callback that the host may keep would have the capability in R and be E0454.

  With External, `self` keeps only its role for the state of an instance. Issue #728 describes an earlier form of this alternative, a `host` value per instance tracked by capture checking, which `capability-identity` removes.
- **One effect removed at the bodies of every handler.** A value that calls the external functions of a module could then be called in the handlers of any module, and the host of a dependency would be reachable from the handlers of the depending side without an effect of the dependency (`[module.pome]`). A per-module External also names the module in diagnostics.
- **A declaration for externals** (`external Raw { @js(...) fn write(s: String) -> () }`): an effect whose handler is the host, neither an effect nor a handler written by the user. External already keeps host side effects inside the handlers of the module, so it is not needed for that. A module that shares its host functions with other modules now writes each of them three times (the `@external` fn, the operation of the effect, the body of the handler, as in the Meaning); such a declaration could write them once, as a shorthand, and can be added later.
- **Reserve the name `External`**, so that a definition or an import named `External` is an error and `External` always names the External of the module. Not chosen: a name added to the Prelude does not break existing code, because a definition or an import shadows it with a warning (W0202), and `External` follows the same rule. What a module that shadows it gives up is writing the built-in External of that module, which is then only inferred.
- **Keep E0407.** It adds nothing once the requirement is in R, and it is what keeps helpers from calling external functions.
- **Removal anywhere in the module.** `pub fn f() { host_write("x") }` would have an empty R and perform a host side effect. The bodies of handlers are where `[ext.call]` keeps host side effects, so that the callers see the effect of the handler.
- **Check at run time.** Every call would pay for it, and the hole would be found only when the code runs.

## Prior art

- Encapsulated effects: an effect whose operations can be discharged only inside the abstraction that declares it, so that it never appears in its interface. Local state threads discharged by a region (`runST` in Haskell) and the local effects of Koka are of this kind; here the region is a module and the place of discharge is the bodies of its handlers.
- Foreign function interfaces that confine foreign calls to designated wrappers, so that the rest of the program reaches the foreign code only through typed interfaces.

## Open questions

## Decision
