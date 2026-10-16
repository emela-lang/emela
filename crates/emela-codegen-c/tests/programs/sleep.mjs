// The host side of `sleep` in these programs on JS (`@external(js, "./sleep.mjs", "sleep")`).
export function sleep(ms, signal) {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(ms), ms);
    signal?.addEventListener("abort", () => clearTimeout(timer), { once: true });
  });
}
