# 14. Dynamic semantics `[dyn]`

Evaluation is strict and proceeds from left to right, and the initialization on entering a with and the release on leaving it come in pairs: a release runs only for a handler whose init has returned, and every such handler is released, except in the cases listed in `[dyn.with]`. The details of this chapter were decided in the triage of 18.2. The formal semantics is planned to be defined on a core calculus obtained by desugaring the surface syntax.

## 14.1 Evaluation order `[dyn.order]`

- All arguments are evaluated before the function is called (call by value).
- Subexpressions are evaluated from left to right, in the order they are written in the source. Named arguments are also evaluated in the order they are written, not in the order of the declaration. The `..expr` of a list literal or a constructor call is evaluated after the other elements and fields.
- `&&` and `||` do not evaluate the right side when the left side determines the result.
- match evaluates the scrutinee exactly once and tries the arms from top to bottom. A guard is evaluated after its pattern matches. If no arm matches, it is a defect (type checking guarantees exhaustiveness, so this does not normally happen).

## 14.2 Running a with `[dyn.with]`

`with H1, ..., Hn { body }` runs in the following order.

1. Evaluate the arguments of the items (`H(arguments)`, 8.3) in the order the items are written. Then sort the handlers by their dependencies, and run their inits in that order. A handler member item `x = E.M(arguments)` or `x = c.M(arguments)` runs the init of M of the innermost handler that provides E by name (or of the handler that provides c), and is ordered with the other items by its requirements (only E or c).
2. Evaluate the body.
3. Run the releases in the reverse order of initialization.
4. The result of the body (a value, an error, a defect or an interruption) is the result of the with.

- Step 3 runs however the body ends. A release does not run, or is cut off, only in these cases. In each of them, the program may be unable to go on to run or finish a release.
  - When the program cannot obtain the memory it needs, whether the releases run is implementation-defined (`[dyn.memory]`). Running a release can itself need memory, and the targets behave differently when memory runs out.
  - When the event loop becomes empty, releases that are running are cut off, except the release of a group or a background, which waits for its tasks to unwind (`[dyn.suspend]`). Nothing is left that could resume a suspended release, and a release is not interrupted (`[task.interrupt]`), so it cannot be unwound as a body is. The tasks of a group or a background are interrupted when the event loop becomes empty, so the wait for them ends.
  - When the process ends before the program leaves the with, no more releases run, because no code of the program runs after the process ends. This happens when an external function ends the process (for example, a C function calls `abort` or is stopped by a signal, `[ext.exception]`), when an exported fn is called while a launch is running on the same thread (`[target.native.export]`), and when the process is ended from outside the program (by a signal, such as the one Ctrl-C sends, or when the host stops). An exported fn called during a launch ends the process because the runtime may be in the middle of an allocation or a switch between tasks, where no code of the program can run safely.
- If evaluating an argument fails, no init has run yet, so nothing is released, and the with ends with that error.
- If an init fails, only the handlers already initialized are released, in reverse order, and the body is not evaluated. The release of the handler that failed is not run (because its init returned no value). The with ends with the error of that init.
- The effects used in the operation bodies of a handler are provided by handlers outside the with, or by the handlers of items without a name initialized earlier in the same with.
- Handlers opened inside an init (8.4) are released after the release of the handler that opened them, in reverse order. A handler opened through a handler that a with inside the init provides is released at the end of that with instead (8.4).

## 14.3 Error propagation `[dyn.error]`

- `fail e` ends the expressions being evaluated, one after another outward. When it reaches the innermost escape that has an arm matching the tag of e, that arm is evaluated, and its result is the value of the escape expression.
- When a with is left on the way, step 3 of 14.2 runs.
- When main ends normally, the program exits with exit code 0, and the return value is discarded without being shown. The exception is a main whose return type is `ExitCode` (`[api.std.process]`): the program exits with the exit code of the value. To see a value, use `dbg`.
- The return type of main SHOULD be `()` or `ExitCode`. A main whose return type is none of `()`, `ExitCode` and Never is W0304; it still runs, and its value is discarded as above.
- The exit code chosen by an `ExitCode` MAY be 1, 101 or 130. The exit code alone then does not tell it apart from an error, a defect or an interruption; only the line written to standard error does.
- An error that reaches main is an abnormal termination of the program. One line `error: <the error value shown>` is printed to standard error, and the program exits with exit code 1. If the type of the error satisfies Show, it is shown the same way as by inspect (10.6) (for `error DbError(message: String) derive Show`, `error: DbError(message: "down")`). Otherwise only the name of the error is shown (`error: Retry`). The E of main does not require Show. The exit code tells it apart from a defect (exit code 101, 7.5). The E of main MAY be non-empty.

