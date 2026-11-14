// Host functions for SystemArgs, SystemStdin, SystemStdout and SystemStderr (process.emel) on JS.

// ---- Args ----

// The launcher of `emela run` puts the arguments after the entry module here before it imports
// the entry. How many leading elements of `process.argv` are the host and the script depends on
// the host and on how it was started, so the launcher, which knows, slices them. Nothing sets it
// under `emela test` or in the server of an HTTP service, where the list is empty.
const ARGS = Symbol.for("emela.args");

let args;

// Copied once, so that every call sees the same list. Most JS hosts have already turned the
// arguments into strings, replacing invalid UTF-8 with U+FFFD.
const argList = () => (args ??= Object.freeze(Array.from(globalThis[ARGS] ?? [], String)));

export const argCount = () => argList().length;

export const argAt = (index) => argList()[index];

// ---- Stdin ----
//
// The bytes received from `process.stdin` are kept in `bytes[start..end]` until an operation takes
// them. `process.stdin` is touched only when an operation first waits (touching it keeps a program
// on a terminal from ending), and it is paused and unreferenced whenever no operation waits, so
// that it does not keep the event loop alive. A waiting operation that is aborted (its fiber was interrupted) takes
// nothing, so what was received stays for the next operation.

let bytes = new Uint8Array(0);
let start = 0;
let end = 0;
let ended = false;
// The message of a read error of the host, until an operation reports it.
let failure = null;
let listening = false;
// The waiting operations: `{ line, wake }`. `line` is true for `waitLine`.
const waiters = new Set();
// The text or the message of the last take.
let last = "";

// `fatal` rejects invalid UTF-8 instead of replacing it; `ignoreBOM` keeps a leading BOM.
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

const NEWLINE = 0x0a;
const RETURN = 0x0d;

function append(chunk) {
  if (end + chunk.length > bytes.length) {
    const kept = end - start;
    const grown = new Uint8Array(Math.max(kept + chunk.length, bytes.length * 2, 4096));
    grown.set(bytes.subarray(start, end));
    bytes = grown;
    start = 0;
    end = kept;
  }
  bytes.set(chunk, end);
  end += chunk.length;
}

const hasLine = () => bytes.subarray(start, end).includes(NEWLINE);

const ready = (line) => ended || failure !== null || (line && hasLine());

function wake() {
  for (const waiter of waiters) {
    if (ready(waiter.line)) {
      waiters.delete(waiter);
      waiter.wake();
    }
  }
  if (waiters.size === 0 && listening) idle();
}

// No operation waits: stop reading, and let the program end even if the input is still open.
// `pause` alone keeps a pipe or a terminal open on Node; `unref` lets it go (a file has no
// `unref`, and its reads end by themselves).
function idle() {
  process.stdin.pause();
  process.stdin.unref?.();
}

function busy() {
  process.stdin.ref?.();
  process.stdin.resume();
}

function listen() {
  if (listening) return;
  listening = true;
  const stdin = process.stdin;
  stdin.on("data", (chunk) => {
    append(typeof chunk === "string" ? new TextEncoder().encode(chunk) : chunk);
    wake();
  });
  stdin.on("end", () => {
    ended = true;
    wake();
  });
  stdin.on("error", (error) => {
    failure = error instanceof Error ? error.message : String(error);
    wake();
  });
}

function wait(line, signal) {
  if (ready(line)) return undefined;
  listen();
  return new Promise((resolve) => {
    const waiter = { line, wake: resolve };
    waiters.add(waiter);
    busy();
    signal?.addEventListener(
      "abort",
      () => {
        waiters.delete(waiter);
        if (waiters.size === 0) idle();
      },
      { once: true },
    );
  });
}

export const waitLine = (signal) => wait(true, signal);

export const waitEnd = (signal) => wait(false, signal);

// Decodes `bytes[from..to]` into `last`: 0, or 2 when it is not valid UTF-8.
function decode(from, to) {
  try {
    last = decoder.decode(bytes.subarray(from, to));
    return 0;
  } catch {
    last = "";
    return 2;
  }
}

// Reports the read error once; the input ends after it.
function failed() {
  last = failure;
  failure = null;
  ended = true;
  start = end;
  return 3;
}

export function takeLine() {
  const at = bytes.subarray(start, end).indexOf(NEWLINE);
  if (at >= 0) {
    const from = start;
    const to = start + at;
    start = to + 1;
    return decode(from, to > from && bytes[to - 1] === RETURN ? to - 1 : to);
  }
  if (failure !== null) return failed();
  if (!ended) return 4;
  if (start === end) {
    last = "";
    return 1;
  }
  const from = start;
  start = end;
  return decode(from, end);
}

export function takeText() {
  if (failure !== null) return failed();
  if (!ended) return 4;
  const from = start;
  start = end;
  return decode(from, end);
}

export const taken = () => last;

// ---- Stdout and Stderr ----
//
// The runtime writes, through the same path as SystemConsole and `dbg`, and handles a closed descriptor
// (`$output` in the runtime). The runtime registers it under this key when it is loaded, before any operation runs.

const OUTPUT = Symbol.for("emela.output");

// Without the runtime (an output that calls nothing of it), the host's console.
function write(fd, text) {
  const output = globalThis[OUTPUT];
  if (output !== undefined) output.write(fd, text);
  else if (fd === 2) console.error(text.endsWith("\n") ? text.slice(0, -1) : text);
  else console.log(text.endsWith("\n") ? text.slice(0, -1) : text);
}

export const writeStdout = (text) => write(1, text);

export const writeStderr = (text) => write(2, text);

// ---- ExitCode ----

// The exit code of a `main` that returns an `ExitCode` (`exit_with` in process.emel). The launcher
// leaves `process.exitCode` alone when main ends with a value, so the host exits with this code
// after the writes have finished.
export const setExitCode = (code) => {
  process.exitCode = code;
};
