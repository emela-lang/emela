// The Emela JS runtime (the pure subset of 0.20).
//
// Generated code only uses names starting with `$`. Emela identifiers never contain `$`, so they cannot clash.
// For the value representation, see crates/emela-codegen-js/NOTES.md.
// When inlined, the leading `export ` of each line is stripped, so write `export` only at the start of a line.

// A defect (spec 16.5). An unrecoverable error, thrown with its own class.
export class $Defect extends Error {
  constructor(message) {
    super(message);
    this.name = "Defect";
  }
}

export function $panic(message) {
  throw new $Defect(message);
}

export function $unreachable() {
  throw new $Defect("unreachable: no match arm matched");
}

// A failure (spec chapter 7). Errors are returned as values (16.5). `fail e` returns `new $Fail(e)`,
// and if the result of a fallible call is a `$Fail`, it is passed on to the caller or to an escape.
// `error` is the error value, and its `$tag` holds a per-error marker (`{ show }`).
//
// The entry module and the test runner do not load the $Fail class, so they recognize it by a `Symbol.for` key.
// The function under the key returns the display of an unhandled error (`NotFound(id: 1)`).
export class $Fail {
  constructor(error) {
    this.error = error;
  }

  [Symbol.for("emela.fail")]() {
    return this.error.$tag.show(this.error);
  }
}

// Turns an exception thrown by an external function (spec 12.5) into a defect. A defect (raised inside an Emela
// callback) is kept as is. The message is prefixed with the exception's name (`TypeError: …`).
export function $foreign(error) {
  if (error instanceof Error) {
    return error.name === "Defect" ? error : new $Defect(`${error.name}: ${error.message}`);
  }
  return new $Defect(`uncaught exception: ${String(error)}`);
}

// Called an external function with no `@external` for the build target (one not reachable from the entry).
export function $noExternal(name) {
  throw new $Defect(`external function \`${name}\` is not available on this target`);
}

// Replaces unpaired surrogates in a string received from the host with the replacement character (spec 12.5).
export function $wellFormed(s) {
  if (typeof s !== "string") return s;
  if (typeof s.toWellFormed === "function") return s.toWellFormed();
  return s.replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, "\uFFFD");
}

// ---- Suspension and resumption (spec 8.4, 14.5, 16.5) ----
//
// Suspending functions are generators, and suspending calls are chained with `yield*`. A call to a suspend fn
// operation is `yield $perform(op, args)` and an `@external suspend fn` is `yield $await(promise)`; the request is
// sent up to the driver ($start) that runs the outermost generator. The driver makes a resume per request and calls the operation's implementation.
// If resume is called right away inside the implementation, execution continues without suspending (14.5, item 3). If it is called later,
// the continuation runs inside that call.

class $Perform {
  constructor(op, args) {
    this.op = op;
    this.args = args;
  }
}

class $Await {
  constructor(promise) {
    this.promise = promise;
  }
}

export function $perform(op, args) {
  return new $Perform(op, args);
}

export function $await(promise) {
  return new $Await(promise);
}

// Attaches the handler and operation names to a suspend fn operation implementation `f` (used in the defect message for a second resume).
export function $suspendOp(handler, op, f) {
  f.$where = `operation \`${op}\` of handler \`${handler}\``;
  return f;
}

const $GeneratorPrototype = Object.getPrototypeOf(function* () {}).prototype;

function $isGenerator(x) {
  return typeof x === "object" && x !== null && $GeneratorPrototype.isPrototypeOf(x);
}

// When a suspending call turns out to call a non-suspending function (or fn operation), return its value as is.
export function* $maybe(result) {
  return $isGenerator(result) ? yield* result : result;
}

// One run (main or a test function). Calls `done(ok, value)` exactly once.
// Continuations left after it ends (discarded without resume, or resumed later) are not run (14.5).
class $Root {
  constructor(done) {
    this.done = done;
    this.over = false;
  }

  finish(ok, value) {
    if (this.over) return;
    this.over = true;
    this.done(ok, value);
  }
}

// Runs the generator `gen` that called a suspending function to completion. `done(ok, value)` is called with
// `(true, return value)` when it finishes and `(false, exception)` on a defect. It is not called if it stays suspended without resume.
// Returns a stop function; calling it discards all later continuations.
export function $start(gen, done) {
  const root = new $Root(done);
  $step(root, gen, (ok, value) => root.finish(ok, value), "next", undefined);
  return () => {
    root.over = true;
  };
}

