# 8. Effects `[effect]`

Side effects can happen only by calling operations declared with effect, through a value obtained with use. A handler holds the implementation, and with provides it.

## 8.1 effect `[effect.decl]`

```emel
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Instant
}
```

| Kind of operation | Meaning |
|---|---|
| `fn` | When the handler returns a value, the caller continues immediately |
| `suspend fn` | The caller may suspend. The handler decides when the caller continues |

- An operation states the types of its parameters and its return value.
- If an operation can fail, its declaration has `fails`. Without it, the operation does not fail. A handler cannot fail beyond the declared range. As a result, replacing the handler does not change the caller's E.
- An effect can have type parameters (`effect State[S]`). Type parameters are limited to ordinary types. Effects with different type arguments are treated as different effects, and in R, `State[Int]` and `State[String]` are distinct. Type arguments cannot be omitted, as in `use State[Int]`. Omitting the type arguments is a compile error (E0423), and a wrong number of them is a compile error (E0304). A type parameter written only in the fails or uses of a function type in an operation's parameters or return value would be a set, so it is a compile error (E0424), but the operation's own `fails S` is accepted as a single error type (7.2), as for fn. In a function type inside a type argument of an effect, omitted fails and uses are empty (as inside declarations in 9.2). However, built-in effects (effects marked `@intrinsic` in the bundled sources; TaskGroup and Spawn, 11.2, 11.3) can have set type parameters and type parameters on the operations themselves (`fn start[A](...)`). Writing type parameters on an operation of a user effect is a compile error (E0426; the diagnostic points at the first type parameter). Users cannot write handlers for built-in effects, so no check is needed for handlers that implement polymorphic operations. The type argument of `use State[S]` can be a type parameter of the enclosing fn. Whether an effect is Immediate is determined by the effect's definition and does not depend on its type arguments.
- Whether two effects in R are the same is decided by comparing them including their type arguments. If a type argument is not yet determined, they are compared by unification, so in `State.run([], || with TestConsole { greet(1) })` the state type is determined from `State[List[String]]` in the body's R. The with rule "a later handler for the same effect takes precedence" and layer duplication (E0421) also count only effects that are the same including type arguments. Type variables left in R are generalized (`fn peek() { modify(|x| x) }` is `∀ A. fn() -> A uses { State[A] }`).
- A handler that implements an effect with type parameters writes its type arguments (`handler Fixed: State[Int]`). They MAY mention the type parameters of the handler (`handler Memory[S](value: S): State[S]`, `[effect.handler]`).
- An operation cannot have a `uses` clause (op_sig in 17.3; writing one is a compile error (E0137), and the diagnostic points at the range of the uses clause). When the implementation of an operation uses another effect, that effect is a requirement of the handler (requires(H) in 9.4) and is satisfied at the with. As a result, replacing the handler does not change the caller's R. A function type in the declaration of an operation is pure unless written otherwise (9.2), and a user effect cannot have a set type parameter (E0424), so an operation cannot take a function that uses other effects. To use a different capability, provide a different handler at the with.
- An effect MAY declare handler members (`handler Open(path: String): ReadFile fails ...`). A handler member is not an operation: it names a handler that every handler of the effect provides. Its parameters are the parameters of that handler's init, and its `fails` is the E of that init. The effect after the colon is the effect that the handler implements.
- A handler member declared `suspend handler` may suspend when it is opened and when the handler opened is released. The init and the release of a handler that implements a handler member without `suspend` MUST satisfy the same condition as the body of an fn operation below (E0417).
- The body of the implementation of a fn operation (one that is not a suspend fn) MUST satisfy one of the following (E0417). Either one alone guarantees that the body does not suspend.
  - The R of the body is Immediate. A body that calls a suspend fn operation inside the with of a handler that resumes on the spot (`fn now() { with InstantClock { (use Clock).sleep(1) } ... }`) passes by this condition.
  - The body contains no call that may suspend.
- When the R of the body is not Immediate, E0417 is reported for each call that may suspend, at the position of that call. The search covers the subexpressions of the body, excluding the bodies of anonymous functions. A call inside an anonymous function counts as a call of a function value where that anonymous function is called.
- Whether a call may suspend is determined as follows.
  - A call of an operation (`clock.now()`, `(use Clock).now()`) is determined by the declaration of the operation. If it is a `suspend fn`, the call may suspend.
  - A call of a function or a function value is determined by the R of the callee's type. If that R is not Immediate, the call may suspend. Calling a closure that captures a capability (`f = || clock.now()`) or an operation taken out as a value (`g = clock.now`) is also a call of a function value.
  - A call of an `@external suspend fn` may suspend.
  - Opening a handler member (`x = E.M(arguments)` in a with, or `E.M(arguments)` inside an init, 8.3, 8.4) is determined by the declaration of the handler member. If it is a `suspend handler`, it may suspend. For a with item, the end of the with, where the handler opened is released, may suspend too.
