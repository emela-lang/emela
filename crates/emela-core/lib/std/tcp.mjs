// Host functions for SystemTcp (tcp.emel) on JS. Uses `node:net` and `node:tls`.
//
// The operations return an outcome object instead of throwing, since an external function cannot
// fail (`[ext.exception]`): `{ error, message, conn, bytes }`. `error` is "" on success, the name
// of the Emela error to fail with, "End" when `read` reached the end of the stream, or the name of
// a defect ("Foreign", "ReadBusy", "WriteBusy", "AlreadyTls", "Busy") that the handler raises.
//
// A registry is made by the init of each SystemTcp and holds the connections it opened. A
// connection records its registry, so a connection of another with, or of a with that has ended,
// is told apart ("Foreign").
//
// A connection reads in flowing mode into its own buffer (`chunks`), and pauses the socket when
// the buffer holds `HIGH` bytes. `read` takes from that buffer, so the bytes that have arrived and
// that `read` has not returned are the buffer and `readableLength` of the socket; `upgradeTls`
// refuses to start the handshake when either is not empty (`[api.std.tcp]`).
//
// Waits take the AbortSignal that the runtime appends (`[ext.call]`). An aborted wait returns at
// once and leaves the connection open; the outcome is dropped by the interrupted fiber. An
// aborted `upgradeTls` lets the handshake finish in the background: the operations that come
// after it wait for it first.
//
// `with Process` imports this module even when the program never opens a connection, so it must
// load on a host without `node:net` too (the docs playground runs programs in a browser Worker).
// There, `connect` fails with TcpFailure.

const onNode = globalThis.process?.versions?.node !== undefined;
const net = globalThis.process?.getBuiltinModule?.("node:net") ?? (onNode ? await import("node:net") : undefined);
const tls = globalThis.process?.getBuiltinModule?.("node:tls") ?? (onNode ? await import("node:tls") : undefined);
const crypto =
  globalThis.process?.getBuiltinModule?.("node:crypto") ?? (onNode ? await import("node:crypto") : undefined);

/** The size of the buffer of a connection above which its socket is paused. */
const HIGH = 64 * 1024;

const outcome = (fields) => ({ error: "", message: "", conn: undefined, bytes: undefined, ...fields });
const failed = (error, message = "") => outcome({ error, message });

const describe = (e) => (e instanceof Error ? e.message : String(e));

// Error codes of the host that mean the connection was closed under the operation.
const CLOSED = new Set([
  "ECONNRESET",
  "EPIPE",
  "ECONNABORTED",
  "ERR_STREAM_DESTROYED",
  "ERR_STREAM_WRITE_AFTER_END",
  "ERR_SOCKET_CLOSED",
]);

const NOT_FOUND = new Set(["ENOTFOUND", "EAI_NONAME", "EAI_NODATA"]);

/** A new registry, for the init of a SystemTcp. */
export const openRegistry = () => ({ released: false, open: new Set() });

/** Closes the connections of `registry` that are still open, for the release of its SystemTcp. */
export function releaseRegistry(registry) {
  registry.released = true;
  for (const conn of [...registry.open]) shut(conn);
}