// Advances `gen` with `mode` ("next" or "throw") and `input` until it next suspends or finishes.
// When it finishes, calls `end(ok, value)`.
function $step(root, gen, end, mode, input) {
  for (;;) {
    if (root.over) return;
    let r;
    try {
      r = mode === "next" ? gen.next(input) : gen.throw(input);
    } catch (error) {
      end(false, error);
      return;
    }
    if (r.done) {
      end(true, r.value);
      return;
    }
    const request = r.value;
    if (request instanceof $Await) {
      // A host exception (rejection) becomes a defect and is thrown at the waiting point (12.5).
      Promise.resolve(request.promise).then(
        (value) => $step(root, gen, end, "next", value),
        (error) => $step(root, gen, end, "throw", $foreign(error)),
      );
      return;
    }
    const now = $performOp(root, gen, end, request);
    if (now === undefined) return;
    mode = now.mode;
    input = now.value;
  }
}

// Calls a suspend fn operation's implementation with a fresh resume as the last argument. Returns what to pass to the continuation
// right away (`{ mode, value }`) if any, or undefined if it suspended.
//
// What is passed to the continuation is exactly one of: the resumed value, a failure of the implementation (`$Fail`, which becomes the
// value of the operation call), or a defect, whichever comes first. A failure of the implementation after resume becomes a defect.
function $performOp(root, gen, end, request) {
  const where = request.op?.$where ?? "a suspend operation";
  let sync = true;
  let now;
  let used = false;
  const pass = (mode, value) => {
    used = true;
    if (sync) now = { mode, value };
    else $step(root, gen, end, mode, value);
  };
  const resume = (value) => {
    if (root.over) return undefined;
    if (used) throw new $Defect(`resume called twice in ${where}`);
    pass("next", value);
    return undefined;
  };
  const settled = (ok, value) => {
    if (!ok) {
      if (used) root.finish(false, value);
      else pass("throw", value);
    } else if (value instanceof $Fail) {
      if (used) root.finish(false, new $Defect(`${where} failed after resume: ${value[Symbol.for("emela.fail")]()}`));
      else pass("next", value);
    }
  };
  try {
    const result = request.op(...request.args, resume);
    // A suspending implementation (a generator) runs as a separate continuation within the same run.
    if ($isGenerator(result)) $step(root, result, settled, "next", undefined);
    else settled(true, result);
  } catch (error) {
    settled(false, error);
  }
  sync = false;
  return now;
}

// A failed assert (spec 13.2). `text` is the source text of the expression and may be empty.
export function $assertFail(text) {
  throw new $Defect(text === "" ? "assertion failed" : `assertion failed: ${text}`);
}

// A failed comparison assert. Both sides are received as displayed strings (displayed only on failure).
export function $assertCmp(text, op, left, right) {
  const head = text === "" ? `left ${op} right` : text;
  throw new $Defect(`assertion failed: ${head}\n  left: ${left}\n right: ${right}`);
}

// Division and remainder on Int (32-bit). Truncates toward zero; the remainder takes the sign of the dividend.
// `| 0` wraps the 2^31 / -1 overflow and turns -0 into 0.
export function $idiv(a, b) {
  if (b === 0) throw new $Defect("division by zero");
  return (a / b) | 0;
}

export function $irem(a, b) {
  if (b === 0) throw new $Defect("division by zero");
  return (a % b) | 0;
}

// Division and remainder on Int64 (BigInt). BigInt `/` truncates toward zero.
export function $ldiv(a, b) {
  if (b === 0n) throw new $Defect("division by zero");
  return BigInt.asIntN(64, a / b);
}

export function $lrem(a, b) {
  if (b === 0n) throw new $Defect("division by zero");
  return a % b;
}

// A List is a chain of cons cells `{ h, t }`, and the empty list is `null`.
export function $list(items, tail = null) {
  let list = tail;
  for (let i = items.length - 1; i >= 0; i--) list = { h: items[i], t: list };
  return list;
}

