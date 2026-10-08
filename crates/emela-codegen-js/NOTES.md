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
- （補）`String.length` は `Intl.Segmenter`（ロケールは既定）で書記素クラスタを数える．Segmenter は最初の呼び出しで1つ作って使い回す．書記素の区切りの Unicode の版はホストの ICU に依存する．固定する版は仕様 18.1 #18 で未決．
