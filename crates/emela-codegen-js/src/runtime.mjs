// Emela の JS ランタイム（0.20 の純粋なサブセット）．
//
// 生成コードは `$` で始まる名前だけを使う．Emela の識別子は `$` を含まないので衝突しない．
// 値の表現は crates/emela-codegen-js/NOTES.md を参照．
// インラインで埋め込むときは行頭の `export ` を取り除くので，`export` は行頭にだけ書く．

// defect（仕様 16.5）．回復できない誤りで，専用のクラスで投げる．
export class $Defect extends Error {
  constructor(message) {
    super(message);
    this.name = "Defect";
  }
}

export function $panic(message) {
  throw new $Defect(message);
}

export function $unreachable() {
  throw new $Defect("unreachable: no match arm matched");
}

// Int（32bit）の除算と剰余．0方向へ切り捨て，剰余は被除数の符号に従う．
// `| 0` で 2^31 / -1 のあふれを巻き戻し，-0 を 0 にする．
export function $idiv(a, b) {
  if (b === 0) throw new $Defect("division by zero");
  return (a / b) | 0;
}

export function $irem(a, b) {
  if (b === 0) throw new $Defect("division by zero");
  return (a % b) | 0;
}

// Int64（BigInt）の除算と剰余．BigInt の `/` は0方向へ切り捨てる．
export function $ldiv(a, b) {
  if (b === 0n) throw new $Defect("division by zero");
  return BigInt.asIntN(64, a / b);
}

export function $lrem(a, b) {
  if (b === 0n) throw new $Defect("division by zero");
  return a % b;
}

// List は cons セル `{ h, t }` の連なりで，空リストは `null`．
export function $list(items, tail = null) {
  let list = tail;
  for (let i = items.length - 1; i >= 0; i--) list = { h: items[i], t: list };
  return list;
}

// 構造の等値比較．長いリストでスタックを使わないよう，明示的なスタックで辿る．
export function $eq(a, b) {
  const stack = [a, b];
  while (stack.length > 0) {
    const y = stack.pop();
    const x = stack.pop();
    if (x === y) continue;
    if (typeof x !== "object" || typeof y !== "object" || x === null || y === null) {
      // NaN 同士は等しくない（IEEE のとおり）．
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

// 文字列の順序はコードポイントの辞書順．返り値は負・0・正．
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

let segmenter;

// String.length は書記素クラスタの数．
export function $strlen(s) {
  segmenter ??= new Intl.Segmenter(undefined, { granularity: "grapheme" });
  let n = 0;
  for (const _ of segmenter.segment(s)) n++;
  return n;
}

export function $showBool(b) {
  return b ? "True" : "False";
}

// 整数値の有限な Float は `1.0` のように小数点を付けて，Int と見分けられるようにする．
export function $showFloat(x) {
  const s = String(x);
  return Number.isFinite(x) && /^-?\d+$/.test(s) ? s + ".0" : s;
}
