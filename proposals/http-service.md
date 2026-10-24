---
title: HTTP services
authors: [Alliana]
status: draft
pr: 428
issue:
decided:
touches: [lib.layers, api.std.http, target.js.module, target.wasm.output, target.wasm.run, target.c.unsupported, cmd.build, diag.list]
requires: [export]
related: []
---

# HTTP services

## 要約

`Std.Http` に HTTP サービスの入口アダプタ `serve` を足す．エントリのモジュールの fn に `@entry(Http.serve)` を付けると，ホストはリクエストが届くたびにその fn を呼ぶ．JS では `main.mjs` の default export の `fetch`，WASM では `wasi:http/handler@0.3.0` の `handle` になる．`emela run` はそのままサーバーを立てる．

## 動機

- `Std.Http` はクライアントだけで，サーバーは「後の段階で作る」となっている（`[lib.layers]`）
- HTTP サーバーのホストの多くは，リクエストごとにプログラムを呼ぶ．Deno（`deno serve`），Bun，Cloudflare Workers は Fetch API の `fetch` を持つ default export を呼び，WASI 0.3 は `wasi:http/service` の component の `handle` を呼ぶ．listen する port はホストが持つ
- app を effect の操作や外部関数のコールバックに渡す形は取れない（`[effect.decl]`，`[type.effect-bound]`）．ホストがエントリの fn を呼ぶ形（下書き export）なら，この制約に掛からない
- 変換（ホストのリクエストと `Std.Http.Request`，405 / 400 / 500）は std の入口アダプタに閉じ込め，アプリを書く人はホストの名前も境界型も見ない

## 提案

### 使う側

```emel
import Std.Http.{Request, Response}
import Std.Platform.{Platform}

@entry(Http.serve)
pub fn app(request: Request) -> Response {
  with Platform { Routes.route(request) }
}
```

- `app` の型は `fn(Request) -> Response fails E use R`．R は Async を除いて空．E は何でもよい
- `app` は普通の fn なので，テストから直接呼べる

### `Std.Http` の入口アダプタ

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

- 本文を読む外部関数は suspend するので，handler の中でしか呼べない（`[ext.call]`）．そのため非公開の effect `HostHttp` と handler `SystemHostHttp` を通す
- ヘッダーは境界を越えられないので，数と添字で1つずつ読む（`headerCount`，`headerName`，`headerValue`）
- WASM では，この外部関数に当たるものをランタイムが持つ（`wasi:http/types` の resource と `stream<u8>`）．std の handler が WASM でホストに届くのと同じ特権（`[target.wasm.external]`）

### モジュールの分け方

std の慣習（`List` 型と `List.map`，`Map` 型と `Map.get`）に合わせ，Request と Response をそれぞれ同じ名前のモジュールに置く．サーバーでは両方を作って読むので，`Request.header` と `Response.header`，`Request.add_header` と `Response.add_header` のように名前をそろえる．

| モジュール | ファイル | 中身 |
|---|---|---|
| `Std.Http` | `std/http.emel` | effect `Http`，`SystemHttp`，`TestHttp`，`TestServer`，error（`InvalidUrl`，`ConnectionFailed`，`ResponseNotText`），`get`，`post`，入口アダプタ `serve`，`Incoming`，`Outgoing` |
| `Std.Http.Request` | `std/http/request.emel` | 型 `Request`，enum `Method`，`new`，`add_header`，`header`，`headers`，`path`，`segments`，`query`，`encode_query`，`decode_query` |
| `Std.Http.Response` | `std/http/response.emel` | 型 `Response`，error `UnexpectedStatus`，`new`，`add_header`，`header`，`headers`，`require_ok` |

- `Method` は `Std.Http.Request` に置く．`Request` は `Method` を持ち，`Std.Http` は `Request` を使うので，`Method` を `Std.Http` に残すとモジュールが循環する（`[module.name]`）
- `Std.Http.Request` と `Std.Http.Response` は互いに依存しない．`Std.Http` が両方を import する