/** Resolves `host` and connects to `port`. */
export function connect(registry, host, port, signal) {
  if (net === undefined) return failed("TcpFailure", "this host has no TCP");
  if (signal?.aborted) return failed("TcpFailure", "aborted");
  return new Promise((resolve) => {
    let socket;
    try {
      socket = net.connect({ host, port, allowHalfOpen: true });
    } catch (e) {
      resolve(failed("TcpFailure", describe(e)));
      return;
    }
    const finish = (o) => {
      signal?.removeEventListener("abort", onAbort);
      socket.removeListener("connect", onConnect);
      socket.removeListener("error", onError);
      resolve(o);
    };
    const onAbort = () => {
      socket.destroy();
      finish(failed("TcpFailure", "aborted"));
    };
    const onConnect = () => finish(outcome({ conn: adopt(registry, socket) }));
    const onError = (e) => {
      socket.destroy();
      finish(classifyConnect(e));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    socket.once("connect", onConnect);
    socket.once("error", onError);
  });
}

/** The outcome of a failed connect. With several addresses, the error is an AggregateError. */
function classifyConnect(e) {
  const codes = Array.isArray(e?.errors) ? e.errors.map((x) => x?.code) : [e?.code];
  if (codes.length > 0 && codes.every((c) => NOT_FOUND.has(c))) return failed("HostNotFound");
  if (codes.length > 0 && codes.every((c) => c === "ECONNREFUSED")) return failed("ConnectionRefused");
  return failed("TcpFailure", describe(e));
}

/** Registers a connected `socket` in `registry` and starts buffering what arrives. */
function adopt(registry, socket) {
  const conn = {
    registry,
    socket,
    raw: socket,
    chunks: [],
    size: 0,
    ended: false,
    lost: false,
    error: undefined,
    closed: false,
    tls: false,
    upgrading: undefined,
    reading: false,
    writing: false,
    wake: undefined,
  };
  listen(conn, socket);
  registry.open.add(conn);
  return conn;
}

/** Wakes the waiting `read`, if any. */
function wake(conn) {
  const w = conn.wake;
  conn.wake = undefined;
  w?.();
}

/** Buffers what `socket` gives into `conn`. The listeners are removed by `unlisten`. */
function listen(conn, socket) {
  conn.listeners = {
    data: (chunk) => {
      conn.chunks.push(chunk);
      conn.size += chunk.length;
      if (conn.size >= HIGH) socket.pause();
      wake(conn);
    },
    end: () => {
      conn.ended = true;
      wake(conn);
    },
    error: (e) => {
      conn.error ??= e;
      wake(conn);
    },
    close: () => {
      if (!conn.ended && conn.error === undefined) conn.lost = true;
      wake(conn);
    },
  };
  for (const [event, f] of Object.entries(conn.listeners)) socket.on(event, f);
}

function unlisten(conn, socket) {
  for (const [event, f] of Object.entries(conn.listeners)) socket.removeListener(event, f);
}

/** Closes `conn` and removes it from its registry. Does nothing if it is closed. */
function shut(conn) {
  if (conn.closed) return;
  conn.closed = true;
  conn.socket.destroy();
  conn.raw.destroy();
  conn.registry.open.delete(conn);
  wake(conn);
}

/** Whether `conn` cannot be used through `registry` (`[api.std.tcp]`). */
const foreign = (registry, conn) => conn.registry !== registry || registry.released;

/** The outcome of an error of the host on a connection. */
function classify(e) {
  if (CLOSED.has(e?.code)) return failed("ConnectionClosed");
  return failed("TcpFailure", describe(e));
}

/** The outcome when `conn` cannot be read or written any more, or undefined. */
function broken(conn) {
  if (conn.closed || conn.lost) return failed("ConnectionClosed");
  if (conn.error !== undefined) return classify(conn.error);
  return undefined;
}

/** Waits for an upgrade that an interrupted `upgradeTls` left running. True if aborted. */
async function settle(conn, signal) {
  while (conn.upgrading !== undefined) {
    if (await abortable(conn.upgrading, signal)) return true;
  }
  return false;
}

/** Waits for `promise`, or for `signal` to abort. True if aborted. */
function abortable(promise, signal) {
  if (signal?.aborted) return Promise.resolve(true);
  return new Promise((resolve) => {
    const onAbort = () => resolve(true);
    signal?.addEventListener("abort", onAbort, { once: true });
    promise.then(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve(false);
    });
  });
}

/** At least 1 and at most `max` bytes of `conn`, "End" at the end of the stream. */
export async function read(registry, conn, max, signal) {
  if (foreign(registry, conn)) return failed("Foreign");
  if (conn.reading) return failed("ReadBusy");
  conn.reading = true;
  try {
    if (await settle(conn, signal)) return failed("TcpFailure", "aborted");
    for (;;) {
      if (conn.closed) return failed("ConnectionClosed");
      if (conn.size > 0) return outcome({ bytes: take(conn, max) });
      const b = broken(conn);
      if (b !== undefined) return b;
      if (conn.ended) return failed("End");
      const arrived = new Promise((resolve) => {
        conn.wake = resolve;
      });
      if (await abortable(arrived, signal)) {
        conn.wake = undefined;
        return failed("TcpFailure", "aborted");
      }
    }
  } finally {
    conn.reading = false;
  }
}

/** Takes up to `max` bytes from the front of the buffer of `conn`. */
function take(conn, max) {
  const all = conn.chunks.length === 1 ? conn.chunks[0] : Buffer.concat(conn.chunks);
  const n = Math.min(max, all.length);
  const rest = all.subarray(n);
  conn.chunks = rest.length > 0 ? [rest] : [];
  conn.size = rest.length;
  if (conn.size < HIGH && !conn.closed) conn.socket.resume();
  return all.subarray(0, n);
}

