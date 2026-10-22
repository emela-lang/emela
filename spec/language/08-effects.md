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
- An effect can have type parameters (`effect State[S]`). Type parameters are limited to ordinary types. Effects with different type arguments are treated as different effects, and in R, `State[Int]` and `State[String]` are distinct. Type arguments cannot be omitted, as in `use State[Int]`. Omitting the type arguments is a compile error (E0423), and a wrong number of them is a compile error (E0304). A type parameter written only in the fails or use of a function type in an operation's parameters or return value would be a set, so it is a compile error (E0424), but the operation's own `fails S` is accepted as a single error type (7.2), as for fn. In a function type inside a type argument of an effect, omitted fails and use are empty (as inside declarations in 9.2). However, built-in effects (effects marked `@intrinsic` in the bundled sources; Scope, 11.2) can have set type parameters and type parameters on the operations themselves (`fn fork[A](...)`). Writing type parameters on an operation of a user effect is a compile error (E0426; the diagnostic points at the first type parameter). Users cannot write handlers for built-in effects, so no check is needed for handlers that implement polymorphic operations. The type argument of `use State[S]` can be a type parameter of the enclosing fn. Whether an effect is Immediate is determined by the effect's definition and does not depend on its type arguments.
- Whether two effects in R are the same is decided by comparing them including their type arguments. If a type argument is not yet determined, they are compared by unification, so in `State.run([], fn() { with TestConsole { greet(1) } })` the state type is determined from `State[List[String]]` in the body's R. The with rule "a later handler for the same effect takes precedence" and layer duplication (E0421) also count only effects that are the same including type arguments. Type variables left in R are generalized (`fn peek() { modify(fn(x) { x }) }` is `∀ A. fn() -> A use State[A]`).
- A handler that implements an effect with type parameters states concrete types (`handler Fixed: State[Int]`). There are no generic handlers that implement the effect for every type.
- An operation cannot have a `use` clause (op_sig in 17.3; writing one is a compile error (E0137), and the diagnostic points at the range of the use clause). When the implementation of an operation uses another effect, that effect is a requirement of the handler (requires(H) in 9.4) and is satisfied at the with. As a result, replacing the handler does not change the caller's R. A function type in the declaration of an operation is pure unless written otherwise (9.2), and a user effect cannot have a set type parameter (E0424), so an operation cannot take a function that uses other effects. To use a different capability, provide a different handler at the with.
- The body of the implementation of a fn operation (one that is not a suspend fn) MUST satisfy one of the following (E0417). Either one alone guarantees that the body does not suspend.
  - The R of the body is Immediate. A body that calls a suspend fn operation inside the with of a handler that resumes on the spot (`fn now() { with InstantClock { (use Clock).sleep(1) } ... }`) passes by this condition.
  - The body contains no call that may suspend.
- When the R of the body is not Immediate, E0417 is reported for each call that may suspend, at the position of that call. The search covers the subexpressions of the body, excluding the bodies of anonymous functions. A call inside an anonymous function counts as a call of a function value where that anonymous function is called.
- Whether a call may suspend is determined as follows.
  - A call of an operation (`clock.now()`, `(use Clock).now()`) is determined by the declaration of the operation. If it is a `suspend fn`, the call may suspend.
  - A call of a function or a function value is determined by the R of the callee's type. If that R is not Immediate, the call may suspend. Calling a closure that captures a capability (`f = fn() { clock.now() }`) or an operation taken out as a value (`g = clock.now`) is also a call of a function value.
  - A call of an `@external suspend fn` may suspend.
