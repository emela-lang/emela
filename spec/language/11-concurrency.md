# 11. Concurrency `[fiber]`

The unit of concurrency is the fiber, and a fiber can be launched only inside a scope. There is no dedicated syntax for asynchrony or concurrency (async, await). Everything is expressed with standard library functions and effects. The concurrency tools are in `Std.Fiber` (Scope, Fiber), `Std.Exit` (Exit, attempt), `Std.Cause` and `Std.Deferred`.

The foundation (Fiber.scope, the operations of Scope, join, await, interrupt, attempt, Deferred, Cause) is `@intrinsic` and implemented by the runtime, because it needs operations that cannot be written in Emela.

- Interrupting another fiber from outside and unwinding the releases of the withs along the way (11.4). An Emela continuation can only be resumed once with `resume(v)`, and there is no way to discard a waiting continuation and unwind it.
- Receiving a child's defect, stopping its siblings, and turning it into a value with attempt (11.5). A defect can be observed only at a boundary (7.5).
- Giving a child the handlers in effect at the call to Fiber.scope (11.2). The handler table is not visible from Emela.
- The yield for fairness (11.1) and the AbortSignal that stops external work (12.2).

Adding a way to discard continuations or receive defects to the language would let the foundation be written in Emela too, but it is not added, because it would break the promises that a continuation is resumed at most once and that a defect is visible only at a boundary. The combinators on top of the foundation (11.3) are written in plain Emela. An intrinsic is not syntax, so it does not contradict the policy above of having no dedicated syntax.

## 11.1 fiber `[fiber.basic]`

- A fiber is a lightweight unit of execution that switches cooperatively on a single thread. It is not parallel, so it is faster only when waits can overlap. Splitting a computation that does not wait into fibers does not change the total time (for parallelism, see 11.6).
- Switches happen only at suspension points. A suspension point is a call to a suspend operation that requires Async, or a yield that the runtime inserts for fairness.
- Yields are inserted only at calls that can suspend (8.1). The runtime counts calls that can suspend, and after a fixed number of them it yields to other fibers and to host callbacks. It counts calls, not time, so the order of switches is the same on every run. The number is unspecified.
- So programs are written on the assumption that a switch to another fiber can happen at any call that can suspend. A stretch with no call that can suspend runs atomically. `st.set(st.get() + 1)` is atomic, but in `x = st.get(); helper(); st.set(x + 1)` other fibers can run in between unless the R of `helper` is Immediate.
- An Immediate function runs atomically as seen from other fibers. No yields are inserted in it either.

## 11.2 Fiber.scope `[fiber.scope]`

```emel
import Std.Fiber.{Fiber, Scope}

profile = Fiber.scope(|| {
  s = use Scope
  a = s.fork(|| fetch_user(1))
  b = s.fork(|| fetch_orders(1))
  Profile(user: Fiber.join(a), orders: Fiber.join(b))
})
```

```emel
## The capability to launch fibers in a scope. E and R are the fails and uses of the children
pub effect Scope[E, R] {
  fn fork[A](
    f: fn() -> A fails E uses { Scope[E, R], R },
  ) -> Fiber[A] # the Scope inside a child is this scope itself
  fn deferred[A]() -> Deferred[A]
  fn semaphore(permits: Int) -> Semaphore
  fn queue[A](capacity: Int) -> Queue[A]
}

@intrinsic
pub fn scope[A, E, R](body: fn() -> A fails E uses { Scope[E, R], Async, R }) -> A
  fails E
  uses { Async, R }
```

- Scope is a built-in effect, and only `Fiber.scope` can provide it. Users cannot write a handler for Scope (E0413). Writing such a handler in a with provides nothing. `use Scope` can be written.
- A `use` expression of a built-in effect MAY omit the type arguments (`use Scope`). Omitted type arguments are inferred. They cannot be omitted in a uses clause or a handler head (E0423, the same rule as State in 8.1).
- The operations of Scope have their own type parameters (`fork[A]`) and set type parameters (E, R). Both are allowed only for built-in effects (8.1).
- `Fiber.fork(f)`, `Fiber.deferred()`, `Fiber.semaphore(permits)` and `Fiber.queue(capacity)` are functions that wrap the operations of `use Scope` of the same names. These are the usual way, and `s = use Scope` is written only to refer to an outer scope.

