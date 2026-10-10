# emela-codegen-js の補足

仕様にない判断を「（補）」として残す．

## 値の表現

| Emela | JS |
| --- | --- |
| Int | number（演算のたびに 32bit へ巻き戻す） |
| Int64 | BigInt（`BigInt.asIntN(64, …)` で巻き戻す） |
| Float | number |
| Bool | boolean |
| String | string |
| `()` | `undefined` |
| タプル | 配列 `[a, b]` |
| List | cons セル `{ h, t }` の連なり，空リストは `null` |
| enum / type の値 | `{ $tag: 添字, …フィールド }` |

- （補）enum の値は `$tag` に `EnumDef::variants` の添字（0始まり）を持つ．位置引数のフィールドは `$0`，`$1`，…，名前付きのフィールドはその名前のプロパティにする．`type`（構成子1つ）も `$tag: 0` を持つ．
- （補）フィールドを持たないバリアントはモジュールの先頭で `const Enum$Variant = Object.freeze({ $tag: k })` として1つだけ作り，共有する．フィールドを持つ値は速さのため freeze しない．
- （補）名前付きフィールド `__proto__` は計算されたキー `["__proto__"]` で書く（オブジェクトリテラルで原型を書き換えないため）．
- （補）List は配列でなく cons セルにした．`[x, ..rest]` のパターンと `[x, ..rest]` の構築が O(1) になる．
- （補）Bool は JS の boolean．enum として持つと `if` と `&&` が遅くなるため．

## 名前

- （補）局所変数は `ヒント$番号`，生成コードの一時変数とラベルは `$t0`，`$m0`，`$L0`，ランタイムの関数は `$` で始まる．Emela の識別子は `$` を含まないので衝突しない．
- （補）トップレベル関数は Emela の名前のまま出す．JS の予約語や生成コードが使う大域名（`Math`，`BigInt` など）と重なるときは末尾に `$` を付け，`export { new$ as new }` のように Emela の名前で公開する．

## 意味

- （補）Int の加減算は `(a + b) | 0`，乗算は `Math.imul`，除算と剰余はランタイムの `$idiv` / `$irem`（ゼロ除算の defect と `| 0` での巻き戻し・`-0` の除去を受け持つ）．
- （補）Int64 の除算 `$ldiv` は `i64::MIN / -1` を `BigInt.asIntN` で巻き戻して `i64::MIN` にする．
- （補）Float の `%` は JS の `%`（fmod，被除数の符号）．仕様 16.1 に反映済み．
- （補）文字列の順序比較はコードポイントの辞書順（`$scmp`）．JS の `<` は UTF-16 の順なので使わない．等値は `===`．仕様 10.6，16.2 に反映済み．
- （補）基本型以外の `==` は `$eq` で構造を比べる．長いリストでもスタックを使わないよう明示的なスタックで辿る．NaN は自分自身とも等しくない．
- （補）補間での表示: Bool は `True` / `False`（enum のバリアント名），Float は整数値で有限なら `1.0` のように `.0` を付ける．それ以外の Float は JS の `String`（`1e+21`，`-2.5e-8`，`Infinity`，`NaN`）．桁数は最短で往復できる桁数（JS の Number#toString と同じ規則）で，WASM 側でもこれに合わせる．Int64 は `n` を付けない．仕様 10.6 に反映済み．
- （補）defect は `$Defect`（`name` が `"Defect"` の `Error`）を投げる．メッセージは英語で `division by zero`，`unreachable: no match arm matched`，`panic` は渡したメッセージ．
- （補）match の対象はリテラルか変数でなければ一時変数に1回だけ入れる．腕は `if` の列で，ガードはパターンの束縛の後に評価する．無条件に合う腕があればそれ以降の腕と `$unreachable()` を出さない．
- （補）評価順: 引数の後ろの方に文を要する式（`let`，`match` など）があると，それより左の引数を先に一時変数へ束縛してから文を出す．リテラル・変数・関数・ラムダは束縛しない．
- （補）`Loop` は `let $t… = 初期値; $L: for (;;) { const 変数 = $t…; … }` で，`Recur` は `$t…` へ代入して `continue $L`．
- （補）IR が不正（演算と型の組み合わせ，引数の数，`Loop` の外の `Recur`）なら `emit_module` は panic する．型検査を通った IR だけが来る前提．

## ランタイム