## 14.4 Defect propagation `[dyn.defect]`

- A defect passes through escape and unwinds to the boundary. Each with on the way runs its releases.
- A release cannot end with an error (its E is empty, 8.4). When a defect happens inside a release, the result is as follows.
  - If the body ended with a value or an error, the defect of the release is the result of the with.
  - If the body already ended with a defect, the first defect is the result, and the defect of the release is added to the output as a note.
  - In both cases, the releases of all the remaining handlers run.
  - The notes have the form `note: the release of handler <name> also failed: <message>`, in the order they happened.
  - When a release becomes a defect while unwinding as interrupted, the result is that defect, not `interrupted`. When a suspending operation inside a release is discarded without being resumed, the result is the body's defect if the body ended with a defect, and `interrupted` otherwise.

## 14.5 Suspension and resumption `[dyn.suspend]`

1. Calling a suspending operation captures the rest of the caller's computation as a continuation, and runs the operation body of the handler.
2. When the handler calls `resume(v)`, the continuation resumes, and the value of the operation call expression is v.
3. When the operation body resumes on the spot, the caller continues without suspending. If the operation body has more to run after the resume, that part does not suspend (E0461, 8.4); it runs to the end first, and then the caller continues.
4. When the operation body resumes later, the caller resumes inside that call of resume, and resume returns (with the value `()`) when the caller next suspends or ends. The rest of the operation body then runs to the end without suspending.

- A continuation can be resumed at most once. A second resume is a defect. The message says which operation of which handler it happened in. Mistakes visible by reading the code, such as calling resume twice on the same path or calling it inside a loop or recursion, get a warning from the linter. The types give no guarantee (adding one later would still accept every correct program).
- When an operation body fails without resuming, that error is the result of the operation call (it is in the caller's E as the operation's fails). When the body fails or becomes a defect after resuming, the caller has already moved on, so it is a defect at the top level of the program (or of the test) (`... failed after resume: <error>`).
- A resume called after main or the test function has ended does nothing (even a second one is not a defect).
- In a non-suspending context (R is Immediate: inside a with of a handler that resumes on the spot), when a suspending operation is not resumed on the spot, that continuation is never resumed (a handler that can resume later has Async in its requirements, so R is no longer Immediate). In this case, the continuation unwinds as interrupted at that point, without waiting for the top level of the program to end (main gives `interrupted` and exit code 130, and a test gives an `interrupted` failure).
- When a continuation is discarded without resume ever being called, it unwinds as interrupted. It unwinds when the group waiting for that task (`Task.Group`, `Task.Background`, or the top level of the program) ends. An implementation need not detect the moment the continuation is discarded.
- When the continuation of main unwinds as interrupted, one line `interrupted` is printed to standard error and the program exits with exit code 130. The exit code tells it apart from an error (1, 14.3) and a defect (101, 7.5). When the continuation of a test function is interrupted, it is reported as a failure of that test.
- Unwinding as interrupted also runs the releases of the withs on the way, from the inside out.
- Switching to another fiber and interruptions taking effect happen only at suspension points (11.1).
- A resume called from another fiber (when resume is passed around as a value) is queued, and does not cut into the atomic section of the caller. A resume from a host callback or from the same fiber continues on the spot, as in 3 and 4.
- When the event loop becomes empty, all fibers are interrupted and all waiting continuations are unwound. This cuts off releases that are running. Only the release of a group or a background waits for its tasks to unwind.

## 14.6 Values and memory `[dyn.memory]`

- All values are immutable. Mutable state is limited to the State effect (8.8), host objects held by handlers, Deferred (11.8, written only once), Semaphore (11.9) and Queue (11.10).
- When memory is freed cannot be observed, and is unspecified.
- If a program cannot obtain the memory it needs, the result is implementation-defined (`[target.unspecified]`), including whether it is a defect and therefore whether the releases of handlers run and whether the failure can be observed at `Exit.attempt` as a `Die` cause (`[task.cause]`). What, if anything, is written and the exit status are implementation-defined as well. A program MUST NOT depend on any of these.
- Resources are cleaned up in the release of a handler. There are no destructors tied to freeing a value.
