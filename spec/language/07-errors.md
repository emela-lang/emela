# 7. Errors `[error]`

There are only two tools for handling errors: `fail` and `escape`. A failure propagates to the caller when nothing is written, and handling it with escape turns it back into an ordinary value.

## 7.1 error declarations `[error.decl]`

```emel
error NotFound(id: Int)
error Unauthorized
error DbError(message: String) derive Show
```

- Errors are identified by their tag names. Fields are values and play no part in identification. `NotFound(id: 1)` and `NotFound(id: 2)` are the same error.
- The errors a function can raise appear in its type as a set of tags, E. Because it is a set, order and duplicates have no meaning.
- The error set is the only place in the language where a union appears.
- An error declaration, like a type, defines a type and a constructor with the same name. `NotFound(id: 1)` is an ordinary value of type `NotFound`, and it MAY be bound to a variable, passed to a function, or put in a data structure.
- Only a single error type can be handled as a value. A type for a value that is "either NotFound or DbError" cannot be made.
- An error declaration can also have derive, like a type (10.5). An error that reaches main or a test is displayed with the Show of its type (13.3, 14.3).

## 7.2 fail `[error.fail]`

```emel
fail NotFound(id:)
```

`fail e` ends the current computation with the error `e`. The type of `e` MUST be an error type. The type of the expression is Never, and the tag of the type of `e` is added to E.

When a type parameter is used both as the type of a value (`err: E`) and as an error set (`fails E`), that type parameter is inferred as a single error type. The same holds when the type of `e` is a type variable that is not yet determined: the type of `fn raise(err) { fail err }` is `∀ E, A. fn(E) -> A fails E`. Passing a type that is not an error to `fail`, and instantiating a type parameter for a single error type with a type that is not an error, are compile errors (E0319; for instantiation, the diagnostic points at the call). Passing a value of type Never to `fail` adds nothing to E.

```emel
fn or_fail[A, E](opt: Option[A], err: E) -> A fails E {
  opt match {
    Some(a) -> a
    None    -> fail err
  }
}

user = users.find(id) |> Option.or_fail(NotFound(id:)) # E = NotFound
```

## 7.3 escape `[error.escape]`

```emel
# Recover: turn it back into an ordinary value
user = find_user(id) escape {
  NotFound(_) -> guest_user()
}

# Handle only some: DbError propagates to the caller
data = load(id) escape {
  NotFound(_) -> default_data()
}

# Turn it into another error
data = load(id) escape {
  DbError(message:) -> fail Unavailable(reason: message)
}

# Handle it as a value
found = Some(find_user(id)) escape {
  NotFound(_) -> None
}
```