// Structural equality. Walks with an explicit stack so long lists do not use up the call stack.
export function $eq(a, b) {
  const stack = [a, b];
  while (stack.length > 0) {
    const y = stack.pop();
    const x = stack.pop();
    if (x === y) continue;
    if (typeof x !== "object" || typeof y !== "object" || x === null || y === null) {
      // NaN is not equal to NaN (as in IEEE).
      return false;
    }
    if (Array.isArray(x)) {
      if (!Array.isArray(y) || x.length !== y.length) return false;
      for (let i = 0; i < x.length; i++) stack.push(x[i], y[i]);
      continue;
    }
    const kx = Object.keys(x);
    if (kx.length !== Object.keys(y).length) return false;
    for (const k of kx) {
      if (!Object.hasOwn(y, k)) return false;
      stack.push(x[k], y[k]);
    }
  }
  return true;
}

// Strings are ordered lexicographically by code point. Returns a negative number, 0, or a positive number.
export function $scmp(a, b) {
  const xs = a[Symbol.iterator]();
  const ys = b[Symbol.iterator]();
  for (;;) {
    const x = xs.next();
    const y = ys.next();
    if (x.done || y.done) return (x.done ? 0 : 1) - (y.done ? 0 : 1);
    const d = x.value.codePointAt(0) - y.value.codePointAt(0);
    if (d !== 0) return d;
  }
}

// The Prelude's `todo()`.
export function $todo() {
  throw new $Defect("not yet implemented");
}

// The Prelude's `dbg(value)`. `show` is the display function generated from the value's type.
export function $dbg(value, show) {
  console.error(show(value));
  return value;
}

// The Prelude's Option (`Some(A)` is index 0, `None` is index 1).
export const $None = Object.freeze({ $tag: 1 });

// ---- String (spec 16.2) ----

let $segmenter;

function $graphemes(s) {
  $segmenter ??= new Intl.Segmenter(undefined, { granularity: "grapheme" });
  return $segmenter.segment(s);
}

// String.length is the number of grapheme clusters.
export function $strlen(s) {
  let n = 0;
  for (const _ of $graphemes(s)) n++;
  return n;
}

// String.byte_size is the number of bytes when encoded as UTF-8.
export function $strbytes(s) {
  let n = 0;
  for (const c of s) {
    const p = c.codePointAt(0);
    n += p < 0x80 ? 1 : p < 0x800 ? 2 : p < 0x10000 ? 3 : 4;
  }
  return n;
}

export function $strcat(a, b) {
  return a + b;
}

// Substring matching works on sequences of code points (grapheme boundaries are ignored).
export function $strhas(s, sub) {
  return s.includes(sub);
}

export function $strstarts(s, prefix) {
  return s.startsWith(prefix);
}

export function $strends(s, suffix) {
  return s.endsWith(suffix);
}

// String.chars is a list of strings, one per grapheme cluster.
export function $strchars(s) {
  const items = [];
  for (const { segment } of $graphemes(s)) items.push(segment);
  return $list(items);
}

// An empty separator splits per grapheme (like chars). JS split("") splits per UTF-16 unit, so it is not used.
export function $strsplit(s, sep) {
  if (sep === "") return $strchars(s);
  return $list(s.split(sep));
}

export function $strjoin(parts, sep) {
  let out = "";
  for (let p = parts; p !== null; p = p.t) {
    if (p !== parts) out += sep;
    out += p.h;
  }
  return out;
}

// Whitespace is the 25 characters with the Unicode White_Space property. JS trim includes U+FEFF and excludes U+0085, so it is not used.
const $ws = "[\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]";
const $trimRe = new RegExp(`^${$ws}+|${$ws}+$`, "g");

export function $strtrim(s) {
  return s.replace($trimRe, "");
}

// String.code_points is a list of code point values. Iterating over a string advances per code point.
export function $strcodepoints(s) {
  const items = [];
  for (const c of s) items.push(c.codePointAt(0));
  return $list(items);
}

// None unless it is a Unicode scalar value (0 to 10FFFF, excluding surrogates).
export function $strfromcodepoint(n) {
  if (n < 0 || n > 0x10ffff || (n >= 0xd800 && n <= 0xdfff)) return $None;
  return { $tag: 0, $0: String.fromCodePoint(n) };
}

// ---- Numeric conversions (spec 16.1) ----

export function $itof(n) {
  return n;
}

export function $itol(n) {
  return BigInt(n);
}

// Int64 to Int wraps to the low 32 bits.
export function $ltoi(n) {
  return Number(BigInt.asIntN(32, n));
}

