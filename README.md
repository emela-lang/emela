# Emela

Emela is a functional language with typed errors and typed effects, compiling to
JavaScript and WebAssembly.

## Status

This branch is the 0.20 rebuild. The current release is **0.20.0**:
the pure subset, typed errors and effects, suspending effects, handlers with
state, fibers, files and an HTTP client compile to JavaScript and run on
Node.js. The 0.11 compiler lives on the [`legacy`](https://github.com/emela-lang/emela/tree/legacy)
branch.

| Release        | Adds                                                                                                                       |
| -------------- | -------------------------------------------------------------------------------------------------------------------------- |
| 0.20.0-alpha.1 | Functions, records, enums, `opaque` types, pattern matching, tail calls, pipes, `List` / `Option`, modules, `@test`, `dbg` |
| 0.20.0-alpha.2 | Typed errors and effects: `error`, `fail`, `escape`, `effect`, `handler`, `use`, `with`, `@external`, `Console`            |
| 0.20.0-alpha.3 | `suspend fn`, `Async`, `Immediate`, `Clock`, `Random`, `Env`, `Instant` / `Duration`                                       |
| 0.20.0-alpha.4 | Handlers with fields, `init` and `release`, `layer`, `State`, the `Std` namespace, `Platform`                              |
| 0.20.0-alpha.5 | Fibers (`Std.Fiber`, `Std.Cause`, `Std.Deferred`), `FileSystem`, `Std.Path`, the `Http` client                             |
| 0.20.0         | Postfix `match`, `const` evaluated once on first use, E0208 for a directory outside the source root, English diagnostics   |
| 0.21 and later | `trait`, `impl`, `derive`, `Map` / `Set` / `Array`, JSON, `Semaphore`, `Queue`, an `Http` server, WebAssembly output       |

The 0.20 language specification is in [`spec/`](spec/README.md).
Changes to it are proposed in [`proposals/`](proposals/README.md). The
[`emela-lang/specification`](https://github.com/emela-lang/specification)
repository describes the 0.1x language, not 0.20.

## Install

Prebuilt binaries are published for macOS (Apple Silicon) and Linux (x86_64).
Running the generated JavaScript (`emela run`, `emela test`) needs Node.js 18
or later. The C target (`--target c`) needs a C11 compiler as `cc` (or set
`EMELA_CC`). The WASM target (`--target wasm`) needs Wasmtime 49 or later, run as a
WASI 0.3 component with `-S p3=y` (or set `EMELA_WASMTIME`).

```sh
curl -fsSL https://emela.dev/install.sh | sh
emela --version
```

This installs the latest release into `$HOME/.emela/bin`; set
`EMELA_INSTALL_DIR` to change the directory, or `EMELA_VERSION` (for example
`v0.20.0-alpha.5`) to pick a version.

## A tour

A project is a directory with a `Pome.toml` (empty is fine) and its sources under
`src/`, with the entry point in `src/main.emel`. Each module is one file:
`src/email.emel` is imported as `Email`. This is
[`examples/tour`](examples/tour):

```
tour/
├── Pome.toml
└── src/
    ├── main.emel
    └── email.emel
```

`src/main.emel`:

```emela
# A tour of the pure part of the language. `dbg` prints to stderr.
import Email

# Variants carry labelled fields.
enum Shape {
  Circle(radius: Float)
  Rect(width: Float, height: Float)
}

# A record type.
type Todo(id: Int, title: String, done: Bool)

fn area(shape: Shape) -> Float {
  shape match {
    Circle(radius:)       -> 3.14 * radius * radius
    Rect(width:, height:) -> width * height
  }
}

# Self tail calls compile to loops, so deep recursion is fine.
fn sum(xs: List[Int], acc: Int) -> Int {
  xs match {
    []          -> acc
    [x, ..rest] -> sum(rest, acc + x)
  }
}

fn fizzbuzz(n: Int) -> String {
  (n % 3, n % 5) match {
    (0, 0) -> "FizzBuzz"
    (0, _) -> "Fizz"
    (_, 0) -> "Buzz"
    _      -> Int.to_string(n)
  }
}

fn main() {
  todo = Todo(id: 1, title: "Write docs", done: False)
  finished = Todo(done: True, ..todo)
  dbg(finished) # Todo(id: 1, title: "Write docs", done: True)

  dbg(area(Rect(width: 2.0, height: 3.0))) # 6.0

  dbg(sum(List.range(1, 10001), 0)) # 50005000

  # "1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz"
  List.range(1, 16)
    |> List.map(fizzbuzz)
    |> String.join(" ")
    |> dbg

  dbg(Email.parse("alice@example.com") |> Option.map(Email.to_string)) # Some("alice@example.com")
  dbg(Email.parse("not an email")) # None
}

# Run with `emela test`.
@test
fn fizzbuzz_of_15() {
  assert fizzbuzz(15) == "FizzBuzz"
}
```

`src/email.emel`:

```emela
# `opaque` hides the constructor and fields outside this module,
# so every Email goes through `parse`.
pub opaque type Email(value: String)

pub fn parse(input: String) -> Option[Email] {
  if String.contains(input, "@") { Some(Email(value: input)) } else { None }
}

pub fn to_string(email: Email) -> String {
  email.value
}
```

```console
$ emela run
Todo(id: 1, title: "Write docs", done: True)
6.0
50005000
"1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz"
Some("alice@example.com")
None
$ emela test
   ✓ fizzbuzz_of_15  0ms

 Tests  1 passed (1)
 Duration  48ms
```

## Commands

Each command takes a project directory or an entry file, and defaults to `.`.
`run`, `build` and `test` take `--target js` (the default), `--target c`, which
compiles to C and builds a native executable with `cc`, or `--target wasm`, which
writes a WASI 0.3 component `main.wasm` (`target/emela/wasm`) and runs it with `wasmtime`;
`[build] target = "..."` in `Pome.toml` changes the default. The C target does
not run suspending code, fibers or `String.length` yet; the WASM target does not
run suspending code, fibers, `Float` operations or host callbacks either.

| Command       | Does                                                    |
| ------------- | ------------------------------------------------------- |
| `emela new`   | Creates a new Pome in a new directory; `--lib` creates a library |
| `emela check` | Reports diagnostics without compiling                   |
| `emela run`   | Compiles and runs: JavaScript on Node.js, a native executable, or WASM with `wasmtime` |
| `emela build` | Compiles under `target/emela/js` (JS), a native executable (C), or `main.wasm` (WASM) |
| `emela test`  | Runs the functions marked `@test`; `--reporter pretty\|plain\|agent` picks the output |
| `emela fmt`   | Formats the sources in place; `--check` only lists them |
| `emela lsp`   | Starts the language server over stdio                   |

The language server reports diagnostics and offers hover (types, `fails`, `use`),
inlay hints, completion, go to definition, references, rename, signature help,
formatting, and a code action that writes inferred annotations.

Programs print with `dbg`, which writes to stderr. The value `main` returns is
not printed.

Side effects go through effects. The bundled `Console` effect writes lines, and
`SystemConsole` provides it on the host. Both live in `Std.Console` and are
imported. This is [`examples/hello`](examples/hello):

```emela
import Std.Console.{Console, SystemConsole}

fn greet(name: String) {
  console = use Console
  console.print_line("Hello, #{name}!")
}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line("hi")
    List.map(["Emela", "world"], greet)
    console.print_error_line("done")
  }
}
```

```console
$ emela run
hi
Hello, Emela!
Hello, world!
done
```

Errors are values declared with `error`. A function's errors (`fails`) and
effects (`use`) are part of its type and inferred unless written. `escape`
handles some errors and lets the rest propagate; an error that reaches `main`
exits with status 1. This is [`examples/errors`](examples/errors):

```emela
# Typed errors and effects. `fails` and `use` are inferred unless written.

import Std.Console.{Console, SystemConsole}

error NotFound(id: Int)
error Invalid(reason: String)

type User(id: Int, name: String)

# An effect declares operations; handlers implement them.
effect Users {
  fn find(id: Int) -> User fails NotFound
}

handler SampleUsers: Users {
  fn find(id) {
    id match {
      1 -> User(id: 1, name: "alice")
      2 -> User(id: 2, name: "bob")
      _ -> fail NotFound(id:)
    }
  }
}

# Inferred: fails NotFound | Invalid, use Users.
fn greeting(id: Int) {
  if id < 0 { fail Invalid(reason: "negative id") }
  user = (use Users).find(id)
  "Hello, #{user.name}!"
}

# `escape` handles NotFound; Invalid still propagates.
fn greeting_or_guest(id: Int) {
  greeting(id) escape {
    NotFound(_) -> "Hello, guest!"
  }
}

fn main() {
  with SystemConsole, SampleUsers {
    console = use Console
    console.print_line(greeting_or_guest(1))
    console.print_line(greeting_or_guest(3))
    console.print_line(greeting_or_guest(-1))
  }
}

# Tests swap the handler instead of mocking.
handler NoUsers: Users {
  fn find(id) { fail NotFound(id:) }
}

@test
fn unknown_users_get_the_guest_greeting() {
  with NoUsers {
    assert greeting_or_guest(1) == "Hello, guest!"
  }
}
```

```console
$ emela run
Hello, alice!
Hello, guest!
error: Invalid(reason: "negative id")
$ emela test
running 1 test
test unknown_users_get_the_guest_greeting ... ok

test result: ok. 1 passed; 0 failed; 0 filtered out
```

A `suspend fn` operation may pause its caller; the handler decides when to
resume. Callers write ordinary calls, so the same `countdown` runs on a real
clock in `main` and on an instant one in the test. This is
[`examples/clock`](examples/clock):

```emela
# Suspending effects. `sleep` is a `suspend fn`: calls to it look like any other
# call, and the handler decides when the caller resumes.

import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}

fn countdown(n: Int) {
  console = use Console
  clock = use Clock
  List.map(List.range(0, n), fn(i) {
    clock.sleep(100)
    console.print_line("#{n - i}")
  })
  console.print_line("liftoff")
}

fn main() {
  with SystemConsole, SystemClock {
    countdown(3)
  }
}

# Resumes on the spot, so tests do not wait and need no `Async`.
handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { Instant.from_unix_milliseconds(0) }
}

@test
fn countdown_does_not_wait() {
  with SystemConsole, InstantClock {
    countdown(3)
  }
}
```

```console
$ emela run
3
2
1
liftoff
```

The bundled std lives under `Std` and is imported: the platform effects
`Std.Console`, `Std.Clock`, `Std.Random`, `Std.Env`, `Std.FileSystem` and
`Std.Http`, each with its host handler (`SystemConsole`, `SystemClock`,
`SystemRandom`, `SystemEnv`, `SystemFileSystem`, `SystemHttp`), the layer
`Std.Platform.Platform` that bundles them, `Std.State`, `Std.Fiber`,
`Std.Cause`, `Std.Deferred`, `Std.Path`, `Std.Instant` and `Std.Duration`. The
test handlers `TestConsole`, `TestClock`, `TestFileSystem` and `TestHttp` keep
their state in `State`. The core modules (`List`, `Option`, `String`, `Int`, `Int64`,
`Float`) need no import.

Handlers can hold state: `init` runs when a `with` is entered and builds the
handler's value, and `release` runs when the `with` is left, however its body
ends. A `layer` names a bundle of handlers, initialized in the order their
dependencies need, and a handler listed later replaces the layer's handler for
the same effect. This is [`examples/resources`](examples/resources):

```console
$ emela run
open store v1
v1/alice
v1/bob
close store v1
```

Mutable state is the `State` effect: `State.run(initial, body)` returns the
body's value and the final state, so a test can read what `TestConsole`
recorded.

```emela
(_, lines) = State.run([], fn() {
  with TestConsole {
    greet("a")
    greet("b")
  }
})
# lines == ["hello, a", "hello, b"]
```

Fibers run concurrently on one thread and switch only where a call may
suspend. They start only inside `Fiber.scope`, and every fiber ends before its
scope does; a failing fiber interrupts its siblings, and the error reaches the
scope. `Fiber.both`, `Fiber.map`, `Fiber.all`, `Fiber.race` and `Fiber.timeout`
are built on the scope, so `Fiber.scope` and `Fiber.fork` are needed only for
the shapes they do not cover. This is [`examples/fibers`](examples/fibers),
where `fetch(id, ms)` sleeps for `ms` and prints `fetched <id>`:

```emela
fn main() {
  with SystemConsole, SystemClock {
    console = use Console
    # The waits overlap: the second fetch ends first.
    (a, b) = Fiber.both(fn() { fetch(1, 200) }, fn() { fetch(2, 100) })
    console.print_line("both: #{a}, #{b}")
    names = Fiber.map([3, 4, 5], fn(id) { fetch(id, 100 * (6 - id)) })
    console.print_line("map: #{names}")
    first = Fiber.race([fn() { fetch(6, 300) }, fn() { fetch(7, 100) }])
    console.print_line("race: #{first}")
    late = Fiber.timeout(100, fn() { fetch(8, 1000) })
    console.print_line("timeout: #{late}")
    # A failing child interrupts its siblings, and the error reaches the scope.
    failed = Fiber.scope(fn() {
      ok = Fiber.fork(fn() { fetch(9, 1000) })
      bad = Fiber.fork(fn() { fetch(-1, 100) })
      "#{Fiber.join(ok)} #{Fiber.join(bad)}"
    }) escape {
      NotFound(id:) -> "not found: #{id}"
    }
    console.print_line(failed)
  }
}
```

```console
$ emela run
fetched 2
fetched 1
both: user 1, user 2
fetched 5
fetched 4
fetched 3
map: ["user 3", "user 4", "user 5"]
fetched 7
race: user 7
timeout: None
fetched -1
not found: -1
```

`FileSystem` reads and writes text, and each operation fails only with the
errors it lists, so `escape` handles exactly those. `Std.Path` builds path
strings. From [`examples/notes`](examples/notes):

```emela
## Appends `text` as a line to `notes/<day>.txt` and returns the number of lines in it.
fn note(day: String, text: String) {
  fs = use FileSystem
  fs.create_directory(DIR)
  path = Path.join(DIR, "#{day}.txt")
  fs.append_text(path, "#{text}\n")
  String.split(fs.read_text(path), "\n") |> List.filter(fn(l) { l != "" }) |> List.length
}

## The text of `notes/<day>.txt`, or a message if there is none.
fn show(day: String) {
  (use FileSystem).read_text(Path.join(DIR, "#{day}.txt")) escape {
    FileNotFound(path:) -> "no notes in #{path}"
  }
}
```

`Http` is a client with one operation, `send`; requests and responses are
values, and a 4xx or 5xx status is a response, not a failure. `TestHttp`
answers with a function, so tests need no network. From
[`examples/http`](examples/http):

```emela
fn search_request(query: String, page: Int) -> Request {
  query_string = Request.encode_query([("q", query), ("page", Int.to_string(page))])
  Request.new(Get, "#{BASE}/search?#{query_string}") |> Request.add_header("accept", "text/plain")
}

fn first_result(query: String) {
  first_line(query) escape {
    UnexpectedStatus(status:, body: _) -> "status #{status}"
    ConnectionFailed(url: _, message:) -> "unreachable: #{message}"
  }
}

@test
fn a_404_is_not_a_failure() {
  server = TestServer(respond: fn(req, n) { Some(Response.new(404, "")) }, sent: [])
  (text, after) = State.run(server, fn() { with TestHttp { first_result("x") } })
  assert text == "status 404"
  assert List.length(after.sent) == 1
}
```

With `with Platform { ... }`, the same code sends real requests through
`fetch`. For a deadline, wrap the call in `Fiber.timeout`; interrupting a fiber
that waits on `send` aborts the request.

An HTTP service is an fn with `@entry(Http.serve)`: the host calls it once for
each request, and it is still an ordinary fn that tests call directly. From
[`examples/http-service`](examples/http-service):

```emela
@entry(Http.serve)
pub fn app(request: Request) -> Response fails NotANumber {
  with Platform {
    (use Console).print_line("#{request.method} #{Request.path(request)}")
    route(request)
  }
}
```

`emela run` serves it on `127.0.0.1:8080` (`--addr <host>:<port>`, or `HOST`
and `PORT`). The same `main.mjs` is the `fetch` of its default export, so
`deno serve`, Bun and Cloudflare Workers run it as it is. An error or a defect
becomes a 500 whose cause goes only to standard error.

## Known limitations

- No binary data: there is no `Bytes`, so `FileSystem` and `Http` handle UTF-8
  text only.
- No JSON yet; it comes in 0.21 with traits.
- The JavaScript target assumes a server host: Node.js 18 or later, Deno, or
  Bun. Browsers are not supported in 0.20.
- No limit on how many fibers run at once (`Semaphore`), no `Queue` and no
  `Http` server yet; they come in 0.21.
- Some checks that scopes, fiber handles, `Deferred` values and handler
  capabilities do not escape run at run time in 0.20 (a defect such as
  `fiber scope was used after it ended`). 0.22 makes them all static.

## Crates

| Crate                | Role                                                                     |
| -------------------- | ------------------------------------------------------------------------ |
| `emela-syntax`       | Lexer, parser, lossless syntax tree, error recovery                      |
| `emela-fmt`          | Formatter                                                                |
| `emela-resolve`      | Module loading, imports, name resolution, `pub` / `opaque`, naming rules |
| `emela-types`        | Type inference, `fails` / `use` inference, traits, exhaustiveness        |
| `emela-core`         | Core IR and lowering: `derive`, tail-call loops, `layer` wiring          |
| `emela-lir`          | Low IR below the Core IR: the Flow IR (structured ANF) and the conversion from monomorphized Core, later the Heap IR |
| `emela-codegen-js`   | JavaScript output and the embedded JS runtime                            |
| `emela-codegen-wasm` | WebAssembly output and the embedded WASM runtime                         |
| `emela-driver`       | Pipeline, diagnostics, `Pome.toml`                                       |
| `emela-lsp`          | Language server                                                          |
| `emela-cli`          | The `emela` binary                                                       |

```
syntax → resolve → types → core ──────→ codegen-js ───┐
   ↓                         └─→ lir → codegen-wasm ──┤
  fmt                                                 ↓
                                        driver → lsp → cli
```

## Building

```sh
cargo build
cargo test
```
