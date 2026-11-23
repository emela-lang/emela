---
title: Task replaces Fiber, and the range of concurrent work is a with item
authors: [Alliana]
status: accepted
issue:
touches: [task, task.basic, task.group, task.background (new), task.combinators, task.interrupt, task.cause, task.parallel, task.deferred, task.semaphore, task.queue, api.std.task, api.std.exit, api.std.cause, api.std.deferred, api.std.semaphore, api.std.queue, api.std.http, api.std.process, module.resolution, kw.annotation, effect.decl, effect.handler, effect.escape, effect.async, effect.label, type.fn, type.rules, ext.call, dyn.suspend, dyn.memory, lib.layers, lib.http-service, lib.api-rules, target.js.outcome, target.js.abort, target.js.event-loop, target.wasm.output, lsp.hover, lsp.completion, lsp.code-action, glossary.context, glossary.terms, open.next, diag.list]
requires: [capability-identity, generic-handler]
related: [scope-handle-values, semaphore-queue, http-service, closure-r-by-caller]
pr: 834
decided: 2026-11-24
---

# Task replaces Fiber, and the range of concurrent work is a with item

## Summary

- The range of concurrent work is no longer the special function `Fiber.scope`. It is an item of a with, provided by a built-in handler.
- Fiber is renamed Task: `Std.Fiber` becomes `Std.Task`, `Fiber[A]` becomes `Task[A]`, and `fork` becomes `start`.
- Work whose results are awaited is started in a `Task.Group`. Work whose results are not looked at is started in a `Task.Background`.
- Deferred, Queue and Semaphore are no longer made by operations of Scope. Each is an item of a with of its own: the capability of a built-in effect, provided by a built-in handler of the same name.
- A task gives its result only through `join`. `await` and `interrupt` are removed, and so is `Interrupt` of `CauseKind`.
- In the implementation of a suspend fn, a call that may suspend cannot follow a direct call of resume (E0461).
- Fiber is removed in the same version, without a period in which both exist.

## Motivation

- Exceptions gather around Scope: set type parameters, type parameters on an operation, no user handler (E0413), type arguments that may be omitted (an exception to E0423), the inclusion rule for the R of a child, a note of its own for E0402, E0425, and checks of lifetimes at run time.
- The rule that decides which handlers a child uses (the handlers in effect where `Fiber.scope` is called) is a third rule, different from both the nesting of withs and `closure-r-by-caller`. A with written in the body of a scope does not reach its children.
- A Deferred, a Semaphore or a Queue can be made only by an operation of Scope, so it stays outside the mechanism that decides the lifetimes of resources: withs and handler members.
- There are four ways to receive a result (join, await, attempt and raise), and they combine with three causes (Fail, Die and Interrupt), as the table of `[fiber.cause]` shows.
- There is no place for work whose result is not looked at, such as work that continues after a response (an audit log, sending metrics). `@entry(Http.serve)` makes one launch per request, so work cannot outlive the launch either.
- An operation implementation that keeps running after resume (`[dyn.suspend]`) is unstructured work that no scope waits for.
- `capability-identity` made the types stop leaks with `Fiber.scope(|s| ...)` and capability-bound types, but the exceptions of Scope remain, and so does the doubling of "s received as a parameter" and "Scope in R".

## Proposal

```emel
import Std.Task.{Task}

with tasks = Task.Group {
  user = tasks.start(|| fetch_user(id))
  orders = tasks.start(|| fetch_orders(id))
  Profile(user: user.join(), orders: orders.join())
}

with bg = Task.Background(on_failure: |cause| log.error(Cause.to_string(cause))) {
  bg.start(|| audit(req))
  respond(req)
}

with jobs = Queue(capacity: 100), tasks = Task.Group {
  tasks.start(|| jobs.offer(job))
  next = jobs.take()
}

(user, orders) = Task.both(|| fetch_user(1), || fetch_orders(1))
```

The examples use syntax that `emela fmt` does not read yet, so they have not been run through it.

### Task.Group