```emel
pub fn fork[A, E, R](f: fn() -> A fails E uses { Scope[E, R], R }) -> Fiber[A]
  uses { Scope[E, R] } {
  (use Scope).fork(f)
}
pub fn deferred[A, E, R]() -> Deferred[A] uses { Scope[E, R] } { (use Scope).deferred() }
pub fn semaphore[E, R](permits: Int) -> Semaphore uses { Scope[E, R] } {
  (use Scope).semaphore(permits)
}
pub fn queue[A, E, R](capacity: Int) -> Queue[A] uses { Scope[E, R] } {
  (use Scope).queue(capacity)
}
```

- A fork can happen only where Scope is provided: inside the function passed to Fiber.scope and inside functions called from it. Helper functions do not take the scope as a parameter. They take it with `use Scope` (capability values cannot be passed as arguments. 8.2). The Scope in R shows which scope a fork goes to. In nested scopes, `use Scope` refers to the innermost scope. To fork into an outer scope, use a capability value taken out in the outer scope (8.3).
- A child fiber always finishes before the scope is exited. Children that remain when the body of the scope returns a value are interrupted, and the scope returns the body's value after the unwinding of all children (including releases) has finished.
- When a child ends with an error or a defect, its sibling fibers (including the body of the scope) are interrupted, and the cause propagates out of the scope. The fails of a child joins the fails of the scope through the E of `Scope[E, R]`. When two or more children fail, the first failure becomes the result of the scope, and later failures are added to the display as notes (`note: another fiber also failed: <cause>`) (the same as a defect in a release in 14.4).
- Inside a child, the handlers passed from the caller (8.3) are the ones in effect at the call to Fiber.scope. The exception is Scope, which refers to the scope the child belongs to (so that a child can fork siblings).
- The E and R of a child MUST be included in the E and R of `Scope[E, R]` (inclusion: a requirement `Scope[E1, R1]` in R is satisfied by `Scope[E2, R2]` with E1 ⊆ E2 and R1 ⊆ R2). With equality, the type of a helper function that forks would be fixed to concrete sets, and they would conflict when another child adds a different error. So the effects that children use MUST be provided outside Fiber.scope. The exception is a requirement for Scope in a child's R, which is satisfied by the scope's own `Scope[E, R]`. In the types, `Scope[E, R]` is written out in the type of the function passed to fork (`uses { Scope[E, R], R }`) and is not put in R. If it is not written out, the Scope inside the child goes into R and leaks out of the scope (this is so that a child can fork siblings. It imposes only that the E and R of the requirement are included in the E and R of the scope). In nested scopes too, a child's Scope is satisfied by the scope the child belongs to, not by an outer one. A with written inside the body of the scope is not passed to children.
- For E0402 about a missing effect, if the expression it comes from is a call to Fiber.scope and the effect is not a top-level element of the R of the function type of the scope body, the effect is treated as coming from a forked child, and a note and help are added (suggesting to write the with outside Fiber.scope). They are not added for an effect the body uses itself. They are not added for one that comes from a Fiber.scope called inside a helper function, because the expression it comes from is then the call to the helper function.

```emel
Fiber.scope(|| {
  s = use Scope
  with PostgresUsers {
    s.fork(|| (use Users).find(1)) # error[E0402]: effect `Users` is not provided
  }
  ()
})
# Fix: with PostgresUsers { Fiber.scope(|| { ... }) }
```

- With this rule, the types prevent a child from using the handler of a with that ends before the child. The exception is when the same effect is also provided outside the scope, and a closure created inside a with in the scope body (or a capability value taken out there) is passed to a child. The child then uses the inner handler, and if that with ends first, it is a defect by the run-time check of 8.6 (capture sets in 0.22 stop it statically).
- fork is not a suspension point (the operations of Scope are fn). The child goes into the queue and starts running the next time the parent suspends. A child interrupted before it starts running ends with Interrupt without calling f.
- The result of the scope is the first Fail or Die among the body and the children, or otherwise the way the body ended. If a child ends with a defect while the body is unwinding because of an interruption from outside, that defect becomes the result (the same direction as a defect in a release in 14.4).
- The scope closes when a child fails (11.8). A fork, deferred, semaphore or queue during the unwinding caused by a sibling's failure is a defect and goes into the notes.
- fork requires Async. Async is not put in the type of the fork operation. It is expressed by the Async that remains in the R of Fiber.scope (because the uses clause of `Fiber.fork` has no Async).
- There are no detached fibers that outlive their parent. Long-running work is forked in a scope directly under main.

