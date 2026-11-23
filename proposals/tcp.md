---
title: Std.Tcp, a TCP client as a platform effect
authors: [Alliana]
status: draft
issue:
touches: [lib.layers, api.std.process, api.std.tcp (new), target.wasm.output, target.wasm.run, target.wasm.external]
requires: []
related: [js-host-variable]
---

# Std.Tcp, a TCP client as a platform effect

## Summary

Adds a TCP client to the platform layer of std. The effect `Tcp` opens connections, reads and writes `Bytes` on them, and closes them. An open connection can be upgraded to TLS in place with `upgrade_tls`. The system handler `SystemTcp` uses `node:net` and `node:tls` on JS, POSIX sockets on native, and `wasi:sockets@0.3.0` on WASM, where TLS runs in a TLS component bundled with the compiler and composed into `main.wasm`. TLS on native and listening for connections are not included.

```emel
import Std.Bytes
import Std.Tcp.{Tcp, HostNotFound, ConnectionRefused, ConnectionClosed, TcpFailure}

fn ping(host: String) -> Option[Bytes]
  fails HostNotFound | ConnectionRefused | ConnectionClosed | TcpFailure
  uses { Tcp } {
  tcp = use Tcp
  conn = tcp.connect(host, 6379)
  tcp.write(conn, Bytes.encode_utf8("PING\r\n"))
  reply = tcp.read(conn, 512)
  tcp.close(conn)
  reply
}
```

## Motivation

- The protocols of databases (PostgreSQL, Redis, MySQL) and message brokers run on TCP. Emela can use them today only through libraries of a host (`Bun.sql`, `bun:sqlite`), and a Pome that does so runs on one host only.
- With TCP, a protocol can be written as a Pome in pure Emela. A Pome that implements the PostgreSQL wire protocol, for example, runs as a handler of `Sql` of x/sql on both JS and native.
- `[ext.decl]` already says "Host features that most programs use are added to std as platform effects or as value types (Socket, Bytes, and so on)". Bytes (`[api.std.bytes]`) is in std; Socket is not yet.
- The workaround today is a Pome that calls `node:net` through `@external`. It runs only on JS, and every user of the Pome has to allow it in `[external]` (`[module.pome]`).

TLS is one operation, applied to a connection after it is opened. Protocols that use TLS start it in one of two ways:

- They exchange a few plaintext messages and then upgrade the same connection: PostgreSQL (after `S` answers SSLRequest), SMTP and IMAP (STARTTLS).
- They speak TLS from the first byte: Redis (`rediss://`), `sslnegotiation=direct` of PostgreSQL 17, HTTPS.

The second is written as `upgrade_tls` right after `connect`, so one operation covers both without a TLS option on `connect`.

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

The SSLRequest of PostgreSQL is written as follows.

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

- `connect` resolves the name and connects. `host` is a name or an IP address written as a string. A `port` outside 1 to 65535 is TcpFailure.
- `read` returns `Some` of at least 1 and at most `max` bytes, as many as have arrived. After the peer has finished sending and closed its side (end of stream), it returns `None`. A `max` less than 1 is a defect.
  - The end of stream is not the empty Bytes: a caller that forgets to check for it would loop forever, and the type would not show it. With `Option`, the contents cannot be taken out without handling the end of stream.
