# 同梱の core と std（提案）

コンパイラに同梱する `.emel` のソースの API の一覧．個々の API は仕様の範囲外（15章）なので，この一覧は確認してもらうための提案として置く．

- core（`lib/core/`）: Prelude と `@intrinsic` を持つモジュール．短い名前（`List`，`String`）で import なしで使える（15.2）
- std（`lib/std/`）: `Std` の下のモジュール．`import Std.Console.{Console, SystemConsole}` のように import して使う
- 「実装」の列: `intrinsic` は本体のない宣言で，組み込み関数の表（`src/intrinsics.rs`）の実装を使う（fiber のものは lowering が Core IR の `Expr::Fiber` に写す）．`ランタイム` は中身をランタイムが持つ型．`Emela` はソースに本体を書いたもの
- 対象を第1引数に置くので，`xs |> List.map(f)` のようにパイプで書ける
- 引数名は名前付き引数のラベルになる（6.2）．`List.fold(xs, from: 0, f: add)` のように書ける．名前は 15.3 の約束に従う（畳み込みの初期値は `from`，2つをつなぐのは `append`，平らにするのは `flatten` と `concat`）

## 同梱のソースにホストのファイルを足す

同梱の `.emel` から `@external(js, "./x.mjs", "name")` でホストの関数を呼ぶときは，`.mjs` をそのソースと同じディレクトリ（`lib/std/` など）に置いて埋め込む．

1. `lib/std/x.mjs` を書く．別の `.mjs` を `./y.mjs` のように相対で import してよい
2. `src/sources.rs` の `HOST_FILES` に `("std/x.mjs", include_str!("../lib/std/x.mjs"))` を足す．パスは `lib/` からの相対で `/` 区切り．載せ忘れはテスト `host_files_match_lib_dir` が見つける
3. `.emel` に `@external(js, "./x.mjs", "name")` を書く．パスはそのソースのディレクトリからの相対

build，run，test では，lowering がその指定を `lib/` からのパス（`std/x.mjs`）にし，JS の出力はそれを `./emela_core/std/x.mjs` として import する．出力先（`target/emela/js/` など）の `emela_core/` の下に同梱の `.mjs` を全部並べて書き出す．どの外部関数も同梱のファイルを指さなければ何も書き出さない．表にないファイルを指すと内部の誤りになる．

C のターゲット（12.1）も同じ手順で，`lib/std/x.c` を書いて `HOST_FILES` に足し，`.emel` に `@external(c, "./x.c", "name")` を書く．suspend fn の外部関数は 12.6 の形（最後に `em_task *` を受け取り，`em_complete_*` で完了を知らせる）で書き，出力先に書き出す `emela.h` を include する．C の出力は，使う外部関数が指す `.c` だけを出力先（`target/emela/c/`）の `emela_core/` の下に書き出し，`main.c` と一緒に cc でコンパイルする．関数のプロトタイプは 12.4 の表で決まる（String は `const char *` で受け，返すときは `malloc` した `char *`）．`-std=c11` でコンパイルするので，POSIX の関数を使うときは機能検査マクロに気をつける．

# core

## Prelude（`core/prelude.emel`）

| 宣言 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `Bool` | `enum Bool { False, True }` | Emela | 真偽値．導出する Ord で `False < True` |
| `Option` | `enum Option[A] { Some(A), None }` | Emela | 値があるか，ないか |
| `Ordering` | `enum Ordering { Less, Equal, Greater }` | Emela | 比較の結果 |
| `panic` | `panic(message: String) -> Never` | intrinsic | defect を起こす |
| `todo` | `todo() -> Never` | intrinsic | `not yet implemented` の defect |
| `dbg` | `dbg[A: Show](value: A) -> A` | intrinsic | 表示を標準エラーに出して値を返す |
| `ParseError` | `error ParseError(input: String)` | Emela | `Int.parse` と `Float.parse` の形の誤り．`input` は入力の全体 |

