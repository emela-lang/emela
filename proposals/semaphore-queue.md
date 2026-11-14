---
title: Semaphore and Queue
authors: [Alliana]
status: accepted
pr: 515
issue:
decided: 2026-11-15
touches: [lib.layers, fiber.scope, fiber.escape, fiber.semaphore (new), fiber.queue (new), api.std.fiber, api.std.semaphore (new), api.std.queue (new), dyn.memory, diag.list, open.next]
requires: [each]
related: []
---

# Semaphore and Queue

## Summary

Two synchronization primitives are added to the effect layer of std, next to Deferred.

- A Semaphore limits how many fibers run a piece of code at once. It is used only through `with_permit(s, f)`, which takes a permit, runs `f`, and gives the permit back however `f` ends. `try_with_permit` does not wait.
- A Queue passes any number of values between fibers, in order, with a capacity. `offer` waits while it is full and `take` waits while it is empty. A queue can be closed: the values already in it can still be taken, and then `take` fails with `QueueClosed`.

Both are made by operations of Scope, as Deferred is (`Fiber.semaphore(n)`, `Fiber.queue(n)`), and belong to that scope.

## Motivation

- The concurrency of std has fibers, scopes and Deferred (`[fiber]`). A Deferred is filled once, so it cannot carry a sequence of values, and nothing limits how many fibers run at once. `[lib.layers]` and `[open.next]` list Semaphore and Queue as the next pieces, and say that a limit on the number of tasks running at once comes before Http.
- Without a limit, `Fiber.map(ids, load)` starts one fiber per element, and a server opens one connection to the database per request. Writing the limit by hand needs a counter in State and a list of Deferreds to wake, and it is wrong in two ways that the author cannot fix:
  - a fiber interrupted while it waits stays in the list, and the permit handed to it is lost (nothing runs when a fiber is interrupted while it waits on a Deferred);
  - putting a Deferred in State is a run-time leak check now and a compile error once capture sets arrive (`[fiber.escape]`).
- A producer and consumers (an accept loop handing connections to workers, a reader handing chunks to a parser) need a channel that applies back-pressure and that can say "no more values". A future Stream and a TCP connection are built on top of this.

## Proposal

### Semaphore

```emel
import Std.Fiber.{Fiber}
import Std.Semaphore.{Semaphore}

fn load_all(ids: List[Int]) -> List[User] {
  Fiber.scope(|| {
    db = Fiber.semaphore(10)
    Fiber.map(ids, |id| db.with_permit(|| load(id)))
  })
}
```

```emel
fn handle(api: Semaphore, request: Request) -> Response {
  api.try_with_permit(|| call(request)) match {
    Some(response) -> response
    None           -> Response.new(503, "Service Unavailable")
  }
}
```

- `Fiber.semaphore(permits)` makes a semaphore with `permits` permits, belonging to the innermost scope. `permits` less than 1 is a defect.
- `with_permits(s, n, f)` waits until n permits are free, takes them, runs `f`, and gives the n permits back when `f` ends: with a value, an error, a defect or an interruption. The releases of the withs inside `f` run before the permits are given back. `with_permit(s, f)` is `with_permits(s, 1, f)`.
- `try_with_permits(s, n, f)` takes n permits only if it can without waiting, runs `f` and returns `Some` of its value. Otherwise it returns `None` without running `f`. It is not a suspension point.
- Waiting fibers are served in the order they started to wait (strict first-in first-out). A fiber that asks for more permits than are free waits, and fibers behind it wait too, even if fewer permits would be enough for them. `try_with_permits` returns `None` while any fiber is waiting.
- A fiber interrupted while it waits leaves the line and takes no permit.
- Permits are not reentrant: a `with_permit` inside `f` of a `with_permit` on the same semaphore asks for another permit.
- `n` less than 1, or greater than the number of permits the semaphore was made with, is a defect.
- `available(s)` is the number of free permits. It does not suspend. Another fiber can run between `available` and `with_permit`, so the two together do not check and take atomically.

### Queue

