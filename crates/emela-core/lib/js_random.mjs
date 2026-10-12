// Random の JS 用のホスト関数．crypto.getRandomValues は暗号論的に安全な乱数を返す．

// Int（32 ビット）の全域で一様．Int32Array で受けるので符号付きになる．
export const int = () => crypto.getRandomValues(new Int32Array(1))[0];

// 0 以上 1 未満で一様．上位 53 ビットを 2^-53 倍する．
export const float = () => {
  const [hi, lo] = crypto.getRandomValues(new Uint32Array(2));
  return ((hi >>> 5) * 67108864 + (lo >>> 6)) / 9007199254740992;
};
