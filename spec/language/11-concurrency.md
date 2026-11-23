# 11. Concurrency `[task]`

The unit of concurrent work is the task. A task is started only through the capability of a with item that bounds its lifetime: `Task.Group` for tasks whose results are awaited, and `Task.Background` for tasks whose results are not. There is no dedicated syntax for asynchrony or concurrency (async, await). The tools are in `Std.Task` (Task, TaskGroup, Spawn, Group, Background), `Std.Deferred`, `Std.Semaphore`, `Std.Queue`, `Std.Exit` and `Std.Cause`.

The handlers `Group`, `Background`, `Deferred`, `Semaphore` and `Queue`, and `join` and `attempt`, are `@intrinsic` and implemented by the runtime, because they need operations that cannot be written in Emela:

- Interrupting a task and unwinding the releases of the withs along the way (11.5). An Emela continuation can only be resumed once with `resume(v)`, and there is no way to discard a waiting continuation and unwind it.
- Receiving a task's defect, stopping its siblings, and turning it into a value with attempt (11.6). A defect can be observed only at a boundary (7.5).
- The yield for fairness (11.1) and the AbortSignal that stops external work (12.2).

Adding a way to discard continuations or receive defects to the language would let these be written in Emela too, but it is not added, because it would break the promises that a continuation is resumed at most once and that a defect is visible only at a boundary. The combinators (11.4) are written in plain Emela on top of them. An intrinsic is not syntax, so it does not contradict the policy above of having no dedicated syntax.

## 11.1 Tasks and fibers `[task.basic]`

- A task runs on a fiber: a lightweight unit of execution that switches cooperatively on a single thread. Fibers are not parallel, so tasks are faster only when waits can overlap. Splitting a computation that does not wait into tasks does not change the total time (for parallelism, see 11.7).
- Switches happen only at suspension points. A suspension point is a call to a suspend operation that requires Async, or a yield that the runtime inserts for fairness.
- Yields are inserted only at calls that can suspend (8.1). The runtime counts calls that can suspend, and after a fixed number of them it yields to other fibers and to host callbacks. It counts calls, not time, so the order of switches is the same on every run. The number is unspecified.
- So programs are written on the assumption that a switch to another task can happen at any call that can suspend. A stretch with no call that can suspend runs atomically. `st.set(st.get() + 1)` is atomic, but in `x = st.get(); helper(); st.set(x + 1)` other tasks can run in between unless the R of `helper` is Immediate.
- An Immediate function runs atomically as seen from other tasks. No yields are inserted in it either.

## 11.2 Task.Group `[task.group]`

```emel
import Std.Task.{Task}

profile = with tasks = Task.Group {
  user = tasks.start(|| fetch_user(1))
  orders = tasks.start(|| fetch_orders(1))
  Profile(user: user.join(), orders: orders.join())
}
```

```emel
## The capability to start tasks whose results are awaited. E and R are the fails and uses of the tasks
@intrinsic
pub effect TaskGroup[E, R] {
  fn start[A](f: fn() -> A fails E uses { R }) -> Task[A]
}

## A task started in a group. It lives only while the capability of the group does (9.4)
@external
pub type Task[A] uses TaskGroup

## Provides TaskGroup. Its release interrupts the tasks left and waits until they have unwound
@intrinsic
pub handler Group[E, R]: TaskGroup[E, R]

@intrinsic
pub fn join[A](task: Task[A]) -> A uses { Async }
```