- If the body contains no call that may suspend, its R is not restricted. The R of `fn now() { (use Clock).now() }` contains Clock, which becomes a requirement of the handler. Mixing `now` of the outer Clock and `sleep` inside an inner `with InstantClock` in the same body satisfies neither condition and is a compile error (E0417). Accepting it would require looking, for each call, at the innermost with that provides its effect, which makes the rule heavier, so this is not done. A call through a function is treated as possibly suspending even if that function calls only fn operations (`fn now() { stamp() }`, which calls `fn stamp() -> Instant use { Clock }`, is a compile error (E0417)). That a function does not suspend is not inferred from its body. The implementations of fn operations in every handler are bound by the same rule, so a call of a fn operation does not suspend whichever handler it reaches.
- An operation that involves waiting (waiting on an `@external suspend fn`, calling a suspend operation) MUST be declared as a suspend fn on the effect side. Otherwise its implementation is a compile error (E0417). This is because the caller can learn whether a call suspends only from the effect's declaration. Only operation declarations of effects carry a mark of whether they suspend. Function signatures (`use Http`) and calls do not.

## 8.2 use `[effect.use]`

```emel
fn greet(id: Int) {
  log = use Logger
  users = use Users

  user = users.find(id) |> Option.or_fail(NotFound(id:))
  log.info("hello #{user.name}")

  (use Clock).sleep(1000)
  user
}
```

- `use X` is an expression that returns a value representing the capability of effect X. It can appear anywhere in a function.
- Operations can be called only through a capability value, in the form `log.info(...)`. There is no form that calls them directly by effect name (`Logger.info(...)`).
- Writing an operation of a capability value without calling it, as `log.info`, gives a function value. The capability value is evaluated at that position, and the type of the function value contains the capability's effect (as for a closure that captures a capability).
- Calling an operation of a capability value adds its effect to R and the operation's `fails` to E. The type of a closure that captures a capability value also contains that effect (the `copy` example below).
- Passing a capability value as an argument, putting it into a data structure (a constructor, a list, a tuple), and returning it from a function are all compile errors. Only the top-level type of the value is checked.
  - Passing it as an argument of a call is E0408. A constructor argument puts it into a data structure, so it is E0409.
  - Putting it into a list, a tuple, or a field of a record update is E0409.
  - Making it the value of the body of a fn, a handler operation, or an anonymous function is E0410. An anonymous function that returns its parameter as is does not get E0410 (the capability value came in as an argument, and the side that passed it gets E0408).
  - This check does not look at the value of a with body (8.6).
- Writing an operation that the capability value's effect does not have is a compile error (E0401). The labels of named arguments are the parameter names in the effect's operation declaration.
- The effect set R of a function that contains `use X` includes X. Even when written inside a branch, X is in the R of the whole function.
- Calls carry no mark (such as await or `!`) that indicates side effects or suspension. Suspend operations are written in the same form as ordinary calls.
- The type of a capability value cannot be written in source. A capability value cannot be passed as a function argument, put into a data structure, or returned.
- To let a function use the capability of a specific handler, pass a closure that captures that capability. Passing a closure to a called function inside the with is fine (8.6).

```emel
fn copy(read: fn() -> Rows use { Db }, write: fn(Rows) -> () use { Db }) {
  write(read())
}

with PrimaryDb {
  primary = use Db
  with ReplicaDb {
    replica = use Db
    copy(fn() { replica.query("...") }, fn(rows) { primary.execute(rows) })
  }
}
```

A syntax for passing a capability value directly as an argument (a capability type such as `from: use Db`) can be added later if needed. It would be an additive change, so existing code would not break.

## 8.3 with `[effect.with]`

```emel
with ConsoleLogger { greet(1) }

with AppTest, FixedClock { greet(1) }
```

