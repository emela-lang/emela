// JsClock（js_clock.emel）のホストの関数．Int64 は JS では BigInt で持つ．

/** Unix エポックからのミリ秒．`Date.now()` を BigInt にする． */
export function nowMilliseconds() {
  return BigInt(Date.now());
}
