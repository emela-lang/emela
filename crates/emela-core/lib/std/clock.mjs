// Host functions for SystemClock and TestClock (clock.emel). Int64 is a BigInt in JS.

// Not `Date.now` itself as the external function: it returns a Number, and Int64 needs a BigInt.
export function nowMilliseconds() {
  return BigInt(Date.now());
}

/**
 * Resolves after `ms` milliseconds. A negative `ms` is 0 (spec 8.8); it is clamped here since Node 24 warns about a
 * negative timeout. Aborting `signal` clears the timer (spec 12.2).
 */
export function sleep(ms, signal) {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, Math.max(0, ms));
    signal?.addEventListener("abort", () => clearTimeout(timer), { once: true });
  });
}

/**
 * Resolves when the run stalls and `at` (milliseconds) is the earliest time waited for (spec 8.8). The runtime
 * implements it; it is reached by a global key since the runtime may be inlined into the output.
 */
export function whenStalled(at, signal) {
  return globalThis[Symbol.for("emela.whenStalled")](at, signal);
}
