// Host functions for Random on JS. crypto.getRandomValues returns cryptographically secure random values.

// Uniform over the whole Int (32-bit) range. Received through an Int32Array, so it is signed.
export const int = () => crypto.getRandomValues(new Int32Array(1))[0];

// Uniform in [0, 1). Multiplies the top 53 bits by 2^-53.
export const float = () => {
  const [hi, lo] = crypto.getRandomValues(new Uint32Array(2));
  return ((hi >>> 5) * 67108864 + (lo >>> 6)) / 9007199254740992;
};
