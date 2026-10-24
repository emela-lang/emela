# 15. Scope of the standard library `[lib]`

The Prelude contains only pure things. Features that touch the host are provided in two parts: an effect declaration and handlers for each target. This chapter defines the scope of the standard library, how it is divided into layers, and the API conventions. `[api]` (`library/api.md`) defines the declarations, signatures, and behavior of the individual modules.

## 15.1 Prelude `[lib.prelude]`

The Prelude is visible from every module without an import. It contains only pure things, so a function written with the Prelude alone is pure.

| Category | Contents |
|---|---|
| Types | Int Int64 Float Bool String Never List Option Ordering |
| Constructors | True False Some None Less Equal Greater |
| Traits | Eq Ord Show |
| Functions | panic todo dbg inspect |

Other functions are called qualified by their module name, as in `List.map`.

- `dbg[A](value: A) -> A` writes the debug representation (10.6) of value to standard error as one line and returns value unchanged. It does not require Show. The output is a side effect, but as an exception for debugging it is not listed in R, and the function is treated as pure.
- `inspect[A: Show](value: A) -> String` returns the result of the Show implementation. A String is enclosed in `"` (10.6).
- Bool (`False < True`) and Ordering satisfy Eq, Ord, and Show. Option satisfies whatever `A` satisfies.

## 15.2 Four layers `[lib.layers]`

The standard library is divided into two parts: core, which cannot be separated from the compiler, and std, which can. core is kept as small as possible, and std is made rich.

| Part | Location | Contents |
|---|---|---|
| core | Short names (`List`, `String`). Usable without an import | The Prelude and the modules of the types the compiler knows as syntax (numbers, String, List, Option, Bool; literals, interpolation, `..`, and so on). They correspond one to one with the table of built-in functions, so they work only with the same version of the compiler |
| std | Under `Std` (`Std.Console`). Used through an import | Everything else. Modules that can be written in pure Emela and the platform handlers can be distributed separately from the compiler. The effect layer that has `@intrinsic` (State, Fiber) works only with the same version of the runtime |

Within std, modules are divided into four layers by their nature.

| Layer | Nature | Contents |
|---|---|---|
| value | Pure and independent of the host | Instant, Duration, Path. Collections (Map, Set, Array) in 0.24 |
| effect | Concurrency and state | State, Fiber, Cause, Deferred (0.20). Queue, Semaphore, Schedule, Stream (later versions. A limit on the number of tasks running at the same time comes before Http) |
| platform | Effect declarations and the host handlers | Console, Clock, Random, Env, FileSystem, Http |
| test | Replacements for testing | Handlers such as TestClock and TestConsole. They keep their state in State (8.8), so they are used inside `State.run`. Value comparison helpers, generators, and property testing are not placed in std (below) |

- The helpers for test assertions (comparison helpers that take the contents out of an Option, generators, property testing) are distributed as the Pome `github.com/emela-lang/brix` and used as a test dependency (4.7). The test layer of std has only the replacement handlers, placed in the same module as their effect. Assertion helpers are a matter of style on which people differ, and a running program does not need them, so they grow separately from the compiler version.
- Modules are placed under `Std` so that adding a module to std does not turn a user's file name into a reserved name (4.1). A user can write `http.emel` without conflicting with `Std.Http`.
- A platform handler does not expose target names such as Js or Wasm in user code. One handler lists the `@external` of its external functions for each target (12.1), and the target being built decides which one is used. An external function reachable from the entry that has no annotation for that target is a compile error (E0412).
- A platform handler also lists `@external` for the `c` target. The operations of a suspend fn are also mapped to C in the form of 12.6. A handler that has no C implementation yet (Http) has no `c` annotation, and calling its operations is a compile error (E0412) (as in 12.1, operations that are not called are not checked).
- A system handler that uses the host has a name prefixed with `System` (`SystemConsole`, `SystemClock`, `SystemRandom`, `SystemEnv`, `SystemFileSystem`, `SystemHttp`). A test handler is prefixed with `Test` (`TestClock`, `TestConsole`). Both are placed in the same module as their effect (`import Std.Clock.{Clock, SystemClock, TestClock}`).
- The layer `Platform`, which bundles all System handlers, is placed in `Std.Platform`. main is usually written `with Platform { ... }`, and to replace some of them, the replacements are listed after it, as in `with Platform, TestClock { ... }` (8.3). No bundle of test handlers is provided (TestClock and TestConsole require separate States, so bundling them only nests `State.run` and makes nothing easier).