- `with H { body }` provides the effects that H implements within the body. Its value is the value of the body.
- Provided effects are removed from the R of the body. However, as with escape, they are not removed from opaque effect sets (such as the hidden set of a parameter's function type). The R of `fn run(f: fn() -> ()) { with ConsoleLogger { f() } }` remains the effect set of f (7.3).
- To remove an effect precisely in the types as well, write the provided effect out in the function type (`use { Logger, R }`; the same as `fails Timeout | E` for errors). An effect written out is outside the set variable, so with removes it.

```emel
fn run[R](f: fn() -> () use { Logger, R }) -> () use R {
  with ConsoleLogger { f() } # R is just R. Logger is removed
}
```
- The requirements of handlers listed in the same with can be satisfied by a handler in the same with that provides a different effect. A requirement on the handler's own effect is satisfied by the outer side. A cycle among the requirements of handlers in the same with is a compile error.
- H is a handler or a layer. Label items (`L`, `L by P`) can also be written (8.9). When several are listed with commas, a later one takes precedence for the same effect. However, an explicit `L by P` takes precedence over provision through `default` (8.9) regardless of order. A layer is expanded into its handlers before being listed. A handler overridden by a later handler is not even initialized (the E of its init is not included in the E of the with).
- An item can be written as `H(arguments)`. The arguments are evaluated outside the with, in the order the items are written, before any init runs. The effects provided by the items of the same with cannot be used in the arguments. The E and R of the arguments of a handler that is not overridden are added to the E and R of the with. The arguments of a handler overridden by a later item are not evaluated, and their E and R are not included in the E and R of the with; they are still resolved and type-checked.
- `use X` refers to the handler provided by the innermost with as seen from the position where it is written. If the same function has no such with, it refers to the one passed from the caller.
- `use X` inside an anonymous function first looks at the withs that enclose the anonymous function (in the same top-level function). If there is none, it refers to the one passed from the side that calls the anonymous function. This is because the R of an anonymous function is enclosed in the anonymous function's type (9.4) and does not appear in the R of the enclosing function.
- A capability value taken out on the outer side keeps referring to the outer handler even if an inner with overrides the same effect.
- The R of main MUST be empty except for Async. main is the fn named `main` in the entry module. If the entry has no main, `emela run` reports a compile error (E0230). Something named `main` that is not a fn (such as a const) does not count as main. `emela build` MAY produce output even without main. The R MUST be empty even if a use clause is written. The R of the value of a const is also empty.

```emel
with PrimaryDb {
  primary = use Db
  with ReplicaDb {
    replica = use Db
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
    set_timeout(ms, fn() { resume(()) })
  }
  fn now() { date_now() }
}
```

- A handler is written as `handler Name: Effect`. The colon means that the left side satisfies the right side (the same as `[A: Ord]`, `trait Ord: Eq`, and `impl User: Show` (10.2)). Writing `handler H implements E` is a compile error (E0145), and the diagnostic shows the form `handler H: E`. The diagnostic points at the `implements` token.
- A handler is a declaration, and it implements each operation of one effect exactly once. The types of an operation's parameters and return value are determined by the effect's declaration, so they are not written. The number of parameters MUST be the same as in the declaration. A missing operation is a compile error (E0404, which lists all missing operations), implementing the same operation twice is a compile error (E0405), and a different number of parameters is a compile error (E0406). Implementing an operation that the effect does not have is a compile error (E0212).
- Whether an operation has `suspend` MUST match the effect's declaration (E0415). When it does not match, the number of parameters and the body are checked according to the form of the implementation.
- The E of an operation's body MUST be contained in the `fails` of the operation's declaration. requires(H) (9.4) is the union of the Rs of init, release, and the operation bodies.
- A handler with state declares fields after its name. Fields are referenced through `self`, as in `self.conn`.
- `init` runs once when entering the with and returns the handler value. It is required for a handler with fields (E0419). A handler without fields can also have init and release. The init of a handler without fields returns the value `ConsoleLogger` for a handler without parentheses, and `H()` for `handler H()`. self cannot be used inside init (because the value has not been made yet). In the operations of a handler without state, self is still the handler value and its type is the handler type (`ConsoleLogger`), as in a handler with state. It has no fields to access (E0312).
- A handler value (`PostgresUsers(conn: ...)`) can be made only inside that handler's init. If it could be made outside, init and release could be skipped. Making it outside is a compile error (E0418). The inside of an anonymous function inside init counts as inside init. A record update (`H(n: 1, ..self)`) also makes a value, so it cannot be written in an operation body. A constructor pattern does not make a value, so it can be written anywhere.
- The `use` expressions inside `init` are the requirements of the handler. The capability value can be bound and used (`config = use Config`) or written inside an expression (`(use Config).db_url()`). Both become requirements. There is no syntax for writing requirements in the handler's header.
- `init` MAY take parameters (`init(url: String) { ... }`). Their types MUST be written. Arguments are passed by writing `H(url: ...)` as an item of a with or a layer, or on the right side of `by` (`[effect.with]`, `[effect.layer]`, `[effect.label]`). A handler whose init takes no parameters cannot be given arguments, and `H()` cannot be written for it.
- `H(...)` at a with is a call of init, not a construction of the handler value. As before, the handler value can be constructed only inside init (E0418).
- `release` runs once when leaving the with. It runs in every case: normal completion, error, defect, or interruption.
- The E of release MUST be empty (E0420, reported for each expression that can raise an error). Cleanup that can fail escapes inside release or turns the failure into a defect with panic. The value of release is discarded, so its type does not matter. release MAY call suspend operations (it is not interrupted; 11.4). release is not a fn operation, so the E0417 check does not apply to it.
- The implementation of a suspend fn receives `resume` as its last parameter. Calling `resume(v)` resumes the suspended caller, and the return value of the operation is `v`.
- The implementation of a suspend fn has the parameters of the operation plus one for resume (the count for E0406 also includes resume). The last parameter is resume, and its name does not matter. The type of resume is `fn(T) -> () fails {} use {}` (T is the return type of the operation). The value of the implementation body is discarded, so the type of the body MUST be `()`.
- `resume` MUST NOT be called more than once. A second call is a defect (`[dyn.suspend]`). It can be called on the spot or later in a callback.

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
- A layer in which two handlers provide the same effect is a compile error (E0421), and layers that include each other are a compile error (E0422). The error is reported only once, at the layer declaration.
  - E0421 is reported at the position of the member that brought in the second handler. Listing the same handler twice is also E0421. For effects with type parameters, only effects that are the same including type arguments are counted, and the message shows them in the form `State[Int]`.
  - E0422 is reported once, at the position of the layer defined first in the cycle.
  - A cycle in handler requirements (E0411) is reported at the position of the layer member.
  - Duplication and cycles that occur only within a nested layer are reported in the inner layer and are not repeated in the outer layer. In a with, too, cycles that occur only within one layer are not reported.
- A requirement not satisfied within the layer is not a compile error and remains in the R of the with (the same as requires(H) in 8.3). The R MUST be empty in main and tests, so it becomes a compile error there. A layer that receives some of its requirements from outside can be written.
- To replace only some handlers, list them after the layer, as in `with AppTest, FixedClock { ... }`.

## 8.6 Capabilities must not leak `[effect.escape]`

A capability value, and closures that capture it, cannot leave the with that provides it.

```emel
fn make_greeter() {
  with ConsoleLogger {
    log = use Logger
    fn() { log.info("hi") } # compile error: leaves the with while holding log
  }
}
```

- The compiler tracks which capabilities a value holds, together with its type (capture set). When a closure goes into another closure or a data structure, that information is carried over.
- If the value of a with body holds a capability provided by that with, it is a compile error.
- Until the capture set check is added (0.22), the check is done at run time. Calling an operation of a handler that has already been released is a defect (`handler PostgresUsers was used after release`). This check applies only to handlers that have fields, init, or release. A handler without state keeps working as before even if it is taken out of the with.
- The capture set is inferred and is not written in source. Users see only error messages.
- A closure that is used up inside the with (`List.map(names, fn(n) { log.info(n) })`) is fine.
- Leaking Scope capabilities, fiber handles, and Deferreds is prevented by the same idea. The table in 11.8 summarizes what can be checked in 0.20 and 0.22.

## 8.7 Async `[effect.async]`

- Async is a built-in effect that represents "being resumed later". Only the runtime can provide it, and users cannot write handlers for it. `handler H: Async` is a compile error (E0413), and that handler provides nothing even when written in a with.
- Async has no operations, so the expression `use Async` is a compile error (E0414). Async can be written in a use clause (`fn f() use Async`).
- The means of calling `resume` later (callbacks of external functions, `@external suspend fn`, fork) require Async. Therefore SystemClock requires Async, and Async remains in the R of `with SystemClock { ... }`.
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
  body: fn() -> A fails E use { State[S], R },
) -> (A, S) fails E use R
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
  (_, lines) = State.run([], fn() { with TestConsole { greet(1) } })
  assert lines == ["Hello, alice!"]
}
```

## 8.9 Labels `[effect.label]`

To provide the same effect with different handlers in different places, use labels. A label is a separate effect that has the same operations as some effect.

```emel
effect AuditLog of Console # must be provided
effect DebugLog of Console default # provided together by a with that provides Console