- `expr escape { arms }` handles errors raised while evaluating the expression. The pattern of an arm MUST be an error constructor (`_` and variables cannot be written). A constructor that is not an error at the top level is E0217, and `_` or a variable is E0320. An escape with such a mistake is checked further as if it handled all the errors of the original expression (E0318 is not reported in addition). Exhaustiveness of the arms is not required.
- An arm that handles an error the original expression cannot raise gets a warning (W0302). If an arm for the same error is covered by the arms above it, it is warned about as a redundant arm (W0301). Both checks also apply to arms that handle only some values. They are checked only when the E of the original expression contains no opaque set variable.
- The result of an arm MUST have the same type as the original expression, or be Never (`fail` or `panic`). When the type of the original expression is Never, the common type of the arms is the type of the whole.
- The tags handled by the arms are removed from E. The tags not handled propagate as they are.
- A tag is removed only when the arms without guards for that error together handle all values of that error (`NotFound(_)`; two arms `Flag(on: True)` and `Flag(on: False)` also work). If there are only arms that handle some of the values (`NotFound(id: 1)`, arms with guards), the tag stays in E, and values that match no arm propagate to the caller as errors.
- Errors raised in an arm are added to E.
- When the E of the expression contains an opaque set variable (a set type parameter `E`, the hidden set variable of a parameter's function type, a type parameter for a single error type, or the set variable of a definition being generalized), escape removes nothing from that variable. From sets being inferred within the same function (such as the E of a local function), tags are removed exactly. At run time the arm does handle that tag, but in the type the variable remains.
- To remove a tag exactly in the type as well, write the tag to handle in the function type (`fails Timeout | E`, 7.4). A written tag is outside the set variable, so escape removes it.

```emel
# The E of f is the hidden set variable ε. After escape the type is still ε (Timeout can remain)
fn retry[A](f: fn() -> A) -> A {
  f() escape { Timeout(_) -> f() }
}

# Writing out Timeout removes the Timeout of the first call from the type as well
fn retry[A, E](f: fn() -> A fails Timeout | E) -> A fails Timeout | E {
  f() escape { Timeout(_) -> f() } # the E of the escape is E, and the second f() gives Timeout | E
}
```

- At the call site the set is determined, so escape in everyday code is always exact. The difference appears only inside generic functions that take functions.

## 7.4 Rules for the fails annotation `[error.fails]`

In a function declaration, the way the return value is written determines how errors are treated.

| Written as | Meaning |
|---|---|
| Return type omitted | E is inferred. Errors propagate as they are |
| `-> T` | Forces no errors. An unhandled error in the body is a compile error |
| `-> T fails NotFound` | Forces the errors to be exactly this set |

Multiple errors are listed separated by vertical bars. Error tags and type parameters that stand for error sets MAY be mixed.

```emel
fn load(id: Int) -> User fails NotFound | DbError { ... }
fn retry[A, E](f: fn() -> A fails Timeout | E) -> A fails Timeout | E { ... }
```

`Timeout | E` is the union of the tag Timeout and the set variable E. Written in the function type of a parameter like this, the errors of the passed function other than Timeout become E. A function that does not raise Timeout MAY be passed.

In a function type written in a parameter position, omission means something different. `fn(A) -> B` means "it can fail with anything, and the errors propagate to the caller". To limit it to functions that do not fail, write `fails {}` (9.2).

The E of a function with a written return type is the written fails clause plus the errors from calling the parameter functions (the hidden set variables of the parameters' function types). Hidden set variables cannot be written, so even with `-> T` the errors of the parameter functions propagate.

```emel
# E is the errors of f. Not a compile error even with -> Int
fn twice(f: fn() -> Int) -> Int { f() + f() }
```

The hidden set variable of a function that a parameter function receives (the inner one in `f: fn(fn() -> A) -> B`) is opaque from the body, so passing a function that fails there is a compile error (E0321).

An error not included in the E of the declaration is E0318. The diagnostic points, for each error, at the first expression that raised it. The first expression is found by following, in written order, the subexpressions whose E contains that error, down to an expression that cannot be followed further (a `fail` or a call). Multiple errors raised from the same expression are reported as one.

## 7.5 defect `[error.defect]`

A defect represents a mistake in the program or an unrecoverable abnormal condition. It does not appear in types and cannot be handled with escape.

| Source | Example | Example message |
|---|---|---|
| `panic(message)` | A branch that should be unreachable | The message itself. For `todo()`, `not yet implemented` |
| A failed `assert` | Verification in a test | `assertion failed: ...` (13.2) |
| Integer division by zero | `1 / 0` | `division by zero` |
| An exception thrown by an external function | A throw in JS | `TypeError: bad` (12.5) |
| A failed conversion from Float to Int | `Float.floor(NaN)` | `cannot convert NaN to Int` |
| A negative shift amount | `Int.shift_left(1, -1)` | `negative shift amount: -1` |
| A value that matches no match arm | When it slips past the exhaustiveness check | `unreachable: no match arm matched` |
| A second resume | The same continuation was resumed twice | ``resume called twice in operation `sleep` of handler `SystemClock` `` (`[dyn.suspend]`) |
| A call to an external function without a target annotation | An external function reached from a public fn | ``external function `console_log` is not available on this target`` (`[ext.decl]`) |

The wording of the messages, like the diagnostics in Appendix A, is not part of the specification and MAY change for improvement.

- Even when a defect happens, the release of handlers runs (Chapter 14).
- A defect can be observed only at a boundary. A boundary is the top level of the program, the test runner, or `Fiber.attempt` (11.5).
- To turn an expected error into a defect, call panic in an escape arm. The conversion in the other direction is possible only at a boundary.
- When a defect reaches the top level of the program, `emela run` prints one line `defect: <message>` to standard error and exits with exit code 101.

## 7.6 What Emela does not have `[error.not]`

There is no Result type, `?` operator, try / catch, or mapError. Conversion from an Option to a failure is written with an ordinary function such as `Option.or_fail(opt, NotFound(id:))`.
