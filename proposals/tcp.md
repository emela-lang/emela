---
title: Std.Tcp, a TCP client as a platform effect
authors: [Alliana]
status: draft
pr: 563
issue:
decided:
touches: [lib.layers, api.std.tcp (new), target.js, target.c, target.wasm.output, target.wasm.run, target.wasm.external, diag.list]
requires: []
related: [js-host-variable]
---

# Std.Tcp, a TCP client as a platform effect

## Summary

std の platform 層に TCP の client を足す．effect `Tcp` は接続を開き，`Bytes` を読み書きし，閉じる．開いた接続は `upgrade_tls` でその場で TLS に上げられる．system handler `SystemTcp` は JS で `node:net` と `node:tls`，C で POSIX の socket，WASM で `wasi:sockets@0.3.0` を使う．WASM の TLS は，コンパイラに同梱した TLS の component を `main.wasm` に合成して動かす．C の TLS と待ち受け（listen / accept）は入れない．

```emel
import Std.Bytes
import Std.Tcp.{Tcp}

fn ping(host: String) -> Option[Bytes] fails TcpError uses { Tcp } {
  tcp = use Tcp
  conn = tcp.connect(host, 6379)
  tcp.write(conn, Bytes.encode_utf8("PING\r\n"))
  reply = tcp.read(conn, 512)
  tcp.close(conn)
  reply
}
```

## Motivation

- データベース（PostgreSQL，Redis，MySQL）やメッセージブローカーのプロトコルは TCP の上にある．今の Emela はこれらを host の library（`Bun.sql`，`bun:sqlite`）経由でしか使えず，その Pome は 1 つの host でしか動かない
- TCP があれば，プロトコルを純 Emela の Pome として書ける．例えば PostgreSQL のワイヤプロトコルを書いた Pome は，x/sql の `Sql` の handler として JS と C の両方で動く
- `[ext.decl]` はすでに「Host features that most programs use are added to std as platform effects or as value types (Socket, Bytes, and so on)」と書いている．Bytes（`[api.std.bytes]`）は入ったが，Socket はまだない
- 今できる回避策は，Pome が `@external` で `node:net` を呼ぶこと．これは JS でしか動かず，その Pome を使う側は `[external]` で許可する必要がある（`[module.pome]`）

TLS は接続を開いた後に上げる 1 つの操作にする．TLS を使うプロトコルは，始め方が 2 通りある．

- 平文で少しやり取りしてから，同じ接続を TLS に上げる．PostgreSQL（SSLRequest に `S` が返ってから），SMTP と IMAP（STARTTLS）
- 接続した直後から TLS で話す．Redis（`rediss://`），PostgreSQL 17 の `sslnegotiation=direct`，HTTPS

後者は「`connect` の直後に `upgrade_tls`」と書けるので，`connect` に TLS の option を付けなくても両方を 1 つの操作で書ける．

## Proposal

### Meaning

```emel
# Std.Tcp
pub opaque type Connection

pub error HostNotFound(host: String)
pub error ConnectionRefused(host: String, port: Int)
pub error ConnectionClosed
pub error TlsFailure(message: String)
pub error TcpFailure(message: String)

## The certificates trusted as roots.
pub enum Roots {
  SystemRoots
  Pem(text: String)
}

## How much of the peer's certificate is checked.
pub enum Verify {
  Full(roots: Roots)
  ChainOnly(roots: Roots)
  NoVerification
}

pub const VERIFY_FULL = Full(roots: SystemRoots)

pub effect Tcp {
  suspend fn connect(host: String, port: Int) -> Connection
    fails HostNotFound | ConnectionRefused | TcpFailure
  suspend fn read(conn: Connection, max: Int) -> Option[Bytes] fails ConnectionClosed | TcpFailure
  suspend fn write(conn: Connection, data: Bytes) -> () fails ConnectionClosed | TcpFailure
  suspend fn upgrade_tls(conn: Connection, server_name: String, verify: Verify) -> ()
    fails ConnectionClosed | TlsFailure | TcpFailure
  suspend fn close(conn: Connection) -> ()
}

pub fn read_exact(conn: Connection, n: Int) -> Bytes fails ConnectionClosed | TcpFailure uses { Tcp }
```