## List（`core/list.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `map` | `map[A, B](xs: List[A], f: fn(A) -> B) -> List[B]` | Emela | 各要素に `f` を適用 |
| `filter` | `filter[A](xs: List[A], f: fn(A) -> Bool) -> List[A]` | Emela | `f` が True の要素だけ残す |
| `fold` | `fold[A, B](xs: List[A], from: B, f: fn(B, A) -> B) -> B` | Emela | 先頭から畳み込む |
| `length` | `length[A](xs: List[A]) -> Int` | Emela | 要素の数 |
| `is_empty` | `is_empty[A](xs: List[A]) -> Bool` | Emela | 空か |
| `get` | `get[A](xs: List[A], index: Int) -> Option[A]` | Emela | `index` 番目（0 始まり）．範囲外と負は `None` |
| `head` | `head[A](xs: List[A]) -> Option[A]` | Emela | 先頭の要素 |
| `tail` | `tail[A](xs: List[A]) -> List[A]` | Emela | 先頭を除いた残り．空なら空 |
| `reverse` | `reverse[A](xs: List[A]) -> List[A]` | Emela | 逆順 |
| `append` | `append[A](xs: List[A], ys: List[A]) -> List[A]` | Emela | 2つのリストをつなぐ |
| `flatten` | `flatten[A](xss: List[List[A]]) -> List[A]` | Emela | リストのリストを平らにする |
| `partition` | `partition[A](xs: List[A], f: fn(A) -> Bool) -> (List[A], List[A])` | Emela | True と False の要素に分ける |
| `any` | `any[A](xs: List[A], f: fn(A) -> Bool) -> Bool` | Emela | True になる要素があるか（見つけたら止める） |
| `all` | `all[A](xs: List[A], f: fn(A) -> Bool) -> Bool` | Emela | すべて True か（False で止める） |
| `find` | `find[A](xs: List[A], f: fn(A) -> Bool) -> Option[A]` | Emela | True になる最初の要素 |
| `range` | `range(from: Int, to: Int) -> List[Int]` | Emela | `from` 以上 `to` 未満 |
| `zip` | `zip[A, B](xs: List[A], ys: List[B]) -> List[(A, B)]` | Emela | 同じ位置の要素の組．短い方に合わせる |

## Option（`core/option.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `map` | `map[A, B](opt: Option[A], f: fn(A) -> B) -> Option[B]` | Emela | 中身に `f` を適用 |
| `and_then` | `and_then[A, B](opt: Option[A], f: fn(A) -> Option[B]) -> Option[B]` | Emela | Option を返す `f` を適用して平らにする |
| `or` | `or[A](opt: Option[A], default: A) -> A` | Emela | 中身か，`None` なら `default` |
| `is_some` | `is_some[A](opt: Option[A]) -> Bool` | Emela | `Some` か |
| `is_none` | `is_none[A](opt: Option[A]) -> Bool` | Emela | `None` か |
| `or_fail` | `or_fail[A, E](opt: Option[A], err: E) -> A fails E` | Emela | 中身か，`None` なら `err` で fail（7.2） |

## String（`core/string.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `length` | `length(s: String) -> Int` | intrinsic | 書記素クラスタの数 |
| `byte_size` | `byte_size(s: String) -> Int` | intrinsic | UTF-8 のバイト数 |
| `append` | `append(a: String, b: String) -> String` | intrinsic | 2つをつなぐ |
| `concat` | `concat(parts: List[String]) -> String` | Emela | 全部を順につなぐ（`join(parts, "")`） |
| `contains` | `contains(s: String, sub: String) -> Bool` | intrinsic | `sub` を含むか |
| `starts_with` | `starts_with(s: String, prefix: String) -> Bool` | intrinsic | `prefix` で始まるか |
| `ends_with` | `ends_with(s: String, suffix: String) -> Bool` | intrinsic | `suffix` で終わるか |
| `split` | `split(s: String, sep: String) -> List[String]` | intrinsic | `sep` で分ける．空なら書記素ごと |
| `chars` | `chars(s: String) -> List[String]` | intrinsic | 書記素クラスタごとのリスト |
| `join` | `join(parts: List[String], sep: String) -> String` | intrinsic | `sep` を挟んでつなぐ |
| `trim` | `trim(s: String) -> String` | intrinsic | 前後の White_Space を取り除く |
| `code_points` | `code_points(s: String) -> List[Int]` | intrinsic | コードポイントの値のリスト |
| `from_code_point` | `from_code_point(n: Int) -> Option[String]` | intrinsic | スカラー値でなければ `None` |

