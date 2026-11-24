// The Emela JS runtime (the pure subset of 0.20).
//
// Generated code only uses names starting with `$`. Emela identifiers never contain `$`, so they cannot clash.
// For the value representation, see the crate docs (crates/emela-codegen-js/src/lib.rs).
// When inlined, the leading `export ` of each line is stripped, so write `export` only at the start of a line.

// A defect (spec 16.5). An unrecoverable error, thrown with its own class.
// `location` is where a failed assert is (`path:line:column`, spec 13.2), kept outside the message for the test
// runner; undefined for other defects.
export class $Defect extends Error {
  constructor(message, location) {
    super(message);
    this.name = "Defect";
    if (location !== undefined) this.location = location;
  }
}

export function $panic(message) {
  throw new $Defect(message);
}

export function $unreachable() {
  throw new $Defect("unreachable: no match arm matched");
}

// The value of a const (spec 5.6), evaluated by `init` on the first reference and remembered. `cell.state` is 0 (not
// evaluated yet), 1 (being evaluated), 2 (`cell.value` is the value) or 3 (`cell.value` is the defect the evaluation
// raised). A defect is remembered like a value: the value is pure, so evaluating again would raise it again. Its
// message names the const. Referring to a const while it is being evaluated is a defect; that defect keeps its
// message and is not remembered by any const it passes through (each of them would raise it again anyway).
export function $constant(cell, name, init) {
  switch (cell.state) {
    case 2:
      return cell.value;
    case 3:
      throw cell.value;
    case 1: {
      const defect = new $Defect(`const ${name} refers to itself while being evaluated`);
      defect.$selfReference = true;
      throw defect;
    }
  }
  cell.state = 1;
  try {
    const value = init();
    cell.value = value;
    cell.state = 2;
    return value;
  } catch (error) {
    if (error?.name !== "Defect" || error.$selfReference) {
      cell.state = 0;
      throw error;
    }
    // The remembered defect keeps the location of a failed assert.
    const defect = new $Defect(`while evaluating const ${name}: ${error.message}`, error.location);
    cell.value = defect;
    cell.state = 3;
    throw defect;
  }
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

  // Notes added when other fibers failed too (11.2) follow the display, one per line.
  [Symbol.for("emela.fail")]() {
    const shown = this.error.$tag.show(this.error);
    return this.notes === undefined ? shown : [shown, ...this.notes].join("\n");
  }
}

