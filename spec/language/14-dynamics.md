# 14. Dynamic semantics `[dyn]`

Evaluation is strict and proceeds from left to right, and the initialization on entering a with and the release on leaving it always come in pairs. The details of this chapter were decided in the triage of 18.2. The formal semantics is planned to be defined on a core calculus obtained by desugaring the surface syntax.

## 14.1 Evaluation order `[dyn.order]`

- All arguments are evaluated before the function is called (call by value).
- Subexpressions are evaluated from left to right, in the order they are written in the source. Named arguments are also evaluated in the order they are written, not in the order of the declaration. The `..expr` of a list literal or a constructor call is evaluated after the other elements and fields.
- `&&` and `||` do not evaluate the right side when the left side determines the result.
- match evaluates the scrutinee exactly once and tries the arms from top to bottom. A guard is evaluated after its pattern matches. If no arm matches, it is a defect (type checking guarantees exhaustiveness, so this does not normally happen).

## 14.2 Running a with `[dyn.with]`

`with H1, ..., Hn { body }` runs in the following order.

1. Sort the handlers by their dependencies, and run their inits in that order.
2. Evaluate the body.
3. Run the releases in the reverse order of initialization.
4. The result of the body (a value, an error, a defect or an interruption) is the result of the with.

- Step 3 runs however the body ends.
- If an init fails, only the handlers already initialized are released, in reverse order, and the body is not evaluated. The release of the handler that failed is not run (because its init returned no value). The with ends with the error of that init.
- The effects used in the operation bodies of a handler are provided by handlers outside the with, or by handlers initialized earlier in the same with.

## 14.3 Error propagation `[dyn.error]`

- `fail e` ends the expressions being evaluated, one after another outward. When it reaches the innermost escape that has an arm matching the tag of e, that arm is evaluated, and its result is the value of the escape expression.
- When a with is left on the way, step 3 of 14.2 runs.
- When main ends normally, its return value is discarded without being shown, and the program exits with exit code 0. To see a value, use `dbg`.
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
3. When the operation body resumes on the spot, the caller continues without suspending. If the operation body has more to run after the resume, that part runs to the end first, and then the caller continues.
4. When the operation body resumes later, the caller resumes inside that call of resume, and resume returns (with the value `()`) when the caller next suspends or ends.

- A continuation can be resumed at most once. A second resume is a defect. The message says which operation of which handler it happened in. Mistakes visible by reading the code, such as calling resume twice on the same path or calling it inside a loop or recursion, get a warning from the linter. The types give no guarantee (adding one later would still accept every correct program).
- When an operation body fails without resuming, that error is the result of the operation call (it is in the caller's E as the operation's fails). When the body fails or becomes a defect after resuming, the caller has already moved on, so it is a defect at the top level of the program (or of the test) (`... failed after resume: <error>`).
- A resume called after main or the test function has ended does nothing (even a second one is not a defect).
- In a non-suspending context (R is Immediate: inside a with of a handler that resumes on the spot), when a suspending operation is not resumed on the spot, that continuation is never resumed (a handler that can resume later has Async in its requirements, so R is no longer Immediate). In this case, the continuation unwinds as interrupted at that point, without waiting for the top level of the program to end (main gives `interrupted` and exit code 130, and a test gives an `interrupted` failure).
- When a continuation is discarded without resume ever being called, it unwinds as interrupted. It unwinds when the scope waiting for that fiber (Fiber.scope, or the top level of the program) ends. An implementation need not detect the moment the continuation is discarded.
- When the continuation of main unwinds as interrupted, one line `interrupted` is printed to standard error and the program exits with exit code 130. The exit code tells it apart from an error (1, 14.3) and a defect (101, 7.5). When the continuation of a test function is interrupted, it is reported as a failure of that test.
- Unwinding as interrupted also runs the releases of the withs on the way, from the inside out.
- After main or the test function has ended with a value, an error or a defect, the withs inside the continuations left over (an operation implementation that keeps running after resume, for example) are not released.
- Switching to another fiber and interruptions taking effect happen only at suspension points (11.1).
- A resume called from another fiber (when resume is passed around as a value) is queued, and does not cut into the atomic section of the caller. A resume from a host callback or from the same fiber continues on the spot, as in 3 and 4.
- An operation implementation that keeps running after resume (3, 4) proceeds as a separate continuation of the same fiber. An interruption of a fiber reaches only the main continuation, and does not stop the continuation of the operation implementation. A scope does not wait for the continuations of operation implementations. When the launch (main or a test) is interrupted, these continuations of operation implementations are also closed (unwound, running their releases), and a defect in those releases becomes the result of the launch.
- When the event loop becomes empty, all fibers are interrupted and all waiting continuations are unwound. This cuts off releases that are running. Only a scope waiting for its children waits for the children to unwind.

## 14.6 Values and memory `[dyn.memory]`

- All values are immutable. Mutable state is limited to the State effect (8.8), host objects held by handlers, and Deferred (11.7, written only once).
- When memory is freed cannot be observed, and is unspecified.
- Resources are cleaned up in the release of a handler. There are no destructors tied to freeing a value.