PostgreSQL の SSLRequest は次のように書ける．

```emel
fn open_postgres(host: String) -> Connection
  fails HostNotFound | ConnectionRefused | ConnectionClosed | TlsFailure | TcpFailure | NoTls
  uses { Tcp } {
  tcp = use Tcp
  conn = tcp.connect(host, 5432)
  tcp.write(conn, Bytes.hex("0000000804d2162f"))
  if Tcp.read_exact(conn, 1) != Bytes.hex("53") { fail NoTls }
  tcp.upgrade_tls(conn, host, VERIFY_FULL)
  conn
}
```

- `connect` は名前を解決して接続する．`host` は名前でも IP アドレスの文字列でもよい．`port` が 1 から 65535 の外なら TcpFailure
- `read` は 1 バイト以上 `max` バイト以下を，届いた分だけ `Some` で返す．相手が送り終えて閉じた（EOF）後は `None` を返す．`max` が 1 未満なら defect
  - EOF を空の Bytes で表さないのは，呼ぶ側が見忘れると無限ループになり，型にも出ないため．`Option` なら EOF を扱わずに中身を取り出せない
- `read_exact` は `read` をくり返して，ちょうど `n` バイトを返す．`n` バイトそろう前に EOF になったら ConnectionClosed．`n` が 0 なら何も読まずに空の Bytes，負なら defect．PostgreSQL と Redis のプロトコルは長さの決まったまとまりを読むのが基本なので，実装のほとんどはこれで書ける．effect の操作ではなく，`read` の上の関数なので，handler を書く人は実装しなくてよい
- `write` は `data` を全部 host に渡してから戻る
- `upgrade_tls` は，その接続の上で TLS の handshake をして，同じ `Connection` を TLS の接続に変える．戻った後の `read` と `write` は TLS を通る．新しい値を返さないのは，上げる前の値を使い続ける誤りを起こさないため
  - `server_name` は SNI に使う．証明書をどこまで確かめるかは `verify` で選ぶ
    - `Full(roots:)`：`roots` までチェーンをたどり，証明書が `server_name` のものかを照らす．ふつうはこれで，`VERIFY_FULL`（host の信頼する root）を渡す
    - `ChainOnly(roots:)`：チェーンだけをたどり，名前は照らさない．PostgreSQL の `verify-ca` を写すために入れる
    - `NoVerification`：証明書を確かめない．暗号化はするが，相手が本物かはわからない
    - `roots` は `SystemRoots`（host の信頼する root）か，`Pem(text:)`（PEM で書いた 1 つ以上の CA 証明書．これだけを信頼する）
  - 検証に失敗したら TlsFailure で，接続は閉じる．`Pem` の中身が PEM の証明書として読めないのも TlsFailure
  - 検証の値を引数にするのは，信頼の設定が接続文字列（PostgreSQL の `sslmode` と `sslrootcert`，Redis の `--cacert` と `--insecure`）として library に届くため．library がそのまま `Verify` に写せる．検証を切る所には `NoVerification` と書かれるので，読めばわかる
  - PEM は `String` で受ける．ファイルからは `FileSystem.read_text` で読める
  - client 証明書（mTLS，PostgreSQL の `sslcert` と `sslkey`）はこの proposal に入れない．入れるときは `upgrade_tls` に引数を足すことになるので，別の proposal にする
  - 呼んだ時点で，届いていて `read` がまだ返していないバイトを handler が持っていたら，handshake を始めずに TlsFailure にして接続を閉じる
    - client が ClientHello を送る前に，server が TLS のバイトを送ることはない．なので，このバイトは handshake の一部ではありえない．届いていたら，平文の段階で経路の途中から差し込まれたものとみなす
    - 捨てて続けたり，TLS の後の平文として渡したりすると，差し込まれた平文を暗号化した接続のデータとして扱う穴になる（PostgreSQL の CVE-2021-23214 と CVE-2021-23222，Postfix の STARTTLS の CVE-2011-0411）
    - `read` は `max` までしか返さないので，host が先に読んだバイトを handler が持つことはある．`read_exact` は要る分だけ読むので，正しい相手との間でこれが起きるのは相手がプロトコルを破ったときだけ
  - TLS の接続をもう一度上げるのは defect．`read` か `write` を待っている接続を上げるのも defect