- `read_exact` calls `read` until it has exactly `n` bytes. If the end of stream comes first, it fails with ConnectionClosed. An `n` of 0 returns the empty Bytes without reading; a negative `n` is a defect. The protocols of PostgreSQL and Redis mostly read pieces of known length, so most of their implementations are written with it. It is a function on top of `read`, not an operation, so a writer of a handler does not implement it.
- `write` returns after all of `data` has been handed to the host.
- `upgrade_tls` performs a TLS handshake on the connection and turns the same `Connection` into a TLS connection. `read` and `write` after it go through TLS. It returns no new value, so the value from before the upgrade cannot be used by mistake.
  - `server_name` is used for the server name indication. `verify` chooses how much of the certificate is checked.
    - `Full(roots:)`: follows the chain up to `roots` and checks that the certificate is for `server_name`. This is the usual choice, passed as `VERIFY_FULL` (the roots the host trusts).
    - `ChainOnly(roots:)`: follows the chain only, and does not check the name. It is there to express `verify-ca` of PostgreSQL.
    - `NoVerification`: does not check the certificate. The connection is encrypted, but whether the peer is genuine is unknown.
    - `roots` is `SystemRoots` (the roots the host trusts) or `Pem(text:)` (one or more CA certificates in PEM, which are then the only roots trusted).
  - When the verification fails, it is TlsFailure and the connection is closed. `text` of `Pem` that cannot be read as certificates in PEM is also TlsFailure.
  - The verification is an argument because the trust settings reach a library in a connection string (`sslmode` and `sslrootcert` of PostgreSQL, `--cacert` and `--insecure` of Redis). The library maps them to `Verify` directly. Where verification is turned off, `NoVerification` is written, so it is visible to a reader.
  - PEM is taken as a `String`. It can be read from a file with `FileSystem.read_text`.
  - Client certificates (mutual TLS, `sslcert` and `sslkey` of PostgreSQL) are not in this proposal. Adding them adds arguments to `upgrade_tls`, so it is a separate proposal.
  - If, when it is called, the handler holds bytes that have arrived and that `read` has not returned, it does not start the handshake: it fails with TlsFailure and closes the connection.
    - A server sends no TLS bytes before the client sends its ClientHello, so such bytes cannot be part of the handshake. If they arrived, they were injected by someone on the path during the plaintext phase.
    - Dropping them and continuing, or handing them over as plaintext after TLS, would treat injected plaintext as data of the encrypted connection (CVE-2021-23214 and CVE-2021-23222 of PostgreSQL, CVE-2011-0411 of the STARTTLS of Postfix).
    - `read` returns at most `max` bytes, so a handler can hold bytes that the host read ahead. `read_exact` reads only what it needs, so with an honest peer this happens only when the peer breaks the protocol.
  - Upgrading a TLS connection again is a defect. Upgrading a connection on which a `read` or a `write` is waiting is also a defect.
- `close` closes the connection. Closing a closed connection does nothing. `read` and `write` on a closed connection are ConnectionClosed.
- `read` and `write` after the peer has reset the connection are also ConnectionClosed.
- There is no timeout. It is written with `Task.timeout`. When a fiber waiting in `read` or `connect` is interrupted, the wait is abandoned (AbortSignal in `[ext.call]`, `em_on_cancel` in `[ext.c-host]`). An interruption does not close the connection.
- The release of `SystemTcp` closes every connection it opened that is still open.
- A connection can be used only inside the with of the handler that opened it. Passing a connection to an operation, `close` included, after that with has ended, or a connection that another handler (`SystemTcp` of another with) opened, is a defect.
  - It is not a failure (ConnectionClosed): that could not be told apart from the peer closing the connection, and would treat an error of the program as a failure of the network. A correct program never does it, so it does not belong in `fails`.
  - Using a handler after its release is a defect in the same way (`[effect.escape]`).
- Two fibers reading the same connection at once (or writing it at once) is a defect. A `read` and a `write` from different fibers at once are allowed (full duplex).
- Errors are split by kind, as in FileSystem (`[lib.layers]`). Other errors of the host are TcpFailure, with the reason in `message`.
- `SystemTcp` requires ProcessHost (`[effect.host]`) and is bundled in the layer `Process`, not `Platform`. A page of a web browser has no TCP. `main`, tests and HTTP services (`Http.serve` provides ProcessHost, `[lib.http-service]`) can use it.

### Targets