```emel
import Std.Platform.{Platform}
import Std.Console.{Console}

fn main() {
  with Platform {
    (use Console).print_line("hi")
  }
}
```
- Collections (Map, Set, Array) are placed in 0.24 as `Std.Map`, `Std.Set`, and `Std.Array`, on top of the traits (Eq, Ord, Show). This is because with the trait-less `==` and display of 0.20, comparison and display could vary with how a value was built even when the contents are the same. The form of the implementation and the ordering are in 18.1 #36.
- Collections are immutable.
- A Map (`Std.Map`) maps keys to values, and a Set (`Std.Set`) is a collection of values. Both are ordered by key (the Ord order) and require `K: Ord` for keys. The contents are a balanced tree written in pure Emela, and the types are opaque. to_list, fold, keys, values, and display are in ascending key order, regardless of the order of construction or the shape of the tree.

```emel
pub opaque type Map[K, V]

pub fn empty[K, V]() -> Map[K, V]
pub fn from_list[K: Ord, V](pairs: List[(K, V)]) -> Map[K, V] # for a repeated key, the later value
pub fn insert[K: Ord, V](m: Map[K, V], key: K, value: V) -> Map[K, V] # replaces an existing value
pub fn remove[K: Ord, V](m: Map[K, V], key: K) -> Map[K, V] # unchanged if the key is absent
pub fn update[K: Ord, V](m: Map[K, V], key: K, f: fn(Option[V]) -> Option[V]) -> Map[K, V]
pub fn get[K: Ord, V](m: Map[K, V], key: K) -> Option[V]
pub fn contains[K: Ord, V](m: Map[K, V], key: K) -> Bool
pub fn size[K, V](m: Map[K, V]) -> Int
pub fn is_empty[K, V](m: Map[K, V]) -> Bool
pub fn fold[K, V, B](m: Map[K, V], from: B, f: fn(B, K, V) -> B) -> B
pub fn to_list[K, V](m: Map[K, V]) -> List[(K, V)]
pub fn keys[K, V](m: Map[K, V]) -> List[K]
pub fn values[K, V](m: Map[K, V]) -> List[V]
pub fn map_values[K, V, W](m: Map[K, V], f: fn(V) -> W) -> Map[K, W]
pub fn filter[K, V](m: Map[K, V], f: fn(K, V) -> Bool) -> Map[K, V]
pub fn union[K: Ord, V](a: Map[K, V], b: Map[K, V]) -> Map[K, V] # for a key in both, the value from a

pub opaque type Set[A]

pub fn empty[A]() -> Set[A]
pub fn from_list[A: Ord](xs: List[A]) -> Set[A]
pub fn insert[A: Ord](s: Set[A], x: A) -> Set[A]
pub fn remove[A: Ord](s: Set[A], x: A) -> Set[A]
pub fn contains[A: Ord](s: Set[A], x: A) -> Bool
pub fn size[A](s: Set[A]) -> Int
pub fn is_empty[A](s: Set[A]) -> Bool
pub fn fold[A, B](s: Set[A], from: B, f: fn(B, A) -> B) -> B
pub fn to_list[A](s: Set[A]) -> List[A]
pub fn filter[A](s: Set[A], f: fn(A) -> Bool) -> Set[A]
pub fn union[A: Ord](a: Set[A], b: Set[A]) -> Set[A]
```

  - update passes the current value of the key (None if absent) to f. If f returns Some, update replaces or adds the value; if None, it removes the key.
  - union takes the value from the first argument for a key present in both.
  - Comparison and display are by contents. Even if the tree shapes differ, two maps are equal when their sequences of key-value pairs are the same (`impl Map[K: Eq, V: Eq]: Eq`, `impl Set[A: Eq]: Eq`). They do not have Ord.
  - Display (Show) uses a source form that can rebuild the value, written `Map.from_list([(1, "a"), (2, "b")])` and `Set.from_list([1, 2])` (`impl Map[K: Show, V: Show]: Show`, `impl Set[A: Show]: Show`). Values inside enclose a String in `"`, as in 10.6. inspect also uses this Show, as in 10.6.
  - With n the number of elements, the Map and Set operations get, insert, remove, update, and contains are O(log n), from_list and union are O(n log n) or less, and the others are O(n). The depth of the tree is kept at O(log n), so even a large Map does not use deep stack.