- `close` は接続を閉じる．閉じた接続に `close` しても何もしない．閉じた接続への `read` と `write` は ConnectionClosed
- 相手が接続を切った（reset）後の `read` と `write` も ConnectionClosed
- 時間切れはない．`Fiber.timeout` で書く．`read` や `connect` を待っている fiber が中断されたら，待ちを取りやめる（`[ext.call]` の AbortSignal，`[ext.c-host]` の `em_on_cancel`）．中断で接続そのものは閉じない
- `SystemTcp` は，release で自分が開いて閉じていない接続を全部閉じる
- 接続は，それを開いた handler の with の中でだけ使える．with を抜けた後の接続や，別の handler（別の with の `SystemTcp`）が開いた接続を操作に渡すのは，`close` も含めて defect
  - fail（ConnectionClosed）にしないのは，相手が閉じた場合と区別がつかず，プログラムの誤りを通信の失敗として扱うことになるため．正しいプログラムでは起きないので，`fails` に出す必要もない
  - release の後に handler を使うのが defect なのと同じ扱い（`[effect.escape]`）
- 同じ接続に 2 つの fiber が同時に `read`（または同時に `write`）するのは defect．`read` と `write` を別の fiber から同時に呼ぶのはよい（全二重）
- エラーは種類ごとに分ける（FileSystem と同じ方針，`[lib.layers]`）．その他の host のエラーは TcpFailure にまとめ，理由を message に入れる

### Targets

- JS：`node:net` の `Socket`．`upgrade_tls` は `node:tls` の `connect` に既存の socket を渡す．Node と Bun で動く．渡す前に，handler の buffer と socket の `readableLength` が空であることを確かめる
- C：POSIX の `getaddrinfo`，ノンブロッキングの `socket` / `connect` / `recv` / `send`．待ちは `em_on_readable` と `em_on_writable`（`[ext.c-host]`）．`getaddrinfo` は止まるので，別のスレッドで呼ぶ．`upgrade_tls` は後の版にする．TLS の library（OpenSSL など）へのリンクが要り，C の出力が今は libc と OS の library しか使わない前提（`[target.c.cc]` の `-pthread` と `-lm` だけ）を変えるため
- WASM：`wasi:sockets@0.3.0`．名前は `ip-name-lookup` の `resolve-addresses` で解決し，`tcp-socket` の `connect` で接続する．読み書きは `receive` と `send` の stream．`wasi:sockets` を出すのは runtime の中だけで，利用者向けの `Std.Sockets` は作らない（Alternatives）
  - wasmtime は既定でネットワークを渡さない．`emela run` は，component が `wasi:sockets` を import するときだけ `-S inherit-network=y,allow-ip-name-lookup=y` を足す
  - `upgrade_tls` は component の中で TLS を動かす．TLS の実装（Rust の rustls）を `wasm32-wasip3` でビルドした component をコンパイラに同梱し，entry から `upgrade_tls` に届くときだけ `main.wasm` に合成する．この component は I/O をしない（暗号文を受け取って平文を返す）ので，socket は runtime が持ったまま．上げる前に届いていたバイトがあるかも，runtime が自分の buffer を見ればわかる
  - host の TLS（`wasi:tls@0.3.0-draft`）は使わない．WASI の提案として Phase 1，wasmtime でも experimental（`-S tls`）で，handshake に guest が渡せるのは server name だけ．`Pem`，`ChainOnly`，`NoVerification` を表せない（Alternatives）
  - WASI のサンドボックスからは OS の証明書ストアが見えない．WASM の `SystemRoots` は，コンパイラに同梱した root（Mozilla の CA の一覧，`webpki-roots`）にする．root の更新はコンパイラのリリースに乗る
  - 試作（2026-11-14）：rustls 0.23 と `rustls-rustcrypto` を Rust 1.101 beta の `wasm32-wasip3` でビルドし，呼ぶ側の component と `wac-graph` で合成して wasmtime 50 で動かした．`Full`，`ChainOnly`，`NoVerification` の成功と失敗が期待どおり．TLS の component は 919KB（gzip で 357KB）．`Pem` は未確認