fn report() -> () use { AuditLog, DebugLog } {
  with AuditLog { audit() } # AuditLog serves the Console inside audit
  with DebugLog { debug() }
}

fn main() {
  with SystemConsole, AuditLog by FileLog { report() } # SystemConsole serves DebugLog
}
```

- `effect L of E` declares an effect L that has the same operations as E. L is called a label of E, and E is called the underlying effect of L. In R, L and E count as different effects. The operations of a capability value obtained with `use L` are the operations of E, and calling them adds L to R. Whether L is Immediate (9.5) is determined by E.
- The underlying effect MUST be a user effect that has operations. Writing a label, or a built-in effect such as Async or Scope, is a compile error (E0429). This keeps the underlying effect always determined in one step, and gives `default` only one meaning. To separate the contents, declare `effect X of Console` flatly and plug it in with `X by AuditLog`.
- A label of an effect with type parameters writes the type arguments of the underlying effect (`effect Hits of State[Int]`). The label itself has no type parameters, so only concrete types can be written. The types of the operations are determined by those type arguments. The effect to the right of `by` can also have type arguments (`Hits by State[Int]`). Omitting the type arguments is a compile error (E0423). Labels can also be used to have two states of the same type (8.8).
- Writing a handler for a label is a compile error (E0433). Plug in a handler of the underlying effect with `by` instead.
- In addition to handlers and layers, a with can contain the following items.
  - `L`: within the body, the provider of L serves E. Provides {E}; requires {L}.
  - `L by P`: plugs P into L. P is a handler of E, E itself, or another label of E. Provides {L}; requires the requirements of P if P is a handler, or {P} if P is an effect. If P serves some other effect, it is a compile error (E0431). Writing E as P removes the distinction of L and goes back to the outer E (merging).
- The right side of `by` can be written as `H(arguments)`. The arguments are treated as for an item of a with.
- Requirements are satisfied and the initialization order is determined in the same way as for handlers (8.3, 14.2). In `with SystemConsole, AuditLog by Console`, the Console on the right is satisfied by SystemConsole in the same with.
- Writing an effect that is not a label in a with or on the left of `by` is a compile error (E0430). Writing `by` twice for the same label in the same with is also a compile error (E0432).
- A label with `default` is provided together by a with that provides E. This is the same as implicitly adding `L by E` to the item that serves E (a handler, or a label item such as `M`), and L falls to the provider of the innermost with that provides E. If the same with has `L by P`, that item takes precedence regardless of order (8.3). L is removed only from the R of the body, and it does not satisfy the requirements of other handlers in the same with. L inside a hidden set is not removed (8.3).
- If a label without `default` is not provided, R is not empty in main and tests, which is a compile error (E0402). This prevents an audit log from silently mixing into standard output. To mix them, the caller writes `L by E` explicitly.
- Function signatures write the label as is (`use { AuditLog, DebugLog }`). Whether it falls to E is decided by the caller, so E is not written. The LSP also shows types with the label as is, and on the name of a label it shows the declaration (`effect DebugLog of Console default`).
- A label item (`L`, `L by P`) that appears neither in the R of the body nor in the requirements of other items in the same with produces a warning (W0401). No warning is produced when the R of the body includes a hidden set.
- A label has no body. Writing `{ ... }` after the declaration is a compile error (E0122).
- Labels are distinguished by name (nominal). Two separately declared labels are different effects even if their underlying effect is the same, and labels with the same name in different modules are also different.
- An `L` item, and an `L by P` item where P is an effect or a label, do not create a new handler. They provide the outer handler (or that of an item initialized earlier in the same with) again under the name L. An `L by P` item where P is a handler initializes P and provides it.