## 11.3 Combinators `[fiber.combinators]`

The combinators are written in plain Emela on top of Fiber.scope as part of the standard library (`Std.Fiber`). They are not language features.

| Function | Type | Meaning |
|---|---|---|
| `all` | `all[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> List[A] fails E uses { Async, R }` | Runs all of them concurrently and collects the results in the original order. If one fails, interrupts the rest and fails |
| `race` | `race[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> A fails E uses { Async, R }` | Takes the first one to finish and interrupts the rest. If the first one to finish is an error, fails with that error |
| `both` | `both[A, B, E, R](f: fn() -> A fails E uses { R }, g: fn() -> B fails E uses { R }) -> (A, B) fails E uses { Async, R }` | Runs two functions of different types concurrently and returns a pair. If one fails, interrupts the other and fails |
| `map` | `map[A, B, E, R](xs: List[A], f: fn(A) -> B fails E uses { R }) -> List[B] fails E uses { Async, R }` | Applies f to each element concurrently and collects the results in the original order |
| `timeout` | `timeout[A, E, R](ms: Int, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { Clock, Async, R }` | Waits for f for up to ms milliseconds. Some if it finishes in time, otherwise interrupts f and returns None. To make it a failure, add `Option.or_fail` |

```emel
pub fn race[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> A fails E uses { Async, R } {
  scope(|| {
    s = use Scope
    d = s.deferred()
    List.map(fs, |f| s.fork(|| Deferred.complete(d, f())))
    Deferred.await(d)
  })
}
```

- Everyday code needs only the combinators. scope and fork appear only in more involved forms, such as joining partway or using a Deferred.

```emel
(user, orders) = Fiber.both(|| fetch_user(1), || fetch_orders(1))
users = Fiber.map(ids, |id| fetch(id))
```

- Something to run later is wrapped in `|| ...` and passed. The style is direct and does not make the computation itself a value, so the wrapping cannot be avoided. The syntax of anonymous functions keeps it short (6.1).
- A version that takes the first success (ignoring children that fail, and failing with the last cause if all of them fail) can be written on top of `attempt` (11.5) and Deferred.
- A limit on how many run at the same time ("1000 items, at most 10 concurrently") is written with a Semaphore (11.9): `Fiber.map(ids, |id| db.with_permit(|| load(id)))`.

## 11.4 Interruption `[fiber.interrupt]`

- An interruption takes effect when the target fiber reaches its next suspension point.
- An interrupted fiber unwinds from that point. For the withs along the way, the releases of the handlers run from the inside out.
- A fiber is not interrupted while a release is running. The interruption takes effect after the running release finishes.
- A child is interrupted in three cases: when the body of the scope returns a value, when a sibling fails, and when `Fiber.interrupt` is called on it.

```emel
## Interrupts the child and waits until its unwinding (including releases) finishes. Does nothing to a child that has already finished
@intrinsic
pub fn interrupt[A](fiber: Fiber[A]) -> () uses { Async }
```

## 11.5 Causes of failure `[fiber.cause]`

When a fiber does not end normally, the cause is one of the following three. The supervising side can observe this distinction.

| Cause | Source | Appears in the type |
|---|---|---|
| Fail | fail | Yes (E) |
| Die | defect | No |
| Interrupt | interruption | No |

Whether an allocation failure is observable as a `Die` cause is implementation-defined (`[dyn.memory]`).

A cause is represented by `Cause[E]` in `Std.Cause`. As in 7.1, no type can be made for a value that is "one of E", so Cause is an opaque type from which the error value inside cannot be taken out. It can only be inspected for its kind and raised again. After raising it again, the caller can receive it as a concrete error by writing escape.

```emel
pub enum Exit[A, E] { Success(A), Failure(Cause[E]) } # Std.Exit

pub opaque type Cause[E] # Std.Cause
pub enum CauseKind { Fail, Die, Interrupt }
pub fn kind[E](c: Cause[E]) -> CauseKind
pub fn to_string[E](c: Cause[E]) -> String # error: ... / defect: ... / interrupted
pub fn raise[A, E](c: Cause[E]) -> A fails E # Fail fails, Die is a defect, Interrupt unwinds as an interruption
```

