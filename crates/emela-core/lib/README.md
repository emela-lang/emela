# 同梱の Prelude と core（提案）

コンパイラに同梱する `.emel` のソースの API の一覧．個々の API は仕様の範囲外（15章）なので，この一覧は確認してもらうための提案として置く．

- 「実装」の列: `intrinsic` は本体のない宣言で，組み込み関数の表（`src/intrinsics.rs`）の実装を使う．`Emela` はソースに本体を書いたもの
- 対象を第1引数に置くので，`xs |> List.map(f)` のようにパイプで書ける
- 引数名は名前付き引数のラベルになる（6.2）．`List.fold(init: 0, f: add)` のように書ける

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
| `or_fail` | `or_fail[A, E](opt: Option[A], error: E) -> A fails E` | （alpha.2） | `None` なら fail．fail の実装を待つ |

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

## 確認してほしい点

- `List.append(xs, ys)` / `List.concat(xss)` の組（Elm，Gleam と同じ）にした．`String.concat(a, b)` と形が違うので，`List.concat(xs, ys)` / `List.flatten(xss)` にそろえる案もある
- `List.tail([])` は `[]` を返す（仕様 17.5 の例 `[x, ..List.tail(xs)]` に合わせた）．`head` と `get` は Option
- 高階関数の引数名はすべて `f`．`List.filter(xs, keep: …)` のような意味のある名前にする案もある
- シフト量が負なら defect，幅以上は全部のビットを押し出した値．JS や WASM のように幅で剰余を取る方式は採らなかった
- 数値の `min` / `max` / `abs` / `compare` はまだ入れていない
