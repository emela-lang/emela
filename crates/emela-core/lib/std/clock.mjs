// Host functions for SystemClock (clock.emel). Int64 is a BigInt in JS.

export function nowMilliseconds() {
  return BigInt(Date.now());
}

/**
 * Calls `f` once after `ms` milliseconds. The argument order is the reverse of `setTimeout`. A negative `ms` behaves
 * like 0; it is clamped here because Node 24 warns (`TimeoutNegativeWarning`) when `setTimeout` gets one.
 */
export function after(ms, f) {
  setTimeout(f, Math.max(ms, 0));
}