- JS: `Socket` of `node:net`. `upgrade_tls` passes the existing socket to `connect` of `node:tls`. It runs on Node and Bun. Before passing it, the handler checks that its own buffer and `readableLength` of the socket are empty.
- Native: `getaddrinfo` of POSIX, and non-blocking `socket`, `connect`, `recv` and `send`. It waits with `em_on_readable` and `em_on_writable` (`[ext.c-host]`). `getaddrinfo` blocks, so it is called on another thread. `upgrade_tls` comes in a later version: it needs a TLS library (OpenSSL or another) to be linked, which changes the assumption that the native output uses only libc and the libraries of the OS (only `-pthread` and `-lm` in `[target.native.cc]`).
- WASM: `wasi:sockets@0.3.0`. Names are resolved with `resolve-addresses` of `ip-name-lookup`, and `connect` of `tcp-socket` connects. Reads and writes go through the streams of `receive` and `send`. `wasi:sockets` is used only inside the runtime; no `Std.Sockets` is offered to users (Alternatives).
  - wasmtime gives no network by default. `emela run` adds `-S inherit-network=y,allow-ip-name-lookup=y` only when the component imports `wasi:sockets`.
  - `upgrade_tls` runs TLS inside the component. A component of a TLS implementation (rustls, in Rust) built for `wasm32-wasip3` is bundled with the compiler, and composed into `main.wasm` only when the entry reaches `upgrade_tls`. The component performs no I/O (it takes ciphertext and returns plaintext), so the runtime keeps the socket. Whether bytes arrived before the upgrade is known from the runtime's own buffer.
  - The host's TLS (`wasi:tls@0.3.0-draft`) is not used. It is a Phase 1 WASI proposal, experimental in wasmtime (`-S tls`), and the only thing a guest can pass to the handshake is the server name. It cannot express `Pem`, `ChainOnly` or `NoVerification` (Alternatives).
  - The sandbox of WASI does not see the certificate store of the OS. `SystemRoots` on WASM is the set of roots bundled with the compiler (the list of certificate authorities of Mozilla, `webpki-roots`). Updates of the roots come with releases of the compiler.
  - Prototype (2026-11-14): rustls 0.23 with `rustls-rustcrypto`, built with the `wasm32-wasip3` of Rust 1.101 beta, composed with a calling component by `wac-graph` and run on wasmtime 50. Success and failure of `Full`, `ChainOnly` and `NoVerification` were as expected. The TLS component is 919KB (357KB with gzip). `Pem` was not tried.

## Diagnostics

None. The defects have messages and no codes. Calling `upgrade_tls` on native is E0412, which already exists.

## Compatibility

- Only `Std.Tcp` is added, and `SystemTcp` in `Process`. No existing program changes meaning.
- `Tcp`, `Connection` and the other names are in `Std.Tcp`, so they do not collide with a user's names unless imported.

## Implementation

- std: `std/tcp.emel`, `std/tcp.mjs`, `std/tcp.c`. `SystemTcp` is added to `Process`.
- Native runtime: calls `getaddrinfo` on another thread and completes the task ("A host function that hands a task to another thread" in `[ext.c-host]`).
- WASM runtime: adds `crates/emela-codegen-wasm/src/emit/runtime/tcp.rs`, and imports `wasi:sockets/types` and `wasi:sockets/ip-name-lookup` only when they are reached.
- TLS on WASM:
  - The TLS component is a new crate (`crates/emela-tls-wasm`, for example), built for `wasm32-wasip3`. It exports, as a WIT interface, a session that performs no I/O (take ciphertext, take out ciphertext to send, read plaintext, write plaintext).
  - The compiler embeds it with `include_bytes!` and composes it with the output of Emela with `wac-graph`. The binary of the compiler grows by about 1MB.
  - The pull request of the implementation chooses the crypto provider (`rustls-rustcrypto` is a pre-release; whether ring builds for `wasm32-wasip3` is not known).
  - Toolchain: `wasm32-wasip3` is stable from Rust 1.100. The pull request of the implementation decides whether to wait until `stable` of `rust-toolchain.toml` is at least that, or to commit the built component.
  - The imports of the Rust std (`wasi:cli` and others) are removed from the TLS component, leaving only `wasi:random` and `wasi:clocks`.
- emela-driver: when the imports of `main.wasm` include `wasi:sockets/types@0.3.0`, adds `-S inherit-network=y,allow-ip-name-lookup=y` to `wasmtime run` and `wasmtime serve` (`run.rs`, and `emela test`).
- The pull requests are split into JS, native and WASM, in that order. If WASM is not ready for v0.29, only that pull request moves to the next version.
- Tests: a local echo server in tests/, run on JS, native and WASM. `emela test` cannot start a server, so the server is started by a Rust test or a step of CI. A self-signed certificate does not pass verification, so TLS is tested in two parts: a test that verification fails (TlsFailure), and a test in CI that connects to a peer with a trusted certificate.
- A handler for tests (`TestTcp`) is not added to std. Reads and writes on TCP can be split anywhere, so the shape of a script of pure functions (as `TestHttp` has) is not settled. It is tried in a Pome of x, and moves to std once its shape is settled.

