// Host functions for SystemClock (clock.emel). Int64 is a BigInt in JS.

export function nowMilliseconds() {
  return BigInt(Date.now());
}

/** Calls `f` once after `ms` milliseconds. The argument order is the reverse of `setTimeout`. A negative `ms` behaves like 0. */
export function after(ms, f) {
  setTimeout(f, ms);
}