## Specification changes

### `[lib.layers]`

In the table of layers, add Tcp to the platform layer: "Console, Clock, Random, Env, FileSystem, Http, Tcp".

Add `SystemTcp` to the list of system handlers, and to the handlers bundled in `Platform`.

Add after the item on Http:

- Tcp (`Std.Tcp`) is a TCP client. An open connection can be upgraded to TLS in place. Its declarations are in `[api.std.tcp]`. It does not listen for connections.

### `[api.std.tcp]` (new)

### Std.Tcp (`std/tcp.emel`, `std/tcp.mjs`, `std/tcp.c`) `[api.std.tcp]`

The platform layer (`[lib.layers]`). A TCP client: it opens connections, reads and writes bytes on them, and upgrades them to TLS.

| Declaration | Signature | Description |
|---|---|---|
| `Connection` | `opaque type Connection` | An open connection, or one that was closed |
| `HostNotFound` | `error HostNotFound(host: String)` | The name could not be resolved |
| `ConnectionRefused` | `error ConnectionRefused(host: String, port: Int)` | Nothing accepted the connection |
| `ConnectionClosed` | `error ConnectionClosed` | The connection was closed by `close`, by the release of the handler, or by the peer resetting it, or the peer finished sending before `read_exact` got its bytes |
| `TlsFailure` | `error TlsFailure(message: String)` | The TLS handshake failed, including a certificate that could not be verified. The reason is in `message` |
| `TcpFailure` | `error TcpFailure(message: String)` | Any other failure of the host. The reason is in `message` |
| `Roots` | `enum Roots { SystemRoots, Pem(text: String) }` | The certificates trusted as roots |
| `Verify` | `enum Verify { Full(roots: Roots), ChainOnly(roots: Roots), NoVerification }` | How much of the peer's certificate `upgrade_tls` checks |
| `VERIFY_FULL` | `const VERIFY_FULL = Full(roots: SystemRoots)` | The usual check |
| `Tcp` | `effect Tcp` | `connect`, `read`, `write`, `upgrade_tls` and `close`, below |
| `read_exact` | `fn read_exact(conn: Connection, n: Int) -> Bytes fails ConnectionClosed \| TcpFailure uses { Tcp }` | Exactly `n` bytes, below |
| `SystemTcp` | `handler SystemTcp: Tcp` | `node:net` and `node:tls` on JS, POSIX sockets on C, `wasi:sockets@0.3.0` on WASM. Bundled in `Platform` |

```emel
pub effect Tcp {
  suspend fn connect(host: String, port: Int) -> Connection
    fails HostNotFound | ConnectionRefused | TcpFailure
  suspend fn read(conn: Connection, max: Int) -> Option[Bytes] fails ConnectionClosed | TcpFailure
  suspend fn write(conn: Connection, data: Bytes) -> () fails ConnectionClosed | TcpFailure
  suspend fn upgrade_tls(conn: Connection, server_name: String, verify: Verify) -> ()
    fails ConnectionClosed | TlsFailure | TcpFailure
  suspend fn close(conn: Connection) -> ()
}

pub fn read_exact(conn: Connection, n: Int) -> Bytes fails ConnectionClosed | TcpFailure uses { Tcp }
```