- A point in time is represented by Instant, and a length of time by Duration. Both are opaque types whose contents are an Int64. An Instant is milliseconds since the Unix epoch (1970-01-01T00:00:00Z), and a Duration is a length in milliseconds that can also be negative. Leap seconds are not counted. An Instant can be created with `Instant.from_unix_milliseconds(ms: Int64)` (to fix the time in a test, or when a user writes a Clock handler). Instant has no invariant to maintain. It is opaque to prevent confusing it with Int64 or Duration and to hide the unit.
- Console has two operations, `print_line(text: String) -> ()` (standard output) and `print_error_line(text: String) -> ()` (standard error). Both write with a trailing newline and do not fail. The system handler is `SystemConsole`.
- Random has two operations, `int() -> Int` (uniform over the whole range of Int) and `float() -> Float` (uniform over 0 inclusive to 1 exclusive), and they do not fail. The system handler SystemRandom draws from a cryptographically secure random source on JS (`crypto.getRandomValues`). Values in a range are provided separately as ordinary functions written on top of `int()`, and seedable pseudo-random numbers as pure values and functions.
- Env has only the operation `get(name: String) -> Option[String]`, and it does not fail. An empty value (`NAME=`) is `Some("")`, and an unset variable is `None`.
- FileSystem (`Std.FileSystem`) handles only text (UTF-8). There is no Bytes, so binary data is left to a later version. All operations are suspend fn (a synchronous API would stop every fiber while reading).

```emel
pub error FileNotFound(path: String)
pub error FileAccessDenied(path: String)
pub error NotADirectory(path: String)
pub error IsADirectory(path: String)
pub error DirectoryNotEmpty(path: String)
pub error InvalidText(path: String)
pub error FileSystemFailure(path: String, message: String)

pub effect FileSystem {
  suspend fn read_text(path: String) -> String
    fails FileNotFound | FileAccessDenied | IsADirectory | InvalidText | FileSystemFailure
  suspend fn write_text(path: String, text: String) -> ()
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  suspend fn append_text(path: String, text: String) -> ()
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  suspend fn exists(path: String) -> Bool
  suspend fn is_directory(path: String) -> Bool
  suspend fn list_directory(path: String) -> List[String]
    fails FileNotFound | FileAccessDenied | NotADirectory | FileSystemFailure
  suspend fn create_directory(path: String) -> ()
    fails FileAccessDenied | NotADirectory | FileSystemFailure
  suspend fn remove(path: String) -> ()
    fails FileNotFound | FileAccessDenied | DirectoryNotEmpty | FileSystemFailure
}
```

  - write_text creates or overwrites, and append_text creates the file if it does not exist. If the parent directory does not exist, the result is FileNotFound. create_directory also creates the parents, and does nothing if the directory already exists. If a file is in the way, the result is NotADirectory. remove deletes a file or an empty directory. There is no recursive deletion.
  - exists and is_directory do not fail. They return False also when the path is not visible because of permissions. list_directory returns only the names, sorted by name.
  - Errors are split by kind, and each operation lists in fails only the ones it can raise. This is so that a tag received by escape is removed from E (if one error had a field for the kind, an arm would receive only some of its values and the error would remain in E; 7.3). The names avoid conflicts with a user's `NotFound` and similar. Any other host error is collected into FileSystemFailure.
  - A file that cannot be read as UTF-8 fails with InvalidText instead of having invalid sequences replaced with the replacement character. This keeps the contents intact when a file is read and written back.
  - A relative path is resolved from the current directory of the process. A path is represented as a String, and there is no normalization (resolving `..`), conversion to an absolute path, or expansion of `~`.
  - The system handler SystemFileSystem uses `node:fs/promises` on JS and is bundled in Platform.
  - The test handler TestFileSystem keeps its state in `State[TestFiles]` and resumes immediately. With `pub type TestFiles(files: List[(String, String)], directories: List[String])`, files keeps pairs of a path and its contents in path order. The ancestors of a file are treated as directories. Paths are compared as strings. Errors are raised under the same rules as SystemFileSystem. FileAccessDenied, InvalidText, and FileSystemFailure cannot be raised, so to test them, the user writes a small handler.
  - The R of FileSystem is not Immediate, so it cannot be used inside the callback of an external function (9.5). This is because a JS callback cannot wait. From a callback, signal a fiber with a Deferred (11.7), and handle the files on the fiber's side.