- （補）ランタイムは `src/runtime.mjs` 1つで，`RUNTIME` として埋め込む．`RuntimeMode::Import` は使った名前だけを `./emela_runtime.mjs` から import し，ファイルの書き出しは呼び出し側が行う．`RuntimeMode::Inline` は行頭の `export ` を外して出力の先頭に埋め込む．そのため runtime.mjs では `export` を行頭にだけ書く．
- （補）組み込み関数は `BuiltinInfo::js` の名前のランタイムの関数を呼ぶ．`Int.to_float` のような恒等の関数も，生成コードを一様にするため関数として呼ぶ．
- （補）`checked_add` などランタイムが返す `None` は `$None` で，Prelude の Option を持つモジュールは `const Option$None = $None;` として同じ値を共有する．
- （補）`String.length` は `Intl.Segmenter`（ロケールは既定）で書記素クラスタを数える．Segmenter は最初の呼び出しで1つ作って使い回す．書記素の区切りの Unicode の版はホストの ICU に依存する．固定する版は仕様 18.1 #18 で未決．

## 表示（Show の仮実装）

- （補）表示関数は型から作る．基本型とリスト，タプルはランタイムの `$showInt`，`$showList(xs, f)`，`$showTuple(t, [f0, f1])` など．enum ごとに `function $show$Name(v, $a0, …)` を使った分だけモジュールの末尾に出し，型引数の表示関数を `$a0`，`$a1`，… で受ける（辞書渡しの形）．再帰する enum もそのまま自分を呼ぶ．
- （補）表示はソースの書き方に合わせる: `Circle(radius: 1.0)`，`Rect(1.0, 2.0)`，`Empty`，`(1, "a")`，`[1, 2, 3]`，`()`，`Some("a")`．`type` も `Point(x: 1, y: 2)`．
- （補）補間では最上位の String だけを引用符なしで埋め込む．リストや構成子の中の String は `dbg` と同じく引用符で囲んでエスケープする（`"#{["a"]}"` は `["a"]`）．
- （補）String の表示のエスケープは言語のエスケープ `\"`，`\\`，`\n`，`\#` を使い，`\#` は `#{` の前だけに付ける．言語にエスケープのない制御文字は `\t`，`\r`，`\u{…}` と出す（エスケープの一覧は仕様 18.1 #11 で未決）．
- （補）`-0.0` の表示は `0.0`（JS の `String(-0)` が `"0"` のため．10.6 の「JS の Number#toString と同じ規則」に従う）．
- （補）`dbg` は `console.error` で表示を1行出す（ブラウザでも動くように `process.stderr` は使わない）．位置（ファイルと行）はまだ出さない．構文木とつなぐときに足す．
- （補）enum のリストのような深い入れ子の表示はリストの部分だけループで辿り，enum の入れ子は再帰で辿る．深い木の表示はスタックの深さに制限される．
- （補）関数型（`Type::Fn`）の表示を求める IR は panic する（10.6．型検査で弾く前提）．`Type::Param` も enum の表示関数の外では panic する．

## assert（仕様 13.2）

- （補）比較の assert（`AssertCond::Compare`）は両辺を左から1回ずつ評価して一時変数に入れ，比較が偽のときだけ両辺を表示関数で文字列にして `$assertCmp` で defect を投げる．表示は `dbg` と同じ（String は引用符付き）
- （補）defect のメッセージは `assertion failed: <式の字面>` の後に `\n  left: <左辺>\n right: <右辺>` を続ける（rustc の `assert_eq!` に近い形）．字面が空なら `left == right` のように演算子だけを出す
- （補）比較でない assert（`AssertCond::Bool`）は `assertion failed: <式の字面>`（字面が空なら `assertion failed`）．値は表示しない
- （補）assert は文を要する式（`is_simple` が偽）として扱う．左の引数は先に一時変数へ束縛され，評価順が保たれる

## テストランナー

- （補）テストの起動用モジュール（`emit_test_main`）は出力したモジュールを `import * as $m` で読み，`test_runner.mjs` を `export` を外して埋め込み，`$runTests([[表示名, $m["名前"]], …])` を呼ぶ．関数は Emela の名前（JS の予約語なら `export { new$ as new }` の公開名）で引く
- （補）結果は1テストごとに `\x1eemela-test {"event":"result","name":…,"outcome":"ok"|"defect"|"error","message":…}`，最後に `{"event":"done"}` を標準出力に書く．頭の制御文字で利用者の出力と見分ける
- （補）`name` が `"Defect"` でない例外（外部関数が投げたもの）も defect として報告し，メッセージに `TypeError: …` のように例外の名前を前に付ける（7.5 の「外部関数が投げた例外」）