- `connect` resolves `host`, which is a name or an IP address written as a string, and connects to `port`. A `port` outside 1 to 65535 fails with TcpFailure.
- `read` returns `Some` of at least 1 and at most `max` bytes, as many as have arrived. After the peer has finished sending and closed its side, it returns `None`. A `max` less than 1 is a defect (``Tcp.read was given max <max>, which is less than 1``).
- `read_exact` calls `read` until it has exactly `n` bytes, and returns them. When `read` returns `None` before that, it fails with ConnectionClosed. An `n` of 0 returns the empty Bytes without reading, and a negative `n` is a defect (``Tcp.read_exact was given n <n>, which is negative``). It is a function on top of `read`, not an operation, so a handler does not implement it.
- `write` returns after all of `data` has been handed to the host.
- `upgrade_tls` performs a TLS handshake on the connection, as the client, and turns the same `Connection` into a TLS connection: `read` and `write` after it go through TLS. `server_name` is sent as the server name indication. `verify` chooses how the certificate of the peer is checked: `Full(roots:)` verifies its chain up to `roots` and checks that it is for `server_name`; `ChainOnly(roots:)` verifies the chain only; `NoVerification` checks nothing, so the connection is encrypted but the peer is not authenticated. `roots` is `SystemRoots`, the roots that the host trusts (on WASM, the roots bundled with emela, `[target.wasm.external]`), or `Pem(text:)`, one or more CA certificates in PEM, which are then the only roots trusted. `VERIFY_FULL` is `Full(roots: SystemRoots)`. When the handshake or the verification fails, or `text` of `Pem` is not one or more certificates in PEM, it fails with TlsFailure and the connection is closed. If, when it is called, the handler holds bytes that have arrived on the connection and that `read` has not returned, it does not start the handshake: it fails with TlsFailure and the connection is closed. A server sends no TLS bytes before the client's first handshake message, so such bytes cannot be part of the handshake. Upgrading a TLS connection is a defect (``Tcp.upgrade_tls was given a connection that is already TLS``), and so is upgrading a connection while a `read` or a `write` on it is waiting (``Tcp.upgrade_tls was given a connection that is being read or written``).
- `close` closes the connection. Closing a closed connection does nothing. `read` and `write` on a closed connection fail with ConnectionClosed, as they do after the peer has reset the connection.
- There is no timeout. Write it with `Fiber.timeout` (`[fiber.combinators]`). When a fiber waiting in `connect`, `read`, `write` or `upgrade_tls` is interrupted, the wait is abandoned (`[ext.call]`, `[ext.c-host]`). The connection stays open.
- The release of `SystemTcp` closes the connections it opened that are still open. A connection can be used only inside the with of the handler that opened it. Passing a connection to an operation, `close` included, after that with has ended, or passing it to another handler than the one that opened it, is a defect (``a connection was used outside the with of the handler that opened it``).
- Two concurrent `read`s on the same connection, or two concurrent `write`s, are a defect (``a connection was read by two fibers at once``, ``a connection was written by two fibers at once``). A `read` and a `write` on the same connection MAY run concurrently.
- The R of Tcp is not Immediate (9.5), so it cannot be used inside the callback of an external function.
- `SystemTcp` uses `node:net` on JS. On C it resolves names with `getaddrinfo` on another thread and waits on non-blocking sockets with `em_on_readable` and `em_on_writable`. On WASM it uses `wasi:sockets@0.3.0`, and performs TLS inside the component (`[target.wasm.external]`). `upgrade_tls` has no C implementation yet, and calling it on C is a compile error (E0412).

### `[target.wasm.output]`

In the item on imports, replace "each only when the entry reaches a host function of `Std.Env`, `Std.Random` or `Std.Clock` that uses it" with "each only when the entry reaches a host function of `Std.Env`, `Std.Random` or `Std.Clock` that uses it, and `wasi:sockets/types@0.3.0` and `wasi:sockets/ip-name-lookup@0.3.0` only when the entry reaches an operation of `SystemTcp`. When the entry reaches `SystemTcp.upgrade_tls`, it also imports `wasi:random/random@0.3.0` and `wasi:clocks/system-clock@0.3.0`".

In the item "`emela build` and `emela run` write exactly one file, `main.wasm`, ...", add after "The runtime is included in the same component": "When the entry reaches `SystemTcp.upgrade_tls`, the TLS implementation bundled with emela is included in the same component as well."

### `[target.wasm.run]`

Add after the first item:

- When `main.wasm` imports `wasi:sockets/types@0.3.0`, `emela run` and `emela test` also pass `-S inherit-network=y,allow-ip-name-lookup=y`, before `main.wasm`. Otherwise they do not, and the program has no access to the network.

### `[target.wasm.external]`

Add rows to the table of handlers that work on WASM:

