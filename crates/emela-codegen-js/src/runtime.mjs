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

// Prelude の `todo()`．
export function $todo() {
  throw new $Defect("not yet implemented");
}

// Prelude の `dbg(value)`．`show` は値の型から生成コードが作った表示関数．
export function $dbg(value, show) {
  console.error(show(value));
  return value;
}

// Prelude の Option（`Some(A)` が添字 0，`None` が添字 1）．
export const $None = Object.freeze({ $tag: 1 });

// ---- String（仕様 16.2）----

let $segmenter;

function $graphemes(s) {
  $segmenter ??= new Intl.Segmenter(undefined, { granularity: "grapheme" });
  return $segmenter.segment(s);
}

// String.length は書記素クラスタの数．
export function $strlen(s) {
  let n = 0;
  for (const _ of $graphemes(s)) n++;
  return n;
}

// String.byte_size は UTF-8 で符号化したときのバイト数．
export function $strbytes(s) {
  let n = 0;
  for (const c of s) {
    const p = c.codePointAt(0);
    n += p < 0x80 ? 1 : p < 0x800 ? 2 : p < 0x10000 ? 3 : 4;
  }
  return n;
}

export function $strcat(a, b) {
  return a + b;
}

// 部分文字列の照合はコードポイントの列で行う（書記素の境界は見ない）．
export function $strhas(s, sub) {
  return s.includes(sub);
}

export function $strstarts(s, prefix) {
  return s.startsWith(prefix);
}

export function $strends(s, suffix) {
  return s.endsWith(suffix);
}

// String.chars は書記素クラスタごとの文字列のリスト．
export function $strchars(s) {
  const items = [];
  for (const { segment } of $graphemes(s)) items.push(segment);
  return $list(items);
}

// 空の区切りでは書記素ごとに分ける（chars と同じ）．JS の split("") は UTF-16 の単位で分けるため使わない．
export function $strsplit(s, sep) {
  if (sep === "") return $strchars(s);
  return $list(s.split(sep));
}

export function $strjoin(parts, sep) {
  let out = "";
  for (let p = parts; p !== null; p = p.t) {
    if (p !== parts) out += sep;
    out += p.h;
  }
  return out;
}

// 空白は Unicode の White_Space 特性の 25 文字．JS の trim は U+FEFF を含み U+0085 を含まないので使わない．
const $ws = "[\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]";
const $trimRe = new RegExp(`^${$ws}+|${$ws}+$`, "g");

export function $strtrim(s) {
  return s.replace($trimRe, "");
}

// ---- 数値の変換（仕様 16.1）----

export function $itof(n) {
  return n;
}

export function $itol(n) {
  return BigInt(n);
}

// Int64 から Int へは下位 32bit に巻き戻す．
export function $ltoi(n) {
  return Number(BigInt.asIntN(32, n));
}

export function $icheckedAdd(a, b) {
  const r = a + b;
  return r > 2147483647 || r < -2147483648 ? $None : { $tag: 0, $0: r };
}

// Float から Int へ．丸めた結果が Int の範囲外か NaN なら defect．`| 0` で -0 を 0 にする．
function $ftoi(x, r) {
  if (!(r >= -2147483648 && r <= 2147483647)) {
    throw new $Defect(`cannot convert ${$showFloat(x)} to Int`);
  }
  return r | 0;
}

export function $ffloor(x) {
  return $ftoi(x, Math.floor(x));
}

export function $fceil(x) {
  return $ftoi(x, Math.ceil(x));
}

// 0.5 は 0 から遠い方へ丸める．Math.round は正の向きへ丸めるので，負の数は符号を外して丸める．
export function $fround(x) {
  return $ftoi(x, x < 0 ? -Math.round(-x) : Math.round(x));
}

export function $ftrunc(x) {
  return $ftoi(x, Math.trunc(x));
}

// ---- 表示（Show の仮実装．仕様 10.6）----
// 型から決まる表示関数は生成コードが組み立てる．ここには基本型とリスト，タプルのものを置く．

export function $showInt(n) {
  return String(n);
}

export function $showInt64(n) {
  return String(n);
}

export function $showBool(b) {
  return b ? "True" : "False";
}

// 整数値の有限な Float は `1.0` のように小数点を付けて，Int と見分けられるようにする．
export function $showFloat(x) {
  const s = String(x);
  return Number.isFinite(x) && /^-?\d+$/.test(s) ? s + ".0" : s;
}

export function $showUnit(_) {
  return "()";
}

// 文字列はソースの書き方で表す．エスケープは `\"`，`\\`，`\n`，`\#`（`#{` の前だけ）．
// 言語にエスケープのない制御文字は `\t`，`\r`，`\u{…}` と出す（仕様 18.1 #11 で未決）．
const $strEscRe = /["\\]|#(?=\{)|[\u0000-\u001f\u007f]/g;

export function $showString(s) {
  const body = s.replace($strEscRe, (c) => {
    switch (c) {
      case '"':
        return '\\"';
      case "\\":
        return "\\\\";
      case "#":
        return "\\#";
      case "\n":
        return "\\n";
      case "\t":
        return "\\t";
      case "\r":
        return "\\r";
      default:
        return `\\u{${c.codePointAt(0).toString(16)}}`;
    }
  });
  return `"${body}"`;
}

export function $showList(xs, show) {
  const items = [];
  for (let p = xs; p !== null; p = p.t) items.push(show(p.h));
  return `[${items.join(", ")}]`;
}

export function $showTuple(t, shows) {
  return `(${t.map((x, i) => shows[i](x)).join(", ")})`;
}
