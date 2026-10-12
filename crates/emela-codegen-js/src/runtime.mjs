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

// 失敗（仕様 7章）．エラーは戻り値として返す（16.5）．`fail e` は `new $Fail(e)` を返し，
// 失敗しうる呼び出しの結果が `$Fail` なら，それを呼び出し元か escape へ渡す．
// `error` は error の値で，`$tag` に error ごとの印（`{ show }`）を持つ．
//
// 起動用モジュールとテストランナーは $Fail のクラスを読み込まないので，`Symbol.for` の鍵で見分ける．
// 鍵の関数は，処理されなかったエラーの表示（`NotFound(id: 1)`）を返す．
export class $Fail {
  constructor(error) {
    this.error = error;
  }

  [Symbol.for("emela.fail")]() {
    return this.error.$tag.show(this.error);
  }
}

// 外部関数（仕様 12.5）が投げた例外を defect にする．defect（Emela のコールバックの中で起きたもの）は
// そのまま．メッセージは例外の名前を前に付ける（`TypeError: …`）．
export function $foreign(error) {
  if (error instanceof Error) {
    return error.name === "Defect" ? error : new $Defect(`${error.name}: ${error.message}`);
  }
  return new $Defect(`uncaught exception: ${String(error)}`);
}

// ビルド対象のターゲットの `@external` がない外部関数（エントリからたどれないもの）を呼んだ．
export function $noExternal(name) {
  throw new $Defect(`external function \`${name}\` is not available on this target`);
}

// ホストから受け取った文字列の，対になっていないサロゲートを置換文字にする（仕様 12.5）．
export function $wellFormed(s) {
  if (typeof s !== "string") return s;
  if (typeof s.toWellFormed === "function") return s.toWellFormed();
  return s.replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, "\uFFFD");
}

// ---- 中断と再開（仕様 8.4，14.5，16.5）----
//
// 中断しうる関数はジェネレータで，中断しうる呼び出しは `yield*` でつなぐ．suspend fn の操作の呼び出しは
// `yield $perform(op, args)`，`@external suspend fn` は `yield $await(promise)` で，要求を最も外側の
// ジェネレータを進める駆動（$start）まで送る．駆動は要求ごとに resume を作って操作の実装を呼ぶ．
// resume が操作の実装の中でその場で呼ばれれば，中断せずにそのまま続ける（14.5 の3）．後から呼ばれれば，
// その呼び出しの中で継続を進める．

class $Perform {
  constructor(op, args) {
    this.op = op;
    this.args = args;
  }
}

class $Await {
  constructor(promise) {
    this.promise = promise;
  }
}

export function $perform(op, args) {
  return new $Perform(op, args);
}

export function $await(promise) {
  return new $Await(promise);
}

// suspend fn の操作の実装 `f` に，ハンドラと操作の名前を付ける（2回目の resume の defect の文面に使う）．
export function $suspendOp(handler, op, f) {
  f.$where = `operation \`${op}\` of handler \`${handler}\``;
  return f;
}

const $GeneratorPrototype = Object.getPrototypeOf(function* () {}).prototype;

function $isGenerator(x) {
  return typeof x === "object" && x !== null && $GeneratorPrototype.isPrototypeOf(x);
}

// 中断しうる呼び出しで呼んだ関数の値（か fn の操作）が中断しないものだったときは，値をそのまま返す．
export function* $maybe(result) {
  return $isGenerator(result) ? yield* result : result;
}

// 1つの起動（main か，テスト関数）．`done(ok, value)` を1回だけ呼ぶ．
// 終わった後に残った継続（resume されずに捨てられたものや，後から resume されたもの）は進めない（14.5）．
class $Root {
  constructor(done) {
    this.done = done;
    this.over = false;
  }

  finish(ok, value) {
    if (this.over) return;
    this.over = true;
    this.done(ok, value);
  }
}

// 中断しうる関数を呼んだジェネレータ `gen` を最後まで進める．`done(ok, value)` は，終われば
// `(true, 戻り値)`，defect なら `(false, 例外)` で呼ばれる．中断したまま resume されなければ呼ばれない．
// 返り値は止める関数で，呼ぶとそれ以降の継続を捨てる．
export function $start(gen, done) {
  const root = new $Root(done);
  $step(root, gen, (ok, value) => root.finish(ok, value), "next", undefined);
  return () => {
    root.over = true;
  };
}