- A built-in handler. It is written as an item of a with, and its init makes the range.
- `tasks.start(f)` returns a `Task[A]`. The task starts running the next time the starting task suspends.
- When the body returns a value, the tasks left are interrupted, and the release waits until they have unwound.
- When a task fails, its siblings are interrupted, and the failure is delivered to the body at its next suspension point, as a failure of the body. The E of the tasks is in the E of the with. The E of the release stays empty (E0420 holds).
- When several tasks fail, the first failure is the result and the others are notes, as now.
- The R of the tasks is provided outside the with of the group: within the same with, only by the items before the group (the direction of `[effect.with]`). A closure that captures a capability made by a with inside the body does not fit the type of start, by `capability-identity`.
- `Task[A]` is a capability-bound type of the group's capability.

### Task.Background

- `bg.start(f)` returns `()`. There is no handle.
- The release does not interrupt the tasks left; it waits until they end, whether the body ended with a value or a failure. The result of the with is the result of the body.
- Its tasks are interrupted only when the with is interrupted from outside. If the interruption arrives while the release waits for its tasks, it is passed on to them, and the release ends after they have unwound. A release still holds off interruptions of its own task; it only passes them on to the tasks it waits for.
- A failure of a task does not stop its siblings. It is passed to `on_failure`.
- `on_failure: fn(Cause[E]) -> () uses { R }` is a required argument. E is the set of the errors of the tasks, and R is provided outside the with, as for the tasks.
- The E of `on_failure` is empty. It receives only Fail and Die.
- `on_failure` is called in the failed task, after that task has unwound. The release waits for it.
- A defect in `on_failure` is treated as a defect in a release (`[dyn.defect]`).

### `@entry(Http.serve)` and Spawn

- The R that the app may have grows from Async to Async and Spawn.
- The adapter provides Spawn by name with a `Task.Background` for each request, and calls the app inside it. When the app returns, the response goes to the host, and the launch ends at the end of the release.
- The `on_failure` of the adapter writes the cause to standard error.
- On Node, Bun and `emela run`, the promise of the response is resolved first. On Cloudflare Workers, the end of the launch is passed to `waitUntil` of the context. The native target has no serve yet.
- The type of serve is `serve[E, F](app: fn(Request) -> Response fails E uses { Spawn[F, { Async }], Async }, incoming: Incoming) -> Outgoing uses { Async }`. The errors F of the work started with Spawn are left open: `on_failure` writes any cause to standard error. The R of that work is Async only. An app writes, for example, `uses { Spawn[{}, { Async }] }`.

```emel
@entry(Http.serve)
pub fn app(request: Request) -> Response uses { Spawn[{}, { Async }] } {
  spawn <- Spawn
  spawn.start(|| audit(request))
  Routes.route(request)
}
```

### Deferred, Queue and Semaphore

- `effect Deferred[A]`, `effect Queue[A]` and `effect Semaphore` are built-in effects. `offer`, `take`, `await` and the other operations that wait are suspend fn operations.
- Each has a built-in handler of the same name, written as an item of a with (`with ready = Deferred, jobs = Queue(capacity: 100), db = Semaphore(permits: 10)`). The type arguments are inferred from how the body uses the item, or written (`Queue[Job](capacity: 100)`).
- All three are capabilities, so they need no capability-bound type and no "belonging to a scope". Their releases do nothing: a task that can use one is in a group or a background whose with is inside the with of the item.

### A handler with the name of its effect

- An effect and a handler share the type namespace (`[module.resolution]`), so the same name twice in a module is E0213.
- Only a built-in handler (`@intrinsic`) may have the name of the effect it implements, and it then implements only that effect.
- The place where the name is written decides which one it is. An item of a with, an item of a layer and the right of `by` mean the handler. `uses`, `use E`, `x <- E`, `L of E` and `E.M(...)` mean the effect.
- The two meet only on the right of `L by P`. There the name means the handler: `L by Queue` plugs in a new queue, and does not merge L into the outer Queue.
- Users cannot do this. Separating an effect from its handlers, and replacing the handler at a with, is central to Emela; the same name would make the abstraction to replace and the production implementation look the same. Deferred, Queue and Semaphore have one implementation each and nothing to replace. This can be widened when it is needed.
- Handlers that implement the effect for every A are those of `generic-handler`. This proposal makes no exception for them.