```emel
# Std.Http.Request
pub enum Method { Get, Head, Post, Put, Patch, Delete, Options }
pub type Request(method: Method, url: String, headers: List[(String, String)], body: String)

pub fn new(method: Method, url: String) -> Request                      # 旧 Http.request
pub fn add_header(req: Request, name: String, value: String) -> Request # 旧 Http.add_header
pub fn header(req: Request, name: String) -> Option[String]
pub fn headers(req: Request, name: String) -> List[String]
pub fn path(req: Request) -> String           # "/api/versions/a%20b"．query を除き，デコードしない
pub fn segments(req: Request) -> List[String] # ["api", "versions", "a b"]．空の segment を除き，デコードする
pub fn query(req: Request) -> List[(String, String)]
pub fn encode_query(pairs: List[(String, String)]) -> String            # 旧 Http.encode_query
pub fn decode_query(s: String) -> List[(String, String)] fails ParseError

# Std.Http.Response
pub type Response(status: Int, headers: List[(String, String)], body: String)
pub error UnexpectedStatus(status: Int, body: String)

pub fn new(status: Int, body: String) -> Response                         # 旧 Http.response
pub fn add_header(res: Response, name: String, value: String) -> Response
pub fn header(res: Response, name: String) -> Option[String]              # 旧 Http.header
pub fn headers(res: Response, name: String) -> List[String]               # 旧 Http.headers
pub fn require_ok(res: Response) -> Response fails UnexpectedStatus       # 旧 Http.require_ok
```

- `path` と `segments` は `url` から作る．`segments` は各 segment をパーセントデコードする
- 壊れた `%`（`%` の後に16進数2桁が続かないもの，デコードした結果が UTF-8 として読めないもの）が path か query にあるリクエストは，入口で 400 にして `app` を呼ばない（下の「変換」）．なので `segments` と `query` は fails を持たない．自分で作った Request（テストやクライアント）に壊れた `%` があれば，その segment や組はデコードせずにそのまま残す
- `decode_query` は `encode_query` の逆．`&` で分け，最初の `=` で名前と値に分け，それぞれの `+` を空白にしてからパーセントデコードする．HTML の form は空白を `+` で送る．`encode_query` は `+` を `%2B` にするので，`encode_query` の出力を読み戻しても値は変わらない．`=` の無い組は値が空．壊れた `%` や UTF-8 として読めない結果は ParseError（`[lib.api-rules]` の「形の誤った入力は fail」）
- `query` は `url` の `?` より後を `decode_query` と同じ規則で読む．`?` が無ければ空
- `header` と `headers` は，名前を ASCII の範囲で大文字と小文字を区別せずに比べる．`add_header` は最後に足す
- 移った名前（`Http.request`，`Http.add_header`，`Http.header`，`Http.headers`，`Http.encode_query`，`Http.require_ok`，`Http.response`）は，`@deprecated` を付けて1つの minor 版だけ `Std.Http` に残す．型と enum と error は移した先から import し直す

pkgs のサンプルは，これで次のように書ける．

```emel
import Std.Http.Request.{Request, Get}
import Std.Http.Response.{Response}

(request.method, Request.segments(request)) match {
  (Get, ["api", "versions", ..path]) -> versions(String.join(path, "/"))
  ...
}

Response.new(200, body) |> Response.add_header("content-type", "application/json")
```

### 変換

リクエスト:

- method が `Method` にないものなら，`app` を呼ばずに 405 を返す．`allow` ヘッダーに `Method` にあるもの全部を書く（RFC 9110）
- `url` は `scheme://authority/path-with-query` の絶対 URL．WASM で scheme が無いときは `http`
- ヘッダーは名前を小文字にし，名前の順に並べ，同じ名前の値を `, ` でつなぐ（`set-cookie` だけは別の組のまま，届いた順）．クライアントの Response と同じ形．JS の Fetch API の Headers はこの形でしか読めないので，WASM もこれにそろえる
- body は最後まで読んでから `app` を呼ぶ．UTF-8 として読めなければ，`app` を呼ばずに 400 を返す．先頭の BOM は取り除く
- path か query に壊れた `%` があれば，`app` を呼ばずに 400 を返す

レスポンス:

- ヘッダーは書いた順に送る．body があって content-type が無ければ `text/plain;charset=UTF-8` を足す
- `app` が error，defect，中断で終わったら，原因を標準エラーに1行書き，500 を返す
- 入口が返す 405，400，500 の body は状態の名前（`Method Not Allowed`，`Bad Request`，`Internal Server Error`）で，`content-type: text/plain;charset=UTF-8` を付ける．原因は body に書かない（上流の URL や error の中身を外に漏らさないため．原因は標準エラーにある）