| Handler | Host function |
|---|---|
| `SystemTcp` (`connect`) | `resolve-addresses` of `wasi:sockets/ip-name-lookup@0.3.0`, then `create` and `connect` of `tcp-socket` in `wasi:sockets/types@0.3.0`, for each address in the order resolved, until one connects. No address is HostNotFound; every address refusing is ConnectionRefused |
| `SystemTcp` (`read`, `write`, `close`) | The stream returned by `receive` and the stream passed to `send` of `tcp-socket`. `close` drops both streams and the socket |

| `SystemTcp` (`upgrade_tls`) | None for TLS itself. The TLS handshake and records are computed inside the component, by the TLS implementation bundled with emela, and the bytes go through the streams of `tcp-socket` above. It uses `get-random-bytes` of `wasi:random/random@0.3.0` and `now` of `wasi:clocks/system-clock@0.3.0`, the latter for the validity period of certificates |

On WASM, `SystemRoots` is the set of root certificates bundled with emela, which is the list of certificate authorities trusted by Mozilla at the release of that version of emela. The roots that the operating system trusts are not read.

### `[diag.list]`

No new codes. The defects of `[api.std.tcp]` have no code.

## Diagnostics

- [ ] なし（defect のメッセージだけ）

## Compatibility

- `Std.Tcp` と，`Platform` に `SystemTcp` が増えるだけ．既存のプログラムの意味は変わらない
- `Tcp`，`Connection` などの名前は `Std.Tcp` の中にあるので，import しなければ利用者の名前とぶつからない

## Implementation

- std：`std/tcp.emel`，`std/tcp.mjs`，`std/tcp.c`．`Platform` に `SystemTcp` を足す
- C の runtime：`getaddrinfo` を別スレッドで呼び，task を完了させる（`[ext.c-host]` の「A host function that hands a task to another thread」）
- WASM の runtime：`crates/emela-codegen-wasm/src/emit/runtime/tcp.rs` を足し，`wasi:sockets/types` と `wasi:sockets/ip-name-lookup` を到達したときだけ import する
- WASM の TLS：
  - TLS の component を新しい crate（例：`crates/emela-tls-wasm`）にし，`wasm32-wasip3` でビルドする．I/O をしない session（暗号文を受け取る，送る暗号文を取り出す，平文を読む，書く）を WIT の interface として export する
  - コンパイラはそれを `include_bytes!` で埋め込み，`wac-graph` で Emela の出力と合成する．コンパイラのバイナリは約 1MB 増える
  - 暗号の provider は実装の PR で選ぶ（`rustls-rustcrypto` はプレリリース．ring が `wasm32-wasip3` で通るかは未確認）
  - toolchain：`wasm32-wasip3` は Rust 1.100 で stable になる．`rust-toolchain.toml` の `stable` がそれ以上になってから入れるか，生成物を commit するかを実装の PR で決める
  - TLS の component から Rust の std の import（`wasi:cli` など）を外し，`wasi:random` と `wasi:clocks` だけにする
- emela-driver：`main.wasm` の import に `wasi:sockets/types@0.3.0` があるとき，`wasmtime run` に `-S inherit-network=y,allow-ip-name-lookup=y` を足す（`run.rs`，`emela test` も）
- PR は JS，C，WASM の順に分ける．WASM が v0.28 に間に合わなければ，その PR だけ次の版に送る
- テスト：tests/ にローカルの echo server を立てて JS，C，WASM で回す．`emela test` からは server を立てられないので，server を立てるのは Rust のテストか CI の手順にする．TLS は自己署名の証明書では検証に通らないので，検証の失敗（TlsFailure）を確かめるテストと，CI で信頼される証明書を持つ相手に繋ぐテストに分ける
- テスト用の handler（`TestTcp`）は std に入れない．TCP は読み書きの区切りが自由で，`TestHttp` のような純関数の台本の形が決まらないので，x の Pome で形を探り，固まってから std に入れる

## Alternatives