```emel
import Std.Fiber.{Fiber}
import Std.Queue.{Queue, QueueClosed}

fn run(jobs: List[Job]) {
  Fiber.scope(|| {
    q = Fiber.queue(100)
    Fiber.fork(|| {
      jobs.each(|job| q.offer(job))
      q.close()
    })
    work(q)
  })
}

fn work(q: Queue[Job]) {
  next = Some(q.take()) escape {
    QueueClosed -> None
  }
  next match {
    Some(job) -> {
      process(job)
      work(q)
    }
    None -> ()
  }
}
```

- `Fiber.queue(capacity)` makes an empty queue that holds up to `capacity` values, belonging to the innermost scope. `capacity` less than 1 is a defect. There are no queues without a capacity.
- `offer(q, value)` adds value at the end. While the queue is full it waits. `take(q)` removes and returns the value at the front. While the queue is empty it waits.
- Waiting fibers are served in the order they started to wait, for offer and for take separately. When a value is offered while fibers wait to take, the first of them receives it. When a value is taken while fibers wait to offer, the value of the first of them is added at the end. So the values come out in the order the offers started.
- A fiber interrupted while it waits leaves the line. An interrupted offer adds nothing.
- `try_offer(q, value)` adds value if the queue is not full and not closed, and returns True. Otherwise it returns False. `poll(q)` removes and returns the value at the front as `Some`, or returns `None` if the queue is empty. Neither suspends.
- `size(q)` is the number of values in the queue.
- `close(q)` closes the queue. Closing a closed queue does nothing. There is no function that asks whether a queue is closed: the answer could change before the next call, as with `available`.
  - The values already in the queue stay, and `take` and `poll` return them in order.
  - `take` on a closed queue that is empty fails with `QueueClosed`. Fibers waiting to take when the queue is closed fail with `QueueClosed`.
  - `offer` on a closed queue fails with `QueueClosed` and adds nothing. Fibers waiting to offer when the queue is closed fail with `QueueClosed`, and their values are not added.
  - `try_offer` on a closed queue returns False.

### Belonging to a scope

- Semaphores and queues belong to the scope that made them, as Deferreds do. Every operation on one whose scope has ended is a defect. A fiber still waiting on one when its scope ends is interrupted with the other children of that scope.
- A fiber of another scope can still be waiting on one when its scope ends, if the Semaphore or Queue was leaked at run time. That fiber stops waiting with the defect `semaphore was used after its scope ended` (or `queue ...`), as any later use would.
- The type of the value of `Fiber.scope` MUST NOT contain `Semaphore` or `Queue` (E0425).

### Typing rules

No new rules. The functions have ordinary signatures (`[api.std.semaphore]`, `[api.std.queue]`). The operations that wait require Async. `with_permits` has the E and R of `f`, plus Async. `try_with_permits` has the E and R of `f` only.

## Specification changes

### `[lib.layers]`

In the table of layers, the row of the effect layer becomes:

| effect | Concurrency and state | State, Fiber, Cause, Deferred, Semaphore, Queue. Schedule and Stream come in later versions |

### `[fiber.scope]`