// Turns an exception thrown by an external function (spec 12.5) into a defect. A defect (raised inside an Emela
// callback) is kept as is, and so is the runtime's own interruption, which a write of the bundled std raises on a
// closed standard output (`$output`). The message is prefixed with the exception's name (`TypeError: …`).
export function $foreign(error) {
  if (error instanceof $Interrupted) return error;
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
// operation is `yield $perform(op, args)`, an `@external suspend fn` is `yield $await(promise)`, and a fiber
// operation that waits (join, Deferred.await, ...) is `yield` of a `$FiberRequest`; the request is sent up to the
// scheduler ($step) that runs the fiber's outermost generator. The scheduler makes a resume per request and calls the
// operation's implementation. If resume is called right away inside the implementation, execution continues
// without suspending (14.5, item 3). If it is called later, the continuation runs inside that call.

class $Perform {
  constructor(op, args) {
    this.op = op;
    this.args = args;
  }
}

class $Await {
  constructor(promise, controller) {
    this.promise = promise;
    // The AbortController whose signal was passed to the external function (12.2); aborted when the waiting
    // continuation is dropped.
    this.controller = controller;
  }
}

// A yield for fairness (11.1): the fiber goes to the end of the queue.
const $PAUSE = Object.freeze({});

// A request of a fiber operation (chapter 11). `run(fiber, wake, wait)` returns what to pass to the waiting fiber
// right away (`{ mode, value }`), or undefined to suspend it until `wake(mode, value)` is called. A `closing` request
// (a scope waiting for its children) is not cut short when the run is unwound.
//
// `wake("served", value)` continues with `value` even if an interruption arrived after the wake: the fiber was given
// what it waited for (permits, a value of a queue) and no longer waits, so the interruption takes effect at its next
// suspension point instead, and nothing it was given is lost (11.9, 11.10). A request that keeps the fiber in a line
// sets `wait.onDrop` to leave the line when the fiber is interrupted while it waits.
class $FiberRequest {
  constructor(run, closing = false) {
    this.run = run;
    this.closing = closing;
  }
}

export function $perform(op, args) {
  return new $Perform(op, args);
}

export function $await(promise, controller) {
  return new $Await(promise, controller);
}

// A controller for the AbortSignal passed as the last argument of a suspend fn external function (12.2).
export function $abortable() {
  return new AbortController();
}

// ---- Fairness (11.1) ----
//
// Every suspending call in a suspending function spends one unit of the running fiber's budget
// (`$spend() && (yield $pause())`). When it runs out, the fiber yields: it goes to the end of the queue, and every
// `$HOST_EVERY`th yield of a run goes through the host's event loop instead, so host callbacks (timers, I/O) run
// too. The count does not depend on time, so the order of the fibers is the same on every run.
const $BUDGET = 1000;
const $HOST_EVERY = 16;

const $later = typeof setImmediate === "function" ? setImmediate : (f) => setTimeout(f, 0);

export function $spend() {
  const fiber = $current;
  return fiber !== null && --fiber.budget <= 0;
}

export function $pause() {
  return $PAUSE;
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

// ---- The scheduler (spec chapter 11, 14.5) ----
//
// A run (main or a test function) is a `$Root`, a scheduler of fibers on one thread. Every fiber is advanced by
// `$step`, and only one runs at a time: a fiber woken by another (a child that ended, a completed Deferred) is put
// on the queue and runs after the running one suspends. Tasks run in the order they were queued, so the order of
// switches is the same on every run. A resume called from a host callback queues the continuation and drains the
// queue inside that call, so the continuation runs inside the resume (14.5, item 4).
//
// Fairness (11.1) is the `$pause` request above: the fiber goes back to the end of the queue.

// The fiber being stepped, or null outside the scheduler (in host callbacks).
let $current = null;

// The runs that have not ended: the ones a closed standard output interrupts (`$output`).
const $roots = new Set();

class $Root {
  constructor(done) {
    this.done = done;
    this.over = false;
    $roots.add(this);
    // Tasks (functions) waiting to run, and whether the queue is being drained.
    this.queue = [];
    this.running = false;
    // Continuations waiting for a resume, a promise or a fiber request (`{ gen, end, fiber, live, closing }`), in
    // the order they started waiting.
    this.waits = new Set();
    // Fibers that have not ended.
    this.fibers = new Set();
    // A run of `$sync`: it never waits, so yields for fairness continue at once.
    this.sync = false;
    // The number of yields for fairness so far.
    this.pauses = 0;
    // Waiting for the run to stall (`$whenStalled`): `{ at, seq, resolve }`.
    this.sleepers = [];
    this.sleeperSeq = 0;
  }

  // Called when nothing can go on: no fiber is runnable and the host has nothing pending (the event loop is empty).
  // Wakes the sleeper with the earliest time (the earliest call among equal times), which is how TestClock moves
  // its time forward (8.8). With no sleeper, unwinds the run as interrupted.
  stalled() {
    if (this.over) return;
    if (this.sleepers.length === 0) {
      this.interrupt();
      return;
    }
    let first = 0;
    for (let i = 1; i < this.sleepers.length; i++) {
      const a = this.sleepers[i];
      const b = this.sleepers[first];
      if (a.at < b.at || (a.at === b.at && a.seq < b.seq)) first = i;
    }
    const [sleeper] = this.sleepers.splice(first, 1);
    // Through a macrotask: the host does not report a stall again after a handler that only settled promises.
    $later(() => sleeper.resolve());
  }

  finish(ok, value) {
    if (this.over) return;
    this.over = true;
    $roots.delete(this);
    this.queue = [];
    // Stops what the continuations that will not be resumed still wait for (timers, requests), so the host can
    // exit.
    for (const wait of this.waits) wait.controller?.abort();
    this.waits.clear();
    this.sleepers = [];
    this.done(ok, value);
  }

  // The root fiber (main or the test function) ended.
  rootEnded(ok, value) {
    this.finish(ok, value);
  }

  schedule(task) {
    this.queue.push(task);
    if (!this.running) this.drain();
  }

  drain() {
    this.running = true;
    try {
      while (!this.over && this.queue.length > 0) this.queue.shift()();
    } finally {
      this.running = false;
    }
  }

  // Unwinds the run as interrupted (14.5): every fiber is interrupted, and every waiting continuation is thrown
  // `$Interrupted` at once, which runs the releases of its open withs from the inside (the `finally` blocks). This
  // also cuts short releases left suspended, which are otherwise not interrupted (11.4). Only scopes waiting for
  // their children keep waiting, since the children are unwound too. A release may suspend again; if it is resumed
  // the unwinding goes on, and if it is left suspended, calling this again cuts it short and goes on outward.
  interrupt() {
    if (this.over) return;
    for (const fiber of this.fibers) {
      if (!fiber.unwinding && !fiber.pending.includes(null)) fiber.pending.push(null);
    }
    const waits = [...this.waits].reverse().filter((wait) => !wait.closing);
    for (const wait of waits) {
      // An operation's implementation waits only before it resumes (after resume it does not suspend, E0461), and
      // is unwound by dropping its caller (`onDrop`).
      if (!wait.live || wait.end !== wait.fiber.end) continue;
      $drop(this, wait);
      const fiber = wait.fiber;
      this.queue.push(() => $step(this, fiber, wait.gen, wait.end, "throw", fiber.deliver()));
    }
    if (!this.running) this.drain();
  }
}

// A fiber (11.1). The root fiber runs main or a test function; the others are forked into a scope.
class $Fiber {
  constructor(root, scope) {
    this.root = root;
    // The `$Scope` it was forked into; null for the root fiber.
    this.scope = scope;
    // The main continuation's wait while it is suspended.
    this.wait = null;
    // Interruptions not yet delivered: null (the whole fiber) or a `$Scope` whose body runs on this fiber.
    this.pending = [];
    // An interruption of the whole fiber has been delivered; it is unwinding.
    this.unwinding = false;
    // Scopes whose body is unwinding by an interruption delivered to this fiber (until the scope receives it).
    this.unwinds = new Set();
    // Above 0 while it runs a release or a scope waits for its children; interruptions wait until then (11.4).
    this.holds = 0;
    // The number of scopes whose body runs on this fiber (the depth of the next one).
    this.scopes = 0;
    // The run-time scope of a background whose release waits for its tasks on this fiber.
    this.waiting = null;
    // Once it ended: `{ ok, value }`, and the cause (a `$Cause`, or null when it returned a value).
    this.result = undefined;
    this.cause = null;
    // Called once when it ends.
    this.watchers = [];
    // Suspending calls left before it yields for fairness (11.1).
    this.budget = $BUDGET;
    this.end = (ok, value) => this.finish(ok, value);
    root.fibers.add(this);
  }

  // Calls the forked function `f` and steps what it returns.
  begin(f) {
    if (this.pending.length > 0) {
      this.finish(false, new $Interrupted());
      return;
    }
    const prev = $current;
    $current = this;
    let r;
    try {
      r = f();
    } catch (error) {
      this.finish(false, error);
      return;
    } finally {
      $current = prev;
    }
    if ($isGenerator(r)) $step(this.root, this, r, this.end, "next", undefined);
    else this.finish(true, r);
  }

  finish(ok, value) {
    if (this.result !== undefined) return;
    this.result = { ok, value };
    this.cause = ok
      ? value instanceof $Fail
        ? new $Cause($CAUSE_FAIL, value)
        : null
      : $isInterrupted(value)
        ? new $Cause($CAUSE_INTERRUPT, undefined)
        : new $Cause($CAUSE_DIE, value);
    this.wait = null;
    this.root.fibers.delete(this);
    if (this.scope === null) {
      this.root.rootEnded(ok, value);
      return;
    }
    this.scope.childEnded(this);
    const watchers = this.watchers;
    this.watchers = [];
    for (const watch of watchers) watch();
  }

  // Asks the fiber to unwind as interrupted (11.4). `target` is null for the whole fiber, or a scope whose body
  // runs on this fiber (only that body unwinds). It takes effect at the fiber's next suspension point, and not while
  // it runs a release.
  interrupt(target) {
    if (this.result !== undefined || this.unwinding) return;
    if (target === null ? this.pending.includes(null) : this.pending.includes(target) || this.unwinds.has(target)) {
      return;
    }
    this.pending.push(target);
    // The release of a background that waits for its tasks passes the interruption on to them (11.3).
    if (this.waiting !== null) this.waiting.interruptedWhileWaiting();
    if (this.holds > 0) return;
    const wait = this.wait;
    if (wait === null || !wait.live) return;
    $drop(this.root, wait);
    this.root.schedule(() => $step(this.root, this, wait.gen, wait.end, "throw", this.deliver()));
  }

  // Whether an interruption is to be delivered at the next suspension point: one is pending, or one delivered
  // earlier is still unwinding (code that caught a failure on the way, such as an escape of a `$Fail` an inner
  // scope returned, is unwound again).
  interrupting() {
    return this.holds === 0 && (this.pending.length > 0 || this.unwinding || this.unwinds.size > 0);
  }

  // Whether an interruption delivered to this fiber is unwinding it.
  isUnwinding() {
    return this.unwinding || this.unwinds.size > 0;
  }

  // Takes the outermost interruption (pending or unwinding) and returns the exception that unwinds it.
  deliver() {
    let target = null;
    if (!this.unwinding && !this.pending.includes(null)) {
      for (const scope of [...this.pending, ...this.unwinds]) {
        if (target === null || scope.depth < target.depth) target = scope;
      }
    }
    if (target === null) {
      this.unwinding = true;
      this.pending = [];
      this.unwinds.clear();
    } else {
      this.pending = this.pending.filter((scope) => scope !== target);
      this.unwinds.add(target);
    }
    return new $Interrupted(target);
  }

  // Continues the main continuation queued with `mode` and `value`. If an interruption arrived in the meantime, it is
  // delivered instead, unless the fiber was `served` (`$FiberRequest`).
  resume(gen, end, mode, value, served = false) {
    if (!served && end === this.end && mode !== "throw" && this.interrupting()) {
      mode = "throw";
      value = this.deliver();
    }
    $step(this.root, this, gen, end, mode, value);
  }

  // Calls `settled()` once the fiber has ended: right away (returning its result) or later (passing its result to
  // `wake`). `settled` returns `{ mode, value }`.
  watch(wake, settled) {
    if (this.result !== undefined) return settled();
    this.watchers.push(() => {
      const now = settled();
      wake(now.mode, now.value);
    });
    return undefined;
  }
}

// Runs the generator `gen` that called a suspending function to completion. `done(ok, value)` is called with
// `(true, return value)` when it finishes and `(false, exception)` on a defect. It is not called while it stays
// suspended without resume.
// Returns the function the launcher calls when the event loop runs dry (`$Root#stalled`). It wakes the earliest
// TestClock sleeper if any. Otherwise, if the run has not ended, it unwinds the run as interrupted, running the
// releases of the open withs (`$Root#interrupt`), and then `done(false, …)` is called with `$Interrupted` (or a
// defect a release raised).
export function $start(gen, done) {
  const root = new $Root(done);
  const main = new $Fiber(root, null);
  root.schedule(() => $step(root, main, gen, main.end, "next", undefined));
  return () => root.stalled();
}

// The stall functions (`$start`'s result) of the launches of exported functions that have not ended.
const $exportLaunches = new Set();
let $exportWatching = false;

// Runs the generator `gen` of an exported function that may suspend as one launch (16.6.2), and returns a Promise of
// its value. The Promise rejects with the defect or the interruption the launch ends with. When the host reports that
// its event loop is empty (`beforeExit`, where the host has one), the launches that have not ended are stalled, as
// the launcher does for main.
export function $export(gen) {
  return new Promise((resolve, reject) => {
    let ended = false;
    let stalled;
    stalled = $start(gen, (ok, value) => {
      ended = true;
      $exportLaunches.delete(stalled);
      if (ok) resolve(value);
      else reject(value);
    });
    if (ended) return;
    $exportLaunches.add(stalled);
    if (!$exportWatching && typeof globalThis.process?.on === "function") {
      $exportWatching = true;
      globalThis.process.on("beforeExit", () => {
        for (const stall of [...$exportLaunches]) stall();
      });
    }
  });
}

// The `fetch` of an HTTP service (`[lib.http-service]`): runs `launch()` (a `$export` of the entry for `request`) and
// returns a Promise of the response. The Promise resolves when the launch hands over the response (`respond` of
// `http.mjs`), before the launch ends; the end of the launch goes to `ctx.waitUntil` where the host has it (Cloudflare
// Workers). A launch that ends with a defect or an interruption before it answers rejects the Promise; one that ends
// so after it answered writes the line `serve.mjs` writes for a rejected Promise to standard error.
// The function that answers goes on the request under a registered symbol, because `respond` is in another module.
export function $serve(request, ctx, launch) {
  return new Promise((resolve, reject) => {
    let answered = false;
    request[Symbol.for("emela.respond")] = (outgoing) => {
      answered = true;
      resolve(outgoing);
    };
    const end = launch().then(resolve, (e) => {
      if (!answered) return reject(e);
      globalThis.console.error(e?.name === "Interrupted" ? "interrupted" : `defect: ${e?.message ?? e}`);
    });
    if (typeof ctx?.waitUntil === "function") ctx.waitUntil(end);
  });
}

// A continuation unwound while interrupted (11.4, 14.5). Thrown into a fiber when it is interrupted, by `$sync`
// when a suspend operation inside a non-suspending function is not resumed in place, and by `Cause.raise` of an
// Interrupt. It unwinds like a defect, but `Exit.attempt` does not catch it. `scope` is the scope whose body is
// being interrupted, or null when the whole fiber is. The launcher and the test runner report it as `interrupted`.
export class $Interrupted extends Error {
  constructor(scope = null) {
    super("interrupted");
    this.name = "Interrupted";
    this.scope = scope;
  }
}

// Runs a suspending call inside a non-suspending function to completion in place (InstantClock, 8.7).
// When R is Immediate, a suspend operation is either resumed in place or dropped without being
// resumed; if dropped, throws $Interrupted. Returns `result` as is when it is not a generator.
export function $sync(result) {
  if (!$isGenerator(result)) return result;
  let out;
  const root = new $Root((ok, value) => {
    out = { ok, value };
  });
  root.sync = true;
  const fiber = new $Fiber(root, null);
  root.schedule(() => $step(root, fiber, result, fiber.end, "next", undefined));
  // Not resumed in place: unwind it right away, running the releases of its open withs (14.5).
  // Each round cuts short the releases left suspended, so this ends.
  while (out === undefined && root.waits.size > 0) root.interrupt();
  if (out === undefined) {
    root.over = true;
    $roots.delete(root);
    throw new $Interrupted();
  }
  if (!out.ok) throw out.value;
  return out.value;
}

// A call to a suspend fn operation inside a non-suspending function.
export function $syncPerform(op, args) {
  return $sync(
    (function* () {
      return yield $perform(op, args);
    })(),
  );
}

// Advances `gen` (a continuation of `fiber`) with `mode` ("next" or "throw") and `input` until it next suspends or
// finishes. When it finishes, calls `end(ok, value)`. While it is suspended, it is in `root.waits`.
function $step(root, fiber, gen, end, mode, input) {
  const prev = $current;
  $current = fiber;
  try {
    $advance(root, fiber, gen, end, mode, input);
  } finally {
    $current = prev;
  }
}

function $advance(root, fiber, gen, end, mode, input) {
  const main = end === fiber.end;
  for (;;) {
    if (root.over) return;
    let r;
    try {
      r = gen[mode](input);
    } catch (error) {
      end(false, error);
      return;
    }
    if (r.done) {
      end(true, r.value);
      return;
    }
    const request = r.value;
    // An interruption takes effect when the fiber reaches a suspension point, unless it runs a release (11.4).
    if (main && fiber.interrupting()) {
      mode = "throw";
      input = fiber.deliver();
      continue;
    }
    const closing = request instanceof $FiberRequest && request.closing;
    const wait = { gen, end, fiber, live: true, closing, controller: undefined, onDrop: undefined };
    const suspend = () => {
      root.waits.add(wait);
      if (main) fiber.wait = wait;
      else end.track?.(wait);
    };
    // Takes the suspended `gen` back to continue it, unless it was unwound in the meantime (`$Fiber#interrupt`,
    // `$Root#interrupt`).
    const take = () => {
      if (!wait.live) return false;
      wait.live = false;
      root.waits.delete(wait);
      if (fiber.wait === wait) fiber.wait = null;
      return true;
    };
    // Within the same fiber it continues on the spot; otherwise it is queued.
    const cont = (mode, value) => {
      if (!take()) return;
      const served = mode === "served";
      if (served) mode = "next";
      if ($current === fiber) $step(root, fiber, gen, end, mode, value);
      else root.schedule(() => fiber.resume(gen, end, mode, value, served));
    };
    if (request === $PAUSE) {
      fiber.budget = $BUDGET;
      if (root.sync) {
        mode = "next";
        input = undefined;
        continue;
      }
      suspend();
      const go = () => {
        if (take()) fiber.resume(gen, end, "next", undefined);
      };
      if (++root.pauses % $HOST_EVERY === 0) $later(() => root.schedule(go));
      else root.queue.push(go);
      return;
    }
    if (request instanceof $Await) {
      wait.controller = request.controller;
      suspend();
      // A host exception (rejection) becomes a defect and is thrown at the waiting point (12.5).
      Promise.resolve(request.promise).then(
        (value) => cont("next", value),
        (error) => cont("throw", $foreign(error)),
      );
      return;
    }
    const now =
      request instanceof $FiberRequest ? request.run(fiber, cont, wait) : $performOp(root, fiber, cont, request, wait);
    if (now === undefined) {
      suspend();
      return;
    }
    mode = now.mode;
    input = now.value;
  }
}

// Calls a suspend fn operation's implementation with a fresh resume as the last argument. Returns what to pass to the continuation
// right away (`{ mode, value }`) if any, or undefined if it suspended.
//
// What is passed to the continuation is exactly one of: the resumed value, a failure of the implementation (`$Fail`, which becomes the
// value of the operation call), or a defect, whichever comes first. A failure of the implementation after resume becomes a defect.
function $performOp(root, fiber, cont, request, wait) {
  const where = request.op?.$where ?? "a suspend operation";
  let sync = true;
  let now;
  let used = false;
  const pass = (mode, value) => {
    used = true;
    if (sync) now = { mode, value };
    else cont(mode, value);
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
    // A suspending implementation (a generator) waits as a separate continuation of the same fiber until it
    // resumes; after resume it does not suspend (E0461), so it runs to its end inside resume or right after it. If
    // the caller is dropped (interrupted) before it is resumed, the implementation is unwound too, which aborts what
    // it waits for (12.2).
    if ($isGenerator(result)) {
      let waiting = null;
      settled.track = (w) => {
        waiting = w;
      };
      wait.onDrop = () => {
        if (waiting === null || !waiting.live) return;
        const w = waiting;
        $drop(root, w);
        // Queued without draining: whoever dropped the caller drains the queue.
        root.queue.push(() => $step(root, fiber, w.gen, w.end, "throw", new $Interrupted()));
      };
      $step(root, fiber, result, settled, "next", undefined);
    }
    else settled(true, result);
  } catch (error) {
    settled(false, error);
  }
  sync = false;
  return now;
}

// Drops a waiting continuation that is unwound or abandoned: it will not be continued, and what it waits for is
// stopped (the AbortSignal of an external function, 12.2; the implementation of the operation it called).
function $drop(root, wait) {
  wait.live = false;
  root.waits.delete(wait);
  if (wait.fiber.wait === wait) wait.fiber.wait = null;
  wait.controller?.abort();
  wait.onDrop?.();
}

// The host function behind TestClock's `sleep` (8.8, std/clock.mjs). Returns a promise resolved when the run of
// the calling fiber stalls and `at` is the earliest time waited for (`$Root#stalled`). Aborting `signal` (the
// fiber was interrupted) takes it back.
function $whenStalled(at, signal) {
  const root = $current?.root;
  if (root === undefined) throw new $Defect("TestClock was used outside a run");
  return new Promise((resolve) => {
    const sleeper = { at, seq: root.sleeperSeq++, resolve };
    root.sleepers.push(sleeper);
    signal?.addEventListener(
      "abort",
      () => {
        root.sleepers = root.sleepers.filter((s) => s !== sleeper);
      },
      { once: true },
    );
  });
}

// std/clock.mjs reaches the runtime through this key, since the runtime may be inlined into the output.
globalThis[Symbol.for("emela.whenStalled")] = $whenStalled;

// ---- Fiber operations (spec chapter 11) ----

// The run-time scope of a group or a background (`[task.group]`, `[task.background]`). Its body (the rest of the
// with) runs on `fiber`, and its tasks are forked fibers.
class $Scope {
  constructor(fiber, background) {
    this.fiber = fiber;
    this.depth = fiber.scopes;
    // A background: its tasks are not interrupted when the body ends or a task fails.
    this.background = background;
    // Tasks that have not ended.
    this.children = new Set();
    // No more starts: the body ended or something failed.
    this.closed = false;
    this.bodyDone = false;
    // The scope returned.
    this.ended = false;
    // The first failure (a Fail or Die `$Cause`) of the body or a task; the result of the scope.
    this.first = undefined;
    // Called when the last task ends while the scope waits for them.
    this.closer = null;
  }

  // A failure of the body or a task. In a group, the first one becomes the result and interrupts the siblings and the
  // body; later ones are added as notes. A failure of a task of a background (a defect of `on_failure`) stops
  // nothing, and the body's result decides what it becomes (`$scope`).
  failed(cause) {
    if (this.first !== undefined) {
      this.first.note(cause);
      return;
    }
    this.first = cause;
    if (this.background) return;
    this.closed = true;
    for (const child of this.children) child.interrupt(null);
    if (!this.bodyDone) this.fiber.interrupt(this);
  }

  childEnded(child) {
    this.children.delete(child);
    const cause = child.cause;
    if (cause !== null && cause.kind !== $CAUSE_INTERRUPT) this.failed(cause);
    if (this.children.size === 0 && this.closer !== null) {
      const closer = this.closer;
      this.closer = null;
      closer();
    }
  }

  // An interruption of the fiber arrived while the release waits: a background passes it on to its tasks (11.3).
  interruptedWhileWaiting() {
    for (const child of this.children) child.interrupt(null);
  }
}

// The run-time scope of a group or a background (`FiberOp::Scope`). Runs `body(scope)` on the current fiber. When the
// body ends, a group interrupts the remaining tasks; a background waits for them without interrupting, unless the
// body unwinds by an interruption. Once all of them ended the scope returns. A group returns the first failure of
// the body or a task if any, otherwise how the body ended. A background returns how the body ended, unless a task
// failed (a defect of `on_failure`): that is a defect of its release (14.4).
export function* $scope(body, background = false) {
  const fiber = $current;
  const scope = new $Scope(fiber, background);
  fiber.scopes++;
  let ok;
  let value;
  try {
    value = yield* $maybe(body(scope));
    ok = true;
  } catch (error) {
    ok = false;
    value = error;
  }
  fiber.scopes--;
  fiber.pending = fiber.pending.filter((s) => s !== scope);
  fiber.unwinds.delete(scope);
  scope.bodyDone = true;
  scope.closed = true;
  const interrupted = !ok && $isInterrupted(value);
  if (!background) {
    if (ok ? value instanceof $Fail : !interrupted) {
      scope.failed(new $Cause(ok ? $CAUSE_FAIL : $CAUSE_DIE, value));
    }
    for (const child of scope.children) child.interrupt(null);
  } else if (interrupted || fiber.pending.length > 0) {
    scope.interruptedWhileWaiting();
  }
  if (scope.children.size > 0) {
    fiber.holds++;
    if (background) fiber.waiting = scope;
    try {
      yield new $FiberRequest((_, wake) => {
        if (scope.children.size === 0) return { mode: "next", value: undefined };
        scope.closer = () => wake("next", undefined);
        return undefined;
      }, true);
    } finally {
      fiber.holds--;
      if (background) fiber.waiting = null;
    }
  }
  scope.ended = true;
  const first = scope.first;
  // A defect of `on_failure` is a defect of the release of the background (11.3, 14.4).
  if (background && first !== undefined) throw $releaseDefect(ok ? undefined : value, first.value, "Task.Background");
  if (background) {
    if (ok) return value;
    throw value;
  }
  if (first !== undefined) {
    if (first.kind === $CAUSE_FAIL) return first.value;
    throw first.value;
  }
  if (ok) return value;
  throw value;
}

// `tasks.start(f)` (11.2). The task starts running when the current fiber next suspends.
export function $fork(scope, f) {
  if (scope.closed) throw new $Defect("task group was used after it closed");
  const root = scope.fiber.root;
  const child = new $Fiber(root, scope);
  scope.children.add(child);
  root.schedule(() => child.begin(f));
  return child;
}

// `Task.join(h)` (11.6). A task that failed or was interrupted has unwound its whole group, so a joiner that sees it
// (only while it runs a release) unwinds as interrupted.
export function $join(h) {
  return new $FiberRequest((_, wake) =>
    h.watch(wake, () => {
      const cause = h.cause;
      if (cause === null) return { mode: "next", value: h.result.value };
      return { mode: "throw", value: new $Interrupted() };
    }),
  );
}

// `Exit.attempt(f)` (11.5). Turns a Fail or a Die of `f` into `Failure(cause)`. An interruption is not caught.
// Returns a generator when `f` returned one.
export function $attempt(f) {
  let r;
  try {
    r = f();
  } catch (error) {
    return $attemptCaught(error);
  }
  return $isGenerator(r) ? $attemptGen(r) : $attemptValue(r);
}

function* $attemptGen(gen) {
  let value;
  try {
    value = yield* gen;
  } catch (error) {
    return $attemptCaught(error);
  }
  return $attemptValue(value);
}

// While the fiber unwinds by an interruption, a defect (a release that failed on the way) is not caught either: it
// stays the fiber's result (14.4).
function $attemptCaught(error) {
  if ($isInterrupted(error) || $current?.isUnwinding()) throw error;
  return $exit(1, new $Cause($CAUSE_DIE, error));
}

function $attemptValue(value) {
  return value instanceof $Fail ? $exit(1, new $Cause($CAUSE_FAIL, value)) : $exit(0, value);
}

// `Exit` (`Success(A)` is index 0, `Failure(Cause[E])` is index 1).
function $exit(tag, value) {
  return { $tag: tag, $0: value };
}

// A Deferred (`[task.deferred]`): written at most once. Its lifetime is the with of the built-in handler `Deferred`.
class $DeferredCell {
  constructor() {
    this.full = false;
    this.value = undefined;
    this.waiters = [];
  }
}

// The init of the built-in handler `Deferred` (`[task.deferred]`).
export function $deferred() {
  return new $DeferredCell();
}

// `complete(value)` of a Deferred (`[task.deferred]`). Wakes every fiber waiting for it.
export function $complete(d, value) {
  if (d.full) return false;
  d.full = true;
  d.value = value;
  const waiters = d.waiters;
  d.waiters = [];
  for (const wake of waiters) wake("next", value);
  return true;
}

// `await()` of a Deferred (`[task.deferred]`).
export function $deferredAwait(d) {
  return new $FiberRequest((_, wake) => {
    if (d.full) return { mode: "next", value: d.value };
    d.waiters.push(wake);
    return undefined;
  });
}

// `poll()` of a Deferred (`[task.deferred]`).
export function $poll(d) {
  return d.full ? { $tag: 0, $0: d.value } : $None;
}

// ---- Semaphore and Queue (`[task.semaphore]`, `[task.queue]`) ----
//
// Fibers wait in first-in first-out lines. A fiber given what it waits for (permits, a value, the news that the
// queue is closed) is taken out of the line and woken as `served`. A fiber interrupted while it waits leaves its
// line when its wait is dropped (`wait.onDrop`). Their lifetimes are the withs of their built-in handlers, and every
// fiber that can use one ends before that with does, so no line is left when it ends.

function $leave(line, entry) {
  const i = line.indexOf(entry);
  if (i >= 0) line.splice(i, 1);
}

class $SemaphoreCell {
  constructor(permits) {
    this.permits = permits;
    this.free = permits;
    // `{ n, wake }`, in the order the fibers started to wait.
    this.line = [];
  }

  // Gives permits to the fibers at the front of the line while enough are free for the first.
  serve() {
    while (this.line.length > 0 && this.line[0].n <= this.free) {
      const entry = this.line.shift();
      this.free -= entry.n;
      entry.wake("served", undefined);
    }
  }
}

function $semaphoreDefect(s, n, name) {
  if (n < 1) return new $Defect(`\`${name}\` needs at least 1 permit, but got ${n}`);
  if (n > s.permits) return new $Defect(`\`${name}\` asked for ${n} permits of a semaphore with ${s.permits}`);
  return undefined;
}

// The init of the built-in handler `Semaphore` (`[task.semaphore]`).
export function $semaphore(permits) {
  if (permits < 1) throw new $Defect(`semaphore needs at least 1 permit, but got ${permits}`);
  return new $SemaphoreCell(permits);
}

// The init of the handler member `Permits` (`[task.semaphore]`): waits until `n` permits are free and no fiber is
// ahead in the line.
export function $acquire(s, n) {
  return new $FiberRequest((_, wake, wait) => {
    const defect = $semaphoreDefect(s, n, "Permits");
    if (defect !== undefined) return { mode: "throw", value: defect };
    if (s.line.length === 0 && s.free >= n) {
      s.free -= n;
      return { mode: "next", value: undefined };
    }
    const entry = { n, wake };
    s.line.push(entry);
    wait.onDrop = () => {
      $leave(s.line, entry);
      s.serve();
    };
    return undefined;
  });
}

// The init of the handler member `TryPermits` (`[task.semaphore]`): takes the permits only if no fiber waits and
// enough are free.
export function $tryAcquire(s, n) {
  const defect = $semaphoreDefect(s, n, "TryPermits");
  if (defect !== undefined) throw defect;
  if (s.line.length > 0 || s.free < n) return false;
  s.free -= n;
  return true;
}

// The release of `Permits` and `TryPermits` (`[task.semaphore]`).
export function $release(s, n) {
  s.free += n;
  s.serve();
}

// `available()` of a Semaphore (`[task.semaphore]`).
export function $available(s) {
  return s.free;
}

class $QueueCell {
  constructor(capacity) {
    this.capacity = capacity;
    this.items = [];
    this.closed = false;
    // `{ wake }` of fibers waiting to take (only while `items` is empty), and `{ value, wake }` of fibers waiting to
    // offer (only while `items` is full).
    this.takers = [];
    this.offerers = [];
  }

  // Adds `value`: to the first fiber waiting to take, or at the end. The caller has checked that it fits.
  put(value) {
    if (this.takers.length > 0) this.takers.shift().wake("served", { $tag: 0, $0: value });
    else this.items.push(value);
  }

  // Removes the value at the front; the value of the first fiber waiting to offer takes its place at the end.
  shift() {
    const value = this.items.shift();
    if (this.offerers.length > 0) {
      const entry = this.offerers.shift();
      this.items.push(entry.value);
      entry.wake("served", true);
    }
    return value;
  }
}

// The init of the built-in handler `Queue` (`[task.queue]`).
export function $queue(capacity) {
  if (capacity < 1) throw new $Defect(`queue needs a capacity of at least 1, but got ${capacity}`);
  return new $QueueCell(capacity);
}

// `offer(value)` of a Queue (`[task.queue]`). False when the queue is closed (lowering makes it the failure
// `QueueClosed`).
export function $offer(q, value) {
  return new $FiberRequest((_, wake, wait) => {
    if (q.closed) return { mode: "next", value: false };
    if (q.items.length < q.capacity) {
      q.put(value);
      return { mode: "next", value: true };
    }
    const entry = { value, wake };
    q.offerers.push(entry);
    wait.onDrop = () => $leave(q.offerers, entry);
    return undefined;
  });
}

// `take()` of a Queue (`[task.queue]`). `None` when the queue is closed and empty.
export function $take(q) {
  return new $FiberRequest((_, wake, wait) => {
    if (q.items.length > 0) return { mode: "next", value: { $tag: 0, $0: q.shift() } };
    if (q.closed) return { mode: "next", value: $None };
    const entry = { wake };
    q.takers.push(entry);
    wait.onDrop = () => $leave(q.takers, entry);
    return undefined;
  });
}

// `try_offer(value)` of a Queue (`[task.queue]`).
export function $tryOffer(q, value) {
  if (q.closed || q.items.length >= q.capacity) return false;
  q.put(value);
  return true;
}

// `poll()` of a Queue (`[task.queue]`).
export function $queuePoll(q) {
  return q.items.length > 0 ? { $tag: 0, $0: q.shift() } : $None;
}

// `size()` of a Queue (`[task.queue]`).
export function $size(q) {
  return q.items.length;
}

// `close()` of a Queue (`[task.queue]`). The fibers waiting to take or to offer stop with `QueueClosed`.
export function $close(q) {
  if (q.closed) return;
  q.closed = true;
  for (const entry of q.takers.splice(0)) entry.wake("served", $None);
  for (const entry of q.offerers.splice(0)) entry.wake("served", false);
}

// `CauseKind` (`Fail`, `Die`, in that order). How an interrupted fiber ended is kept as a third kind, which is never a
// value of `CauseKind` (an interruption is not a cause, 11.6).
const $CAUSE_FAIL = 0;
const $CAUSE_DIE = 1;
const $CAUSE_INTERRUPT = 2;
const $causeKinds = [Object.freeze({ $tag: 0 }), Object.freeze({ $tag: 1 })];

// Why a fiber did not end with a value (11.5). `value` is the `$Fail` of a Fail and the thrown exception of a Die.
class $Cause {
  constructor(kind, value) {
    this.kind = kind;
    this.value = value;
  }

  // Adds a later failure `other` as a note to the display of this one (11.2).
  note(other) {
    const note = `note: another task also failed: ${$causeToString(other)}`;
    if (this.kind === $CAUSE_FAIL) (this.value.notes ??= []).push(note);
    else if (this.value instanceof Error) this.value.message += `\n${note}`;
  }
}

export function $causeKind(c) {
  return $causeKinds[c.kind];
}

export function $causeToString(c) {
  switch (c.kind) {
    case $CAUSE_FAIL:
      return `error: ${c.value[Symbol.for("emela.fail")]()}`;
    default:
      return `defect: ${$foreign(c.value).message}`;
  }
}

// `Cause.raise(c)` (11.6). A Fail is returned as the failure, and a Die is thrown again.
export function $causeRaise(c) {
  if (c.kind === $CAUSE_FAIL) return c.value;
  throw c.value;
}

// Marks the start of a release that may suspend: the current fiber is not interrupted until `$unhold` (11.4).
export function $hold() {
  const fiber = $current;
  if (fiber !== null) fiber.holds++;
  return fiber;
}

export function $unhold(fiber) {
  if (fiber !== null) fiber.holds--;
}

// ---- Releasing handlers (spec 8.4, 8.6, 14.2, 14.4) ----

// What a with ends with when the release of handler `name` threw `error` (a defect, or `$Interrupted` if a suspend
// operation in it was dropped). `thrown` is what the body (and the releases inside it) threw, or undefined if it
// ended with a value or an error, or was closed as interrupted.
// If the body already ended in a defect, that first defect stays the result and the release's defect is added to
// its message as a note. Otherwise the release's defect becomes the result (14.4).
export function $releaseDefect(thrown, error, name) {
  if (thrown === undefined || $isInterrupted(thrown)) return error;
  if (thrown instanceof Error && !$isInterrupted(error)) {
    const message = error instanceof Error ? error.message : String(error);
    thrown.message += `\nnote: the release of handler ${name} also failed: ${message}`;
  }
  return thrown;
}

function $isInterrupted(x) {
  return x instanceof Error && x.name === "Interrupted";
}

// A failed assert (spec 13.2). `text` is the source text of the expression and may be empty. `location` is where the
// assert is, or undefined.
export function $assertFail(text, location) {
  throw new $Defect(text === "" ? "assertion failed" : `assertion failed: ${text}`, location);
}

// A failed comparison assert. Both sides are received as displayed strings (displayed only on failure).
export function $assertCmp(text, op, left, right, location) {
  const head = text === "" ? `left ${op} right` : text;
  throw new $Defect(`assertion failed: ${head}\n  left: ${left}\n right: ${right}`, location);
}

// A failed assert whose message was built by the caller (sides shown through a written Show impl).
export function $assertDefect(message, location) {
  throw new $Defect(message, location);
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

// ---- Standard output and standard error ([api.std.process]) ----
//
// On a host with a standard output, every write of the program goes through `$output`: SystemStdout,
// SystemStderr, SystemConsole, `dbg`, the lines the launcher writes when execution stops, and the results of the
// test runner. It writes synchronously to file descriptor 1 or 2, so writes keep their order across all of them,
// also when both descriptors are one pipe: there is no buffer to hand over before a write to standard error or a
// wait for standard input. `console.log` is not mixed in, because Node queues its write when the descriptor would
// block and synchronous writes would overtake it.
//
// A host has a standard output when it gives `node:fs` through `process.getBuiltinModule` (Node and Bun). On any
// other host (a browser, a Worker), writes go to `console.log` and `console.error` without their final `\n`, which
// is all `Console` promises there; what SystemStdout does there is not specified.

const $fs = (() => {
  try {
    return globalThis.process?.getBuiltinModule?.("node:fs");
  } catch {
    return undefined;
  }
})();
const $hasStdout = typeof $fs?.writeSync === "function";

// Set once a write found the reader of the descriptor gone. Later writes to it are discarded.
let $stdoutClosed = false;
let $stderrClosed = false;
// What `$writeAll` waits on for 1 ms when the descriptor would block.
let $waitCell;
const $encoder = new TextEncoder();

// Writes every byte of `bytes` to `fd`. A descriptor that someone made non-blocking (Node does for a pipe once
// `process.stdout` is touched) refuses a write with EAGAIN while the reader is behind: the writer waits 1 ms and
// writes the rest again, so a slow reader slows the program instead of a buffer growing.
function $writeAll(fd, bytes) {
  let at = 0;
  while (at < bytes.length) {
    try {
      at += $fs.writeSync(fd, bytes, at, bytes.length - at);
    } catch (error) {
      if (error?.code !== "EAGAIN") throw error;
      $waitCell ??= new Int32Array(new SharedArrayBuffer(4));
      Atomics.wait($waitCell, 0, 0, 1);
    }
  }
}

// Writes `text` to standard output (`fd` 1) or standard error (`fd` 2).
//
// A write to a standard output whose reader has closed it (EPIPE) does not return: every fiber of every run is
// interrupted (11.4), and the fiber that wrote unwinds as interrupted from here. The launcher then exits with 141
// and writes nothing. A closed standard error discards the write, and the program goes on. Other errors of the
// host (no descriptor at all) discard the write, as `console.log` does.
export function $output(fd, text) {
  if (!$hasStdout) {
    const line = text.endsWith("\n") ? text.slice(0, -1) : text;
    if (fd === 2) console.error(line);
    else console.log(line);
    return;
  }
  if (fd === 2 ? $stderrClosed : $stdoutClosed) return;
  try {
    $writeAll(fd, $encoder.encode(text));
  } catch (error) {
    if (error?.code !== "EPIPE") return;
    if (fd === 2) {
      $stderrClosed = true;
      return;
    }
    $stdoutClosed = true;
    for (const root of [...$roots]) {
      for (const fiber of [...root.fibers]) {
        if (fiber !== $current) fiber.interrupt(null);
      }
    }
    throw new $Interrupted();
  }
}

// std/process.mjs, std/console.mjs and the test runner reach the output through this key, since the runtime may be
// inlined into the output.
globalThis[Symbol.for("emela.output")] = { write: $output, stdoutClosed: () => $stdoutClosed };

// Launches `main` when this `main.mjs` is the entry module of the JS host (`[target.js.module]`). `entry` is
// `import.meta.main` of `main.mjs`, and `url` its `import.meta.url`. A host that does not define `import.meta.main`
// (older releases of Node 22) is asked through the real path of `process.argv[1]` instead.
export function $launch(entry, url, main) {
  const proc = globalThis.process;
  if (typeof proc?.on !== "function") return;
  if (entry === true) $launchMain(proc, main);
  else if (entry === undefined) {
    $startedWith(proc, url).then((started) => {
      if (started) $launchMain(proc, main);
    });
  }
}
// Whether the real path of `process.argv[1]` is the path of `url`. `false` when either cannot be read: under
// `node -e '...' word`, `process.argv[1]` is a word, not a file, and importing `main.mjs` there must not throw.
// `node:fs` is imported only on Node and Bun, and only when `process.getBuiltinModule` is missing (before Node 22.3).
async function $startedWith(proc, url) {
  try {
    const script = proc.argv?.[1];
    if (typeof script !== "string" || proc.versions?.node === undefined) return false;
    const fs = proc.getBuiltinModule?.("node:fs") ?? (await import("node:fs"));
    const nodeUrl = proc.getBuiltinModule?.("node:url") ?? (await import("node:url"));
    return fs.realpathSync(script) === nodeUrl.fileURLToPath(url);
  } catch {
    return false;
  }
}
// Runs `main` and exits with the standard error and the exit code of `[cmd.build]`.
function $launchMain(proc, main) {
  let over = false;
  function closed() {
    if (!$stdoutClosed) return false;
    proc.exit(141);
    return true;
  }
  function stop(code, text) {
    if (closed()) return;
    proc.exitCode = code;
    $output(2, text);
    proc.stderr.write("", () => proc.exit(code));
  }
  function finish(value) {
    over = true;
    if (closed()) return;
    const show = value?.[Symbol.for("emela.fail")];
    if (typeof show === "function") {
      $output(2, `error: ${show.call(value)}\n`);
      proc.exitCode = 1;
    }
  }
  function crash(error) {
    if (over) return;
    over = true;
    if (error instanceof Error && error.name === "Interrupted") {
      stop(130, "interrupted\n");
    } else if (error instanceof Error && error.name === "Defect") {
      stop(101, `defect: ${error.message}\n`);
    } else {
      stop(1, `${error?.stack ?? String(error)}\n`);
    }
  }
  let cancel;
  proc.on("uncaughtException", crash);
  proc.on("beforeExit", () => {
    if (over) return;
    if (cancel !== undefined) {
      cancel();
      return;
    }
    over = true;
    if (closed()) return;
    $output(2, "interrupted\n");
    proc.exitCode = 130;
  });
  globalThis[Symbol.for("emela.args")] = proc.argv.slice(2);
  try {
    const value = main();
    if (Object.prototype.toString.call(value) === "[object Generator]") {
      cancel = $start(value, (ok, result) => (ok ? finish(result) : crash(result)));
    } else {
      finish(value);
    }
  } catch (error) {
    crash(error);
  }
}

// The Prelude's `dbg(value)`. `show` is the display function generated from the value's type.
export function $dbg(value, show) {
  $output(2, `${show(value)}\n`);
  return value;
}

// `dbg(value)` for a value shown through its Show impl: the shown text, printed as it is.
export function $debugPrint(text) {
  $output(2, `${text}\n`);
}

// The Prelude's `inspect(value)` (spec 10.6). `show` is the display function generated from the
// value's type.
export function $inspect(value, show) {
  return show(value);
}

// The cell that holds the state of `State.run` (spec 8.8). Only the lowering creates and reads it.
export function $cellNew(value) {
  return { value };
}

export function $cellGet(cell) {
  return cell.value;
}

export function $cellSet(cell, value) {
  cell.value = value;
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

// String.to_ascii_lowercase changes only ASCII A to Z, and to_ascii_uppercase only a to z: every
// other character, including non-ASCII letters, is copied unchanged.
export function $strAsciiLower(s) {
  return s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));
}

export function $strAsciiUpper(s) {
  return s.replace(/[a-z]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 32));
}

// ---- Bytes (`[api.std.bytes]`) ----
//
// A Bytes is a `Uint8Array` that nothing changes after it is made (`[target.js.value]`). The
// functions that make one always allocate a new array, so a Bytes never shares a buffer with
// another value or with the host.

// None if an element is outside 0 to 255.
export function $bytesFromList(xs) {
  const items = [];
  for (let p = xs; p !== null; p = p.t) {
    if (p.h < 0 || p.h > 255) return $None;
    items.push(p.h);
  }
  return { $tag: 0, $0: Uint8Array.from(items) };
}

export function $bytesToList(b) {
  return $list(Array.from(b));
}

export function $bytesSize(b) {
  return b.length;
}

export function $bytesGet(b, i) {
  return i >= 0 && i < b.length ? { $tag: 0, $0: b[i] } : $None;
}

// Both ends are clamped to 0 to the size; empty if `from` is not less than `to`.
export function $bytesSlice(b, from, to) {
  const f = Math.min(Math.max(from, 0), b.length);
  const t = Math.min(Math.max(to, 0), b.length);
  return f < t ? b.slice(f, t) : new Uint8Array(0);
}

export function $bytesConcat(parts) {
  let size = 0;
  for (let p = parts; p !== null; p = p.t) size += p.h.length;
  const out = new Uint8Array(size);
  let at = 0;
  for (let p = parts; p !== null; p = p.t) {
    out.set(p.h, at);
    at += p.h.length;
  }
  return out;
}

const $utf8Encoder = new TextEncoder();
// `ignoreBOM` keeps a leading BOM as U+FEFF. The input is already valid UTF-8 (`decode_utf8`
// checks it in Emela), so nothing is replaced.
const $utf8Decoder = new TextDecoder("utf-8", { ignoreBOM: true });

export function $bytesEncodeUtf8(s) {
  return $utf8Encoder.encode(s);
}

export function $bytesDecodeValidUtf8(b) {
  return $utf8Decoder.decode(b);
}

// The source form `Bytes.hex("89504e47")`, as the Show impl in `std/bytes.emel` writes it.
export function $showBytes(b) {
  let digits = "";
  for (const x of b) digits += x.toString(16).padStart(2, "0");
  return `Bytes.hex("${digits}")`;
}

// A Bytes received from the host (`[target.js.value]`): a `Uint8Array` (or a subclass such as
// Node.js `Buffer`) of this realm, copied so that later changes to the host's buffer do not reach
// it. Only the bytes the view shows are copied. `message` is the defect for any other value.
export function $hostBytes(b, message) {
  if (!(b instanceof Uint8Array)) throw new $Defect(message);
  return new Uint8Array(b);
}

// ---- Array (`[api.std.array]`) ----
//
// An Array is a JS `Array` that nothing changes after it is made (`[target.js.value]`). The
// functions that make one always make a new array, so `set` copies (`[target.impl-hint]`), and an
// Array never shares its elements' storage with another value or with the host. Arrays from the
// host are copied by `$hostArray`, so every Array is dense.

export function $arrayFromList(xs) {
  const out = [];
  for (let p = xs; p !== null; p = p.t) out.push(p.h);
  return out;
}

export function $arrayToList(a) {
  return $list(a);
}

export function $arraySize(a) {
  return a.length;
}

export function $arrayGet(a, i) {
  return i >= 0 && i < a.length ? { $tag: 0, $0: a[i] } : $None;
}

export function $arraySet(a, i, value) {
  if (!(i >= 0 && i < a.length)) {
    throw new $Defect(`Array.set was given position ${i} in an array of size ${a.length}`);
  }
  const out = a.slice();
  out[i] = value;
  return out;
}

// Both ends are clamped to 0 to the size; empty if `from` is not less than `to`.
export function $arraySlice(a, from, to) {
  const f = Math.min(Math.max(from, 0), a.length);
  const t = Math.min(Math.max(to, 0), a.length);
  return f < t ? a.slice(f, t) : [];
}

export function $arrayConcat(parts) {
  const out = [];
  for (let p = parts; p !== null; p = p.t) {
    for (let i = 0; i < p.h.length; i++) out.push(p.h[i]);
  }
  return out;
}

// The source form `Array.from_list([1, 2])`, as the Show impl in `std/array.emel` writes it.
export function $showArray(a, show) {
  return `Array.from_list([${a.map(show).join(", ")}])`;
}

// An Array received from the host (`[target.js.value]`): a value with `Array.isArray`, copied so
// that later changes to the host's array do not reach it, each element converted by `convert`
// (`null` when the elements pass as they are). A hole is read as `undefined`. `message` is the
// defect for any other value.
export function $hostArray(a, message, convert) {
  if (!Array.isArray(a)) throw new $Defect(message);
  const out = new Array(a.length);
  for (let i = 0; i < a.length; i++) out[i] = convert === null ? a[i] : convert(a[i]);
  return out;
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

// `Int.parse` (spec 15.3): decimal digits with an optional sign, within the Int range. `None` otherwise.
export function $iparse(s) {
  if (!/^[+-]?[0-9]+$/.test(s)) return $None;
  const n = Number(s);
  if (n < -2147483648 || n > 2147483647) return $None;
  return { $tag: 0, $0: n | 0 };
}

// `Float.parse` (spec 15.3): the forms `Float.to_string` writes. `None` otherwise.
export function $fparse(s) {
  if (!/^[+-]?([0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?|Infinity)$|^NaN$/.test(s)) return $None;
  return { $tag: 0, $0: Number(s) };
}

// A finite Float with an integral value gets a decimal point, like `1.0`, so it can be told apart from an Int.
export function $showFloat(x) {
  const s = String(x);
  return Number.isFinite(x) && /^-?\d+$/.test(s) ? s + ".0" : s;
}

export function $showUnit(_) {
  return "()";
}

// A function value in the debug display (`dbg`, spec 10.6).
export function $showFn(_) {
  return "<fn>";
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