There are four ways to receive a result.

| Function | Type | Meaning |
|---|---|---|
| `join` | `join[A](fiber: Fiber[A]) -> A uses { Async }` | Waits for the child to finish and returns its value |
| `await` | `await[A](fiber: Fiber[A]) -> Exit[A, {}] uses { Async }` | Waits for the child to finish and returns how it ended |
| `Exit.attempt` | `attempt[A, E, R](f: fn() -> A fails E uses { R }) -> Exit[A, E] uses { R }` | Runs f in place and turns Fail and Die into values. It starts no fiber |
| `Cause.raise` | As above | Raises a received cause again |

- Only the fiber that calls join or await waits. Other fibers keep running in the meantime. Nothing marks the caller (8.2).
- A child's Fail and Die propagate to the scope (11.2), so only success or Interrupt reaches join and await. The value of a finished child is remembered, and from the second time on it is returned immediately.
- Joining an interrupted child is a defect (`fiber was interrupted`), regardless of how the interruption happened (`Fiber.interrupt`, the end of the scope, or a continuation discarded without being resumed).
- Interruptions are held off while a release is running (11.4), so only during that time can join and await see a failed child. In that case join unwinds the calling fiber as an interruption (because the type of join has no E), and await returns `Failure(cause)` as it is.
- `Fiber.interrupt` on the fiber itself unwinds right there (after the release, if inside a release).
- An interruption that has arrived continues as unwinding until it reaches the frame that catches it (a scope or the top level of the program). Even if the fiber returns to normal processing along the way, for example because escape receives the result of an inner scope, it is interrupted again at the next suspension point. attempt during unwinding does not receive Die either (to keep the defect of a release in the result of the fiber).
- To see the result of a child that can be interrupted, use await.
- To keep a child's error from propagating to the scope, wrap it with attempt inside the child. There are no additional kinds of fork.

```emel
s.fork(|| {
  Exit.attempt(|| handle(conn)) match {
    Success(_) -> ()
    Failure(cause) -> (use Console).print_error_line(Cause.to_string(cause))
  }
})
```

- attempt is a boundary where a defect can be turned into a value (7.5). Outside attempt, a defect passes through escape as before.
- attempt does not receive an interruption of its own fiber (Interrupt). If it did, a fiber could swallow the interruption and keep running. Interrupt can be observed only when looking at a child with await.

| Cause | attempt | await | Scope |
|---|---|---|---|
| Fail | Received as Failure | Does not arrive | Interrupts siblings and propagates |
| Die | Received as Failure | Does not arrive | Interrupts siblings and propagates |
| Interrupt | Not received, unwinds | Received as Failure | Only that child ends |

## 11.6 Parallelism `[fiber.parallel]`

Parallel execution on multiple threads is outside the scope of the first version. In the future, parallel primitives limited to pure functions, and messaging between workers with separate heaps, will be considered. Threads and locks will not be exposed in the language.

## 11.7 Deferred `[fiber.deferred]`

A Deferred is a container that can be filled with a value only once, and it passes values between fibers. It belongs to a scope.

```emel
d = (use Scope).deferred() # Deferred[A], belongs to the scope
```

| Function (`Std.Deferred`) | Type | Meaning |
|---|---|---|
| `complete` | `complete[A](d: Deferred[A], value: A) -> Bool` | Stores the value only the first time and returns True. From the second time on, does nothing and returns False. Does not suspend |
| `await` | `await[A](d: Deferred[A]) -> A uses { Async }` | Waits until a value is stored |
| `poll` | `poll[A](d: Deferred[A]) -> Option[A]` | `Some(value)` if a value is stored, `None` if not yet. Does not wait. Does not suspend |

- To tie its lifetime to the scope, a Deferred can be created only by the Scope operation `deferred`.
- A child waiting on a Deferred that no one fills is interrupted at the end of the scope.
- There are three uses: waiting for any one of several things to finish (race), receiving an intermediate value from a child that keeps running (a signal that it is ready to accept), and distributing a stop signal (checked with `poll` at each step). Cancelling forcibly from outside is done with `Fiber.interrupt` (11.4). Any number of fibers MAY wait on one Deferred, and when a value is stored, all of them resume.
- A Deferred is a mutable container (14.6). Only two states can be observed: "not filled yet" and "filled with this value".