- If the body contains no call that may suspend, its R is not restricted. The R of `fn now() { (use Clock).now() }` contains Clock, which becomes a requirement of the handler. Mixing `now` of the outer Clock and `sleep` inside an inner `with InstantClock` in the same body satisfies neither condition and is a compile error (E0417). Accepting it would require looking, for each call, at the innermost with that provides its effect, which makes the rule heavier, so this is not done. A call through a function is treated as possibly suspending even if that function calls only fn operations (`fn now() { stamp() }`, which calls `fn stamp() -> Instant uses { Clock }`, is a compile error (E0417)). That a function does not suspend is not inferred from its body. The implementations of fn operations in every handler are bound by the same rule, so a call of a fn operation does not suspend whichever handler it reaches.
- An operation that involves waiting (waiting on an `@external suspend fn`, calling a suspend operation) MUST be declared as a suspend fn on the effect side. Otherwise its implementation is a compile error (E0417). This is because the caller can learn whether a call suspends only from the effect's declaration. Only operation declarations of effects carry a mark of whether they suspend. Function signatures (`uses { Http }`) and calls do not.
- An effect declared in source MUST have at least one operation or handler member (E0446). The effects without operations and handler members are the host effects, which only the compiler declares (`[effect.host]`).

## 8.2 use `[effect.use]`

```emel
fn greet(id: Int) {
  log <- Logger
  users <- Users

  user = users.find(id) |> Option.or_fail(NotFound(id:))
  log.info("hello #{user.name}")
  user
}
```

- `x <- E` is a statement that declares x as an effect alias of E for the rest of the block. It does not make a value. E is an effect, with its type arguments, or a label. A type annotation cannot be written on it.
- `x.op(arguments)` calls the operation op of the handler of E that is innermost at the time the call is evaluated, and adds E to the R of the innermost function that contains the call. Inside an anonymous function or a local fn, the withs that enclose the function do not provide E to the call; the side that calls the function provides it.
- `x.op` without a call gives a function value whose R contains E. E is resolved each time the function value is called.
- x can be written only as the receiver of `x.op(...)` or `x.op`. Writing x anywhere else is a compile error (E0453). A handler member cannot be opened through x (E0453).
- `(use E).op(arguments)` means `{ h <- E; h.op(arguments) }`, where h is a fresh name. `(use E).op` without a call is expanded in the same way, and gives a function value that resolves E each time it is called.
- `(use E).M(arguments)` that opens a handler member is not expanded into an effect alias. It opens the handler member M of the innermost handler that provides E at that position.
- `use E` written anywhere else is a capability value. It refers to the handler that provides E at that position, and keeps referring to it.
- `use E` written as an expression, in either form, is deprecated (W0403). It is read for one more minor version. In 0.29, `use E` written as an expression is a capability of the name E in types. From 0.30 it is a compile error.

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

- `use X` is an expression that returns a value representing the capability of effect X. It can appear anywhere in a function.
- Operations can be called only through a capability value, in the form `log.info(...)`. There is no form that calls them directly by effect name (`Logger.info(...)`).
- Writing an operation of a capability value without calling it, as `log.info`, gives a function value. The capability value is evaluated at that position (`c.op` where c is a capability value resolves at the position where it is written), and the type of the function value contains the capability's effect (as for a closure that captures a capability).
- Calling an operation of a capability adds that capability to R. The with that made the capability removes it from R. Whether a call may suspend (E0417) is decided by the effect of the capability.
- A capability is not widened to its effect. An effect set that contains a capability is not included in an effect set that contains only the effect of the capability.
- If the type of a value that a closure or a local fn captures contains capabilities, including through capability-bound types (9.4), those capabilities are in the R of the function.
- In an operation body of a handler, calling an operation of a capability held in a field through `self` (`self.inner.read(max)`) does not make its effect a requirement of the handler. The capability belongs to a with that encloses the with of the handler, or to a handler member opened in its init (8.4); both outlive the handler. The call still counts as a call of its effect for whether the body may suspend: it may suspend if the operation is a suspend fn, and the R of the operation body is not Immediate if the effect is not (8.1, E0417). The same holds in release, except that if the operation is a suspend fn, Async is a requirement of the handler (9.4), so that the R of the with shows that leaving it may suspend.
- Calling an operation adds the operation's `fails` to E.
- The type of a capability of E is written `use E` (or `use E1 + ... + En`). E is an effect, with its type arguments, or a label. `use E` is a type, and can be written wherever a type can. Type checking records in it a set of capabilities. Unification takes the union of sets. Calling an operation of a value whose type has a set adds every capability of the set to R.
- A parameter of type `use E` gives the function a type parameter that ranges over sets of capabilities of E, decided at each call by the argument. An effect alias cannot be an argument (E0453). The result of a function can be a capability type; its set comes from the parameters.
- `use E` written as a parameter of a function type is quantified at that function type. A closure checked against such a type receives an unknown set, which cannot be in its result. A closure without an expected type is not quantified. A quantified type is never inferred.
- A capability value can be passed as an argument, put into a data structure, and returned from a function, as any other value. It cannot leave the with that made it (8.6).
- Only the operations of E can be called on a value of type `use E`, or taken as function values. There is no operation that inspects a capability value: no trait other than Capability is satisfied by `use E`, and `==` is not defined for it.
- `use E` satisfies the built-in trait Capability (10.6). A type parameter with `[C: Capability]` accepts a capability of any effect.
- The type of a capability of several effects is written `use E1 + ... + En`. A capability can be passed to a parameter of type `use F1 + ... + Fm` when every Fi is one of the Ej. Only the operations of the Fi can be called through the parameter. There is no way to recover the other effects from it. Passing a capability that does not have some Fi is a compile error (E0452).
- A property that a type has to guarantee, such as contents that do not change or exclusive access, is expressed by a separate effect that no handler implements together with an effect that breaks the property.
- On a capability c of an effect E that has handler members, `c.M(arguments)` opens the handler member M of the handler that provides c. It can be written only as a with item `x = c.M(arguments)` (8.3) and inside an init (8.4).
- Writing an operation that the capability value's effect does not have is a compile error (E0401). The labels of named arguments are the parameter names in the effect's operation declaration.
- The effect set R of a function that contains `use X` includes X. Even when written inside a branch, X is in the R of the whole function.
- Calls carry no mark (such as await or `!`) that indicates side effects or suspension. Suspend operations are written in the same form as ordinary calls.
- To let a function use the capability of a specific handler, pass the capability.