The declaration of Scope becomes:

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
```

The paragraph on `Fiber.fork` and `Fiber.deferred` becomes:

- `Fiber.fork(f)`, `Fiber.deferred()`, `Fiber.semaphore(permits)` and `Fiber.queue(capacity)` are functions that wrap the operations of `use Scope` of the same names. These are the usual way, and `s = use Scope` is written only to refer to an outer scope.

```emel
pub fn fork[A, E, R](f: fn() -> A fails E uses { Scope[E, R], R }) -> Fiber[A] uses { Scope[E, R] } {
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

The paragraph "The scope closes when a child fails (11.8)" becomes:

- The scope closes when a child fails (11.8). A fork, deferred, semaphore or queue during the unwinding caused by a sibling's failure is a defect and goes into the notes.

### `[fiber.escape]`

The first sentence becomes:

The Scope capability, fiber handles (`Fiber[A]`), Deferreds, Semaphores and Queues cannot be leaked out of their scope.

The second item becomes:

- If the type of the value of `Fiber.scope` contains `Fiber`, `Deferred`, `Semaphore` or `Queue` anywhere, it is a compile error (E0425). This includes `Option[Fiber[User]]`. The type of the value is examined all the way through after the sets are solved. One diagnostic is reported at the call to Fiber.scope (including a pipe, `f |> Fiber.scope()`). Outside the scope the children have already finished, so no correct program is rejected.

The third item becomes:

- Leaks that the types do not stop (putting it in State, capturing it in a closure and returning it) are checked at run time. An operation of Scope on a finished scope; join, await or interrupt on a handle of a finished scope; complete, await or poll on a Deferred of a finished scope; and any function of `Std.Semaphore` or `Std.Queue` on a Semaphore or Queue of a finished scope are defects.

After the item on the messages of Deferred, add:

- The messages for Semaphores and Queues are `semaphore was used after its scope ended` and `queue was used after its scope ended`. A fiber that is not a child of the scope and is still waiting on the Semaphore or Queue when the scope ends stops waiting with the same defect.

In the table, the third and fourth rows become:

| Returning a value whose type contains `Fiber`, `Deferred`, `Semaphore` or `Queue` from Fiber.scope | Static (E0425) | Static |
| Putting a `Fiber`, `Deferred`, `Semaphore` or `Queue` in State, or capturing it in a closure and taking it out | Run time | Static |

The paragraph on 0.22 becomes: When capture sets (8.6) arrive in 0.22, handles, Deferreds, Semaphores and Queues will be treated as "values that hold the capability of the scope that created them", and all of these will be stopped statically.

### `[fiber.semaphore]` (new)

Added after `[fiber.escape]` as 11.9 Semaphore.

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

### `[fiber.queue]` (new)

Added after `[fiber.semaphore]` as 11.10 Queue.

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

### `[api.std.fiber]`

The row of Scope becomes:

| `Scope` | `@intrinsic effect Scope[E, R] { fn fork[A](f: fn() -> A fails E uses { R }) -> Fiber[A]; fn deferred[A]() -> Deferred[A]; fn semaphore(permits: Int) -> Semaphore; fn queue[A](capacity: Int) -> Queue[A] }` | built-in | The capability to launch fibers in a scope. Only `scope` can provide it |

After the row of `deferred`, add:

| `semaphore` | `semaphore[E, R](permits: Int) -> Semaphore uses { Scope[E, R] }` | Emela | `(use Scope).semaphore(permits)` |
| `queue` | `queue[A, E, R](capacity: Int) -> Queue[A] uses { Scope[E, R] }` | Emela | `(use Scope).queue(capacity)` |

### `[api.std.semaphore]` (new)

Added after `[api.std.deferred]`.

### Std.Semaphore (`std/semaphore.emel`) `[api.std.semaphore]`

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Semaphore` | `@external type Semaphore` | runtime | A fixed number of permits. Only `semaphore` of Scope can create one |
| `with_permit` | `with_permit[A, E, R](s: Semaphore, f: fn() -> A fails E uses { R }) -> A fails E uses { Async, R }` | Emela | `with_permits(s, 1, f)` |
| `with_permits` | `with_permits[A, E, R](s: Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> A fails E uses { Async, R }` | intrinsic | Waits for n permits, runs `f`, and gives them back however `f` ends |
| `try_with_permit` | `try_with_permit[A, E, R](s: Semaphore, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { R }` | Emela | `try_with_permits(s, 1, f)` |
| `try_with_permits` | `try_with_permits[A, E, R](s: Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { R }` | intrinsic | Runs `f` only if n permits can be taken without waiting |
| `available` | `available(s: Semaphore) -> Int` | intrinsic | The number of free permits |

### `[api.std.queue]` (new)

Added after `[api.std.semaphore]`.

### Std.Queue (`std/queue.emel`) `[api.std.queue]`

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Queue` | `@external type Queue[A]` | runtime | Values in order, up to a capacity. Only `queue` of Scope can create one |
| `QueueClosed` | `error QueueClosed` | Emela | The queue is closed (and, for `take`, empty) |
| `offer` | `offer[A](q: Queue[A], value: A) -> () fails QueueClosed uses { Async }` | intrinsic | Adds value at the end. Waits while full |
| `take` | `take[A](q: Queue[A]) -> A fails QueueClosed uses { Async }` | intrinsic | Removes the value at the front. Waits while empty |
| `try_offer` | `try_offer[A](q: Queue[A], value: A) -> Bool` | intrinsic | Adds value if neither full nor closed |
| `poll` | `poll[A](q: Queue[A]) -> Option[A]` | intrinsic | Removes the value at the front if there is one |
| `size` | `size[A](q: Queue[A]) -> Int` | intrinsic | The number of values |
| `close` | `close[A](q: Queue[A]) -> ()` | intrinsic | Closes the queue. The values in it can still be taken |

### `[dyn.memory]`

The first item becomes:

- All values are immutable. Mutable state is limited to the State effect (8.8), host objects held by handlers, Deferred (11.7, written only once), Semaphore (11.9) and Queue (11.10).

### `[diag.list]`

The row of E0425 becomes:

| E0425 | The type of the value of `Fiber.scope` contains `Fiber`, `Deferred`, `Semaphore` or `Queue` (11.8). The note is ``= note: every fiber has ended once `Fiber.scope` returns, so the value would be of no use outside it`` | ``a `Fiber` cannot leave the `Fiber.scope` that started it`` |

### `[open.next]`

The item "Remaining for 0.20" no longer lists a limit on how many run at once (Semaphore) and Queue.

## Diagnostics

- [ ] No new code. E0425 also covers `Semaphore` and `Queue`.
- [ ] New defects: `semaphore needs at least 1 permit, but got <n>`, `` `with_permits` needs at least 1 permit, but got <n> ``, `` `with_permits` asked for <n> permits of a semaphore with <m> `` (and the same two for `try_with_permits`), `queue needs a capacity of at least 1, but got <n>`, `semaphore was used after its scope ended`, `queue was used after its scope ended`.

## Compatibility

- Additive. Scope gains two operations, but users cannot write handlers for Scope (E0413), so no program breaks.
- `Std.Semaphore` and `Std.Queue` are under `Std`, so they do not take a user's module name (`[lib.layers]`).
- `Std.Fiber` gains `semaphore` and `queue`. A program that defines its own `semaphore` or `queue` and imports `Std.Fiber.{semaphore}` by name is not affected, because these are reached as `Fiber.semaphore`.

## Implementation

The path is the one Deferred takes.

- `emela-core/lib/std/`: `semaphore.emel` and `queue.emel` with the `@external type` and the `@intrinsic` functions, `QueueClosed`, and the Emela wrappers (`with_permit`, `try_with_permit`). `fiber.emel`: the two Scope operations and `Fiber.semaphore`, `Fiber.queue`.
- `emela-types`: the Scope operations (`fiber.rs`), and E0425 for `Semaphore` and `Queue` (`body/capabilities.rs`).
- `emela-core`: new `FiberOp` variants for creating, `with_permits`, `try_with_permits`, `available`, and the Queue operations, with the ones that wait marked as suspending (`ir.rs`, `lower/fiber.rs`, `verify.rs`).
- `emela-lir`: `from_core.rs`, `verify.rs`.
- Runtimes: JS (`runtime.mjs`), C (`emela_runtime.c`) and WASM. Each keeps the waiting fibers in first-in first-out lists, removes a fiber from its list when it is interrupted, and checks that the scope has not ended. `with_permits` gives the permits back on every way `f` ends, as a with runs its releases.
- Tests: programs under `emela-codegen-c/tests/programs/` (a limit of 2 with 5 fibers, the first-in first-out order, interruption while waiting, `try_with_permit` while full, a queue of capacity 1 with several producers, close with values left, close with waiting fibers, use after the scope ended), also in `wasm_parity.rs`.

## Alternatives

- **Written in Emela with State and Deferred.** A counter and a list of Deferreds to wake in State. Rejected: a fiber interrupted while waiting cannot be removed from the list, so the permit handed to it is lost; Deferreds in State are a run-time leak check now and a compile error with capture sets; and State is per type, so two queues of the same type would share one state.
- **Exposing `acquire` and `release`.** Taking and giving back in separate calls allows giving back from another fiber. Rejected for now: a fiber interrupted or failing between the two keeps its permits, and Emela has no construct other than a with that runs code however a computation ends. They can be added later without breaking anything; removing them later would break programs.
- **A handler that holds a permit (`with Permit(s) { ... }`).** init takes the permit and release gives it back. Rejected for now: the handler would implement an effect without operations, two such items in one with override each other so the first is never initialized (`[effect.with]`), and an interruption during init is not specified. A handler without an effect, if added later for other resources, could provide this form.
- **A Semaphore as an effect (`Limit.run(3, ...)`).** Its lifetime is the call, so no scope is needed. Rejected: two semaphores need two types, as two States do.
- **Making them in their own modules (`Semaphore.make(n)`, `Queue.bounded(n)`).** Keeps `Std.Fiber` small. Not chosen for now: the name `Fiber.` shows that the value belongs to a scope, and outside a scope the call is E0402 with no hint about Fiber.scope. Moving them later can be done together with Deferred.
- **`take` on a closed queue returning `Option[A]`.** Rejected: a closed queue is the end of the values, not an absence (`[lib.api-rules]`), and an error lets a consumer loop end with an escape arm that removes `QueueClosed` from E.
- **Closing a queue by interrupting the waiting fibers.** Rejected: an interruption cannot be received by escape, and the cases in which a fiber is interrupted are fixed (11.4).
- **Queues without a capacity, and queues that drop new or old values when full.** Not added: without a capacity, a fast producer grows memory without bound. Dropping can be written with `try_offer` and `poll`.
- **A Semaphore that lets a later fiber needing fewer permits go first.** More permits are used at once, but a fiber asking for many permits can wait forever behind a stream of small requests. Rejected for strict first-in first-out, which is also the simpler rule. For the same reason `try_with_permits` returns `None` while a fiber is waiting, so that it does not go ahead of the line.
- **Only the one-permit forms.** Rejected: a limit by weight (bytes, rows) needs n permits, and the rules and defects for n are already fixed.
- **`is_closed(q)`, and the name `shutdown`.** `is_closed` is not added: as with `available`, its answer can change before the next call, and it can be added later without breaking anything. `close` names what the producer does; the queue still gives out the values already in it.

## Prior art

- Effect (TypeScript): `Semaphore.make(n)` with `withPermits(n)(effect)`, `withPermitsIfAvailable(n)`, and also `take(n)` and `release(n)`. `Queue.bounded(n)`, `dropping`, `sliding`, `unbounded`; `Queue.end(q)` makes `take` fail with `Done` after the values are taken, and `Queue.shutdown(q)` interrupts the waiting fibers.
- Tokio (Rust): `Semaphore::acquire` returns a permit that is given back when it is dropped. `mpsc::Receiver::recv` returns `None` when the channel is closed and empty.
- Kotlin coroutines: `Channel(capacity)`, where `receive` throws `ClosedReceiveChannelException` once the channel is closed and empty.
- Go: buffered channels; a receive from a closed and empty channel returns the zero value and `false`.

## Open questions

## Decision

A Deferred is filled once, so fibers had no way to pass a sequence of values, and nothing limited how many of them run at once. Writing a limit by hand with State and Deferreds loses the permit of a fiber interrupted while it waits, and puts a Deferred in State. Semaphore and Queue are made by Scope, as Deferred is, so they belong to the scope that made them and E0425 and the run-time checks cover them. A Semaphore is used only through `with_permits`, which gives the permits back however `f` ends; `acquire` and `release` are left out because Emela has no other construct that runs code however a computation ends, and they can be added later. Waiting fibers are served strictly first-in first-out, so a fiber that asks for many permits is never starved by smaller requests, and `try_with_permits` does not go ahead of the line. The n-permit forms are kept for limits by weight. A Queue always has a capacity, so a fast producer cannot grow memory without bound. Closing it lets the consumer take the values left and then end on `QueueClosed`, an error rather than an absence. There is no `is_closed`, since its answer can change before the next call, as with `available`. A fiber of another scope still waiting when the scope ends stops with the same defect as any use after the scope ended.