- TaskGroup is a built-in effect, and only `Task.Group` implements it. Users cannot write a handler for it (E0413). Its set type parameters E and R, and the type parameter of `start`, are allowed only for built-in effects (8.1). `Group` and `Background` (11.3) are the only handlers with set type parameters.
- `with tasks = Task.Group { body }` makes a group. Its E and R are inferred from the tasks started in it (`[effect.with]`).
- `tasks.start(f)` starts f as a task of the group and returns a `Task[A]`. start is not a suspension point (it is a fn operation). The task goes into the queue and starts running the next time the starting task suspends. A task interrupted before it starts running ends without calling f.
- The E and R of f MUST be included in the E and R of the group. The exception is the capability of the group itself (and TaskGroup provided by name by it): the R of f MAY contain it, so that a task can start siblings.
- A requirement `TaskGroup[E1, R1]` in an R is satisfied by a group `TaskGroup[E2, R2]` with E1 ⊆ E2 and R1 ⊆ R2. A function that starts tasks in a group it does not make writes its requirement with type parameters (`fn crawl[E, R](tasks: use TaskGroup[E, R], ...)`), so that another task can add a different error or effect to the same group.
- R is provided outside the with of the group: by the withs that enclose it, and by items written before the group in the same with (8.3). A task cannot use a capability made inside the body of the group, or by an item written after the group in the same with: the R of f then contains a capability that the R of the group cannot contain, because the type arguments of an item are fixed outside its body (`[effect.with]`, `[effect.escape]`).
- The E of the group is added to the E of the with, and its R to the R of the with, together with Async.
- When the body ends with a value, the tasks left are interrupted, and the release of the group waits until all of them have unwound (including their releases). The value of the with is the value of the body.
- When a task ends with a failure (Fail or Die), its siblings are interrupted, and the failure is delivered to the body at its next suspension point: the body fails there with that cause, and unwinds. The with ends with that failure after all tasks have unwound. When two or more tasks fail, the first failure becomes the result, and later failures are added to the display as notes (`note: another task also failed: <cause>`) (the same as a defect in a release in 14.4).
- If a task ends with a defect while the body is unwinding because of an interruption from outside, that defect becomes the result (the same direction as a defect in a release in 14.4).
- The group closes when its body ends or a task fails. A start on a closed group is a defect (`task group was used after it closed`), and goes into the notes when it happens during the unwinding caused by a task's failure.
- `Task[A]` is a capability-bound type of TaskGroup (9.4). The task returned by `tasks.start` records tasks, so it cannot leave the with of the group, and a task that captures a Task of its own group has the group in its R, which the R of the group cannot contain.
- There are no tasks that outlive the with that started them. Long-running work is started in a group or a background whose with encloses the work that depends on it.
- For E0402 about a missing effect, if the effect comes from the R of a task of a group, a note points at the call of start, and the help suggests providing the effect outside the with of the group.

```emel
with tasks = Task.Group {
  with PostgresUsers {
    tasks.start(|| (use Users).find(1)) # error[E0402]: effect `Users` is not provided
  }
  ()
}
# Fix: with PostgresUsers, tasks = Task.Group { ... }
```

## 11.3 Task.Background `[task.background]`

```emel
with bg = Task.Background(on_failure: |cause| log.error(Cause.to_string(cause))) {
  bg.start(|| audit(request))
  respond(request)
}
```

```emel
## The capability to start tasks whose results are not awaited
@intrinsic
pub effect Spawn[E, R] {
  fn start(f: fn() -> () fails E uses { R }) -> ()
}

## Provides Spawn. Its release waits until the tasks have ended
@intrinsic
pub handler Background[E, R]: Spawn[E, R] {
  init(on_failure: fn(Cause[E]) -> () uses { R })
}
```

- Spawn is a built-in effect, and only `Task.Background` implements it (E0413). Its set type parameters are treated as those of TaskGroup (11.2), including the inclusion of a requirement `Spawn[E1, R1]` in a provided `Spawn[E2, R2]`.
- `bg.start(f)` starts f as a task and returns `()`. f returns `()`: a task whose value is wanted is started in a group. The rules of 11.2 on start, on the E and R of f, and on where R is provided apply to a background as they are.
- The release of a background waits until all its tasks have ended. It does not interrupt them, whether the body ended with a value or a failure. The value or failure of the with is that of the body.
- When the with is interrupted from outside, its tasks are interrupted too. An interruption that arrives while the release waits is held off for the release (11.5), and is passed on to the tasks: they are interrupted, and the release ends after they have unwound.
- A task that ends with a failure (Fail or Die) does not stop its siblings or the body. `on_failure` is called with its cause, in that task, after the task has unwound. The release waits for `on_failure` too.
- The E of `on_failure` is empty. Its R is provided as the R of the tasks. A defect in `on_failure` is treated as a defect in a release (14.4): it becomes the result of the with if the body ended with a value or an error, and a note otherwise.
- `on_failure` is not called for a task interrupted because the with was interrupted.

## 11.4 Combinators `[task.combinators]`

The combinators are written in plain Emela on top of `Task.Group` and `Deferred` as part of the standard library (`Std.Task`). They are not language features.