// `gen` を `mode`（"next" か "throw"）と `input` で進め，次に中断するか終わるまで続ける．
// 終われば `end(ok, value)` を呼ぶ．
function $step(root, gen, end, mode, input) {
  for (;;) {
    if (root.over) return;
    let r;
    try {
      r = mode === "next" ? gen.next(input) : gen.throw(input);
    } catch (error) {
      end(false, error);
      return;
    }
    if (r.done) {
      end(true, r.value);
      return;
    }
    const request = r.value;
    if (request instanceof $Await) {
      // ホストの例外（拒否）は defect にして，待っていた位置へ投げる（12.5）．
      Promise.resolve(request.promise).then(
        (value) => $step(root, gen, end, "next", value),
        (error) => $step(root, gen, end, "throw", $foreign(error)),
      );
      return;
    }
    const now = $performOp(root, gen, end, request);
    if (now === undefined) return;
    mode = now.mode;
    input = now.value;
  }
}

// suspend fn の操作の実装を，作った resume を最後の引数にして呼ぶ．その場で継続へ渡すもの
// （`{ mode, value }`）があれば返し，中断したなら undefined を返す．
//
// 継続へ渡すのは，resume された値か，操作の実装の失敗（`$Fail`，操作の呼び出しの値になる）か defect の
// どれか1つで，先に起きたもの．resume した後に実装が失敗したら defect にする．
function $performOp(root, gen, end, request) {
  const where = request.op?.$where ?? "a suspend operation";
  let sync = true;
  let now;
  let used = false;
  const pass = (mode, value) => {
    used = true;
    if (sync) now = { mode, value };
    else $step(root, gen, end, mode, value);
  };
  const resume = (value) => {
    if (root.over) return undefined;
    if (used) throw new $Defect(`resume called twice in ${where}`);
    pass("next", value);
    return undefined;
  };
  const settled = (ok, value) => {
    if (!ok) {
      if (used) root.finish(false, value);
      else pass("throw", value);
    } else if (value instanceof $Fail) {
      if (used) root.finish(false, new $Defect(`${where} failed after resume: ${value[Symbol.for("emela.fail")]()}`));
      else pass("next", value);
    }
  };
  try {
    const result = request.op(...request.args, resume);
    // 中断しうる実装（ジェネレータ）は，別の継続として同じ起動の中で進める．
    if ($isGenerator(result)) $step(root, result, settled, "next", undefined);
    else settled(true, result);
  } catch (error) {
    settled(false, error);
  }
  sync = false;
  return now;
}

// assert（仕様 13.2）の失敗．`text` はソースの式の字面で，空でもよい．
export function $assertFail(text) {
  throw new $Defect(text === "" ? "assertion failed" : `assertion failed: ${text}`);
}

// 比較の assert の失敗．両辺は表示した文字列で受ける（失敗したときだけ表示する）．
export function $assertCmp(text, op, left, right) {
  const head = text === "" ? `left ${op} right` : text;
  throw new $Defect(`assertion failed: ${head}\n  left: ${left}\n right: ${right}`);
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

// String.code_points はコードポイントの値のリスト．文字列の反復はコードポイントごとに進む．
export function $strcodepoints(s) {
  const items = [];
  for (const c of s) items.push(c.codePointAt(0));
  return $list(items);
}

// Unicode のスカラー値（0〜10FFFF でサロゲートを除く）でなければ None．
export function $strfromcodepoint(n) {
  if (n < 0 || n > 0x10ffff || (n >= 0xd800 && n <= 0xdfff)) return $None;
  return { $tag: 0, $0: String.fromCodePoint(n) };
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

// ---- ビット演算（仕様 6.6）----
// シフト量が負なら defect，幅（32 か 64）以上なら全部のビットを押し出した値にする．
// JS の `<<` などはシフト量の下位 5bit しか見ないので，範囲外を先に処理する．

function $shiftBy(by) {
  if (by < 0) throw new $Defect(`negative shift amount: ${by}`);
  return by;
}

export function $iand(a, b) {
  return a & b;
}

export function $ior(a, b) {
  return a | b;
}

export function $ixor(a, b) {
  return a ^ b;
}

export function $inot(a) {
  return ~a;
}

export function $ishl(a, by) {
  return $shiftBy(by) >= 32 ? 0 : a << by;
}

export function $ishr(a, by) {
  return a >> Math.min($shiftBy(by), 31);
}

export function $iushr(a, by) {
  return $shiftBy(by) >= 32 ? 0 : (a >>> by) | 0;
}

// Int64 は BigInt．and / or / xor / not は2の補数として働くので巻き戻しは要らない．
export function $land(a, b) {
  return a & b;
}

export function $lor(a, b) {
  return a | b;
}

export function $lxor(a, b) {
  return a ^ b;
}

export function $lnot(a) {
  return ~a;
}

export function $lshl(a, by) {
  return $shiftBy(by) >= 64 ? 0n : BigInt.asIntN(64, a << BigInt(by));
}

export function $lshr(a, by) {
  return a >> BigInt(Math.min($shiftBy(by), 63));
}

export function $lushr(a, by) {
  return $shiftBy(by) >= 64 ? 0n : BigInt.asIntN(64, BigInt.asUintN(64, a) >> BigInt(by));
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