## 11.8 No leaking `[fiber.escape]`

The Scope capability, fiber handles (`Fiber[A]`), Deferreds, Semaphores and Queues cannot be leaked out of their scope.

- Scope is a capability, so the capability rules (8.2, 8.6) apply as they are. Passing it as an argument, putting it in data, or returning it is a compile error (E0408 to E0410).
- If the type of the value of `Fiber.scope` contains `Fiber`, `Deferred`, `Semaphore` or `Queue` anywhere, it is a compile error (E0425). This includes `Option[Fiber[User]]`. The type of the value is examined all the way through after the sets are solved. One diagnostic is reported at the call to Fiber.scope (including a pipe, `f |> Fiber.scope()`). Outside the scope the children have already finished, so no correct program is rejected.
- Leaks that the types do not stop (putting it in State, capturing it in a closure and returning it) are checked at run time. An operation of Scope on a finished scope; join, await or interrupt on a handle of a finished scope; complete, await or poll on a Deferred of a finished scope; and any function of `Std.Semaphore` or `Std.Queue` on a Semaphore or Queue of a finished scope are defects (`fiber scope was used after it ended`, `fiber was joined after its scope ended`, and so on).
- A scope closes when its body returns a value or when a child fails. A fork after it closes is a defect.
- The messages are `fiber scope was used after it ended`, `fiber was joined after its scope ended` (the same form for awaited and interrupted), and `deferred was completed after its scope ended` (the same form for awaited). poll is checked in the same way.
- The message of the defect for poll on a Deferred of a finished scope is `deferred was polled after its scope ended`.
- The messages for Semaphores and Queues are `semaphore was used after its scope ended` and `queue was used after its scope ended`. A fiber that is not a child of the scope and is still waiting on the Semaphore or Queue when the scope ends stops waiting with the same defect.

When capture sets (8.6) arrive in 0.22, handles, Deferreds, Semaphores and Queues will be treated as "values that hold the capability of the scope that created them", and all of these will be stopped statically. A program that is a compile error in 0.20 is also a compile error in 0.22. The only programs that stop compiling when upgrading are those that were run-time defects in 0.20.

| How it leaks | 0.20 | 0.22 |
|---|---|---|
| Passing a capability value (including Scope) as an argument, putting it in data, returning it | Static (E0408 to E0410) | Static |
| Taking a closure that captures a capability out of a with or a scope | Run time (stateful handlers and Scope) | Static |
| Returning a value whose type contains `Fiber`, `Deferred`, `Semaphore` or `Queue` from Fiber.scope | Static (E0425) | Static |
| Putting a `Fiber`, `Deferred`, `Semaphore` or `Queue` in State, or capturing it in a closure and taking it out | Run time | Static |

## 11.9 Semaphore `[fiber.semaphore]`

A Semaphore limits how many fibers run a piece of code at the same time. It has a fixed number of permits, and a fiber runs the code while it holds some of them.

```emel
db = Fiber.semaphore(10) # Semaphore, belongs to the scope
rows = db.with_permit(|| query(sql))
```

| Function (`Std.Semaphore`) | Type | Meaning |
|---|---|---|
| `with_permit` | `with_permit[A, E, R](s: Semaphore, f: fn() -> A fails E uses { R }) -> A fails E uses { Async, R }` | `with_permits(s, 1, f)` |
| `with_permits` | `with_permits[A, E, R](s: Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> A fails E uses { Async, R }` | Waits for n permits, runs `f`, and gives them back |
| `try_with_permit` | `try_with_permit[A, E, R](s: Semaphore, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { R }` | `try_with_permits(s, 1, f)` |
| `try_with_permits` | `try_with_permits[A, E, R](s: Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { R }` | Runs `f` only if n permits can be taken without waiting |
| `available` | `available(s: Semaphore) -> Int` | The number of free permits. Does not suspend |