| Function | Type | Meaning |
|---|---|---|
| `all` | `all[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> List[A] fails E uses { Async, R }` | Runs all of them concurrently and collects the results in the original order. If one fails, interrupts the rest and fails |
| `race` | `race[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> A fails E uses { Async, R }` | Takes the first one to finish and interrupts the rest. If the first one to finish is an error, fails with that error |
| `both` | `both[A, B, E, R](f: fn() -> A fails E uses { R }, g: fn() -> B fails E uses { R }) -> (A, B) fails E uses { Async, R }` | Runs two functions of different types concurrently and returns a pair. If one fails, interrupts the other and fails |
| `map` | `map[A, B, E, R](xs: List[A], f: fn(A) -> B fails E uses { R }) -> List[B] fails E uses { Async, R }` | Applies f to each element concurrently and collects the results in the original order |
| `timeout` | `timeout[A, E, R](ms: Int, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { Clock, Async, R }` | Waits for f for up to ms milliseconds. Some if it finishes in time, otherwise interrupts f and returns None. To make it a failure, add `Option.or_fail` |

```emel
pub fn race[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> A fails E uses { Async, R } {
  with first = Deferred, tasks = Group {
    List.map(fs, |f| tasks.start(|| first.complete(f())))
    first.await()
  }
}
```

- Everyday code needs only the combinators. A group and start appear only in more involved forms, such as joining partway or using a Deferred.

```emel
(user, orders) = Task.both(|| fetch_user(1), || fetch_orders(1))
users = Task.map(ids, |id| fetch(id))
```

- Something to run later is wrapped in `|| ...` and passed. The style is direct and does not make the computation itself a value, so the wrapping cannot be avoided. The syntax of anonymous functions keeps it short (6.1).
- A version that takes the first success (ignoring tasks that fail, and failing with the last cause if all of them fail) can be written on top of `attempt` (11.6) and Deferred.
- A limit on how many run at the same time ("1000 items, at most 10 concurrently") is written with a Semaphore (11.9): `Task.map(ids, |id| Semaphore.with_permit(db, || load(id)))`.

## 11.5 Interruption `[task.interrupt]`

- A task of a group is interrupted in two cases: when the body of its group ends, and when a sibling in its group fails. A task of a background is interrupted only when the with of the background is interrupted from outside. There is no function that interrupts a task. To stop one task, start it in a group of its own and end the with of that group, or pass it a stop signal with a Deferred (11.8).
- An interruption takes effect when the target task reaches its next suspension point.
- An interrupted task unwinds from that point. For the withs along the way, the releases of the handlers run from the inside out.
- A task is not interrupted while a release is running. The interruption takes effect after the running release finishes. The release of a background passes the interruption on to its tasks (11.3).
- An interruption that has arrived continues as unwinding until it reaches the frame that catches it (the release of a group or a background, or the top level of the program). Even if the task returns to normal processing along the way, for example because escape receives the failure of an inner group, it is interrupted again at the next suspension point.

## 11.6 Causes of failure `[task.cause]`

When a computation fails, the cause is one of the following two. The side that receives it can observe this distinction.

| Cause | Source | Appears in the type |
|---|---|---|
| Fail | fail | Yes (E) |
| Die | defect | No |

An interruption is not a cause: it is never received as a value. Whether an allocation failure is observable as a `Die` cause is implementation-defined (`[dyn.memory]`).

A cause is represented by `Cause[E]` in `Std.Cause`. As in 7.1, no type can be made for a value that is "one of E", so Cause is an opaque type from which the error value inside cannot be taken out. It can only be inspected for its kind and raised again. After raising it again, the caller can receive it as a concrete error by writing escape.

```emel
pub enum Exit[A, E] { Success(A), Failure(Cause[E]) } # Std.Exit

pub opaque type Cause[E] # Std.Cause
pub enum CauseKind { Fail, Die }
pub fn kind[E](c: Cause[E]) -> CauseKind
pub fn to_string[E](c: Cause[E]) -> String # error: ... / defect: ...
pub fn raise[A, E](c: Cause[E]) -> A fails E # Fail fails, Die is a defect
```

| Function | Type | Meaning |
|---|---|---|
| `join` | `join[A](task: Task[A]) -> A uses { Async }` | Waits for the task to finish and returns its value |
| `Exit.attempt` | `attempt[A, E, R](f: fn() -> A fails E uses { R }) -> Exit[A, E] uses { R }` | Runs f in place and turns Fail and Die into values. It starts no task |
| `Cause.raise` | As above | Raises a received cause again |