- Http (`Std.Http`) has a client, and the entry adapter `serve` for HTTP services (`[lib.http-service]`). The client has one operation, `send`, and requests and responses are values. Common forms are functions on top of send. A handler author only has to implement send, and the code that builds requests can be tested as ordinary functions. Requests are in the module `Std.Http.Request` and responses in `Std.Http.Response`, each with the functions on them (`[api.std.http.request]`, `[api.std.http.response]`).

```emel
# Std.Http.Request
pub enum Method { Get, Head, Post, Put, Patch, Delete, Options }
pub type Request(method: Method, url: String, headers: List[(String, String)], body: String)

pub fn new(method: Method, url: String) -> Request # no body, no headers
pub fn add_header(req: Request, name: String, value: String) -> Request
pub fn encode_query(pairs: List[(String, String)]) -> String # q=%E6%9D%B1&page=2

# Std.Http.Response
pub type Response(status: Int, headers: List[(String, String)], body: String)
pub error UnexpectedStatus(status: Int, body: String)

pub fn new(status: Int, body: String) -> Response # a response without headers
pub fn header(res: Response, name: String) -> Option[String] # the first value, case-insensitive
pub fn headers(res: Response, name: String) -> List[String] # all values
pub fn require_ok(res: Response) -> Response fails UnexpectedStatus # fails on anything other than 2xx

# Std.Http
pub error InvalidUrl(url: String)
pub error ConnectionFailed(url: String, message: String)
pub error ResponseNotText(url: String)

pub effect Http {
  suspend fn send(request: Request) -> Response fails InvalidUrl | ConnectionFailed | ResponseNotText
}

pub fn get(url: String) -> Response fails InvalidUrl | ConnectionFailed | ResponseNotText use Http
pub fn post(url: String, body: String) -> Response fails InvalidUrl | ConnectionFailed | ResponseNotText use Http
```

  - Bodies are text only. A body is sent as UTF-8 and received by reading it as UTF-8; if it cannot be read, send fails with ResponseNotText (the same approach as InvalidText in FileSystem). A BOM at the start of the body is removed (FileSystem keeps the BOM so that reading and writing back does not change a file). If no content-type is written, it is `text/plain;charset=UTF-8`. The whole body is read before returning (streams are for a later version).
  - Headers are held as `List[(String, String)]`. This is because Map arrives in 0.24, and because several headers with the same name can appear. Response header names are lowercased and sorted by name, and values with the same name are joined with `, ` (only `set-cookie` is kept as separate pairs, one per value). Request headers are sent in the order written.
  - The method is an enum, and a method not in the list cannot be sent.
  - A URL whose scheme is neither `http:` nor `https:` is InvalidUrl (it cannot be sent, so it is distinguished from ConnectionFailed, which means the request did not arrive). An empty body for GET and HEAD is sent as no body. A non-empty body for GET or HEAD, and an invalid header, are ConnectionFailed. header and headers compare names case-insensitively within the ASCII range.
  - 4xx and 5xx are not failures and are returned in status. This is because there are many situations where code needs to branch on 404 or 409. To make them failures, insert require_ok. send fails only when it could not receive a response: name resolution, connection, TLS, and exceeding the redirect limit are all collected into ConnectionFailed. The detailed reason goes in message. Redirects are followed automatically.
  - There is no timeout option. Write it with `Fiber.timeout` (11.3) or race. When a fiber waiting on send is interrupted, the request is aborted (the AbortSignal of 12.2).
  - encode_query converts each name and value to a UTF-8 byte sequence, replaces every byte other than ASCII letters and digits and `-` `_` `.` `!` `~` `*` `'` `(` `)` with `%` and two uppercase hexadecimal digits (percent-encoding), and then joins the `name=value` pairs with `&`. There is no type that splits a URL into parts.
  - The system handler SystemHttp uses JS `fetch` and is bundled in Platform. It does not store cookies and does not use the proxy environment variables.
  - The test handler TestHttp keeps its state in `State[TestServer]` and resumes immediately. With `pub type TestServer(respond: fn(Request, Int) -> Option[Response], sent: List[Request])`, respond is a pure function that takes a request and its count (from 0) and returns a response; None means ConnectionFailed. The requests sent are appended to sent in order. InvalidUrl and ResponseNotText cannot be raised. Slow responses cannot be produced, and tests involving time rely on TestClock (15.2).