- To tie its lifetime to the scope, a Semaphore can be created only by the Scope operation `semaphore`. `permits` less than 1 is a defect (`semaphore needs at least 1 permit, but got 0`).
- `with_permits(s, n, f)` waits until n permits are free and takes them. It then runs `f`, and gives the n permits back when `f` ends, however it ends: with a value, an error, a defect or an interruption. The releases of the withs inside `f` run before the permits are given back. The result of `with_permits` is the result of `f`.
- `try_with_permits(s, n, f)` takes n permits if they are free and no fiber is waiting on s, runs `f` as `with_permits` does, and returns `Some` of the value of `f`. Otherwise it returns `None` without running `f`. It is not a suspension point.
- `n` less than 1 is a defect (`` `with_permits` needs at least 1 permit, but got 0 ``). `n` greater than the number of permits s was made with is a defect (`` `with_permits` asked for 4 permits of a semaphore with 3 ``). The same holds for `try_with_permits`, with its name in the message.
- Fibers that wait are served in the order they started to wait. Permits given back go to the first waiting fiber when enough are free for it. While it waits, no fiber behind it takes permits, even if fewer permits would be enough. Fibers that receive their permits resume at a later suspension point of the fiber that gave them back (11.1); giving permits back does not suspend.
- A fiber interrupted while it waits leaves the line without taking any permit. The interruption takes effect there (11.4).
- Permits are counted per call. A `with_permits` inside `f` of a `with_permits` on the same semaphore takes further permits. When no fiber can make progress, the fibers are interrupted as in `[dyn.suspend]`.
- Fibers forked inside `f` do not hold the permits. The permits are given back when `f` ends, which, when `f` contains a Fiber.scope, is after that scope's children have ended.
- `available(s)` counts the permits that no fiber holds. Another fiber can run between `available` and a later call, so `available` cannot be used to check and take atomically.
- A Semaphore is a mutable container (14.6). It has no Eq and no Show; dbg shows it as `<semaphore>`.

## 11.10 Queue `[fiber.queue]`

A Queue passes values between fibers in order. It holds up to a fixed number of values, and it can be closed to say that no more values will come.

```emel
q = Fiber.queue(100) # Queue[A], belongs to the scope
q.offer(job)
job = q.take()
```

| Function (`Std.Queue`) | Type | Meaning |
|---|---|---|
| `offer` | `offer[A](q: Queue[A], value: A) -> () fails QueueClosed uses { Async }` | Adds value at the end. Waits while the queue is full |
| `take` | `take[A](q: Queue[A]) -> A fails QueueClosed uses { Async }` | Removes the value at the front. Waits while the queue is empty |
| `try_offer` | `try_offer[A](q: Queue[A], value: A) -> Bool` | Adds value if the queue is neither full nor closed. Does not suspend |
| `poll` | `poll[A](q: Queue[A]) -> Option[A]` | Removes the value at the front if there is one. Does not suspend |
| `size` | `size[A](q: Queue[A]) -> Int` | The number of values in the queue |
| `close` | `close[A](q: Queue[A]) -> ()` | Closes the queue |

```emel
pub error QueueClosed
```

- To tie its lifetime to the scope, a Queue can be created only by the Scope operation `queue`. `capacity` less than 1 is a defect (`queue needs a capacity of at least 1, but got 0`).
- `offer(q, value)` adds value at the end of q. While q holds `capacity` values, it waits. `take(q)` removes the value at the front of q and returns it. While q is empty, it waits.
- Fibers that wait to offer and fibers that wait to take are each served in the order they started to wait. A value offered while fibers wait to take goes to the first of them. When a value is taken while fibers wait to offer, the value of the first of them is added at the end and that fiber resumes. The values therefore come out in the order their offers started.
- A fiber interrupted while it waits leaves the line. An interrupted offer adds nothing. The interruption takes effect there (11.4).
- `try_offer(q, value)` adds value and returns True if q is neither full nor closed. Otherwise it returns False and adds nothing. A value it adds while fibers wait to take goes to the first of them. `poll(q)` removes the value at the front and returns it as `Some`, or returns `None` if q is empty. Neither is a suspension point.
- `close(q)` closes q. Closing a closed queue does nothing. After q is closed:
  - the values already in q stay, and `take` and `poll` return them in order;
  - `take` on q when it is empty fails with QueueClosed, and so do the fibers waiting to take when q is closed;
  - `offer` fails with QueueClosed and adds nothing, and so do the fibers waiting to offer when q is closed; their values are not added;
  - `try_offer` returns False.
- A Queue is a mutable container (14.6). It has no Eq and no Show; dbg shows it as `<queue>`.