## Int（`core/int.emel`）

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `to_string` | `to_string(n: Int) -> String` | intrinsic | 10進の文字列 |
| `parse` | `parse(s: String) -> Int fails ParseError` | Emela（中は intrinsic） | 10進の数字と先頭の `-` か `+`．それ以外と範囲外は `ParseError` |
| `checked_add` | `checked_add(a: Int, b: Int) -> Option[Int]` | intrinsic | あふれたら `None` |
| `bit_and` | `bit_and(a: Int, b: Int) -> Int` | intrinsic | ビットごとの論理積 |
| `bit_or` | `bit_or(a: Int, b: Int) -> Int` | intrinsic | ビットごとの論理和 |
| `bit_xor` | `bit_xor(a: Int, b: Int) -> Int` | intrinsic | ビットごとの排他的論理和 |
| `bit_not` | `bit_not(n: Int) -> Int` | intrinsic | ビットの反転 |
| `shift_left` | `shift_left(n: Int, by: Int) -> Int` | intrinsic | 左シフト．`by` が 32 以上で 0，負で defect |
| `shift_right` | `shift_right(n: Int, by: Int) -> Int` | intrinsic | 算術右シフト．32 以上で 0 か -1 |
| `shift_right_unsigned` | `shift_right_unsigned(n: Int, by: Int) -> Int` | intrinsic | 論理右シフト．32 以上で 0 |

## Int64（`core/int64.emel`）

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

## Float（`core/float.emel`）

Int への変換は，結果が Int の範囲外か NaN，±Infinity なら defect．

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `from_int` | `from_int(n: Int) -> Float` | intrinsic | Float にする（どの Int も正確に表せる） |
| `parse` | `parse(s: String) -> Float fails ParseError` | Emela（中は intrinsic） | `to_string` の書く形（`1.5`，`1e+21`，`Infinity`，`NaN`，先頭の符号）．それ以外は `ParseError` |
| `to_string` | `to_string(x: Float) -> String` | intrinsic | 最短で往復できる桁数．整数値なら `.0` を付ける |
| `floor` | `floor(x: Float) -> Int` | intrinsic | 負の無限大の方向へ丸める |
| `ceil` | `ceil(x: Float) -> Int` | intrinsic | 正の無限大の方向へ丸める |
| `round` | `round(x: Float) -> Int` | intrinsic | 最も近い整数．0.5 は 0 から遠い方へ |
| `truncate` | `truncate(x: Float) -> Int` | intrinsic | 0 の方向へ丸める |

# std

## Std.Duration（`std/duration.emel`）

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

## Std.Instant（`std/instant.emel`）

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

## Std.State（`std/state.emel`）

仕様 8.8 の可変な状態．`run` はランタイムが供給するハンドラで，lowering がその場で写す（NOTES.md）．

| 宣言 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `State` | `effect State[S] { fn get() -> S; fn set(value: S) -> (); fn update(f: fn(S) -> S) -> () }` | Emela | 型 `S` の状態を読み書きする |
| `run` | `run[S, A, E, R](initial: S, body: fn() -> A fails E use { State[S], R }) -> (A, S) fails E use R` | ランタイム | 初期値 `initial` の状態で `body` を実行し，値と最後の状態を返す．失敗したら状態は捨てる |

```emel
import Std.State.{State}

fn count() -> Int use { State[Int] } {
  st = use State[Int]
  st.update(fn(n) { n + 1 })
  st.get()
}

fn main() {
  (n, last) = State.run(0, fn() { count() })
}
```

## Std.Console（`std/console.emel`，`std/console.c`）

platform 層と test 層（15.2）．

| 宣言 | 内容 | 説明 |
|---|---|---|
| `Console` | `effect Console { fn print_line(text: String) -> (); fn print_error_line(text: String) -> () }` | 標準出力と標準エラーに1行ずつ書く．失敗しない |
| `SystemConsole` | `handler SystemConsole implements Console` | ホストの標準出力と標準エラー（JS の `console.log` と `console.error`，C の `stdout` と `stderr`）．状態を持たない |
| `TestConsole` | `handler TestConsole implements Console` | `print_line` の行を `State[List[String]]` に書いた順に足す．`print_error_line` は捨てる．`State.run` の中で使う |

```emel
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    (use Console).print_line("hi")
  }
}
```