- `Std.Path` has pure functions that build path strings: `join(a, b)`, `parent(path)`, `file_name(path)`, and `extension(path)`. Only `/` is handled as a separator. The hosts of the JS target accept `/` as a separator on Windows as well. `parent`, `file_name`, and `extension` return an Option (`Path.parent("a.txt")` is None).
- JSON conversion (Encode, Decode) is provided by the standard library as traits with deriving rules.
- The Prelude and the core modules are `.emel` sources bundled with the compiler. std is bundled too. This is because the central platform and effect layers of std are tied to the host and the runtime, and can be distributed only together with the compiler. std can grow large, but it includes only things that most programs use and that have a single settled form (the host boundary, concurrency and state, basic value types, formats such as JSON, test handlers). The helpers for test assertions are distributed in brix (15.2). Pure Emela components whose form is not yet settled are released first in the Pome `github.com/emela-lang/x` (root name `X`; `X.Random` and so on) and are taken into std once they settle through use. Things on which preferences differ, such as frameworks, are not put in std and become independent Pomes. A function that cannot be written in Emela is declared in that source as an fn without a body, marked with `@intrinsic`, with its signature in Emela syntax. `@external` is not used so that the boundary type restriction of 12.4 (List and similar cannot be passed) does not apply.

## 15.3 HTTP services `[lib.http-service]`

```emel
@entry(Http.serve)
pub fn app(request: Request) -> Response {
  with Platform { route(request) }
}
```

- A program whose entry module has an fn with `@entry(Http.serve)` is an HTTP service. The host calls that fn once for each request it receives, and each call is one launch. Launches share no state: a `with` inside the fn runs its init and release once per request.
- The host MAY call the fn again before an earlier call has returned. Launches in progress switch only at suspension points (`[fiber.basic]`).
- The request is converted to a `Request` as follows.
  - A method that is not one of `Method` is answered with status 405 and the header `allow: GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS`, without calling the fn.
  - `url` is the absolute URL of the request, `<scheme>://<authority><path-with-query>`, as the host parsed it. On JS the host parses it by the URL Standard, so it is normalized there: the scheme and the host are lowercase, `.` and `..` segments are removed from the path, and characters outside ASCII in the path and the query are percent-encoded. On WASM the path and the query are as the host received them, and the scheme and the authority are what the host gives: `http` when it gives no scheme, and empty when it gives no authority (`wasmtime serve` gives neither, so `url` is `http:///<path-with-query>` there). `url`, and `Request.path`, `Request.segments` and `Request.query`, of the same request can therefore differ between the targets.
  - Header names are lowercased and sorted by name, and values with the same name are joined with `, `. Only `set-cookie` is kept as separate pairs, in the order received. This is the form of the headers of a `Response` of the client.
  - The headers `connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `proxy-connection`, `transfer-encoding`, `upgrade`, `host` and `http2-settings` are left out. They belong to the connection, and hosts of `wasi:http` do not hand them over, so they are left out on every target.
  - The whole body is read before the fn is called. A body that cannot be read as UTF-8 is answered with status 400, without calling the fn. A BOM at the start of the body is removed.
  - A request whose path or query has a malformed `%` sequence (`%` not followed by two hexadecimal digits), or one whose decoded bytes are not UTF-8, is answered with status 400, without calling the fn. So `Request.segments` and `Request.query` of a request received this way keep nothing as written.
- The `Response` is converted as follows. Values of headers with the same name are sent in the order written, and MAY be joined with `, ` into one header (not `set-cookie`). The order between headers with different names is not specified. When the body is not empty and there is no `content-type` header, `content-type: text/plain;charset=UTF-8` is added. The body is sent as UTF-8.
- A `Response` that the host cannot send ends the launch like an error: one line `invalid response: <reason>` is written to standard error, and the response has status 500. `<reason>` is one of the following, checked in this order.
  - `status <status>`: the status is not from 200 to 599.
  - `status <status> cannot have a body`: the status is 204, 205 or 304 and the body is not empty.
  - ``header name `<name>` ``: a header name is empty or has a character that is not an ASCII letter, a digit, or one of ``!#$%&'*+-.^_`|~``.
  - ``header value of `<name>` ``: a header value has a character other than a tab and the printable ASCII characters (U+0020 to U+007E). The Headers of the JS Fetch API take only Latin-1, and `wasi:http` rejects control characters, so this is what both accept.