/** Hands all of `data` to the host. */
export async function write(registry, conn, data, signal) {
  if (foreign(registry, conn)) return failed("Foreign");
  if (conn.writing) return failed("WriteBusy");
  conn.writing = true;
  try {
    if (await settle(conn, signal)) return failed("TcpFailure", "aborted");
    const b = broken(conn);
    if (b !== undefined) return b;
    if (data.length === 0) return outcome({});
    const written = new Promise((resolve) => {
      try {
        conn.socket.write(data, (e) => resolve(e));
      } catch (e) {
        resolve(e);
      }
    });
    let e;
    const aborted = await abortable(
      written.then((x) => {
        e = x;
      }),
      signal,
    );
    if (aborted) return failed("TcpFailure", "aborted");
    if (conn.closed) return failed("ConnectionClosed");
    if (e !== undefined && e !== null) return classify(e);
    return outcome({});
  } finally {
    conn.writing = false;
  }
}

/** The CA certificates in `pem`, or undefined if it is not one or more certificates in PEM. */
function certificates(pem) {
  const blocks = pem.match(/-----BEGIN CERTIFICATE-----[\s\S]*?-----END CERTIFICATE-----/g) ?? [];
  if (blocks.length === 0) return undefined;
  try {
    for (const block of blocks) new crypto.X509Certificate(block);
  } catch {
    return undefined;
  }
  return blocks;
}

/**
 * Performs a TLS handshake on `conn` as the client. `mode` is 0 (Full), 1 (ChainOnly) or 2
 * (NoVerification); `pem` holds the roots when `systemRoots` is false.
 */
export async function upgradeTls(registry, conn, serverName, mode, systemRoots, pem, signal) {
  if (foreign(registry, conn)) return failed("Foreign");
  if (conn.reading || conn.writing) return failed("Busy");
  if (await settle(conn, signal)) return failed("TcpFailure", "aborted");
  if (conn.tls) return failed("AlreadyTls");
  const b = broken(conn);
  if (b !== undefined) return b;
  // Bytes that arrived before the client's first handshake message cannot be part of the
  // handshake: someone on the path injected them in the plaintext phase.
  if (conn.size > 0 || conn.socket.readableLength > 0) {
    shut(conn);
    return failed("TlsFailure", "bytes arrived before the TLS handshake");
  }
  let ca;
  if (!systemRoots) {
    ca = certificates(pem);
    if (ca === undefined) {
      shut(conn);
      return failed("TlsFailure", "the roots are not certificates in PEM");
    }
  }
  const raw = conn.socket;
  unlisten(conn, raw);
  const options = {
    socket: raw,
    host: serverName,
    rejectUnauthorized: mode !== 2,
    ...(net.isIP(serverName) === 0 ? { servername: serverName } : {}),
    ...(ca === undefined ? {} : { ca }),
    ...(mode === 1 ? { checkServerIdentity: () => undefined } : {}),
  };
  const handshake = new Promise((resolve) => {
    let secure;
    try {
      secure = tls.connect(options);
    } catch (e) {
      resolve(failed("TlsFailure", describe(e)));
      return;
    }
    // Taken until the handshake ends, so that `shut` closes it too.
    conn.socket = secure;
    const onSecure = () => {
      secure.removeListener("error", onError);
      resolve(outcome({}));
    };
    const onError = (e) => {
      secure.removeListener("secureConnect", onSecure);
      resolve(failed("TlsFailure", describe(e)));
    };
    secure.once("secureConnect", onSecure);
    secure.once("error", onError);
  }).then((o) => {
    conn.upgrading = undefined;
    if (o.error === "" && !conn.closed) {
      conn.tls = true;
      listen(conn, conn.socket);
    } else {
      shut(conn);
    }
    return o;
  });
  conn.upgrading = handshake;
  let result;
  const aborted = await abortable(
    handshake.then((o) => {
      result = o;
    }),
    signal,
  );
  if (aborted) return failed("TcpFailure", "aborted");
  if (conn.closed && result.error === "") return failed("ConnectionClosed");
  return result;
}

/** Closes `conn`. Does nothing if it is closed. */
export function close(registry, conn) {
  if (foreign(registry, conn)) return failed("Foreign");
  shut(conn);
  return outcome({});
}

export const error = (o) => o.error;
export const message = (o) => o.message;
export const connection = (o) => o.conn;
export const bytes = (o) => o.bytes;
