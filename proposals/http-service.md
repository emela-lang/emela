---
title: HTTP services
authors: [Alliana]
status: accepted
pr: 428
issue:
decided: 2026-10-25
touches: [lib.layers, api.std.http, target.js.module, target.wasm.output, target.wasm.run, target.c.unsupported, cmd.build, diag.list]
requires: [export]
related: []
---

# HTTP services

## Summary

Add `serve`, an entry adapter for HTTP services, to `Std.Http`. When an fn of the entry module has `@entry(Http.serve)`, the host calls that fn for every request it receives. On JS this is the `fetch` of the default export of `main.mjs`; on WASM it is `handle` of `wasi:http/handler@0.3.0`. `emela run` starts a server as it is.

## Motivation

- `Std.Http` has only a client; a server is "built at a later stage" (`[lib.layers]`).
- Most HTTP server hosts call the program once per request. Deno (`deno serve`), Bun and Cloudflare Workers call the `fetch` of a default export (Fetch API), and WASI 0.3 calls `handle` of a component of the world `wasi:http/service`. The host owns the port it listens on.
- Passing the app to an effect operation or to a callback of an external function is not possible (`[effect.decl]`, `[type.effect-bound]`). The host calling an fn of the entry (the proposal export) is not limited by this.
- The conversion (between the host's request and `Std.Http.Request`, and the 405, 400 and 500 responses) is kept inside the entry adapter in std. The person writing the app sees neither the host's names nor boundary types.

## Proposal

### Using it

```emel
import Std.Http.{Request, Response}
import Std.Platform.{Platform}

@entry(Http.serve)
pub fn app(request: Request) -> Response {
  with Platform { Routes.route(request) }
}
```

- The type of `app` is `fn(Request) -> Response fails E use R`. R is empty apart from Async. E can be anything.
- `app` is an ordinary fn, so tests can call it directly.

### The entry adapter of `Std.Http`

```emel
## The request as the host handed it over (JS: a Fetch API Request).
@external
pub type Incoming

## The response for the host to send (JS: a Fetch API Response).
@external
pub type Outgoing

## The entry of an HTTP service. Each call runs one launch of `app`.
@export(js, "default", "fetch")
@export(wasm, "wasi:http/handler@0.3.0", "handle")
pub fn serve[E](
  app: fn(Request) -> Response fails E use { Async },
  incoming: Incoming,
) -> Outgoing use { Async } {
  host = with SystemHostHttp { (use HostHttp).read(incoming) }
  if !host_is_text(host) { status_only(400) } else {
    to_request(host) match {
      None -> status_only(405)
      Some(request) -> Fiber.attempt(fn() { app(request) }) match {
        Success(res) -> to_outgoing(res)
        Failure(cause) -> {
          with SystemConsole { (use Console).print_error_line(Cause.to_string(cause)) }
          status_only(500)
        }
      }
    }
  }
}
```

- The external function that reads the body suspends, so it can only be called inside a handler (`[ext.call]`). It goes through a private effect `HostHttp` and its handler `SystemHostHttp`.
- Headers cannot cross the boundary, so they are read one by one by count and index (`headerCount`, `headerName`, `headerValue`).
- On WASM, the runtime has what corresponds to these external functions (the resources of `wasi:http/types` and `stream<u8>`). This is the same privilege by which the handlers of std reach the host on WASM (`[target.wasm.external]`).

### Modules

Following the convention of std (the type `List` and `List.map`, the type `Map` and `Map.get`), Request and Response are each put in a module of the same name. A server both builds and reads both, so the names match: `Request.header` and `Response.header`, `Request.add_header` and `Response.add_header`.

| Module | File | Contents |
|---|---|---|
| `Std.Http` | `std/http.emel` | effect `Http`, `SystemHttp`, `TestHttp`, `TestServer`, errors (`InvalidUrl`, `ConnectionFailed`, `ResponseNotText`), `get`, `post`, the entry adapter `serve`, `Incoming`, `Outgoing` |
| `Std.Http.Request` | `std/http/request.emel` | type `Request`, enum `Method`, `new`, `add_header`, `header`, `headers`, `path`, `segments`, `query`, `encode_query`, `decode_query` |
| `Std.Http.Response` | `std/http/response.emel` | type `Response`, error `UnexpectedStatus`, `new`, `add_header`, `header`, `headers`, `require_ok` |

- `Method` is in `Std.Http.Request`. `Request` has a `Method` and `Std.Http` uses `Request`, so leaving `Method` in `Std.Http` would make the modules depend on each other in a cycle (`[module.name]`).
- `Std.Http.Request` and `Std.Http.Response` do not depend on each other. `Std.Http` imports both.

```emel
# Std.Http.Request
pub enum Method { Get, Head, Post, Put, Patch, Delete, Options }
pub type Request(method: Method, url: String, headers: List[(String, String)], body: String)

pub fn new(method: Method, url: String) -> Request                      # formerly Http.request
pub fn add_header(req: Request, name: String, value: String) -> Request # formerly Http.add_header
pub fn header(req: Request, name: String) -> Option[String]
pub fn headers(req: Request, name: String) -> List[String]
pub fn path(req: Request) -> String           # "/api/versions/a%20b": without the query, not decoded
pub fn segments(req: Request) -> List[String] # ["api", "versions", "a b"]: without empty segments, decoded
pub fn query(req: Request) -> List[(String, String)]
pub fn encode_query(pairs: List[(String, String)]) -> String            # formerly Http.encode_query
pub fn decode_query(s: String) -> List[(String, String)] fails ParseError

# Std.Http.Response
pub type Response(status: Int, headers: List[(String, String)], body: String)
pub error UnexpectedStatus(status: Int, body: String)

pub fn new(status: Int, body: String) -> Response                         # formerly Http.response
pub fn add_header(res: Response, name: String, value: String) -> Response
pub fn header(res: Response, name: String) -> Option[String]              # formerly Http.header
pub fn headers(res: Response, name: String) -> List[String]               # formerly Http.headers
pub fn require_ok(res: Response) -> Response fails UnexpectedStatus       # formerly Http.require_ok
```

- `path` and `segments` are made from `url`. `segments` percent-decodes each segment.
- A request whose path or query has a malformed `%` (`%` not followed by two hexadecimal digits, or decoded bytes that are not UTF-8) is answered with 400 at the entry, and `app` is not called (see "Conversion" below). So `segments` and `query` do not fail. If a Request built by hand (in a test or for the client) has a malformed `%`, that segment or pair is kept as written, without decoding.
- `decode_query` is the inverse of `encode_query`. It splits at `&`, splits each pair at the first `=`, replaces each `+` with a space, and then percent-decodes. HTML forms send a space as `+`. `encode_query` writes `+` as `%2B`, so reading back the output of `encode_query` does not change the values. A pair without `=` has an empty value. A malformed `%`, or a result that is not UTF-8, fails with ParseError (`[lib.api-rules]`: malformed input fails).
- `query` reads the part of `url` after `?` by the rules of `decode_query`. It is empty if there is no `?`.
- `header` and `headers` compare names case-insensitively within ASCII. `add_header` appends at the end.
- The moved names (`Http.request`, `Http.add_header`, `Http.header`, `Http.headers`, `Http.encode_query`, `Http.require_ok`, `Http.response`) remain in `Std.Http` with `@deprecated` for one minor version. Types, enums and errors must be imported from their new modules.

With this, the pkgs sample is written as follows.

```emel
import Std.Http.Request.{Request, Get}
import Std.Http.Response.{Response}

(request.method, Request.segments(request)) match {
  (Get, ["api", "versions", ..path]) -> versions(String.join(path, "/"))
  ...
}

Response.new(200, body) |> Response.add_header("content-type", "application/json")
```

### Conversion

Requests:

- If the method is not one of `Method`, return 405 without calling `app`. The `allow` header lists every member of `Method` (RFC 9110).
- `url` is the absolute URL `scheme://authority/path-with-query`. On WASM, when there is no scheme, it is `http`.
- Header names are lowercased and sorted by name, and values with the same name are joined with `, ` (only `set-cookie` stays as separate pairs, in the order received). This is the form of the client's Response. The Headers of the JS Fetch API can only be read in this form, so WASM uses it too.
- The body is read to the end before `app` is called. If it cannot be read as UTF-8, return 400 without calling `app`. A BOM at the start is removed.
- If the path or query has a malformed `%`, return 400 without calling `app`.

Responses:

- Headers are sent in the order written. When there is a body and no content-type, `text/plain;charset=UTF-8` is added.
- When `app` ends with an error, a defect or an interruption, the cause is written as one line to standard error, and 500 is returned.
- The 405, 400 and 500 returned by the entry have the status name as the body (`Method Not Allowed`, `Bad Request`, `Internal Server Error`) with `content-type: text/plain;charset=UTF-8`. The cause is not written in the body, so that upstream URLs and the contents of errors do not leak out; the cause is in standard error.

Launches:

- One call is one launch. The init and release of a `with` inside `app` run for each request. Emela has no way to keep a value across launches, so even if the host reuses an instance or module, launches cannot observe each other.
- The host MAY call again before the previous call has finished. Launches in progress switch only at suspension points (`[fiber]`); they do not run in parallel on threads. By default, `wasmtime serve` sends up to 16 concurrent requests to a p3 component.
- A heavy init runs for each request. A form for long-running processes, where init runs once per process, is left to a separate proposal, http-serve.

### Targets

- JS: the `fetch` of the default export of `main.mjs`. Deno (`deno serve main.mjs`), Bun (`bun main.mjs`) and Workers (`main` in `wrangler.toml`) run it as it is. Workers passes further arguments (`env`, `ctx`), which are ignored.
- WASM: with `@entry(Http.serve)`, `main.wasm` is a component of the world `wasi:http/service@0.3.0` and does not export `wasi:cli/run`.
- C: not supported; a compile error.

### `emela build` and `emela run`

- JS: `emela build` and `emela run` also write `serve.mjs`. `serve.mjs` listens with `node:http`, turns each request into a Fetch API Request, and passes it to `fetch`. The address it listens on is decided in this order: `--addr <host>:<port>`, the environment variables `HOST` and `PORT`, then `127.0.0.1:8080`. Either of `HOST` and `PORT` may be set alone; the missing one is `127.0.0.1` or `8080`. On a PaaS that passes only `PORT`, set `HOST=0.0.0.0` (by default the server is not open to the outside). `emela run` runs `serve.mjs` with node.
- WASM: `emela run` runs `wasmtime serve -S p3=y,cli=y main.wasm`. The arguments after `--` are passed before `main.wasm`.
- Without `main`, `emela run` does not report E0230 when there is `@entry(Http.serve)`.

## Specification changes

### `[api.std.http]`

Split into three sections, one per module. The types, functions, and their descriptions are unchanged except as listed.

> ### Std.Http (`std/http.emel`, `std/http.mjs`) `[api.std.http]`
>
> The client effect, its errors, and the entry adapter for HTTP services. Requests and responses are in `[api.std.http.request]` and `[api.std.http.response]`.
>
> Keeps `InvalidUrl`, `ConnectionFailed`, `ResponseNotText`, `Http`, `get`, `post`, `SystemHttp`, `TestServer`, `TestHttp`, and adds:
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Incoming` | `@external type Incoming` | The request as the host handed it over. Used only when writing an export by hand |
> | `Outgoing` | `@external type Outgoing` | The response for the host to send |
> | `serve` | `serve[E](app: fn(Request) -> Response fails E use { Async }, incoming: Incoming) -> Outgoing use { Async }` | An entry adapter (`[ext.entry]`). `@entry(Http.serve)` makes the fn the entry of an HTTP service (`[lib.http-service]`) |
>
> `request`, `add_header`, `header`, `headers`, `encode_query`, `require_ok`, and `response` remain for one minor version with `@deprecated`, pointing to their new places.
>
> ### Std.Http.Request (`std/http/request.emel`) `[api.std.http.request]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Method` | `enum Method { Get, Head, Post, Put, Patch, Delete, Options }` | Moved from `Std.Http` |
> | `Request` | `type Request(method: Method, url: String, headers: List[(String, String)], body: String)` | Moved from `Std.Http` |
> | `new` | `new(method: Method, url: String) -> Request` | A request with no headers and no body. Formerly `Http.request` |
> | `add_header` | `add_header(req: Request, name: String, value: String) -> Request` | Appends a header at the end |
> | `header` | `header(req: Request, name: String) -> Option[String]` | The first value whose name matches. Case-insensitive (ASCII) |
> | `headers` | `headers(req: Request, name: String) -> List[String]` | All values whose name matches |
> | `path` | `path(req: Request) -> String` | The path of `url`, without the query, as written (not decoded). `/` if empty |
> | `segments` | `segments(req: Request) -> List[String]` | The path split at `/`, without empty segments, each percent-decoded. A segment with a malformed `%` sequence, or whose decoded bytes are not UTF-8, is kept as written |
> | `query` | `query(req: Request) -> List[(String, String)]` | The part of `url` after `?`, read as `decode_query` does. Empty if there is no `?`. A pair with a malformed `%` sequence, or whose decoded bytes are not UTF-8, is kept as written |
> | `encode_query` | `encode_query(pairs: List[(String, String)]) -> String` | Moved from `Std.Http` |
> | `decode_query` | `decode_query(s: String) -> List[(String, String)] fails ParseError` | The inverse of `encode_query`. Splits at `&`, splits each pair at the first `=` (a pair without `=` has an empty value), replaces each `+` in the name and the value with a space, and then percent-decodes them. A malformed `%` sequence, or decoded bytes that are not UTF-8, fail with ParseError |
>
> ### Std.Http.Response (`std/http/response.emel`) `[api.std.http.response]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Response` | `type Response(status: Int, headers: List[(String, String)], body: String)` | Moved from `Std.Http` |
> | `UnexpectedStatus` | `error UnexpectedStatus(status: Int, body: String)` | Moved from `Std.Http` |
> | `new` | `new(status: Int, body: String) -> Response` | A response without headers. Formerly `Http.response` |
> | `add_header` | `add_header(res: Response, name: String, value: String) -> Response` | Appends a header at the end |
> | `header` | `header(res: Response, name: String) -> Option[String]` | Moved from `Std.Http` |
> | `headers` | `headers(res: Response, name: String) -> List[String]` | Moved from `Std.Http` |
> | `require_ok` | `require_ok(res: Response) -> Response fails UnexpectedStatus` | Moved from `Std.Http` |

### `[lib.http-service]` (new, after `[lib.layers]`)

> ## 15.3 HTTP services `[lib.http-service]`
>
> ```emel
> @entry(Http.serve)
> pub fn app(request: Request) -> Response {
>   with Platform { route(request) }
> }
> ```
>
> - A program whose entry module has an fn with `@entry(Http.serve)` is an HTTP service. The host calls that fn once for each request it receives, and each call is one launch. Launches share no state: a `with` inside the fn runs its init and release once per request.
> - The host MAY call the fn again before an earlier call has returned. Launches in progress switch only at suspension points (`[fiber.basic]`).
> - The request is converted to a `Request` as follows.
>   - A method that is not one of `Method` is answered with status 405 and the header `allow: GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS`, without calling the fn.
>   - `url` is the absolute URL of the request, `<scheme>://<authority><path-with-query>`.
>   - Header names are lowercased and sorted by name, and values with the same name are joined with `, `. Only `set-cookie` is kept as separate pairs, in the order received. This is the form of the headers of a `Response` of the client.
>   - The whole body is read before the fn is called. A body that cannot be read as UTF-8 is answered with status 400, without calling the fn. A BOM at the start of the body is removed.
>   - A request whose path or query has a malformed `%` sequence (`%` not followed by two hexadecimal digits), or one whose decoded bytes are not UTF-8, is answered with status 400, without calling the fn. So `Request.segments` and `Request.query` of a request received this way keep nothing as written.
> - The `Response` is converted as follows. Headers are sent in the order written. When the body is not empty and there is no `content-type` header, `content-type: text/plain;charset=UTF-8` is added. The body is sent as UTF-8.
> - When the launch ends with an error, a defect or an interruption, one line describing the cause (`Cause.to_string`) is written to standard error, and the response has status 500. The cause is not written in the response.
> - The responses with status 405, 400 and 500 above have the body `Method Not Allowed`, `Bad Request` and `Internal Server Error`, with `content-type: text/plain;charset=UTF-8`.
> - On JS, `serve` is the property `fetch` of the default export of `main.mjs` (`[target.js.module]`). On WASM, `main.wasm` is a component of the world `wasi:http/service@0.3.0` that exports `handle` of `wasi:http/handler@0.3.0` (`[target.wasm.output]`). The C target does not support HTTP services yet (`[target.c.unsupported]`).

### `[lib.layers]`

Replace "Http (`Std.Http`) has only a client (a server will be built on fibers at a later stage)" with "Http (`Std.Http`) has a client, and the entry adapter `serve` for HTTP services (`[lib.http-service]`)".

### `[target.wasm.output]`

Replace "It exports the interface `wasi:cli/run@0.3.0`, whose `run` calls `pub fn main` of the entry module" with:

> - When the program is an HTTP service (`[lib.http-service]`), `main.wasm` is a component of the world `wasi:http/service@0.3.0`. It exports `wasi:http/handler@0.3.0` and does not export `wasi:cli/run@0.3.0`. A request without a scheme has the scheme `http`.
> - Otherwise it exports the interface `wasi:cli/run@0.3.0`, whose `run` calls `pub fn main` of the entry module.

### `[target.wasm.run]`

Add: "When the program is an HTTP service, `emela run` runs `wasmtime serve -S p3=y,cli=y main.wasm`. The arguments written after `--` are passed to wasmtime before `main.wasm`."

### `[target.js.module]`

Add `serve.mjs` to the table of files:

> | `serve.mjs` | Only for an HTTP service. Listens with `node:http` and passes each request to the `fetch` of the default export of `main.mjs`. The address is `--addr <host>:<port>` if given, otherwise the environment variables `HOST` and `PORT`, otherwise `127.0.0.1:8080`. When only one of `HOST` and `PORT` is set, the other is `127.0.0.1` or `8080` |

### `[cmd.build]`

- "If the entry module has no fn named `main`, `run` reports a compile error (E0230)" becomes "If the entry module has no fn named `main` and the program is not an HTTP service (`[lib.http-service]`), `run` reports a compile error (E0230)".
- For `js`, `run` of an HTTP service runs `node serve.mjs` with the arguments after `--`.

### `[target.c.unsupported]`

Add: "HTTP services (`[lib.http-service]`). `@entry(Http.serve)` is a compile error when the target is `c`."

## Diagnostics

- [ ] E0442: `@entry(Http.serve)` on the C target (not supported)
- [ ] Do not report E0230 for an HTTP service

Type mismatches (E0438) and wrong R (E0402) belong to the proposal export.

## Compatibility

`Request`, `Response`, `Method`, `UnexpectedStatus` and the functions on them move to `Std.Http.Request` and `Std.Http.Response`. Imports such as `Std.Http.{Request, Response, Get}` break. The old names of the moved functions remain for one minor version with `@deprecated`, so calls only get a warning. Imports of the types, enums and errors must be fixed.

## Implementation

- std: `Incoming`, `Outgoing`, `serve` and the private `HostHttp` in `http.emel`. Functions such as `readRequest` in `http.mjs`.
- emela-codegen-js: the `fetch` of the default export, and `serve.mjs`.
- emela-codegen-wasm: vendor `wasi:http@0.3.0` into `wit/` and have the `service` world. Export `handle` with an async lift, and push a root continuation onto the scheduler for each launch (using the async lift and waitable sets of `run`). Add to the runtime the request and response resources, reading and writing `stream<u8>`, and completing `future`.
- emela-driver: `run` calls `serve.mjs` or `wasmtime serve`.

## Alternatives

- Listening with `wasi:sockets` on WASM, and writing the accept loop and HTTP parsing in Emela: cannot be written without Bytes. There is no TLS either, and it does not fit platforms built on `wasi:http/service`.
- Opening a `with` inside `main` and receiving with `Http.serve(app)`, so init runs once per process: this does not hold where the host owns the lifetime of a request. Workers cannot use an I/O object created in one request from another request, and cuts off waits left after the response. wasmtime discards instances after a number of requests or an idle time, and has no chance to run release. It is left to the proposal http-serve as a separate form for long-running processes.
- Turning errors and defects into the host's error (`err(internal-error)` on WASM, a rejection on JS): the response would differ between hosts, and the results would not match across targets.

## Prior art

- The world `wasi:http/service` and the interface `handler` of wasi-http 0.3.
- The Request and Response of the Fetch API, and modules whose default export has `fetch`.

## Open questions

## Decision

HTTP services are an entry adapter in std, built on export, so the compiler does not special-case HTTP and the same app runs as the fetch of a default export on JS and as a wasi:http/service component on WASM. Each request is one launch: this is what hosts such as Workers and wasmtime serve allow, and a long-running form with process-wide init is left to http-serve. The entry answers 405, 400 and 500 itself with the status name as the body, and writes the cause only to standard error, so results match across targets and nothing leaks. Request and Response move to modules of their own, following the convention of List and Map, and malformed percent-encoding is rejected at the entry so segments and query need no fails.