- When the fn ends with an error or a defect, one line describing the cause (`Cause.to_string`) is written to standard error, and the response has status 500. The cause is not written in the response.
- When the launch ends with an interruption (it stalled, `[dyn.suspend]`), no response is made, and the host gets an error: on JS the Promise that `fetch` returned rejects (`[target.js.module]`). `serve.mjs` then writes `interrupted` to standard error and answers with status 500.
- The responses with status 405, 400 and 500 above have the body `Method Not Allowed`, `Bad Request` and `Internal Server Error`, with `content-type: text/plain;charset=UTF-8`.
- On JS, `serve` is the property `fetch` of the default export of `main.mjs` (`[target.js.module]`). On WASM, `main.wasm` is a component of the world `wasi:http/service@0.3.0` that exports `handle` of `wasi:http/handler@0.3.0` (`[target.wasm.output]`). The C target does not support HTTP services yet (`[target.c.unsupported]`).

## 15.4 API conventions `[lib.api-rules]`

Functions in the standard library (core and std) are named according to the following conventions. Conventions are added starting from those decided in 18.1 #34.

- The subject is the first argument, so the function can be written with a pipe, as in `xs |> List.map(f)`.
- An argument that takes a function is named `f` and is passed by position, not by name (`List.map(xs, fn(x) { ... })`). The proposal to use `with` and `where` as argument names was not adopted: both are reserved words (3.1, 3.3) and cannot be used as parameter names, and the case is not strong enough to add syntax that separates the external argument name from the internal name.
- A function that joins two values is `append` (`List.append(xs, ys)`, `String.append(a, b)`), a function that flattens a sequence is `List.flatten(xss)` or `String.concat(parts)`, and a function that joins with a separator in between is `join` (`String.join(parts, sep)`).
- The argument for the initial value of a fold is named `from` (`List.fold(xs, from: 0, f: add)`).
- How a function reports that something did not work depends on the situation.
  - Not found, or out of range: return an Option (`List.get`, `List.find`, `Env.get`).
  - The input is malformed: declare an error and fail (`Int.parse(s) -> Int fails ParseError`). There is no Option version; if one is wanted, convert with escape.
  - A failure in the outside world: list it in the `fails` of an effect operation and fail (8.1).
  - A precondition of the caller is broken: it is a defect.
- Name patterns are as follows. `to_*` is a conversion that does not fail, `from_*` is a creation that does not fail, `parse` is a conversion from a string that can fail, and `is_*` is a test that returns Bool. The default value of an Option is `Option.or(opt, default)`, and chaining is `Option.and_then`.
- A conversion between two types is placed, in both directions together, in the module of the type added later (the dependent one) (`Int64.from_int` and `Int64.to_int`, `Instant.from_unix_milliseconds` and `Instant.to_unix_milliseconds`). This is because a basic type does not know the types added later. However, conversion to String is placed in each type's own module as `to_string`. The same conversion is not placed in both types.
- The error that represents malformed input is the Prelude's `error ParseError(input: String)` (`Int.parse`, `Float.parse`). Conversion between Int and Float is in `Float.from_int`, and between Int and Int64 in `Int64.from_int` and `Int64.to_int`.