- `@external` のまま，Pome ごとに `node:net` を呼ぶ．host ごとに書き直しになり，`[external]` の許可が要る
- `connect` に TLS の option を付ける（`connect(host, port, tls: True)`）．接続の途中で TLS に上げるプロトコル（PostgreSQL，STARTTLS）が書けない．`upgrade_tls` があれば最初から TLS の場合も書けるので，両方を持つ理由がない
- 検証の段階ごとに操作を分ける（`upgrade_tls`，`upgrade_tls_trusting`，`upgrade_tls_unverified`）．段階が増えるたびに操作が増え，接続文字列から選ぶ library は結局 match で操作を振り分ける
- 何を信頼するかを handler に持たせる（`with SystemTcp(trust: ...)`）．library は名前だけを渡せばよくなるが，信頼の設定は接続文字列で library に届くので，それを with の側へ戻す配線が要る
- `ChainOnly` を入れない．名前を照らさない検証は他のプロトコルではまれだが，入れないと PostgreSQL の `sslmode=verify-ca` を写せない
- 検証せずに証明書を返し，利用者が確かめる．チェーンの検証を利用者が書くことになり，暗号の誤りを招く
- `upgrade_tls` が新しい `Connection` を返し，元の値を閉じた扱いにする．元の値を使い続ける誤りが，ConnectionClosed として実行時にしか見つからない
- TLS を別の proposal にする．JS では `node:tls` に socket を渡すだけで書け，PostgreSQL と Redis の両方がこれを要る．C の TLS だけを後に回せば足りる
- EOF を空の Bytes で表す．「1 バイト以上返す」約束があるので値は取り違えないが，見忘れると無限ループになり，型にも出ない
- 待ち受け（listen / accept）も入れる．PostgreSQL と Redis の client には要らず，HTTP のサーバは `Http.serve` がある．要る場面が出てから別の proposal にする
- C を後の版にする（Tier 2 は遅れてよい，`[target]`）．純 Emela で書いたプロトコルをネイティブでも動かすのが動機なので，C の平文の client は最初から入れる．C の runtime にはもう `em_on_readable` と `em_on_writable` があり，足すのは `getaddrinfo` を別のスレッドで呼ぶところが主
- 名前を `Std.Socket` や `Std.Net` にする．UDP や Unix domain socket を入れる予定がないので，中身の名前にした
- WASM のために `wasi:sockets` の粒度（socket の作成，bind，アドレス）の `Std.Sockets` を出し，`Std.Tcp` をその上に Emela で書く．JS の `node:net` は名前を渡して接続する口しかないので，JS と C にも同じ粒度の実装が要る．TLS は socket そのものを host に渡す形（`node:tls`）なので Emela では書けず，platform との境目は `Tcp` の高さに置くしかない．UDP が要るようになったら `Std.Udp` を同じ高さの effect として別に提案する
- WASM の `upgrade_tls` を `wasi:tls@0.3.0-draft` で入れ，`Verify` は `Full(roots: SystemRoots)` だけを受けて，ほかは TlsFailure にする．`main.wasm` は小さく済むが，PostgreSQL の `sslmode=require`（`NoVerification`）や自前の CA（`Pem`）が WASM でだけ失敗する．Phase 1 の draft と experimental な flag に仕様の動作を載せると，wasmtime を上げるたびに壊れる．`wasi:tls` が安定して検証の設定を渡せるようになったら，同梱の TLS から移すかを考える
- WASM の `upgrade_tls` を後の版にする（E0412）．WASM から PostgreSQL と Redis にほぼつなげなくなる．試作で，同梱の TLS が 3 つの `Verify` を守れることを確かめた
- TLS を純 Emela で書く．暗号を Emela で書き直すことになり，量と安全性のどちらも割に合わない
- WASM を後の版にする．`wasi:sockets@0.3.0` はもう vendor してあり，wasmtime 50 は `-S p3=y` でこれを動かせる．runtime には WASI 0.3 の stream を使う import の前例（`fs.rs`）がある

## Prior art

- Cloudflare Workers の `connect()` と `startTls()`：開いた socket をその場で TLS に上げる．検証は host の root だけで，自前の CA は渡せない
- `wasi-pg-client`（Rust）：WASI 0.3 の `wasi:sockets` の上で，guest の中の rustls と `webpki-roots` で TLS を張る PostgreSQL の client．`sslmode` の `require`，`verify-ca`，`verify-full` を持つ

## Open questions

## Decision