- Only the task that calls join waits. Other tasks keep running in the meantime. Nothing marks the caller (8.2). The value of a finished task is remembered, and from the second time on it is returned immediately.
- A task's Fail and Die go to its group (11.2), and a task is interrupted only when its whole group unwinds, so join returns a value whenever the calling task is not itself unwinding. While a release holds off an interruption (11.5), join on a task that failed or was interrupted unwinds the calling task as interrupted (the type of join has no E).
- To keep a task's error from reaching its group, wrap it with attempt inside the task, or start it in a background (11.3).

```emel
tasks.start(|| {
  Exit.attempt(|| handle(conn)) match {
    Success(_) -> ()
    Failure(cause) -> (use Console).print_error_line(Cause.to_string(cause))
  }
})
```

- attempt is a boundary where a defect can be turned into a value (7.5). Outside attempt, a defect passes through escape as before.
- attempt does not receive an interruption of its own task. If it did, a task could swallow the interruption and keep running. attempt during unwinding does not receive Die either (to keep the defect of a release in the result).

## 11.7 Parallelism `[task.parallel]`

Parallel execution on multiple threads is outside the scope of the first version. In the future, parallel primitives limited to pure functions, and messaging between workers with separate heaps, will be considered. Threads and locks will not be exposed in the language.

## 11.8 Deferred `[task.deferred]`

A Deferred is filled with a value only once, and passes it between tasks.

```emel
@intrinsic
pub effect Deferred[A] {
  fn complete(value: A) -> Bool
  suspend fn await() -> A
  fn poll() -> Option[A]
}

@intrinsic
pub handler Deferred[A]: Deferred[A]
```

```emel
with ready = Deferred, tasks = Task.Group {
  tasks.start(|| serve(ready))
  port = ready.await()
}
```

- `with d = Deferred` makes an empty Deferred, and d is its capability. Its type argument is inferred (`[effect.with]`), or written (`Deferred[Port]`).
- `d.complete(value)` stores the value only the first time and returns True. From the second time on, it does nothing and returns False. `d.await()` waits until a value is stored. `d.poll()` is `Some(value)` if a value is stored and `None` otherwise. complete and poll do not suspend.
- Any number of tasks MAY wait on one Deferred, and when a value is stored, all of them resume. A task waiting on a Deferred that no one fills is interrupted when its group ends.
- There are three uses: waiting for any one of several things to finish (race), receiving an intermediate value from a task that keeps running (a signal that it is ready to accept), and distributing a stop signal (checked with `poll` at each step, or awaited in `race`).
- Its release does nothing. A task that can use d is in a group or a background whose with is inside the with of d (11.2), so none is left when it ends.
- A Deferred is a mutable container (14.6). Only two states can be observed: "not filled yet" and "filled with this value". dbg shows the capability as `<deferred>`.

## 11.9 Semaphore `[task.semaphore]`

A Semaphore limits how many tasks run a piece of code at the same time. It has a fixed number of permits. Permits are resources opened with a handler member: its init takes them and its release gives them back.

```emel
@intrinsic
pub effect Semaphore {
  suspend handler Permits(n: Int): Permit
  handler TryPermits(n: Int): Permit fails NoPermits
  fn available() -> Int
}

@intrinsic
pub effect Permit {
  fn count() -> Int
}

@intrinsic
pub handler Semaphore(permits: Int): Semaphore

pub error NoPermits

pub fn with_permits[A, E, R](s: use Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> A
  fails E
  uses { Async, R }
{
  with _held = s.Permits(n:) { f() }
}

pub fn try_with_permits[A, E, R](s: use Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> Option[A]
  fails E
  uses { R }
{
  Some(with _held = s.TryPermits(n:) { f() }) escape {
    NoPermits -> None
  }
}
```

```emel
with db = Semaphore(permits: 10) {
  rows = Task.map(ids, |id| Semaphore.with_permit(db, || load(id)))
}
```

