# 同梱の Prelude と core（提案）

コンパイラに同梱する `.emel` のソースの API の一覧．個々の API は仕様の範囲外（15章）なので，この一覧は確認してもらうための提案として置く．

- 「実装」の列: `intrinsic` は本体のない宣言で，組み込み関数の表（`src/intrinsics.rs`）の実装を使う．`Emela` はソースに本体を書いたもの
- 対象を第1引数に置くので，`xs |> List.map(f)` のようにパイプで書ける
- 引数名は名前付き引数のラベルになる（6.2）．`List.fold(init: 0, f: add)` のように書ける

## core に JS のファイルを足す

core の `.emel` から `@external(js, "./x.mjs", "name")` でホストの関数を呼ぶときは，`.mjs` を `lib/` に置いて埋め込む．

1. `lib/x.mjs` を書く．別の `.mjs` を `./y.mjs` のように相対で import してよい（`lib/` の下のサブディレクトリも使える）
2. `src/sources.rs` の `JS_FILES` に `("x.mjs", include_str!("../lib/x.mjs"))` を足す．パスは `lib/` からの相対で `/` 区切り．載せ忘れはテスト `js_files_match_lib_dir` が見つける
3. core の `.emel` に `@external(js, "./x.mjs", "name")` を書く．パスは `lib/` の直下からの相対（core のソースは `lib/` の直下にある）

build，run，test では，lowering がその指定を `./emela_core/x.mjs` に直し，出力先（`target/emela/js/` など）の `emela_core/` の下に同梱の `.mjs` を全部並べて書き出す．どの外部関数も同梱のファイルを指さなければ何も書き出さない．表にないファイルを指すと内部の誤りになる．

## Prelude（`prelude.emel`）

| 宣言 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `Bool` | `enum Bool { False, True }` | Emela | 真偽値．導出する Ord で `False < True` |
| `Option` | `enum Option[A] { Some(A), None }` | Emela | 値があるか，ないか |
| `Ordering` | `enum Ordering { Less, Equal, Greater }` | Emela | 比較の結果 |
| `panic` | `panic(message: String) -> Never` | intrinsic | defect を起こす |
| `todo` | `todo() -> Never` | intrinsic | `not yet implemented` の defect |
| `dbg` | `dbg[A: Show](value: A) -> A` | intrinsic | 表示を標準エラーに出して値を返す |

## List（`list.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `map` | `map[A, B](xs: List[A], f: fn(A) -> B) -> List[B]` | Emela | 各要素に `f` を適用 |
| `filter` | `filter[A](xs: List[A], f: fn(A) -> Bool) -> List[A]` | Emela | `f` が True の要素だけ残す |
| `fold` | `fold[A, B](xs: List[A], init: B, f: fn(B, A) -> B) -> B` | Emela | 先頭から畳み込む |
| `length` | `length[A](xs: List[A]) -> Int` | Emela | 要素の数 |
| `is_empty` | `is_empty[A](xs: List[A]) -> Bool` | Emela | 空か |
| `get` | `get[A](xs: List[A], index: Int) -> Option[A]` | Emela | `index` 番目（0 始まり）．範囲外と負は `None` |
| `head` | `head[A](xs: List[A]) -> Option[A]` | Emela | 先頭の要素 |
| `tail` | `tail[A](xs: List[A]) -> List[A]` | Emela | 先頭を除いた残り．空なら空 |
| `reverse` | `reverse[A](xs: List[A]) -> List[A]` | Emela | 逆順 |
| `append` | `append[A](xs: List[A], ys: List[A]) -> List[A]` | Emela | 2つのリストをつなぐ |
| `concat` | `concat[A](xss: List[List[A]]) -> List[A]` | Emela | リストのリストを平らにする |
| `partition` | `partition[A](xs: List[A], f: fn(A) -> Bool) -> (List[A], List[A])` | Emela | True と False の要素に分ける |
| `any` | `any[A](xs: List[A], f: fn(A) -> Bool) -> Bool` | Emela | True になる要素があるか（見つけたら止める） |
| `all` | `all[A](xs: List[A], f: fn(A) -> Bool) -> Bool` | Emela | すべて True か（False で止める） |
| `find` | `find[A](xs: List[A], f: fn(A) -> Bool) -> Option[A]` | Emela | True になる最初の要素 |
| `range` | `range(from: Int, to: Int) -> List[Int]` | Emela | `from` 以上 `to` 未満 |
| `zip` | `zip[A, B](xs: List[A], ys: List[B]) -> List[(A, B)]` | Emela | 同じ位置の要素の組．短い方に合わせる |