### Receiving results

- A task keeps only `join`. `await` and `interrupt` are removed.
- A task is interrupted only when the body of its group returns a value, or when a sibling fails. Both unwind the whole group, so no task that is not unwinding joins an interrupted task.
- The rule "joining an interrupted task is a defect" is removed. Joining a failed or interrupted task while a release holds off an interruption unwinds the caller as interrupted, as now.
- `CauseKind` has two variants, Fail and Die.
- To stop one task, nest a with and end it. To stop tasks that are added dynamically, pass them a stop signal with a Deferred.
- `Exit.attempt` and `Cause.raise` stay, independent of tasks.

### Names

| Now | New |
|---|---|
| `Std.Fiber` | `Std.Task` |
| `Fiber[A]` | `Task[A]` |
| `Fiber.scope` | `with tasks = Task.Group` |
| `fork` | `start` |
| `Fiber.all`, `race`, `both`, `map`, `timeout` | `Task.all`, `race`, `both`, `map`, `timeout` |

- The word fiber stays in chapter 14 and the chapters of the targets, as a term of the run time.
- It agrees with `em_task *` of the native target, a piece of work whose completion is awaited.

### Names of effects and handlers

- An effect names what a capability can do, and appears in `uses { ... }` and `use ...`. A handler names how it is provided (the policy of its lifetime), and is written `Module.Handler`.
- `Task.Group` implements the effect `TaskGroup`, whose `start` returns `Task[A]`.
- `Task.Background` implements the effect `Spawn`, whose `start` returns `()`. The lifetime is decided by the handler, so it is not in the name of the effect.
- One name is a noun and the other a verb: a group is a collection, and spawning is an act.
- A `use` expression and an effect alias may omit the type arguments of TaskGroup and Spawn. A uses clause and a type position write them, as for State (E0423). A function that receives a group writes them with type parameters, and puts the effects of its tasks in the set:

```emel
fn crawl[E, R](tasks: use TaskGroup[E, { Http, Async, R }], url: String) -> () {
  tasks.start(|| fetch(url))
  ()
}
```

- No rule is added to omit them in a signature. Most code starts tasks inside `with tasks = Task.Group`, where the type arguments are inferred. Whether to allow omitting them in a signature, and what the omission would mean, can be decided by another proposal after this one is used.

### No suspension after resume

- In the body of the implementation of a suspend fn, a call that may suspend and can be evaluated after a direct call of `resume(...)`, or after another use of resume (stored, captured in a closure, passed as an argument, `[effect.async]`), is a compile error (E0461). With another use, resume can come from a callback or another fiber while the implementation is still running.
- The check is syntactic and errs on the safe side, like the check for Async (`[effect.async]`). If one branch resumes, what follows the branch is after resume.
- This is the only place where a continuation is left behind. The R of a host callback is Immediate (`[type.effect-bound]`), so it cannot suspend after resume, and a resume from another fiber is run by that fiber itself.
- What remains after resume does not suspend, and runs to its end on the spot.
- Work that has to continue is started explicitly with Spawn, which becomes a requirement of the handler. The start is visible in the types, because it is in R.

```emel
handler RemoteLog: Logger {
  suspend fn info(msg, resume) {
    spawn <- Spawn
    spawn.start(|| send(msg))
    resume(())
  }
}
```

- Removed from `[dyn.suspend]`: an operation implementation runs as another continuation of the same fiber; an interruption does not reach it; a scope does not wait for it; it is closed at the end of the launch; the withs of continuations left after main are not released.
- Kept: failing after resume is a defect (`failed after resume`).
- A simple search found no such handler in std and examples (resume is at the end of the body). Tests and `tests/laws` are checked by the implementation.

### Type arguments of a named item

- `generic-handler` already infers the type arguments of a named item (`with x = H`) from how x is used (`[effect.with]`). The A of `with ready = Deferred` is unified with the type that `ready.await()` returns, and is E0459 if it is not determined.
- This proposal adds no rule for it.

