# 13. Testing `[test]`

A test is a function with `@test`, and checks are written with `assert`. Replacing handlers is all it takes to write a test, so the language has no mechanism for mocks.

```emel
@test
fn greet_returns_user() {
  with AppTest {
    user = greet(1)
    assert user.name == "alice"
  }
}

@test
fn greet_fails_for_unknown_id() {
  with AppTest {
    result = Some(greet(999)) escape { NotFound(_) -> None }
    assert result == None
  }
}
```

## 13.1 @test `[test.attr]`

- `@test` can be attached only to a fn with no parameters.
- Attaching `@test` to a fn with no body (`@external`) or to anything other than a top-level fn (a fn inside a type or an impl, for example), or giving it arguments as in `@test(...)`, is a compile error (E0226). A fn with an invalid `@test` is not collected as a test function.
- Tests can be written in any file. A test written in the same file as the implementation can test private functions directly.
- Test functions are removed in a normal build.
- A test function can be referred to only from inside a test function (E0227), because test functions are removed in a normal build. Test functions MAY refer to each other. A reference includes a reference as a value, not only a call.

The test root:

```emel
# test/support.emel (can hold helper functions that take parameters)
import Brix.Pbt.Gen.{Gen}

pub fn small_lists() -> Gen[List[Int]] {
  Gen.list(Gen.int_between(0, 9))
}

# test/sort_test.emel
import Brix.Pbt.Property
import Sort
import Support

@test
fn sort_keeps_length() {
  Property.check(Support.small_lists(), fn(xs) {
    assert List.length(Sort.sort(xs)) == List.length(xs)
  })
}
```

- Every module in the test root (4.1) is used only for testing. The declarations in it are removed in a normal build even without `@test`, so the test root can hold helper functions that take parameters, and generators. A fn with `@test` runs as a test function.
- A module in the test root can import modules in the source root (private items are not visible), other modules in the test root, and the Pomes in `[dependencies]` and `[dev-dependencies]`.
- A module in the source root cannot import modules in the test root or the Pomes in `[dev-dependencies]` (E0239). A test in the same file as the implementation can use std and the Pomes in `[dependencies]`.
- `check`, `test` and the language server read the test root, and `build` and `run` do not. For a dependency Pome, neither the test root nor the test functions are read. The source root does not refer to test dependencies, so a dependency Pome can be read without its test dependencies (4.7).
- Helpers for comparing values, generators and property testing are not in std. They are distributed as the Pome `github.com/emela-lang/brix` (15.2), used as a test dependency.

## 13.2 assert `[test.assert]`

- In `assert expr`, the type of the expression is Bool. When it is False, it is a defect. The type of the whole assert expression is `()`.
- A comparison assert evaluates each side once, from left to right. When it fails, the defect message is `assertion failed: <source text of the expression>`, and if the types of both sides of the comparison satisfy Show, the values of both sides follow, shown as by inspect (10.6). Otherwise only the source text of the expression is shown. assert does not require Show.
- assert is a keyword, and the compiler looks at the form of the expression.
- When a comparison assert fails, the message is `assertion failed: <source text of the expression>`, followed by a line break and `  left: <left side shown>`, then a line break and ` right: <right side shown>`. `left` is preceded by two spaces and `right` by one space, so that the colons line up. Both sides are shown the same way as by inspect (`[trait.builtin]`), and a String is enclosed in quotes.
- When an assert that is not a comparison fails, the message is `assertion failed: <source text of the expression>`, and no values are shown.
- A comparison assert shows both sides only when the comparison is False. When it is True, the functions that show the values are not called.
- assert can also be used outside test functions. It is not removed in a normal build either.

## 13.3 Constraints on test functions `[test.constraints]`

- The R of a test function MUST be empty except for Async. The test runner provides Async.
- An error that is not handled is reported as a failure of that test. The E of a test function MAY be non-empty. Errors are shown the same way as for main in 14.3.
- A defect is also reported as a failure of that test. The other tests keep running.

## 13.4 emela test `[test.command]`

- `emela test [path] [filter]` runs the test functions of a program that passed checking, one at a time.
- The name of a test in the entry module is just the function name. A test in another module has the module name in front (`Http.Client.gets`). Tests in modules of the test root also run, and are named the same way.
- When a filter is given, only the tests whose names contain it as a substring (case-sensitive) run. In single-file mode, only the tests in modules reachable by import from the entry point run.
- Results go to standard output: a line `test name ... ok` or `FAILED` for each test, and a summary line at the end (`test result: ok. 3 passed; 0 failed; 0 filtered out`).
- The exit code is 1 if at least one test fails, and 0 otherwise (also 0 when there are no tests). When the program exits in the middle of a test, the running test is a failure.
- A test that ends while suspended without being resumed is reported as an `interrupted` failure, and the remaining tests continue (14.5).
- The output is written to `target/emela/test` and does not overwrite the output of a normal build.