## Std.Clock（`std/clock.emel`，`std/clock.mjs`，`std/clock.c`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `Clock` | `effect Clock { suspend fn sleep(ms: Int) -> (); fn now() -> Instant }` | 待つ（負なら 0 と同じ）．現在の時点 |
| `SystemClock` | `handler SystemClock implements Clock` | `sleep` は `setTimeout` を `@external suspend fn` で待つ（中断でタイマーを消す，12.2），`now` は `Date.now()`（C では `timespec_get`）．C の `sleep` はランタイムのイベントループのタイマー（`em_after`）で待ち，中断されたらすぐ完了してタイマーを消す（12.6）．状態を持たない．Async に依存する（8.7） |
| `TestClock` | `handler TestClock implements Clock` | 時刻は `State[Instant]` の仮の時刻（8.8）．`sleep(ms)` は本当に待ち，どの fiber も進めずホストの処理もないときに，一番早い予定の時刻まで進めて起こす（同じ時刻なら呼んだ順，負なら 0）．`now` はその時刻．Async に依存する．待つ外部関数 `when_stalled` は JS も C もランタイムが持つ（C は `em_when_stalled`） |

## Std.Random（`std/random.emel`，`std/random.mjs`，`std/random.c`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `Random` | `effect Random { fn int() -> Int; fn float() -> Float }` | Int の全域で一様な値．0 以上 1 未満で一様な値 |
| `SystemRandom` | `handler SystemRandom implements Random` | JS の `crypto.getRandomValues`，C の `arc4random_buf`（macOS）か `getrandom`（Linux）で引く．状態を持たない |

## Std.Env（`std/env.emel`，`std/env.mjs`，`std/env.c`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `Env` | `effect Env { fn get(name: String) -> Option[String] }` | 環境変数の値．設定されていなければ `None`．空の値は `Some("")` |
| `SystemEnv` | `handler SystemEnv implements Env` | JS の `process.env`，C の `getenv` を読む．状態を持たない |

## Std.Fiber（`std/fiber.emel`）

並行性（仕様 11章）．`import Std.Fiber.{Fiber, Scope}` で使う．

| 宣言 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `Fiber` | `@external type Fiber[A]` | ランタイム | fiber のハンドル．スコープの外へ持ち出せない（11.8） |
| `Exit` | `enum Exit[A, E] { Success(A), Failure(Cause[E]) }` | Emela | fiber の終わり方 |
| `Scope` | `@intrinsic effect Scope[E, R] { fn fork[A](f: fn() -> A fails E use R) -> Fiber[A]; fn deferred[A]() -> Deferred[A] }` | 組み込み | スコープの中で fiber を起動する能力．供給できるのは `scope` だけ |
| `scope` | `scope[A, E, R](body: fn() -> A fails E use { Scope[E, R], Async, R }) -> A fails E use { Async, R }` | intrinsic | 本体を実行し，すべての子が終わってから値を返す．子は `scope` を呼んだ時点のハンドラを使う |
| `fork` | `fork[A, E, R](f: fn() -> A fails E use R) -> Fiber[A] use Scope[E, R]` | Emela | `(use Scope).fork(f)` |
| `deferred` | `deferred[A, E, R]() -> Deferred[A] use Scope[E, R]` | Emela | `(use Scope).deferred()` |
| `join` | `join[A](fiber: Fiber[A]) -> A use Async` | intrinsic | 子が終わるのを待って値を返す．中断された子なら defect |
| `await` | `await[A](fiber: Fiber[A]) -> Exit[A, {}] use Async` | intrinsic | 子が終わるのを待って終わり方を返す |
| `interrupt` | `interrupt[A](fiber: Fiber[A]) -> () use Async` | intrinsic | 子を中断し，巻き戻りが終わるまで待つ |
| `attempt` | `attempt[A, E, R](f: fn() -> A fails E use R) -> Exit[A, E] use R` | intrinsic | `f` をその場で実行し，Fail と Die を値に変える．自分への中断は受けない |
| `all` | `all[A, E, R](fs: List[fn() -> A fails E use R]) -> List[A] fails E use { Async, R }` | Emela | すべてを並行に実行して元の順に集める |
| `race` | `race[A, E, R](fs: List[fn() -> A fails E use R]) -> A fails E use { Async, R }` | Emela | 最初に終わったものを採り，残りを中断する |
| `both` | `both[A, B, E, R](f: fn() -> A fails E use R, g: fn() -> B fails E use R) -> (A, B) fails E use { Async, R }` | Emela | 2つを並行に実行して組で返す |
| `map` | `map[A, B, E, R](xs: List[A], f: fn(A) -> B fails E use R) -> List[B] fails E use { Async, R }` | Emela | 各要素に `f` を並行に適用する |
| `timeout` | `timeout[A, E, R](ms: Int, f: fn() -> A fails E use R) -> Option[A] fails E use { Clock, Async, R }` | Emela | `ms` ミリ秒まで待つ．間に合わなければ `f` を中断して `None` |