### Permits of a Semaphore are a handler member

- Permits are a resource opened with a handler member of Semaphore. Its init takes them, and its release gives them back.
- `suspend handler Permits(n: Int): Permit` waits for them. `handler TryPermits(n: Int): Permit fails NoPermits` does not wait, and its init fails if they cannot be taken.
- `with_permits` and `try_with_permits` are plain Emela functions of std that open a member and call f. No exception is needed for an operation that takes a function and adds its E and R to the caller.
- "Given back however it ends", "the withs inside f close first" and "an interruption while waiting leaves without taking any" follow from the rules of release, the nesting of withs and init (`[effect.handler]`, `[dyn.with]`).
- The effect `Permit` (`fn count() -> Int`) and the error `NoPermits` are added. Permit has one operation, to avoid E0446.
- If f itself fails with NoPermits (it opened TryPermits of another semaphore), `try_with_permits` returns None, because the escape that receives NoPermits of its own TryPermits receives that one too. NoPermits comes only from TryPermits, so this is rare. Telling them apart needs a built-in function, which would go against keeping these functions in plain Emela.

### The function of Spawn.start returns ()

- `fn start(f: fn() -> () fails E uses { R }) -> ()`, the same form as `f: fn(A) -> ()` of `List.each`.
- Starting work whose result is wanted in a background (`bg.start(|| fetch_user(1))`) is a type error.

### Names of sections

- `[fiber.*]` are renamed `[task.*]`. The old names are kept in `spec/history.md` as renamed sections, as for `[target.c.export]`.
- Adopted proposals keep the old names. The comments in the code are updated by the implementation.

| Now | New |
|---|---|
| `[fiber]` | `[task]` |
| `[fiber.basic]` | `[task.basic]` |
| `[fiber.scope]` | `[task.group]` |
| (new) | `[task.background]` |
| `[fiber.combinators]` | `[task.combinators]` |
| `[fiber.interrupt]` | `[task.interrupt]` |
| `[fiber.cause]` | `[task.cause]` |
| `[fiber.parallel]` | `[task.parallel]` |
| `[fiber.deferred]`, `[fiber.semaphore]`, `[fiber.queue]` | `[task.deferred]`, `[task.semaphore]`, `[task.queue]` |
| `[fiber.escape]` | Retired (the rules of `capability-identity` are enough), kept in history |
| `[api.std.fiber]` | `[api.std.task]` |

### What goes and what stays

- Removed: `Fiber.scope`, the effect `Scope` and the exceptions for its type, making resources through Scope, `Fiber.await`, `Fiber.interrupt` and `CauseKind.Interrupt`.
- Kept: the group is built in. The exception of a function that takes any E and R moves to `start` of Group and Background.
- Kept: capability-bound types (`Task[A]`), as a general mechanism.
- Unchanged: there is no syntax for async and await, a caller does not mark a suspending call, and Async and Immediate.

## Specification changes

The changes are applied to `spec/` in this pull request.

- `[task]` (chapter 11, was `[fiber]`): replaced. `[fiber.*]` are renamed to `[task.*]`, `[task.background]` is new, and `[fiber.escape]` is retired. The old names are kept in `spec/history.md` as renamed and retired sections.
- `[module.resolution]`: a built-in handler can have the name of the one effect it implements.
- `[effect.decl]`, `[effect.escape]`, `[effect.async]`, `[effect.label]`, `[type.fn]`, `[type.rules]`, `[kw.annotation]`: TaskGroup and Spawn take the place of Scope.
- `[effect.handler]`, `[dyn.suspend]`, `[ext.call]`, `[target.js.abort]`: no call that may suspend after resume (E0461). The continuations left after resume are gone.
- `[api.std.task]` (was `[api.std.fiber]`), `[api.std.deferred]`, `[api.std.semaphore]`, `[api.std.queue]`, `[api.std.exit]`, `[api.std.cause]`: the new declarations. `CauseKind` loses Interrupt.
- `[api.std.http]`, `[lib.http-service]`, `[target.wasm.output]`: the app can use Spawn, and its tasks run after the response.
- `[lsp.code-action]`: the quickfix for a closure without parameters passed to `Fiber.scope` is removed. `[lsp.hover]`, `[lsp.completion]`: examples.
- `[diag.list]`: E0205, E0213, E0402, E0413, E0455 and the new E0461.
- Other sections: the names Fiber and Scope, and numbers of sections, in `[api.std.process]`, `[dyn.memory]`, `[lib.layers]`, `[lib.api-rules]`, `[target.js.outcome]`, `[target.js.event-loop]`, `[glossary.context]`, `[glossary.terms]` and `[open.next]`.