## Option（`option.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `map` | `map[A, B](opt: Option[A], f: fn(A) -> B) -> Option[B]` | Emela | 中身に `f` を適用 |
| `and_then` | `and_then[A, B](opt: Option[A], f: fn(A) -> Option[B]) -> Option[B]` | Emela | Option を返す `f` を適用して平らにする |
| `unwrap_or` | `unwrap_or[A](opt: Option[A], default: A) -> A` | Emela | 中身か，`None` なら `default` |
| `is_some` | `is_some[A](opt: Option[A]) -> Bool` | Emela | `Some` か |
| `is_none` | `is_none[A](opt: Option[A]) -> Bool` | Emela | `None` か |
| `or_fail` | `or_fail[A, E](opt: Option[A], err: E) -> A fails E` | Emela | 中身か，`None` なら `err` で fail（7.2） |

## Console（`console.emel`）

platform 層（15.2）のエフェクト．宣言だけで，ハンドラはターゲットごとのモジュールに置く．

| 操作 | シグネチャ | 説明 |
|---|---|---|
| `print_line` | `fn print_line(text: String) -> ()` | `text` と改行を標準出力に書く |
| `print_error_line` | `fn print_error_line(text: String) -> ()` | `text` と改行を標準エラーに書く |

## JsConsole（`js_console.emel`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `JsConsole` | `handler JsConsole implements Console` | JS の `console.log` と `console.error` で書く．状態を持たない |

```emel
fn main() {
  with JsConsole {
    (use Console).print_line("hi")
  }
}
```

## Random（`random.emel`）

platform 層（15.2）のエフェクト．宣言だけで，ハンドラはターゲットごとのモジュールに置く．

| 操作 | シグネチャ | 説明 |
|---|---|---|
| `int` | `fn int() -> Int` | Int の全域で一様な値 |
| `float` | `fn float() -> Float` | 0 以上 1 未満で一様な値 |

## JsRandom（`js_random.emel`，`js_random.mjs`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `JsRandom` | `handler JsRandom implements Random` | JS の `crypto.getRandomValues` で引く．状態を持たない |

## Env（`env.emel`）

platform 層（15.2）のエフェクト．宣言だけで，ハンドラはターゲットごとのモジュールに置く．

| 操作 | シグネチャ | 説明 |
|---|---|---|
| `get` | `fn get(name: String) -> Option[String]` | 環境変数の値．設定されていなければ `None`．空の値は `Some("")` |

## JsEnv（`js_env.emel`，`js_env.mjs`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `JsEnv` | `handler JsEnv implements Env` | JS の `process.env` を読む．状態を持たない |

```emel
fn main() {
  with JsConsole, JsRandom, JsEnv {
    home = (use Env).get("HOME") |> Option.unwrap_or("?")
    (use Console).print_line("#{home} #{(use Random).int()}")
  }
}
```

## Clock（`clock.emel`）

platform 層（15.2）のエフェクト．宣言だけで，ハンドラはターゲットごとのモジュールに置く．

| 操作 | シグネチャ | 説明 |
|---|---|---|
| `sleep` | `suspend fn sleep(ms: Int) -> ()` | `ms` ミリ秒待つ．負なら 0 と同じ |
| `now` | `fn now() -> Instant` | 現在の時点 |

## JsClock（`js_clock.emel`，`js_clock.mjs`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `JsClock` | `handler JsClock implements Clock` | `sleep` は `setTimeout`，`now` は `Date.now()`．状態を持たない．Async に依存する（8.7） |

```emel
fn main() {
  with JsConsole, JsClock {
    clock = use Clock
    start = clock.now()
    clock.sleep(100)
    (use Console).print_line(Instant.to_string(start))
  }
}
```

## State（`state.emel`．まだ同梱しない）

仕様 8.8 の可変な状態．宣言だけを置き，`run` の実装（ランタイムが供給するハンドラ）と同梱は次の段（NOTES.md）．

| 宣言 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `State` | `effect State[S] { fn get() -> S; fn set(value: S) -> (); fn update(f: fn(S) -> S) -> () }` | Emela | 型 `S` の状態を読み書きする |
| `run` | `run[S, A, E, R](initial: S, body: fn() -> A fails E use { State[S], R }) -> (A, S) fails E use R` | intrinsic | 初期値 `initial` の状態で `body` を実行し，値と最後の状態を返す |

```emel
fn count() -> Int use { State[Int] } {
  st = use State[Int]
  st.update(fn(n) { n + 1 })
  st.get()
}

fn main() {
  (n, last) = State.run(0, fn() { count() })
}
```

