// JsClock（js_clock.emel）のホストの関数．Int64 は JS では BigInt で持つ．

/** Unix エポックからのミリ秒．`Date.now()` を BigInt にする． */
export function nowMilliseconds() {
  return BigInt(Date.now());
}

/** `ms` ミリ秒後に `f` を1回呼ぶ．`setTimeout` と引数の順が逆．負の `ms` は 0 と同じ． */
export function after(ms, f) {
  setTimeout(f, ms);
}