## Std.Cause（`std/cause.emel`）

| 宣言 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `Cause` | `opaque type Cause[E]` | ランタイム | fiber が正常に終わらなかった原因．中身のエラーは取り出せない |
| `CauseKind` | `enum CauseKind { Fail, Die, Interrupt }` | Emela | 原因の種類 |
| `kind` | `kind[E](c: Cause[E]) -> CauseKind` | intrinsic | 原因の種類 |
| `to_string` | `to_string[E](c: Cause[E]) -> String` | intrinsic | `error: …`，`defect: …`，`interrupted` |
| `raise` | `raise[A, E](c: Cause[E]) -> A fails E` | intrinsic | 起こし直す．Fail は fail，Die は defect，Interrupt は中断の巻き戻し |

## Std.Deferred（`std/deferred.emel`）

| 宣言 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `Deferred` | `@external type Deferred[A]` | ランタイム | 一度だけ値を入れられる入れ物．作れるのは Scope の `deferred` だけ |
| `complete` | `complete[A](d: Deferred[A], value: A) -> Bool` | intrinsic | 最初の1回だけ値を入れて True．2回目からは False |
| `await` | `await[A](d: Deferred[A]) -> A use Async` | intrinsic | 値が入るまで待つ |
| `poll` | `poll[A](d: Deferred[A]) -> Option[A]` | intrinsic | 入っていれば `Some`．待たない |

## Std.Http（`std/http.emel`，`std/http.mjs`）

クライアントだけを持つ．要求と応答は値で，send の上の関数はハンドラなしでも試せる．

| 宣言 | 内容 | 説明 |
|---|---|---|
| `Method` | `enum Method { Get, Head, Post, Put, Patch, Delete, Options }` | 送れるメソッド |
| `Request` | `type Request(method: Method, url: String, headers: List[(String, String)], body: String)` | 要求．ヘッダは書いた順に送る．本文は UTF-8 で送る |
| `Response` | `type Response(status: Int, headers: List[(String, String)], body: String)` | 応答．ヘッダの名前は小文字で名前の順，同じ名前は `, ` でつなぐ（`set-cookie` だけは1つずつ） |
| `InvalidUrl` | `error InvalidUrl(url: String)` | URL として読めない．`http:` と `https:` 以外も |
| `ConnectionFailed` | `error ConnectionFailed(url: String, message: String)` | 応答を受け取れなかった（名前解決，接続，TLS，リダイレクトの回数超え）．`message` に原因 |
| `ResponseNotText` | `error ResponseNotText(url: String)` | 応答の本文が UTF-8 として読めない |
| `UnexpectedStatus` | `error UnexpectedStatus(status: Int, body: String)` | `require_ok` で 2xx でない |
| `Http` | `effect Http { suspend fn send(request: Request) -> Response fails InvalidUrl \| ConnectionFailed \| ResponseNotText }` | 要求を送る．4xx と 5xx は失敗にしない |
| `request` | `request(method: Method, url: String) -> Request` | ヘッダなし，本文なしの要求 |
| `add_header` | `add_header(req: Request, name: String, value: String) -> Request` | ヘッダを末尾に足す |
| `get` | `get(url: String) -> Response fails ... use Http` | GET を送る |
| `post` | `post(url: String, body: String) -> Response fails ... use Http` | 本文つきの POST を送る |
| `header` | `header(res: Response, name: String) -> Option[String]` | 名前の合う最初の値．大文字小文字（ASCII）を区別しない |
| `headers` | `headers(res: Response, name: String) -> List[String]` | 名前の合うすべての値 |
| `encode_query` | `encode_query(pairs: List[(String, String)]) -> String` | `encodeURIComponent` でつないだクエリ（`q=%E6%9D%B1&page=2`） |
| `require_ok` | `require_ok(res: Response) -> Response fails UnexpectedStatus` | 2xx 以外を失敗にする |
| `response` | `response(status: Int, body: String) -> Response` | ヘッダなしの応答 |
| `SystemHttp` | `handler SystemHttp implements Http` | JS の `fetch`．リダイレクトを辿り，Cookie を覚えない．Async に依存する |
| `TestServer` | `type TestServer(respond: fn(Request, Int) -> Option[Response], sent: List[Request])` | TestHttp の相手．`respond` は要求と何回目か（0 から）を受ける |
| `TestHttp` | `handler TestHttp implements Http` | 状態は `State[TestServer]`．`sent` に要求を足し，`respond` の応答でその場で再開する．`None` なら ConnectionFailed |

