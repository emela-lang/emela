---
title: A background does not close
authors: [Alliana]
status: draft
issue: 861
touches: [task.background, task.interrupt, api.std.task, lib.http-service]
requires: []
related: [task, http-service]
---

# A background does not close

## Summary

A `Task.Background` does not close. Its tasks can start siblings while the body runs, after a task has failed, and after the body has ended while the release waits for the tasks. The release waits for the tasks started while it waits as well, so no task outlives the with. A task started after the with was interrupted from outside is interrupted before it starts running: it ends without calling its function, and `on_failure` is not called for it. A start on a background is never a defect.

## Motivation

`[task.background]` brings in the rules of `[task.group]` on start "as they are", and `[task.group]` says that a group closes when its body ends or a task fails, and that a start on a closed group is a defect (`task group was used after it closed`). `[task.background]` says nothing of its own about closing. Read as written, a background closes when one of its tasks fails. This contradicts the next rule of `[task.background]`: a failure of a task does not stop its siblings or the body.

The implementation does something else (#861). On the JS, WASM and C targets a background does not close when a task fails, and closes when its body ends, as a group does. So a task that runs after the body has ended cannot start a sibling: the start is a defect, and goes to `on_failure`.

This makes a common shape impossible to write for an HTTP service. `@entry(Http.serve)` calls the fn inside a `Task.Background` for each request and provides Spawn by name (`[lib.http-service]`). The body of that background ends when the fn returns its `Response`. Work started with Spawn runs after the response (an audit record, sending metrics), and that work cannot split itself into further tasks:

```emel
@entry(Http.serve)
pub fn app(request: Request) -> Response uses { Spawn[{}, { Async }] } {
  spawn <- Spawn
  spawn.start(|| {
    say("audit runs after the response")
    spawn.start(|| say("follow-up started by audit"))
  })
  Response.new(200, "ok")
}
```

On every target the inner start is a defect (`defect: task group was used after it closed` on standard error), and the follow-up never runs.

Closing a background serves no purpose. A group closes so that its tasks are interrupted and the with ends once the body has its value; a background waits for its tasks anyway. While the release waits, any start comes from a task of the background that has not ended (or from `on_failure`, which runs in such a task), since the capability cannot leave the with (`[effect.escape]`). The release therefore has not ended at the time of the start, and it can wait for the new task too.

## Proposal

### Syntax

No change.

### Meaning

`[task.background]`:

- The sentence that brings in "the rules of 11.2 on start ... as they are" is replaced by the rules it brings in, named one by one: `bg.start(f)` starts f as a task and returns `()`; f returns `()` (a task whose value is wanted is started in a group); as in 11.2, start is not a suspension point, the task starts running the next time the starting task suspends, a task interrupted before it starts running ends without calling f, and the rules on the E and R of f and on where R is provided apply. The rule of 11.2 on closing is not among them.
- A background does not close. `bg.start(f)` is allowed while the body runs, after a task has failed, and after the body has ended while the release waits. The release waits for the tasks started while it waits as well, so the rule of 11.2 that no task outlives the with that started it holds.
- A task started after the with was interrupted from outside (while the body unwinds, or while the release waits) is interrupted before it starts running: it ends without calling f (11.2), and `on_failure` is not called for it.
- A start on a background is therefore never a defect.

`[task.interrupt]`: the rule that a task of a background is interrupted only when the with of the background is interrupted from outside is unchanged. A task started after that interruption counts as interrupted by it.

`[api.std.task]`: the row of `Background` says that the release waits for the tasks started while it waits, and that a start on a background is never a defect.

`[lib.http-service]`: a task started with Spawn MAY start further tasks after the fn has returned, and the release of the background of the launch waits for them too.

### Typing rules

No change.

## Diagnostics

- [ ] No diagnostic is added or removed. The defect `task group was used after it closed` is no longer raised for a background; it stays for a group.

## Compatibility

A program that started a task on a background after its body ended got a defect, which went to `on_failure`; it now runs the task. No program that worked before changes its behavior, except that the release of such a background may now end later (it waits for the new tasks). A program that compiles keeps compiling.

A group is not changed. A start on a group after its body ended, including one in a release while the group is interrupted from outside, stays a defect; whether that case should be treated as this proposal treats a background is a separate question.

A task can keep a background alive by starting a new task before it ends, as a task that loops can now. This is not a new kind of program: the release of a background already waits for tasks that do not end.

## Implementation

The three runtimes (`crates/emela-codegen-js/src/runtime.mjs`, `crates/emela-codegen-wasm/src/emit/runtime/fiber.rs`, `crates/emela-codegen-c/src/emela_runtime.c`) share the shape:

- When the body of a background ends, the scope is not marked closed.
- The scope records that the with was interrupted from outside (the body unwound by an interruption, or one arrived while the release waits).
- `fork` on a background that records the interruption makes the child, adds it to the tasks the release waits for, and interrupts it at once, so it ends without calling f.

Tests: the two programs of #861 (a start after the body ended, a start after a failure), a start from `on_failure`, and a start after an interruption from outside, in the JS, WASM and C tests and in `wasm_parity`; the `@entry(Http.serve)` example above in the CLI tests.

## Alternatives

- Keep the current behavior and write it down: a background closes when its body ends, not when a task fails. The rule is consistent, but the shape in the motivation stays impossible, and the only workaround is to start every piece of follow-up work from the body before it ends, which the body cannot know of in advance.
- Make a start after an interruption from outside a defect, as a start on a closed group is. A start comes after the interruption in two ways: a release that runs while a task unwinds starts a task (to record that it stopped, for example), or a task that has not yet reached its next suspension point, where the interruption takes effect (`[task.interrupt]`), starts one without knowing of the interruption. The second cannot be avoided by the program. A defect raised in a release is a defect of that release (14.4), so the with that was interrupted would end with a defect in place of the interruption. Starting the task interrupted gives the same effect (f does not run) without a defect, and uses the rule of 11.2 for a task interrupted before it starts running.
- Run the task started after the interruption as usual. The interruption would not reach that task, and the release of an interrupted with could wait for it without end.
- Keep the sentence that brings in the rules of 11.2 "as they are" and add "except that a background does not close". Each rule later added to 11.2 about start would raise the same question of whether it applies to a background. Naming the rules brought in leaves the rule on closing out from the start, and `[task.group]` needs no change.
- Use a defect message that names the background (`background was used after it closed`). With this proposal a background raises no such defect, so the message is not needed.

## Prior art

Trio's nursery is marked closed, so that no new task can be started in it, once all its tasks have finished, and a nursery object can be passed to a task so that the task starts siblings in it. A task started in a nursery inherits the nursery's cancel scopes, and a cancelled scope raises `Cancelled` at every checkpoint inside it. Its documentation also says that `start_soon` raises an error when the nursery's `async with` block has exited, and does not say which rule wins for a start from a task after the block has exited while tasks are still running.

## Open questions

