---
title: Handlers with type parameters
authors: [Alliana]
status: implemented
pr: 817
issue:
decided: 2026-11-23
touches: [effect.decl, effect.with, effect.handler, effect.layer, effect.label, type.rules, grammar.decl, grammar.expr, diag.list, lsp.hover, lsp.completion, lsp.inlay-hint]
requires: []
related: [capability-identity, handler-init-arguments, handler-member, semaphore-queue]
---

# Handlers with type parameters

## Summary

- A handler can declare type parameters of ordinary types (`handler Memory[S](value: S): State[S]`). Set type parameters (E, R) stay out, as for effects (E0424).
- Each item of a with determines the type arguments. They are inferred from the arguments of init and from what uses the effects the item provides; when they are not determined, they are written (`with InMemoryRepo[UserId, User] { ... }`).
- Type parameters can have trait bounds, checked at the item.
- An item of a layer writes its type arguments unless the arguments of init determine them.
- Type parameters on handler members (`handler Open[T](...)` declared in an effect) are out of scope.

## Motivation

- A draft that replaces Fiber with Task makes Deferred, Queue and Semaphore items of a with, provided by built-in handlers. That needs a handler that implements `Queue[A]` for every A. `[effect.decl]` forbids it, so either the built-in handlers get an exception, or every handler may have type parameters. This proposal takes the second, so that no exception is made.
- A test handler for an effect with type parameters (`Repo[K, V]`, `Cache[K, V]`) has to be written again for each pair of types it is used with.
- The rule "There are no generic handlers that implement the effect for every type" has no reason written in the specification or in the proposals. It came in with the type parameters of effects (#183, 2026-10-12), together with an implementation that reads the effects of a handler in a context without type parameters (`Signatures::handler_effect`).
- `State.run[S]` is already generic, and provides `State[S]` for every S. The approximation of the run-time key that looks up a handler by type arguments (`[open.list]` #35) already applies to it, so handlers with type parameters do not bring a new case of it.

What the specification allows today:

- Write one handler for each type. This is enough when there are few types.
- Use `State.run` directly for state.
- Neither works for a `Queue[A]` where A is a type of the user.

## Proposal

```emel
effect Repo[K, V] {
  fn find(key: K) -> Option[V]
  fn save(key: K, value: V) -> ()
}

handler InMemoryRepo[K: Eq, V]: Repo[K, V] {
  fn find(key) {
    st <- State[List[(K, V)]]
    List.find(st.get(), |(k, _)| k == key) |> Option.map(|(_, v)| v)
  }

  fn save(key, value) {
    st <- State[List[(K, V)]]
    st.update(|xs| [(key, value), ..xs])
  }
}

handler Constant[S](value: S): State[S] {
  init(value: S) { Constant(value:) }

  fn get() { self.value }
  fn set(value) { () }
  fn update(f) { () }
}

fn main() {
  with Constant(value: 0) { count() } # S = Int, from the arguments of init
  with InMemoryRepo { repo_test() } # K, V from Repo[UserId, User] in the R of the body
  with repo = InMemoryRepo[UserId, User] { seed(repo) } # written
}

layer AppTest {
  InMemoryRepo[UserId, User] # a layer writes them unless init determines them
  Constant(value: 0)
}
```

The examples use syntax that `emela fmt` does not read yet, so they have not been run through it.

### Declaration

- Type parameters follow the name, before the fields (`handler Memory[S]`, `handler Cache[K: Eq + Hash, V]`).
- They are ordinary types. One that appears only in the fails or uses of a function type would be a set, which is E0424, as for an effect.
- They are visible in the fields, in the effects after the colon, and in init, release, the operation bodies and the inner handlers.
- Each MUST appear in the effects after the colon, in the type of a field, or in the type of a parameter of init (E0462). One that appears only in bodies (`handler Recording[A]: Logger { fn info(msg) { st <- State[List[A]] ... } }`) or nowhere (`handler H[A]: Logger`) is an error. The type of a handler cannot be written in a type position (E0217), so unlike a type parameter of a type that appears in no field, such a parameter tells nothing apart, and only makes identical copies. The requirements of a handler are thus never the only thing that determines its type arguments.
- The effects after the colon write their type arguments; leaving them out is still E0423.
- Bounds (`[K: Eq]`) are allowed, and the functions of the bounds can be called in the handler.

### Items of a with

- An item determines the type arguments as a call determines those of a function: once, at the position of the item. The handler is not polymorphic inside the body.
- They are inferred from the arguments of init; from the R of the body, by the unification that removes the effects the item provides (as `State.run` determines `State[List[String]]` today, `[effect.decl]`); from how x is used, for `with x = H`; and from the requirements of the other items of the same with that the item satisfies (`with InMemoryRepo, UserService { serve() }`, where UserService requires `Repo[UserId, User]`).
- The requirements of the item itself flow into the R of the with and unify with what the outside provides (`State.run(users, || with InMemoryRepo { ... })` unifies `State[List[(K, V)]]` with `State[S]`). No rule is added for this. Since every type parameter appears in the head, the fields or init (E0462), this is the only source only when the item provides nothing that is used.
- They can always be written (`with Memory[Int](initial: 0)`, `with InMemoryRepo[UserId, User]`).
- Type arguments that are not determined are E0459 (new). E0423 is not extended: E0423 is leaving out what must be written, and E0459 is inference failing where leaving them out is allowed.
- E0459 is decided when the inference of the enclosing top-level declaration ends. A variable left that appears in the type of the declaration is generalized with it (`fn peek() { with x = Pass { x.get() } }` is `∀ S. fn() -> S uses { State[S] }`), as type variables left in R are generalized today. Only a variable that does not appear there is E0459. An item is not stricter than a call.
- When two or more effects in the R of the body (and the requirements of the other items) unify with an effect the item provides (`State[List[Int]]` and `State[List[String]]` in the body of `with Empty`), and the arguments of init do not narrow them to one, it is E0459. The first one that unifies is not taken: which effect is provided would then depend on the order of the body, and the other one would be found only outside. The approximation of #35 for `State.run` is not changed by this proposal.
- "A later item overrides an earlier one for the same effect" (`[effect.with]`) compares only effects whose type arguments are determined by the arguments of init or written. If an effect of an item whose type arguments are not determined unifies with an effect of another item of the same with, it is E0459, and the type arguments are written (`with AppLive, InMemoryRepo[UserId, User]`). Comparing by unification would let the override take the type arguments: in `with Fixed, Blank { (use State[String]).get() }`, Blank would get S = Int from `State[Int]` of Fixed and override it.
- Bounds are checked at the item (E0326). Type arguments on a handler without type parameters, or a wrong number of them, are E0304.
- provides(H) and requires(H) are instantiated with the type arguments of the item. The R of `with InMemoryRepo { ... }` keeps `State[List[(UserId, User)]]`.
- The type arguments are fixed at the position of the with, outside its body, so they cannot contain a capability made inside the body (`capability-identity`: a type fixed outside cannot contain the capability). A field holds a capability only through the arguments of init, as today (`Memory(initial: replica)`).

### Items of a layer

- The type arguments are determined from the arguments of init. If they are not, they are written (E0459).
- A layer has no body to infer them from, and E0421 compares effects including their type arguments, so the layer needs concrete types where it is declared.
- The arguments of an item of a layer are pure expressions (`[effect.layer]`), so the types they determine are concrete.

### Labels

- The handler on the right of `by` can have type arguments (`L by Memory[Int](initial: 0)`). Today type arguments on the right of `by` are allowed only for an effect.
- They are inferred from the arguments of init and from the underlying effect of L (`Hits by Blank` with `effect Hits of State[Int]` gives S = Int). A label has no type parameters, so its underlying effect has concrete type arguments and fits in one way only. Without this, the body of the with sees only L, and the type arguments would always have to be written.

### Out of scope

- Type parameters on handler members. An inner handler (the implementation of a member) cannot declare type parameters (E0460), but it sees those of its outer handler.

### Names

- A handler is in the type namespace, and a handler with fields also in the constructor namespace (`[module.resolution]`). This does not change.
- The constructor `Memory(value: x)` can be written only inside init (E0418). init returns `Memory[S]` with the handler's own type parameters, so `Memory(value: 1)` in the init of `Memory[S]` is a type mismatch (E0301).
- There is no syntax for type arguments on a constructor in an expression. Inside init, inference is enough.
- A handler in a type position is still E0217. The type of a handler appears only as the type of `self`, which is `Memory[S]`.
- `[module.resolution]` needs no change. The scope of the type parameters is written in `[effect.handler]`.

### Monomorphisation on WASM and native

- Lowering turns a handler into the functions of init and release and a table of the lambdas of its operations (`Expr::Handler`). With type parameters, these become ordinary generic functions, and monomorphisation copies them for each instantiation, as for any function.
- The keys of the evidence table are effect instances with their type arguments, which monomorphisation already rebuilds (`State[$modify.0]` becomes `State[Int]`, `Mono::effect_here`). No new kind of key is needed.
- The parameter types of an operation call are substituted in each copy (the slots of `Callee::Op`), so no new path through `coerce` appears.
- #717 (widening R through a List or a tuple is an internal error on WASM and native) comes from `coerce` not wrapping inside a List or a tuple. A handler with `S = List[fn() -> () uses { Console }]` hits it in the same way as `State.run` with the same type. It is not a new path. This was not run on WASM or native.
- The C and WASM backends monomorphise the IR they receive again, which changes nothing on monomorphic IR.
- The output grows by one copy of the handler for each set of type arguments.

### `State.run`

With handlers with type parameters, `State.run` can be written as a handler:

```emel
pub fn run[S, A, E, R](initial: S, body: fn() -> A fails E uses { State[S], R }) -> (A, S)
  fails E
  uses { R }
{
  with st = Memory(initial:) {
    a = body()
    (a, st.get())
  }
}
```

- A user cannot hold a mutable place (`[dyn.memory]`: mutable state is State, host objects held by handlers, Deferred, Semaphore and Queue), so Memory would be a handler of the bundled core over an `@intrinsic` cell.
- It would remove the special lowering of `State.run`, and let state be an item of a with, as Queue would be.
- This proposal changes neither `State.run` nor adds a handler of State. Making State an item of a with adds a commitment of the standard library, and its name follows the rule chosen for the built-in handlers of Task, so it is a separate proposal.
- Rewriting the inside of `State.run` with a bundled handler does not change what can be observed, so it can be done without changing the specification once this proposal is adopted.

## Specification changes

The changes are applied to `spec/` in this pull request.

- `[effect.decl]`: a handler writes the type arguments of its effects, which may mention its own type parameters.
- `[effect.handler]`: type parameters of handlers, E0462, the type of a handler value, E0460.
- `[effect.with]`: how an item determines type arguments, E0459, overriding, bounds, capabilities.
- `[effect.layer]`: items of a layer determine type arguments from init only.
- `[effect.label]`: type arguments on the right of `by`, inferred also from the underlying effect.
- `[type.rules]`: provides(H) and requires(H) are instantiated.
- `[grammar.decl]`, `[grammar.expr]`: `handler_decl`, `supply` and `by_right`.
- `[diag.list]`: E0304, E0423, E0424, E0459, E0460, E0462.
- `[lsp.hover]`, `[lsp.completion]`, `[lsp.inlay-hint]`: type parameters and inferred type arguments.

## Diagnostics

- [x] E0304 covers handlers
- [x] E0423: new help in the head of a handler
- [x] E0424 covers type parameters of handlers
- [x] E0459 (new): the type arguments of an item are not determined
- [x] E0460 (new): type parameters on an inner handler
- [x] E0462 (new): a type parameter of a handler appears in none of its effects, fields and init parameters

## Compatibility

- Only additions. No program that compiles today changes meaning.
- `handler H[S]` is a syntax error today (E0110), so no existing code conflicts.
- The help of E0423 changes.

## Implementation

- Syntax: type_params in `handler_decl`; type arguments in `supply` and `by_right`.
- Type checking: quantify the signature of a handler over its type parameters (instead of reading its effects in a context without type parameters in `Signatures::handler_effect`). Instantiate it with fresh variables at each item, and unify with the arguments of init, the removal from the R of the body, the uses of x, the requirements of the other items, and the underlying effect of the label on the left of `by`. At the end of the inference of a declaration, report the variables left that are not generalized as E0459.
- Lowering: give the type parameters of the handler to init, release and the table of operations.
- Monomorphisation: copy per instantiation, as for functions. The substitution of evidence keys works as it is.
- Language server: hover, completion, and inlay hints for inferred type arguments at items.

## Alternatives

- Type parameters only on built-in handlers (`@intrinsic`). The same machinery is built, and there is no reason to keep it from users.
- One handler per type (what is done today). It does not work for a `Queue[A]` where A is a type of the user.
- One label per type (`effect JobQueue of Queue[Job]`). A label cannot have a handler (E0433), so a handler with type parameters is still needed.
- Infer type arguments only from the arguments of init, and write them otherwise. The rule is smaller, but a handler without arguments (`with InMemoryRepo`) would always write them. Inference from what uses the provided effects was chosen.
- Infer in a layer as in a with. A layer has nothing to infer from, and E0421 could not be decided.

## Prior art

- Koka: handlers are values, and can be written as polymorphic functions (`fun state(init: s, action)`).
- Effekt: interfaces and their implementations can take type parameters.
- OCaml 5: an effect handler can be installed inside an ordinary polymorphic function, and its types are fixed at the call.

## Open questions

## Decision

The rule against handlers with type parameters had no recorded reason; it came in with the type parameters of effects as a limit of the implementation. `State.run[S]` already provides `State[S]` for every S, so lifting it brings no new problem with the run-time key of handlers, and monomorphisation copies a handler per instantiation as it copies functions.

Task needs a handler that implements `Queue[A]` for every A, and this does it without an exception for built-in handlers. Type arguments are inferred where the provided effects are used, and when inference cannot decide (nothing determines them, several effects fit, or an item may override another), they are written instead of guessed. A layer writes them unless init determines them, since it has no body to infer from.