```emel
import Std.Http.{Http, Request, Response, TestServer, TestHttp}
import Std.State.{State}

@test
fn fetches() {
  server = TestServer(respond: fn(req, n) { Some(Http.response(200, "ok")) }, sent: [])
  (res, after) = State.run(server, fn() { with TestHttp { Http.get("https://example.com") } })
  assert res.body == "ok"
  assert List.length(after.sent) == 1
}
```

## Std.FileSystem（`std/file_system.emel`，`std/file_system.mjs`）

platform 層と test 層（15.2）．テキスト（UTF-8）だけを扱う．操作はすべて `suspend fn`．パスは String のままで，正規化しない．

| 宣言 | 内容 | 説明 |
|---|---|---|
| error | `FileNotFound(path)`，`FileAccessDenied(path)`，`NotADirectory(path)`，`IsADirectory(path)`，`DirectoryNotEmpty(path)`，`InvalidText(path)`，`FileSystemFailure(path, message)` | 種類ごとの error．操作ごとに起こりうるものだけを `fails` に書く |
| `read_text` | `suspend fn read_text(path: String) -> String fails FileNotFound \| FileAccessDenied \| IsADirectory \| InvalidText \| FileSystemFailure` | UTF-8 として読めなければ InvalidText（置換文字にしない．先頭の BOM は残す） |
| `write_text` | `suspend fn write_text(path: String, text: String) -> () fails FileNotFound \| FileAccessDenied \| IsADirectory \| FileSystemFailure` | 作るか上書きする．親のディレクトリがなければ FileNotFound |
| `append_text` | `suspend fn append_text(path: String, text: String) -> ()`（`fails` は write_text と同じ） | なければ作る |
| `exists` | `suspend fn exists(path: String) -> Bool` | 失敗しない．見えなければ False |
| `is_directory` | `suspend fn is_directory(path: String) -> Bool` | 失敗しない |
| `list_directory` | `suspend fn list_directory(path: String) -> List[String] fails FileNotFound \| FileAccessDenied \| NotADirectory \| FileSystemFailure` | 名前だけを名前の順（コードポイントの辞書順）に並べる |
| `create_directory` | `suspend fn create_directory(path: String) -> () fails FileAccessDenied \| NotADirectory \| FileSystemFailure` | 親もまとめて作る．既にあれば何もしない．そのパスか途中にファイルがあれば NotADirectory |
| `remove` | `suspend fn remove(path: String) -> () fails FileNotFound \| FileAccessDenied \| DirectoryNotEmpty \| FileSystemFailure` | ファイルか空のディレクトリを消す |
| `SystemFileSystem` | `handler SystemFileSystem implements FileSystem` | JS の `node:fs/promises`．状態を持たない |
| `TestFiles` | `type TestFiles(files: List[(String, String)], directories: List[String])` | TestFileSystem の状態．files はパスの順．ファイルとディレクトリの祖先もディレクトリとみなす |
| `TestFileSystem` | `handler TestFileSystem implements FileSystem` | `State[TestFiles]` に置き，その場で再開する．エラーは SystemFileSystem と同じ規則．FileAccessDenied，InvalidText，FileSystemFailure は起こさない |

```emel
import Std.FileSystem.{FileSystem, FileNotFound}

fn show(path: String) {
  (use FileSystem).read_text(path) escape {
    FileNotFound(..) -> ""
  }
}
```