```emel
fn copy(from: use Db, to: use Db) {
  to.execute(from.query("..."))
}

with primary = PrimaryDb {
  with replica = ReplicaDb {
    copy(replica, primary)
  }
}
```

## 8.3 with `[effect.with]`

```emel
with ConsoleLogger { greet(1) }

with AppTest, FixedClock { greet(1) }
```

- `with H { body }` provides the effects that H implements within the body. Its value is the value of the body.
- Provided effects are removed from the R of the body. However, as with escape, they are not removed from opaque effect sets (such as the hidden set of a parameter's function type). The R of `fn run(f: fn() -> ()) { with ConsoleLogger { f() } }` remains the effect set of f (7.3).
- To remove an effect precisely in the types as well, write the provided effect out in the function type (`uses { Logger, R }`; the same as `fails Timeout | E` for errors). An effect written out is outside the set variable, so with removes it.

```emel
fn run[R](f: fn() -> () uses { Logger, R }) -> () uses { R } {
  with ConsoleLogger { f() } # R is just R. Logger is removed
}
```
- The requirements of handlers listed in the same with can be satisfied by an item without a name in the same with that provides a different effect. A named item provides nothing by name (below), so it does not satisfy those requirements. A requirement on the handler's own effect is satisfied by the outer side. A cycle among the requirements of handlers in the same with is a compile error.
- H is a handler or a layer. Label items (`L`, `L by P`) can also be written (8.9). When several items without a name are listed with commas, a later one takes precedence for the same effect. However, an explicit `L by P` takes precedence over provision through `default` (8.9) regardless of order. A layer is expanded into its handlers before being listed. A handler overridden by a later handler is not even initialized (the E of its init is not included in the E of the with). A named item (`x = Item`, below) provides nothing by name, so it is never overridden, and it is always initialized.
- An item can be written as `H(arguments)`. The arguments are evaluated outside the with, in the order the items are written, before any init runs. The effects provided by the items of the same with cannot be used in the arguments. The E and R of the arguments of a handler that is not overridden are added to the E and R of the with. The arguments of a handler overridden by a later item are not evaluated, and their E and R are not included in the E and R of the with; they are still resolved and type-checked.
- For a handler with type parameters (`[effect.handler]`), an item determines its type arguments once, at the position of the item, as a call determines the type arguments of a function; the handler is not polymorphic inside the body. They are inferred from the arguments of init; for an item without a name, from the R of the body, by the unification that removes the effects the item provides (`[effect.decl]`), and from the requirements of the other items of the same with that the item satisfies; and, for `x = Item`, from how x is used. They MAY be written after the name (`with Memory[Int](initial: 0)`, `with InMemoryRepo[UserId, User]`). Writing type arguments for a handler without type parameters, or a wrong number of them, is a compile error (E0304). The bounds of the type parameters are checked at the item (E0326).
- Type arguments of an item that are not determined are a compile error (E0459). This is decided when the inference of the enclosing top-level declaration ends: a type argument left as a variable that appears in the type of the declaration is generalized with it (`fn peek() { with x = Pass { x.get() } }` is `∀ S. fn() -> S uses { State[S] }`), and only one that does not appear there is E0459.
- When the R of the body, together with the requirements of the other items of the same with, has two or more effects that unify with an effect that an item without a name provides (`State[List[Int]]` and `State[List[String]]` in the body of `with Empty`), and the arguments of init do not narrow them to one, it is a compile error (E0459). The item does not pick one of them.
- Whether a later item without a name overrides an earlier one for the same effect is decided only with the effects whose type arguments are determined by the arguments of init or written. If an effect of an item without a name whose type arguments are not determined unifies with an effect provided by another item without a name of the same with (including the handlers of a layer), it is a compile error (E0459); the type arguments are written (`with AppLive, InMemoryRepo[UserId, User]`).
- The type arguments of an item are fixed at the position of the with, outside its body. They cannot contain a capability made inside the body.
- `use X` refers to the handler provided by the innermost with as seen from the position where it is written. If the same function has no such with, it refers to the one passed from the caller.
- The handlers that the calls inside an anonymous function or a local fn need are passed by the side that calls the function. The withs that enclose the function do not provide them. A capability value that the function captures keeps referring to its handler. This is because the R of these functions is enclosed in their types (9.4) and does not appear in the R of the enclosing function.
- The capability of a named item keeps referring to its handler even if an inner with overrides the same effect.
- The R of main MUST be empty except for the host effects that main is given (Async and ProcessHost, `[effect.host]`). main is the fn named `main` in the entry module. If the entry has no main, `emela run` reports a compile error (E0230). Something named `main` that is not a fn (such as a const) does not count as main. `emela build` MAY produce output even without main. The R MUST be empty even if a uses clause is written. The R of the value of a const is also empty. The same holds for an `@export` fn (`[ext.export]`).
- The return type of main is `()` or `ExitCode` (`[api.std.process]`); another type, other than Never, is W0304 (`[dyn.error]`). This is separate from the rule on R above.
- Each item of a with runs its init and makes a capability. Leaving the with releases it, and the capability ends.
- An item can be written as `x = Item` (a named item). Within the body, x is bound to the capability of the item (of type `use F`, 8.2), and refers to the handler of that item even when another item of the same with provides the same effect. An item without a name provides its effect by name only. A named item provides nothing by name: the effects that the body uses by name (through the R of a function the body calls, or through an effect alias) are provided by the withs that enclose this with, or by the caller. The rule holds for every item that can be bound: a handler (`x = H`), a handler member (`x = E.M(...)`, `x = c.M(...)`) and a label plugged into a handler (`x = L by H`). To provide an effect by name, the item is written without a name.
- `E.M(arguments)`, where E is an effect and M one of its handler members, can be written as an item only in the form `x = E.M(arguments)`. It calls the init of M of the innermost handler that provides E at that position. `x = c.M(arguments)`, where c is a capability of E, calls the init of M of the handler that provides c. The arguments are treated as for `H(arguments)`. The item requires E (or c), not the requirements of the inner handler.
- As for every named item, the effect of a handler member item is used only through the bound capability.
- A handler member cannot be an item of a layer, and a layer cannot be bound (`with x = AppLayer` is a compile error), since it provides more than one effect.
- A label item that makes no handler cannot be bound (E0447): `x = L`, and `x = L by P` where P is the underlying effect of L or another label of it. Such an item only refers to a handler that the item does not make, and a named item provides nothing by name, so the item would do nothing but fix x to that handler. To use the label in the body, the item is written without a name, and the body declares an effect alias (`with AuditLog by Console { audit <- AuditLog; ... }`, `[effect.use]`).
- A handler that implements several effects provides all of them. Its init and release run once.

```emel
with primary = PrimaryDb {
  with replica = ReplicaDb {
    replica.query("...") # ReplicaDb
    primary.execute("...") # PrimaryDb
  }
}
```

## 8.4 handler `[effect.handler]`

```emel
handler ConsoleLogger: Logger {
  fn info(msg) { console_log(msg) }
}

handler PostgresUsers(conn: Connection): Users {
  init {
    config = use Config
    PostgresUsers(conn: pg_connect(config.db_url()))
  }

  release {
    pg_close(self.conn)
  }

  fn find(id) { ... self.conn ... }
}

handler SystemClock: Clock {
  suspend fn sleep(ms, resume) {
    set_timeout(ms, || resume(()))
  }
  fn now() { date_now() }
}
```

- A handler is written as `handler Name: Effect`. The colon means that the left side satisfies the right side (the same as `[A: Ord]`, `trait Ord: Eq`, and `impl User: Show` (10.2)). Writing `handler H implements E` is a compile error (E0145), and the diagnostic shows the form `handler H: E`. The diagnostic points at the `implements` token.
- A handler is a declaration, and it implements each operation of the effects after its colon exactly once (`handler SystemTx(conn: Connection): Query + Execute`). All of them share the same `self`. Listing two effects that have an operation of the same name is a compile error (E0450). Listing the same effect twice is a compile error (E0451). The types of an operation's parameters and return value are determined by the effect's declaration, so they are not written. The number of parameters MUST be the same as in the declaration. A missing operation is a compile error (E0404, which lists all missing operations), implementing the same operation twice is a compile error (E0405), and a different number of parameters is a compile error (E0406). Implementing an operation that the effect does not have is a compile error (E0212).
- Whether an operation has `suspend` MUST match the effect's declaration (E0415). When it does not match, the number of parameters and the body are checked according to the form of the implementation.
- The E of an operation's body MUST be contained in the `fails` of the operation's declaration. requires(H) (9.4) is the union of the Rs of init, release, and the operation bodies.
- A handler with state declares fields after its name. Fields are referenced through `self`, as in `self.conn`.
- A handler MAY declare type parameters after its name, before its fields (`handler Memory[S](value: S): State[S]`). Type parameters are limited to ordinary types. A type parameter that appears only in the fails or uses of a function type would be a set, so it is a compile error (E0424). A type parameter MAY have bounds (`[K: Eq]`), and the functions of the bounds can be called in the handler. The type arguments are determined at each item of a with, a layer, or the right of `by` (`[effect.with]`, `[effect.layer]`, `[effect.label]`).
- Each type parameter of a handler MUST appear in the effects after its colon, in the type of a field, or in the type of a parameter of its init (E0462). A type parameter that appears only in the bodies of init, release or the operations, or nowhere, is an error: the type of a handler cannot be written in a type position, so such a parameter would tell nothing apart.
- The type parameters of a handler are visible in its fields, in the effects after its colon, and in its init, release, operation bodies and inner handlers.
- The type of a handler value with type parameters is the handler applied to them (`self: Memory[S]`). init returns a value of that type: inside the init of `Memory[S]`, `Memory(value: 1)` does not have the type `Memory[S]` and is a type mismatch (E0301).
- An inner handler cannot declare type parameters (E0460). It can use the type parameters of its outer handler.
- `init` runs once when entering the with and returns the handler value. It is required for a handler with fields (E0419). A handler without fields can also have init and release. The init of a handler without fields returns the value `ConsoleLogger` for a handler without parentheses, and `H()` for `handler H()`. self cannot be used inside init (because the value has not been made yet). In the operations of a handler without state, self is still the handler value and its type is the handler type (`ConsoleLogger`), as in a handler with state. It has no fields to access (E0312).
- A handler value (`PostgresUsers(conn: ...)`) can be made only inside that handler's init. If it could be made outside, init and release could be skipped. Making it outside is a compile error (E0418). The inside of an anonymous function inside init counts as inside init. A record update (`H(n: 1, ..self)`) also makes a value, so it cannot be written in an operation body. A constructor pattern does not make a value, so it can be written anywhere.
- Inside the init of a handler, its value can also be constructed as `Self(...)` (`Self` for a handler without parentheses, `Self()` for `handler H()`). It has the same meaning as writing the handler's name, and the same restriction (E0418). `Self` is not a constructor pattern, and does not construct types in traits and impls.
- The `use` expressions and the effect aliases inside `init` are the requirements of the handler. The effect can be named by an effect alias (`config <- Config`), or the capability value can be bound and used (`config = use Config`) or written inside an expression (`(use Config).db_url()`). All of them become requirements. There is no syntax for writing requirements in the handler's header.
- `init` MAY take parameters (`init(url: String) { ... }`). Their types MUST be written. Arguments are passed by writing `H(url: ...)` as an item of a with or a layer, or on the right side of `by` (`[effect.with]`, `[effect.layer]`, `[effect.label]`). A handler whose init takes no parameters cannot be given arguments, and `H()` cannot be written for it.
- A field of a handler can have the type `use E`. The capability it holds belongs to a with that encloses the with of the handler, because the arguments of init are evaluated outside the with (`[effect.with]`). Calling its operations through `self` does not make its effect a requirement of the handler. In an operation body, it counts for whether the body may suspend; in release, calling a suspend fn that way makes Async a requirement of the handler (8.2).
- The set of a field of a capability type or a capability-bound type (9.4) is decided when an item of a with runs the init.
- A parameter of type `use E` of an operation is decided at each call and is an unknown set in the implementation; it cannot be stored in a field or in State.
- In an init, the fields given to `Self(...)` MUST NOT record a capability made by a with inside the init, or the name of an effect that a with inside the init provides by name (E0455). The capabilities received as arguments of init, and the handlers opened by the init outside such a with, can be stored.
- The result type of an operation MUST NOT contain a capability type, or a capability-bound type of another effect (E0456). The result type of an operation of E can contain a capability-bound type of E itself (`start` of TaskGroup returns `Task[A]`); the value records the capability the operation is called on, or the name E when it is called through the name of E (9.4).
- `self` in an operation body and in release is the capability of the handler, with the added power of reading the fields. It can be passed to a parameter of type `use E`. A closure that captures `self` has the capability in its R.
- A handler of an effect with handler members implements each of them with an inner handler of the same name (`handler Open(handle: Handle, path: String): ReadFile { ... }`). The parentheses after the name declare its fields, and its init MUST take the parameters of the handler member, with the same names and types. A missing handler member is E0404, as for an operation. If the handler member is not a `suspend handler`, the init and the release of the inner handler are checked as the body of an fn operation (E0417, 8.1).
- The effects used in the init, the release and the operation bodies of an inner handler are requirements of the outer handler (requires(H), 9.4). They are provided where the outer handler is provided.
- An inner handler cannot be referred to by name outside its outer handler, and cannot refer to the `self` of its outer handler.
- Inside an init, a handler member can be written as an expression. `E.M(arguments)` runs the init of the handler member M of the innermost handler that provides E, and `c.M(arguments)`, where c is a capability of E, runs that of the handler that provides c. Its value is the capability of the handler opened. The handler opened lives as long as the handler whose init opened it: it is released after that handler's release, in the reverse order of opening. If the init fails, the handlers it opened so far are released in reverse order. A handler opened through a handler that a with inside the init provides does not outlive that with: it is released at the end of that with, before the release of the handler that provides it, in the reverse order of opening. A handler member of the handler's own effect refers to the outer handler.
- `H(...)` at a with is a call of init, not a construction of the handler value. As before, the handler value can be constructed only inside init (E0418).
- `release` runs once when leaving the with. It runs in every case: normal completion, error, defect, or interruption.
- The E of release MUST be empty (E0420, reported for each expression that can raise an error). Cleanup that can fail escapes inside release or turns the failure into a defect with panic. The value of release is discarded, so its type does not matter. release MAY call suspend operations (it is not interrupted; 11.5). release is not a fn operation, so the E0417 check does not apply to it.
- The implementation of a suspend fn receives `resume` as its last parameter. Calling `resume(v)` resumes the suspended caller, and the return value of the operation is `v`.
- The implementation of a suspend fn has the parameters of the operation plus one for resume (the count for E0406 also includes resume). The last parameter is resume, and its name does not matter. The type of resume is `fn(T) -> () fails {} uses {}` (T is the return type of the operation). The value of the implementation body is discarded, so the type of the body MUST be `()`.
- `resume` MUST NOT be called more than once. A second call is a defect (`[dyn.suspend]`). It can be called on the spot or later in a callback.
- In the implementation of a suspend fn, a call that may suspend (8.1) MUST NOT be evaluated after resume may have been called (E0461). resume may have been called after a direct call of it (`resume(v)`), and after any other use of it (8.7: storing it in a variable, capturing it in a closure, passing it as an argument), since resume can then be called from a callback or from another fiber while the implementation is still running. The check is syntactic and errs on the safe side, as the check for Async (8.7): a call that may suspend is reported if some path through the body reaches it after a direct call or another use of resume, including a path through only one branch of an if or a match. Work that has to continue after the caller has resumed is started with Spawn (11.3), which makes Spawn a requirement of the handler.

```emel
handler RemoteLog: Logger {
  suspend fn info(msg, resume) {
    spawn <- Spawn
    spawn.start(|| send(msg))
    resume(())
  }
}
```

## 8.5 layer `[effect.layer]`

```emel
layer AppLive {
  EnvConfig
  ConsoleLogger
  SystemClock
  PostgresUsers # uses Config in init
}

fn main() {
  with AppLive { run_server() }
}
```

- A layer is a declaration that gives a name to a bundle of handlers. It lists handlers or other layers.
- The order of the list has no meaning. The compiler determines the initialization order from requires(H) (the R of init, release, and operation bodies; 9.4), and release runs in the reverse order. This is because effects used by operation bodies and release are also provided by handlers initialized earlier (14.2). Where the requirements do not determine the order, the written order is kept.
- An item of a layer can be written as `H(arguments)`. The arguments are limited to pure expressions (E and R are empty), as for the right-hand side of a const (`[data.const]`): `use` and `fail` cannot be written, and function calls can. The order of the items has no meaning, so arguments with E or R would have no defined order of evaluation. To pass a value that is not pure, list the handler after the layer in the with (`with AppLive, WorkerKv(binding: name) { ... }`).
- The type arguments of a handler with type parameters listed in a layer are determined from the arguments of its init. If they are not, they MUST be written (`InMemoryRepo[UserId, User]`); otherwise it is a compile error (E0459). A layer has no body to infer them from, and E0421 compares effects including their type arguments.
- A layer in which two handlers provide the same effect is a compile error (E0421), and layers that include each other are a compile error (E0422). The error is reported only once, at the layer declaration.
  - E0421 is reported at the position of the member that brought in the second handler. Listing the same handler twice is also E0421. For effects with type parameters, only effects that are the same including type arguments are counted, and the message shows them in the form `State[Int]`.
- A handler that implements several effects counts as providing each of them for E0421.
  - E0422 is reported once, at the position of the layer defined first in the cycle.
  - A cycle in handler requirements (E0411) is reported at the position of the layer member.
  - Duplication and cycles that occur only within a nested layer are reported in the inner layer and are not repeated in the outer layer. In a with, too, cycles that occur only within one layer are not reported.
- A requirement not satisfied within the layer is not a compile error and remains in the R of the with (the same as requires(H) in 8.3). The R MUST be empty in main and tests, so it becomes a compile error there. A layer that receives some of its requirements from outside can be written.
- To replace only some handlers, list them after the layer, as in `with AppTest, FixedClock { ... }`.

## 8.6 Capabilities must not leak `[effect.escape]`

A capability cannot leak.

```emel
fn make_greeter() {
  with log = ConsoleLogger {
    || log.info("hi") # E0455: the R of the closure contains log, which leaves the with
  }
}
```

- If the type of the value of a with body, the error type that leaves the body, or the R of the with contains a capability made by that with, it is a compile error (E0455).
- A with that provides E by name counts the name E in types as its own capability.
- A function provides E by name to an argument when the R of the function type of that parameter has E written out and the R of the function does not have E (`body` of `State.run`, and `app` of `Http.serve` for Spawn). At a call of such a function, if the result type or the error type of the function passed to that parameter contains the name E, it is a compile error (E0455). A hidden set variable does not write E out, so it does not provide E by name.
- A type fixed outside the with (the type argument of an outer `State`, the parameter types of an operation of a handler provided outside) cannot contain the capability, so other paths out do not type check. They are reported as E0455 too, at the place where the capability enters the type fixed outside.
- A closure that is used up inside the with (`List.map(names, |n| log.info(n))`) is fine.
- An effect alias (8.2) is not a capability. A closure that captures it has E in its R, which the caller provides.

## 8.7 Async `[effect.async]`

- Async is a host effect (`[effect.host]`) that represents "being resumed later". The rules of `[effect.host]` apply: only the host provides it, `handler H: Async` is a compile error (E0413), and `use Async` is a compile error (E0414).
- The means of calling `resume` later (callbacks of external functions, `@external suspend fn`, starting a task, 11.2, 11.3) require Async. Therefore SystemClock requires Async, and Async remains in the R of `with SystemClock { ... }`.
- When the body of a suspend fn implementation uses `resume` in any way other than calling it directly (`resume(v)`), the compiler assumes that resume may be called later and adds Async to the handler's requirements. Uses other than direct calls include storing it in a variable, capturing it in a closure, and passing it as an argument. Calling an `@external suspend fn` also adds Async. This decision is purely syntactic and errs on the safe side.
- A handler that resumes on the spot (such as the test handler InstantClock) does not require Async. `with InstantClock { ... }` does not actually suspend, and Async does not appear in the types either.
- The constraint Immediate, which requires that something does not suspend, is defined in 9.5.

## 8.8 State `[effect.state]`

Mutable state is represented by State, an effect in the bundled core. The state lives in the frame of `State.run`, and values themselves stay immutable.

```emel
pub effect State[S] {
  fn get() -> S
  fn set(value: S) -> ()
  fn update(f: fn(S) -> S) -> ()
}

pub fn run[S, A, E, R](
  initial: S,
  body: fn() -> A fails E uses { State[S], R },
) -> (A, S) fails E uses { R }
```

- `State.run(initial, body)` provides a state whose initial value is initial, runs body, and returns the pair of the value of body and the final state. State[S] is removed from the R of body, so from outside it looks pure.
- When body fails, the state is discarded and only the error propagates. To also get the state at the point of failure, escape inside body and turn the error into a value.
- Even if body suspends, the state carries over after it resumes.
- The bundled TestConsole's `print_line` appends lines to the end of the state in the order they are written, and `print_error_line` discards them. TestClock runs on fake time. `sleep` really waits, and when no fiber can make progress and no host work is pending, it advances the time in the state to the earliest scheduled wake-up time and wakes that fiber. Fibers wake in order of their scheduled times, and fibers with the same time wake in the order they called sleep. With a single fiber, the result is the same as sleep advancing the time and continuing directly. TestClock requires Async, so it cannot be used where Immediate is required. A negative `ms` is treated as 0 (the same applies to SystemClock).
- The function passed to update is pure, because it is a function type inside an operation declaration (9.2). Therefore update runs atomically, and even when the state is shared between fibers, intermediate states are not visible (11.1).
- To have two states of the same type, wrap them in different types to distinguish them (`State[Hits]` and `State[Misses]`).
- A handler that uses state is written as a handler that requires State. Test handlers take this form.

```emel
handler TestConsole: Console {
  fn print_line(text) {
    st = use State[List[String]]
    st.set(List.append(st.get(), [text]))
  }
  fn print_error_line(text) { () }
}

@test
fn greets() {
  (_, lines) = State.run([], || with TestConsole { greet(1) })
  assert lines == ["Hello, alice!"]
}
```

## 8.9 Labels `[effect.label]`

To provide the same effect with different handlers in different places, use labels. A label is a separate effect that has the same operations as some effect.

```emel
effect AuditLog of Console # must be provided
effect DebugLog of Console default # provided together by a with that provides Console

fn report() -> () uses { AuditLog, DebugLog } {
  with AuditLog { audit() } # AuditLog serves the Console inside audit
  with DebugLog { debug() }
}

fn main() {
  with SystemConsole, AuditLog by FileLog { report() } # SystemConsole serves DebugLog
}
```

- `effect L of E` declares an effect L that has the same operations as E. L is called a label of E, and E is called the underlying effect of L. In R, L and E count as different effects. The operations of a capability value obtained with `use L` are the operations of E, and calling them adds L to R. Whether L is Immediate (9.5) is determined by E.
- A label L of E has the handler members of E. `x = L.M(arguments)` in a with opens M of the handler that provides L, and requires L. On `c: use L`, `c.M(arguments)` opens M of the handler that provides c.
- The underlying effect MUST be a user effect that has operations or handler members. Writing a label, or a built-in effect such as Async or TaskGroup, is a compile error (E0429). This keeps the underlying effect always determined in one step, and gives `default` only one meaning. To separate the contents, declare `effect X of Console` flatly and plug it in with `X by AuditLog`.
- A label of an effect with type parameters writes the type arguments of the underlying effect (`effect Hits of State[Int]`). The label itself has no type parameters, so only concrete types can be written. The types of the operations are determined by those type arguments. The effect to the right of `by` can also have type arguments (`Hits by State[Int]`). Omitting the type arguments is a compile error (E0423). Labels can also be used to have two states of the same type (8.8).
- Writing a handler for a label is a compile error (E0433). Plug in a handler of the underlying effect with `by` instead.
- In addition to handlers and layers, a with can contain the following items.
  - `L`: within the body, the provider of L serves E. Provides {E}; requires {L}.
  - `L by P`: plugs P into L. P is a handler of E, E itself, or another label of E. A built-in handler that has the name of its effect is a handler here (`[module.resolution]`). Provides {L}; requires the requirements of P if P is a handler, or {P} if P is an effect. If P serves some other effect, it is a compile error (E0431). Writing E as P removes the distinction of L and goes back to the outer E (merging).
  - In `L by H`, where H implements several effects, H serves L through the effect that is the underlying effect of L. If none of them is, it is E0431.
- The right side of `by` can be written as `H(arguments)`, and with type arguments for a handler with type parameters (`Hits by Memory[Int](initial: 0)`). The arguments and the type arguments are treated as for an item of a with, except that the type arguments are inferred from the arguments of init and from the underlying effect of L (`Hits by Blank`, with `effect Hits of State[Int]` and `handler Blank[S]: State[S]`, gives S = Int). The underlying effect of a label has concrete type arguments, so it fits in one way only.
- Requirements are satisfied and the initialization order is determined in the same way as for handlers (8.3, 14.2). In `with SystemConsole, AuditLog by Console`, the Console on the right is satisfied by SystemConsole in the same with.
- Writing an effect that is not a label in a with or on the left of `by` is a compile error (E0430). Writing `by` twice for the same label in the items without a name of the same with is also a compile error (E0432). Named items provide nothing by name (`[effect.with]`), so `with a = AuditLog by FileLog, b = AuditLog by SystemConsole` is not E0432.
- A label with `default` is provided together by an item without a name that provides E. This is the same as implicitly adding `L by E` to the item that serves E (a handler, or a label item such as `M`), and L falls to the provider of the innermost with that provides E. If the same with has `L by P`, that item takes precedence regardless of order (8.3). L is removed only from the R of the body, and it does not satisfy the requirements of other handlers in the same with. L inside a hidden set is not removed (8.3). A named item provides nothing by name (`[effect.with]`), so it does not provide L either.
- If a label without `default` is not provided, R is not empty in main and tests, which is a compile error (E0402). This prevents an audit log from silently mixing into standard output. To mix them, the caller writes `L by E` explicitly.
- Function signatures write the label as is (`uses { AuditLog, DebugLog }`). Whether it falls to E is decided by the caller, so E is not written. The LSP also shows types with the label as is, and on the name of a label it shows the declaration (`effect DebugLog of Console default`).
- A label item (`L`, `L by P`) that appears neither in the R of the body nor in the requirements of other items in the same with produces a warning (W0401). No warning is produced when the R of the body includes a hidden set. No warning is produced for a named label item (`x = L by H`): it provides nothing by name (`[effect.with]`), and it is used through x.
- A label has no body. Writing `{ ... }` after the declaration is a compile error (E0122).
- Labels are distinguished by name (nominal). Two separately declared labels are different effects even if their underlying effect is the same, and labels with the same name in different modules are also different.
- An `L` item, and an `L by P` item where P is an effect or a label, do not create a new handler. They provide the outer handler (or that of an item initialized earlier in the same with) again under the name L. An `L by P` item where P is a handler initializes P and provides it.

## 8.10 Host effects `[effect.host]`

- A host effect is an effect whose only handler is the host. `Async` (`[effect.async]`), `ProcessHost` and `PageHost` are the host effects.
  - `Async`: the host can resume a computation later.
  - `ProcessHost`: the host is a process of the operating system, with arguments, standard input and output, environment variables and a file system.
  - `PageHost`: the host is a page of a web browser.
- Users cannot write a handler for a host effect. `handler H: ProcessHost` is a compile error (E0413), and that handler provides nothing even when written in a with. `use` of a host effect is a compile error (E0414). A host effect can be written in a uses clause (`fn f() uses { ProcessHost }`).
- A host effect enters R only through what host functions need. Calling an `@external suspend fn` adds Async (`[effect.async]`). Calling an external function declared with `uses { ProcessHost }` or `uses { PageHost }` adds that host effect (`[ext.call]`). A handler whose init, release or operation bodies make such calls requires the host effect (requires(H) in `[effect.handler]`), so the host effect remains in the R of a with that lists the handler, or a layer that contains it.
- A with does not remove a host effect. Only the entry of a launch provides host effects.

| Entry | Host effects provided |
|---|---|
| `main` (`[effect.with]`) | Async, ProcessHost |
| A test function (`[test.constraints]`) | Async, ProcessHost |
| An `@export` fn that is not an entry adapter (`[ext.export]`) | Async |
| An entry adapter (`[ext.entry]`) | Async, and the host effect the adapter declares |

- `ProcessHost` and `PageHost` are Immediate (`[type.effect-bound]`).
- Host effects do not name targets. A program whose entry type checks runs on every target that supports that entry (`[target]`).