- `with s = Semaphore(permits: p)` makes a semaphore with p permits. p less than 1 is a defect (`semaphore needs at least 1 permit, but got 0`).
- `x = s.Permits(n: n)` waits until n permits are free and takes them. The release of x gives the n permits back. A release runs however the body ends, so the permits are given back when the body ends with a value, an error, a defect or an interruption, and after the releases of the withs inside the body.
- A task interrupted while it waits in the init of Permits leaves the line without taking any permit. The interruption takes effect there (11.5). The init returns no value, so no release runs (14.2).
- `x = s.TryPermits(n: n)` takes n permits if they are free and no task is waiting on s. Otherwise its init fails with NoPermits. It does not suspend.
- n less than 1 is a defect (`` `Permits` needs at least 1 permit, but got 0 ``). n greater than the number of permits s was made with is a defect (`` `Permits` asked for 4 permits of a semaphore with 3 ``). The same holds for TryPermits, with its name in the message.
- `x.count()` is n.
- `with_permits(s, n, f)` opens Permits and runs f in it, and `try_with_permits(s, n, f)` opens TryPermits and runs f in it, returning `Some` of the value of f, or `None` without running f if the permits cannot be taken. `with_permit(s, f)` and `try_with_permit(s, f)` are `with_permits(s, 1, f)` and `try_with_permits(s, 1, f)`. All four are plain Emela functions of `Std.Semaphore`.
- Tasks that wait are served in the order they started to wait. Permits given back go to the first waiting task when enough are free for it. While it waits, no task behind it takes permits, even if fewer permits would be enough. Tasks that receive their permits resume at a later suspension point of the task that gave them back (11.1); giving permits back does not suspend.
- Permits are counted per opening. Opening Permits of the same semaphore inside the body of another takes further permits. When no task can make progress, the tasks are interrupted as in `[dyn.suspend]`.
- Tasks started inside the body do not hold the permits. The permits are given back at the release of the Permits item, which, when the body contains the with of a group or a background, is after its tasks have ended.
- `s.available()` counts the permits that no task holds. Another task can run between `available` and a later call, so `available` cannot be used to check and take atomically.
- A Semaphore is a mutable container (14.6). dbg shows the capability as `<semaphore>`.

## 11.10 Queue `[task.queue]`

A Queue passes values between tasks in order. It holds up to a fixed number of values, and it can be closed to say that no more values will come.

```emel
@intrinsic
pub effect Queue[A] {
  suspend fn offer(value: A) -> () fails QueueClosed
  suspend fn take() -> A fails QueueClosed
  fn try_offer(value: A) -> Bool
  fn poll() -> Option[A]
  fn size() -> Int
  fn close() -> ()
}

@intrinsic
pub handler Queue[A](capacity: Int): Queue[A]

pub error QueueClosed
```

```emel
with jobs = Queue(capacity: 100), tasks = Task.Group {
  tasks.start(|| jobs.offer(job))
  next = jobs.take()
}
```

- `with q = Queue(capacity: c)` makes an empty queue that holds up to c values. c less than 1 is a defect (`queue needs a capacity of at least 1, but got 0`). Its type argument is inferred (`[effect.with]`), or written (`Queue[Job](capacity: 100)`).
- `q.offer(value)` adds value at the end of q. While q holds c values, it waits. `q.take()` removes the value at the front of q and returns it. While q is empty, it waits.
- Tasks that wait to offer and tasks that wait to take are each served in the order they started to wait. A value offered while tasks wait to take goes to the first of them. When a value is taken while tasks wait to offer, the value of the first of them is added at the end and that task resumes. The values therefore come out in the order their offers started.
- A task interrupted while it waits leaves the line. An interrupted offer adds nothing. The interruption takes effect there (11.5).
- `q.try_offer(value)` adds value and returns True if q is neither full nor closed. Otherwise it returns False and adds nothing. A value it adds while tasks wait to take goes to the first of them. `q.poll()` removes the value at the front and returns it as `Some`, or returns `None` if q is empty. Neither is a suspension point.
- `q.size()` is the number of values in q.
- `q.close()` closes q. Closing a closed queue does nothing. After q is closed:
  - the values already in q stay, and `take` and `poll` return them in order;
  - `take` on q when it is empty fails with QueueClosed, and so do the tasks waiting to take when q is closed;
  - `offer` fails with QueueClosed and adds nothing, and so do the tasks waiting to offer when q is closed; their values are not added;
  - `try_offer` returns False.
- Its release does nothing, for the reason given for Deferred (11.8).
- A Queue is a mutable container (14.6). dbg shows the capability as `<queue>`.