起動:

- 1回の呼び出しが1回の起動．`app` の中の `with` の init と release はリクエストごとに走る．Emela には起動をまたいで値を持つ手段が無いので，ホストが instance や module を使い回しても，起動どうしは互いを観測できない
- ホストは前の呼び出しが終わる前に次を呼んでよい．同時に進む起動は suspend の点でだけ切り替わる（`[fiber]`）．スレッドで並列に走るのではない．`wasmtime serve` は p3 の component に既定で同時に 16 件まで送る
- 重い init はリクエストごとに走る．init をプロセスで1回にしたい常駐プロセス向けの形は，別の下書き http-serve で扱う

### ターゲットごと

- JS: `main.mjs` の default export の `fetch`．Deno（`deno serve main.mjs`），Bun（`bun main.mjs`），Workers（`wrangler.toml` の `main`）はそのまま動かせる．Workers は2つ目以降の引数（`env`，`ctx`）を渡すが，無視する
- WASM: `@entry(Http.serve)` があれば，`main.wasm` は `wasi:http/service@0.3.0` の component になり，`wasi:cli/run` は export しない
- C: 未対応で compile error

### `emela build` と `emela run`

- JS: `emela build` と `emela run` は `serve.mjs` も書く．`serve.mjs` は `node:http` で listen し，リクエストを Fetch API の Request に直して `fetch` に渡す．listen する address は，`--addr <host>:<port>`，環境変数 `HOST` と `PORT`，`127.0.0.1:8080` の順に決まる．`HOST` と `PORT` は片方だけでもよく，無い方は `127.0.0.1` と `8080`．`PORT` だけを渡す PaaS では `HOST=0.0.0.0` を設定する（既定で外に開かないため）．`emela run` は `serve.mjs` を node で動かす
- WASM: `emela run` は `wasmtime serve -S p3=y,cli=y main.wasm` で動かす．`--` の後の引数は `main.wasm` の前に渡す
- `main` が無くても，`@entry(Http.serve)` があれば `emela run` は E0230 にしない

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

## 診断

- [ ] C のターゲットでの `@entry(Http.serve)`（未対応）
- [ ] E0230 を HTTP サービスでは出さない

型の誤り（E0438）と R の誤り（E0402）は下書き export のもの．

## 互換性

`Request`，`Response`，`Method`，`UnexpectedStatus` と，それらを扱う関数が `Std.Http.Request` と `Std.Http.Response` に移る．`Std.Http.{Request, Response, Get}` のような import は壊れる．移った関数の古い名前は `@deprecated` を付けて1つの minor 版だけ残すので，呼び出しは警告で済む．型と enum と error の import は直す必要がある

## 実装

- std: `http.emel` に `Incoming`，`Outgoing`，`serve`，非公開の `HostHttp`．`http.mjs` に `readRequest` などの関数
- emela-codegen-js: default export の `fetch` と `serve.mjs`
- emela-codegen-wasm: `wit/` に `wasi:http@0.3.0` を vendor し，`service` world を持つ．`handle` を async lift で export し，起動ごとにスケジューラへ根の継続を積む（`run` の async lift と waitable set の仕組みを使う）．request と response の resource，`stream<u8>` の読み書き，`future` の完了を runtime に足す
- emela-driver: `run` が `serve.mjs` か `wasmtime serve` を呼ぶ

## 採らなかった案

- WASM で `wasi:sockets` を listen し，accept のループと HTTP のパースを Emela で書く: Bytes が無いと書けない．TLS も無く，`wasi:http/service` を前提にした基盤に載らない
- `main` の中で `with` を張り，`Http.serve(app)` で受けて init をプロセスで1回にする: リクエストの寿命をホストが握る所では成り立たない．Workers はあるリクエストで作った I/O の object を別のリクエストから使えず，response の後に残る待ちを打ち切る．wasmtime は instance を件数か待機時間で捨て，release を走らせる機会が無い．常駐プロセス向けの別の形として下書き http-serve で扱う
- error と defect をホストの error（WASM の `err(internal-error)`，JS の reject）にする: ホストごとに返る response が変わり，ターゲットの間で結果がそろわない

## 先行する設計

- wasi-http 0.3 の `wasi:http/service` world と `handler` interface
- Fetch API の Request と Response，`fetch` を default export する module の形

## 未決


## 決定