export function $icheckedAdd(a, b) {
  const r = a + b;
  return r > 2147483647 || r < -2147483648 ? $None : { $tag: 0, $0: r };
}

// Float to Int. A defect if the rounded result is out of Int range or NaN. `| 0` turns -0 into 0.
function $ftoi(x, r) {
  if (!(r >= -2147483648 && r <= 2147483647)) {
    throw new $Defect(`cannot convert ${$showFloat(x)} to Int`);
  }
  return r | 0;
}

export function $ffloor(x) {
  return $ftoi(x, Math.floor(x));
}

export function $fceil(x) {
  return $ftoi(x, Math.ceil(x));
}

// 0.5 rounds away from zero. Math.round rounds toward positive infinity, so negative numbers are rounded without their sign.
export function $fround(x) {
  return $ftoi(x, x < 0 ? -Math.round(-x) : Math.round(x));
}

export function $ftrunc(x) {
  return $ftoi(x, Math.trunc(x));
}

// ---- Bitwise operations (spec 6.6) ----
// A negative shift amount is a defect; an amount at or above the width (32 or 64) shifts out every bit.
// JS `<<` and friends only look at the low 5 bits of the shift amount, so out-of-range amounts are handled first.

function $shiftBy(by) {
  if (by < 0) throw new $Defect(`negative shift amount: ${by}`);
  return by;
}

export function $iand(a, b) {
  return a & b;
}

export function $ior(a, b) {
  return a | b;
}

export function $ixor(a, b) {
  return a ^ b;
}

export function $inot(a) {
  return ~a;
}

export function $ishl(a, by) {
  return $shiftBy(by) >= 32 ? 0 : a << by;
}

export function $ishr(a, by) {
  return a >> Math.min($shiftBy(by), 31);
}

export function $iushr(a, by) {
  return $shiftBy(by) >= 32 ? 0 : (a >>> by) | 0;
}

// Int64 is a BigInt. and / or / xor / not act on two's complement, so no wrapping is needed.
export function $land(a, b) {
  return a & b;
}

export function $lor(a, b) {
  return a | b;
}

export function $lxor(a, b) {
  return a ^ b;
}

export function $lnot(a) {
  return ~a;
}

export function $lshl(a, by) {
  return $shiftBy(by) >= 64 ? 0n : BigInt.asIntN(64, a << BigInt(by));
}

export function $lshr(a, by) {
  return a >> BigInt(Math.min($shiftBy(by), 63));
}

export function $lushr(a, by) {
  return $shiftBy(by) >= 64 ? 0n : BigInt.asIntN(64, BigInt.asUintN(64, a) >> BigInt(by));
}

// ---- Display (a provisional Show implementation, spec 10.6) ----
// Display functions determined by type are built by the generated code. This holds the ones for primitive types, lists, and tuples.

export function $showInt(n) {
  return String(n);
}

export function $showInt64(n) {
  return String(n);
}

export function $showBool(b) {
  return b ? "True" : "False";
}

// A finite Float with an integral value gets a decimal point, like `1.0`, so it can be told apart from an Int.
export function $showFloat(x) {
  const s = String(x);
  return Number.isFinite(x) && /^-?\d+$/.test(s) ? s + ".0" : s;
}

export function $showUnit(_) {
  return "()";
}

// Strings are shown as written in source. Escapes are `\"`, `\\`, `\n`, and `\#` (only before `#{`).
// Control characters with no escape in the language are shown as `\t`, `\r`, or `\u{…}` (undecided in spec 18.1 #11).
const $strEscRe = /["\\]|#(?=\{)|[\u0000-\u001f\u007f]/g;

export function $showString(s) {
  const body = s.replace($strEscRe, (c) => {
    switch (c) {
      case '"':
        return '\\"';
      case "\\":
        return "\\\\";
      case "#":
        return "\\#";
      case "\n":
        return "\\n";
      case "\t":
        return "\\t";
      case "\r":
        return "\\r";
      default:
        return `\\u{${c.codePointAt(0).toString(16)}}`;
    }
  });
  return `"${body}"`;
}

export function $showList(xs, show) {
  const items = [];
  for (let p = xs; p !== null; p = p.t) items.push(show(p.h));
  return `[${items.join(", ")}]`;
}

export function $showTuple(t, shows) {
  return `(${t.map((x, i) => shows[i](x)).join(", ")})`;
}