## String（`string.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `length` | `length(s: String) -> Int` | intrinsic | 書記素クラスタの数 |
| `byte_size` | `byte_size(s: String) -> Int` | intrinsic | UTF-8 のバイト数 |
| `concat` | `concat(a: String, b: String) -> String` | intrinsic | 2つをつなぐ |
| `contains` | `contains(s: String, sub: String) -> Bool` | intrinsic | `sub` を含むか |
| `starts_with` | `starts_with(s: String, prefix: String) -> Bool` | intrinsic | `prefix` で始まるか |
| `ends_with` | `ends_with(s: String, suffix: String) -> Bool` | intrinsic | `suffix` で終わるか |
| `split` | `split(s: String, sep: String) -> List[String]` | intrinsic | `sep` で分ける．空なら書記素ごと |
| `chars` | `chars(s: String) -> List[String]` | intrinsic | 書記素クラスタごとのリスト |
| `join` | `join(parts: List[String], sep: String) -> String` | intrinsic | `sep` を挟んでつなぐ |
| `trim` | `trim(s: String) -> String` | intrinsic | 前後の White_Space を取り除く |
| `from_int` | `from_int(n: Int) -> String` | intrinsic | 10進の文字列（`Int.to_string` と同じ） |
| `code_points` | `code_points(s: String) -> List[Int]` | intrinsic | コードポイントの値のリスト |
| `from_code_point` | `from_code_point(n: Int) -> Option[String]` | intrinsic | スカラー値でなければ `None` |

## Int（`int.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `to_string` | `to_string(n: Int) -> String` | intrinsic | 10進の文字列 |
| `to_float` | `to_float(n: Int) -> Float` | intrinsic | Float にする |
| `to_int64` | `to_int64(n: Int) -> Int64` | intrinsic | Int64 にする |
| `checked_add` | `checked_add(a: Int, b: Int) -> Option[Int]` | intrinsic | あふれたら `None` |
| `bit_and` | `bit_and(a: Int, b: Int) -> Int` | intrinsic | ビットごとの論理積 |
| `bit_or` | `bit_or(a: Int, b: Int) -> Int` | intrinsic | ビットごとの論理和 |
| `bit_xor` | `bit_xor(a: Int, b: Int) -> Int` | intrinsic | ビットごとの排他的論理和 |
| `bit_not` | `bit_not(n: Int) -> Int` | intrinsic | ビットの反転 |
| `shift_left` | `shift_left(n: Int, by: Int) -> Int` | intrinsic | 左シフト．`by` が 32 以上で 0，負で defect |
| `shift_right` | `shift_right(n: Int, by: Int) -> Int` | intrinsic | 算術右シフト．32 以上で 0 か -1 |
| `shift_right_unsigned` | `shift_right_unsigned(n: Int, by: Int) -> Int` | intrinsic | 論理右シフト．32 以上で 0 |

## Int64（`int64.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `from_int` | `from_int(n: Int) -> Int64` | intrinsic | Int から作る |
| `to_string` | `to_string(n: Int64) -> String` | intrinsic | 10進の文字列．接尾辞なし |
| `to_int` | `to_int(n: Int64) -> Int` | intrinsic | 下位 32bit に巻き戻す |
| `bit_and` / `bit_or` / `bit_xor` | `(a: Int64, b: Int64) -> Int64` | intrinsic | ビットごとの演算 |
| `bit_not` | `bit_not(n: Int64) -> Int64` | intrinsic | ビットの反転 |
| `shift_left` | `shift_left(n: Int64, by: Int) -> Int64` | intrinsic | 左シフト．`by` が 64 以上で 0，負で defect |
| `shift_right` | `shift_right(n: Int64, by: Int) -> Int64` | intrinsic | 算術右シフト．64 以上で 0 か -1 |
| `shift_right_unsigned` | `shift_right_unsigned(n: Int64, by: Int) -> Int64` | intrinsic | 論理右シフト．64 以上で 0 |

## Float（`float.emel`）

Int への変換は，結果が Int の範囲外か NaN，±Infinity なら defect．

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `to_string` | `to_string(x: Float) -> String` | intrinsic | 最短で往復できる桁数．整数値なら `.0` を付ける |
| `floor` | `floor(x: Float) -> Int` | intrinsic | 負の無限大の方向へ丸める |
| `ceil` | `ceil(x: Float) -> Int` | intrinsic | 正の無限大の方向へ丸める |
| `round` | `round(x: Float) -> Int` | intrinsic | 最も近い整数．0.5 は 0 から遠い方へ |
| `truncate` | `truncate(x: Float) -> Int` | intrinsic | 0 の方向へ丸める |