## Diagnostics

- [ ] E0461 (new): a call that may suspend after resume
- [ ] E0413 covers TaskGroup, Spawn, Deferred, Semaphore, Permit and Queue
- [ ] E0213: the exception for built-in handlers
- [ ] E0402: the note moves from `Fiber.scope` to the tasks of a group
- [ ] E0205: the help for `Std.Fiber`
- Numbers: E0455 to E0458 are used by `capability-identity`, and E0459, E0460 and E0462 by `generic-handler`. E0461 is free.

## Compatibility

- `capability-identity` (#732, implemented in #813) and `generic-handler` (#817, implemented in #825) are implemented. The `Fiber.scope(|s| ...)` part of `capability-identity` is replaced by this proposal. This proposal goes into 0.29, so that the change of `Fiber.scope` in #813 and this one are one breaking change.
- Fiber is removed at once, with no period in which the old names are accepted with a warning. Deferred, Semaphore and Queue change shape, so old names would not keep their meaning, and having Fiber and Task side by side would make two sets of rules hold at the same time.
- Importing `Std.Fiber` is E0205, with a help that names `Std.Task`.
- Programs that stop compiling: code that uses `Std.Fiber`; the functions of Deferred, Semaphore and Queue (`Deferred.await(d)` and so on, which become operations of a capability); a match on `CauseKind.Interrupt`; `Fiber.await` and `Fiber.interrupt`; a handler that may suspend after resume (E0461).
- Unchanged: an `@entry(Http.serve)` app that does not use Spawn, `Exit.attempt` and `Cause.raise`.

## Implementation

- `@entry(Http.serve)` on WASM: the response is returned first, and the work started with Spawn runs in the same task after `task.return`. The entry now blocks in `waitable-set.wait` when it waits, so it keeps the lock of the instance meanwhile, and the next `handle` waits. This is written as a limit of the WASM target, and does not change the meaning of the language. Rebuilding the entry as a callback that returns WAIT is a separate issue.
- Spawn is not a compile error on WASM: an entry whose type checks runs on every target (`[effect.host]`).
- Places to change: `respond` in `crates/emela-codegen-wasm/src/emit/runtime/http.rs` (split into making the response and what follows), `Run::Service` in `emit/mod.rs`, and `serve` in `std/http.emel`.

## Alternatives

- Stop at `Fiber.scope(|s| ...)` of `capability-identity`. The exceptions of Scope stay, and there is still no place for work whose result is not looked at.
- Make the scheduler a handler that users can write. It needs a use of resume that discards a continuation, and an operation that takes a function with effects. The latter conflicts with "replacing a handler does not change the R of the caller" (`[effect.decl]`). It stays as a long-term direction.
- No handles, only combinators. They do not cover joining partway, work per connection, or a signal that something is ready.
- Names: Fiber (the name of the mechanism), Concurrent (long, easy to mistype), Bud and other words of fruit trees (a common word was chosen), Job, Thread, Coroutine.
- A background that interrupts its tasks when its body fails: the record of a failed request would be lost.
- `on_failure` as an effect: taking `Cause[E]` as an argument of an operation needs a set type parameter (E0424).
- An app that receives the capability as an argument: the type of every app would change.
- Making Deferred and the others operations of a group again, or `Queue.run(100, |q| ...)`.
- Keeping `await` and `interrupt`.
- A release of a handler that waits for its operation implementations that run after resume: every with of every handler would wait, and the runtime would track each handler. Requiring Spawn writes the same thing.
- A handler with the name of its effect for users, a common handler name (`Queue.New(100)`, which reads unnaturally in a with), a default handler declared by the effect (`with Console` could be written in general, and test and production handlers would be confused), and `Deferred.Empty`, `Queue.Bounded`, `Semaphore.Permits` (long).
- `with_permits` as a built-in operation (an exception for an operation that takes a function), `acquire` and `release` as operations (they need a way to receive an interruption), and an `@intrinsic` function (the R of a function without a body cannot name a capability).
- `Spawn.start` taking any A and discarding it (a mistake goes through), or warning when it discards one (a new warning).
- Introducing Task as an experimental feature next to Fiber, and removing Fiber later. The tools of Fiber and Task would share names with different shapes, and two sets of rules would hold at once.

## Prior art

- Fiber, Scope and Deferred of Effect-TS, from which the current chapter 11 came.
- Structured concurrency (nurseries, task groups).

## Combinators that may be added later

`all`, `race`, `both`, `map` and `timeout` move to `Task`. The following are candidates to add when they are needed. This proposal does not add them.

| Function | Type | Meaning | Why |
|---|---|---|---|
| `until` | `until[A, E, R](stop: use Deferred[()], f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { Async, R }` | Runs f until stop is filled. If stop is filled first, interrupts f and returns None | Stopping tasks that are added dynamically, one by one, now that `interrupt` is gone |
| `map_limit` | `map_limit[A, B, E, R](xs: List[A], limit: Int, f: fn(A) -> B fails E uses { R }) -> List[B] fails E uses { Async, R }` | map with at most limit running at the same time | "1000 items, at most 10 concurrently" in one line |
| `each` | `each[A, E, R](xs: List[A], f: fn(A) -> () fails E uses { R }) -> () fails E uses { Async, R }` | Runs f on each element concurrently without collecting results | The `each` of `[lib.api-rules]`, instead of discarding the result of `map` |
| `first_success` | `first_success[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> A fails E uses { Async, R }` | The first success, ignoring failures. Fails with the last cause if all fail | Reading from replicas |
| `settle` | `settle[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> List[Exit[A, E]] uses { Async, R }` | Runs all of them without stopping on failure, and returns their Exits | Batch work that uses the successes and reports the failures |
| `workers` | `workers[A, E, R](jobs: use Queue[A], n: Int, f: fn(A) -> () fails E uses { R }) -> () fails E uses { Async, R }` | n tasks take from jobs and run f, until jobs is closed and empty | Producers and consumers, the `work` of the example of `semaphore-queue` made general |

- The names follow `[lib.api-rules]` (`map`, `each`). `map_limit` is a separate name, because `limit:` cannot be a parameter that may be left out.
- Supervision (restarting on failure) is left out: there are many policies, and std cannot choose one. It can be considered once users can write it, in the direction of the scheduler as a handler.

## Open questions

## Decision

Make the range of concurrent work an item of a with, so that tasks follow the same rules as every other capability: the lifetime is the with, the R of a task is provided outside it, and a leak is a type error by `capability-identity`. This removes the exceptions that gathered around Scope (its special typing, the handlers taken where `Fiber.scope` is called, the run-time checks of lifetimes, E0425), and gives a place to work whose result is not looked at, with `Task.Background` and Spawn, including work that continues after the response of `@entry(Http.serve)`.

- Deferred, Queue and Semaphore become capabilities of built-in effects, provided by built-in handlers of the same name, so their lifetimes are decided by withs and handler members like any other resource. Only built-in handlers may share the name of their effect.
- A task gives its result only through `join`. A task is interrupted only when its whole group unwinds, so `await`, `interrupt` and `CauseKind.Interrupt` are no longer needed.
- A call that may suspend cannot follow resume in a suspend fn (E0461), which removes the continuations that no scope waited for. Work that continues is started with Spawn, and shows in R.
- The type arguments of TaskGroup and Spawn are written in signatures, as for State; no rule to omit them is added until it is needed.
- This lands in 0.29 together with #813, so that Fiber changes once. Fiber is removed at once rather than kept beside Task, since the tools would share names with different shapes and two sets of rules would hold at the same time.
