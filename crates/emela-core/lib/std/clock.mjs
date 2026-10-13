// Host functions for SystemClock and TestClock (clock.emel). Int64 is a BigInt in JS.

export function nowMilliseconds() {
  return BigInt(Date.now());
}

/** Resolves after `ms` milliseconds (a negative `ms` behaves like 0). Aborting `signal` clears the timer (spec 12.2). */
export function sleep(ms, signal) {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
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