## Alternatives

- Keep `@external`, with each Pome calling `node:net`. It is rewritten for each host and needs an `[external]` permission.
- A TLS option on `connect` (`connect(host, port, tls: True)`). Protocols that upgrade partway (PostgreSQL, STARTTLS) cannot be written with it. `upgrade_tls` also covers TLS from the start, so there is no reason to have both.
- One operation for each level of verification (`upgrade_tls`, `upgrade_tls_trusting`, `upgrade_tls_unverified`). Each new level adds an operation, and a library that chooses from a connection string ends up dispatching to the operations with a match.
- The handler holds what is trusted (`with SystemTcp(trust: ...)`). A library would pass only the name, but the trust settings reach the library in a connection string, and would have to be wired back to the with.
- No `ChainOnly`. Verification without the name is rare in other protocols, but without it `sslmode=verify-ca` of PostgreSQL cannot be expressed.
- Return the certificate without verifying it, and let the user check it. The user would write the verification of the chain, which invites cryptographic mistakes.
- `upgrade_tls` returns a new `Connection` and treats the old value as closed. Using the old value by mistake would be found only at run time, as ConnectionClosed.
- TLS in a separate proposal. On JS it is a matter of passing the socket to `node:tls`, and both PostgreSQL and Redis need it. Postponing only TLS on native is enough.
- The empty Bytes for the end of stream. Since `read` returns at least 1 byte otherwise, the values cannot be confused, but a caller that forgets to check loops forever, and the type does not show it.
- Listening (listen and accept) as well. Clients of PostgreSQL and Redis do not need it, and HTTP servers have `Http.serve`. It is a separate proposal when it is needed.
- Native in a later version (Tier 2 may lag, `[target]`). Running protocols written in pure Emela natively is part of the motivation, so the plaintext client on native comes from the start. The native runtime already has `em_on_readable` and `em_on_writable`; most of what is added is calling `getaddrinfo` on another thread.
- The name `Std.Socket` or `Std.Net`. There is no plan for UDP or Unix domain sockets, so the module is named after what it has.
- For WASM, offer `Std.Sockets` at the level of `wasi:sockets` (creating sockets, bind, addresses), and write `Std.Tcp` on it in Emela. `node:net` on JS only connects by name, so JS and native would also need implementations at that level. TLS hands the socket itself to the host (`node:tls`), so it cannot be written in Emela, and the boundary with the platform has to be at the level of `Tcp`. When UDP is needed, `Std.Udp` is proposed separately as an effect at the same level.
- `upgrade_tls` on WASM through `wasi:tls@0.3.0-draft`, accepting only `Full(roots: SystemRoots)` as `Verify` and failing with TlsFailure for the others. `main.wasm` would be smaller, but `sslmode=require` of PostgreSQL (`NoVerification`) and a private CA (`Pem`) would fail only on WASM. Behavior of the specification resting on a Phase 1 draft and an experimental flag would break with each update of wasmtime. Moving from the bundled TLS is considered when `wasi:tls` is stable and can take settings of verification.
- `upgrade_tls` on WASM in a later version (E0412). WASM could then hardly connect to PostgreSQL or Redis. The prototype showed that the bundled TLS keeps the three kinds of `Verify`.
- TLS written in pure Emela. It would rewrite cryptography in Emela, which pays off neither in volume nor in safety.
- WASM in a later version. `wasi:sockets@0.3.0` is already vendored, and wasmtime 50 runs it with `-S p3=y`. The runtime already has an import that uses streams of WASI 0.3 (`fs.rs`).
- `SystemTcp` in `Platform`. `Platform` bundles the handlers that every host has, and a page of a web browser has no TCP. With ProcessHost, `with Platform` in a page keeps type checking, and every entry that runs in a process (main, tests, HTTP services) still has TCP through `Process`.

## Prior art

- `connect()` and `startTls()` of Cloudflare Workers: upgrades an open socket to TLS in place. Verification uses only the host's roots; a private CA cannot be passed.
- `wasi-pg-client` (Rust): a PostgreSQL client on `wasi:sockets` of WASI 0.3 that runs TLS with rustls and `webpki-roots` inside the guest. It has `require`, `verify-ca` and `verify-full` of `sslmode`.

## Open questions

## Decision