## Std.Path（`std/path.emel`）

値の層（15.2）．パスの文字列を組み立てる純粋な関数．区切りは `/` だけ．正規化しない．末尾の `/` は無視する．

| 関数 | シグネチャ | 実装 | 説明 |
|---|---|---|---|
| `join` | `join(a: String, b: String) -> String` | Emela | `/` を1つ挟んでつなぐ．`b` が `/` で始まれば `b`．片方が空ならもう片方 |
| `parent` | `parent(path: String) -> Option[String]` | Emela | 最後の要素を除いたもの．`parent("/a")` は `Some("/")`．要素が1つ，`/`，空は `None` |
| `file_name` | `file_name(path: String) -> Option[String]` | Emela | 最後の要素．`/`，空，`.` と `..` で終わるものは `None` |
| `extension` | `extension(path: String) -> Option[String]` | Emela | ファイル名の最後の `.` より後．`.` がない，先頭の `.` だけ（`.bashrc`），`.` で終わるものは `None` |

## Std.Platform（`std/platform.emel`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `Platform` | `layer Platform { SystemConsole, SystemClock, SystemRandom, SystemEnv, SystemHttp, SystemFileSystem }` | System のハンドラの束．一部を差し替えるときは `with Platform, TestClock { ... }` と後ろに並べる |

```emel
import Std.Platform.{Platform}
import Std.Console.{Console}

fn main() {
  with Platform {
    (use Console).print_line("hi")
  }
}
```

## テスト用のハンドラの例

```emel
import Std.Clock.{Clock, TestClock}
import Std.Console.{TestConsole}
import Std.Instant
import Std.State.{State}

@test
fn greets() {
  (_, lines) = State.run([], fn() { with TestConsole { greet(1) } })
  assert lines == ["Hello, 1!"]
}

@test
fn waits() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, fn() { with TestClock { (use Clock).sleep(1000) } })
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(1000)
}
```

## 確認してほしい点

- `List.tail([])` は `[]` を返す（仕様 17.5 の例 `[x, ..List.tail(xs)]` に合わせた）．`head` と `get` は Option
- 高階関数の引数名はすべて `f`．`List.filter(xs, keep: …)` のような意味のある名前にする案もある
- シフト量が負なら defect，幅以上は全部のビットを押し出した値．JS や WASM のように幅で剰余を取る方式は採らなかった
- 数値の `min` / `max` / `abs` / `compare` はまだ入れていない
- Console の操作は `print_line` と `print_error_line` の2つにした．改行なしの `print` や，入力の読み取りはまだ入れていない
- Instant と Duration の単位はミリ秒にした（JS の `Date.now()` に合わせた）．ナノ秒にすると Int64 で ±292 年になる
- `Instant.from_unix_milliseconds` は公開した．テストの固定の時刻や，利用者の書く Clock のハンドラが Instant を作れるように．現在の時点を取れるのは Clock だけ
- 利用者の書く Clock のハンドラの `now` は，`Instant.from_unix_milliseconds`（#172）か外側の Clock の `now`（#173）で書く．時刻を進める TestClock は State に時刻を置く
- `Duration.to_string` は `1500ms` の形にした．`1.5s` や `1h2m3s` のような形にする案もある
- Random の操作は `int()` と `float()` の2つにした．範囲つきの `between(min, max)` や種を渡せる PRNG はまだ入れていない
- Env の操作は `get` だけにした．全部の変数を返す操作はまだ入れていない
- Http の `header` と `headers` は名前を ASCII の範囲だけで大文字小文字を区別せずに比べる．`String` に小文字にする関数はまだない
- GET と HEAD の空の本文は本文なしで送る．GET と HEAD に空でない本文を書くと ConnectionFailed（fetch が受け付けない）
- FileSystem の外部関数は List も失敗も返せない（12.4，12.5）ので，ホストは結果を `@external type Outcome` で返し，ハンドラが取り出して fail に変える．取り出す関数は `use {}`（純粋）にした
- `Path.join(a, b)` は `b` が絶対パスなら `b` にした（Rust の `Path::join` と Python と同じ．Node の `path.join` は単につなぐ）
- `Path.extension("b.")` は `None` にした（Rust は `Some("")`）