## Duration（`duration.emel`）

時間の長さ．中身はミリ秒の Int64 の opaque な型（15.2）．あふれは Int64 の規則で巻き戻る．

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `milliseconds` | `milliseconds(n: Int64) -> Duration` | Emela | `n` ミリ秒 |
| `seconds` | `seconds(n: Int64) -> Duration` | Emela | `n` 秒 |
| `minutes` | `minutes(n: Int64) -> Duration` | Emela | `n` 分 |
| `hours` | `hours(n: Int64) -> Duration` | Emela | `n` 時間 |
| `to_milliseconds` | `to_milliseconds(d: Duration) -> Int64` | Emela | ミリ秒の数 |
| `to_seconds` | `to_seconds(d: Duration) -> Int64` | Emela | 秒の数．端数は 0 の方向へ切り捨て |
| `add` | `add(d: Duration, other: Duration) -> Duration` | Emela | 和 |
| `subtract` | `subtract(d: Duration, other: Duration) -> Duration` | Emela | 差．負になりうる |
| `compare` | `compare(d: Duration, other: Duration) -> Ordering` | Emela | 比べる |
| `to_string` | `to_string(d: Duration) -> String` | Emela | `1500ms` |

## Instant（`instant.emel`）

ある時点．中身は Unix エポックからのミリ秒の Int64 の opaque な型（15.2）．現在の時点は Clock のエフェクトが返す（このモジュールでは取れない）．

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `from_unix_milliseconds` | `from_unix_milliseconds(ms: Int64) -> Instant` | Emela | Unix エポックから `ms` ミリ秒の時点 |
| `to_unix_milliseconds` | `to_unix_milliseconds(t: Instant) -> Int64` | Emela | Unix エポックからのミリ秒 |
| `add` | `add(t: Instant, d: Duration) -> Instant` | Emela | `d` だけ後 |
| `subtract` | `subtract(t: Instant, d: Duration) -> Instant` | Emela | `d` だけ前 |
| `since` | `since(t: Instant, earlier: Instant) -> Duration` | Emela | `earlier` から `t` までの長さ |
| `compare` | `compare(t: Instant, other: Instant) -> Ordering` | Emela | 早い方が小さい |
| `to_string` | `to_string(t: Instant) -> String` | Emela | UTC の ISO 8601（`2025-10-12T03:04:05.678Z`） |

## 確認してほしい点

- `List.append(xs, ys)` / `List.concat(xss)` の組（Elm，Gleam と同じ）にした．`String.concat(a, b)` と形が違うので，`List.concat(xs, ys)` / `List.flatten(xss)` にそろえる案もある
- `List.tail([])` は `[]` を返す（仕様 17.5 の例 `[x, ..List.tail(xs)]` に合わせた）．`head` と `get` は Option
- 高階関数の引数名はすべて `f`．`List.filter(xs, keep: …)` のような意味のある名前にする案もある
- シフト量が負なら defect，幅以上は全部のビットを押し出した値．JS や WASM のように幅で剰余を取る方式は採らなかった
- 数値の `min` / `max` / `abs` / `compare` はまだ入れていない
- Console の操作は `print_line` と `print_error_line` の2つにした．改行なしの `print` や，入力の読み取りはまだ入れていない
- エフェクトとハンドラのモジュールは `Console` と `JsConsole` に分けた（ターゲットの名前を前に付ける）．`Js` 1つに JS 用のハンドラを全部まとめる案もある
- Instant と Duration の単位はミリ秒にした（JS の `Date.now()` に合わせた）．ナノ秒にすると Int64 で ±292 年になる
- `Instant.from_unix_milliseconds` は公開した．テストの固定の時刻や，利用者の書く Clock のハンドラが Instant を作れるように．現在の時点を取れるのは Clock だけ
- 利用者の書く Clock のハンドラの `now` は，`Instant.from_unix_milliseconds`（#172）か外側の Clock の `now`（#173）で書く．時刻を進める TestClock は状態を持つハンドラの段で足す
- `Duration.to_string` は `1500ms` の形にした．`1.5s` や `1h2m3s` のような形にする案もある
- Random の操作は `int()` と `float()` の2つにした．範囲つきの `between(min, max)` や種を渡せる PRNG はまだ入れていない
- Env の操作は `get` だけにした．全部の変数を返す操作はまだ入れていない
