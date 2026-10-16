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
| error の値 | `{ $tag: $err$名前, …フィールド }`（`$tag` は error ごとの印） |
| 失敗 | ランタイムの `$Fail`（`error` にエラーの値） |
| ハンドラの値（能力の値） | 操作の名前を鍵に関数を持つオブジェクト `{ print_line: (text) => … }` |
| 証拠の表 | エフェクトの名前を鍵にハンドラの値を持つオブジェクト．`__proto__` で外側の表につなぐ |

- （補）enum の値は `$tag` に `EnumDef::variants` の添字（0始まり）を持つ．位置引数のフィールドは `$0`，`$1`，…，名前付きのフィールドはその名前のプロパティにする．`type`（構成子1つ）も `$tag: 0` を持つ．
- （補）フィールドを持たないバリアントはモジュールの先頭で `const Enum$Variant = Object.freeze({ $tag: k })` として1つだけ作り，共有する．フィールドを持つ値は速さのため freeze しない．
- （補）名前付きフィールド `__proto__` は計算されたキー `["__proto__"]` で書く（オブジェクトリテラルで原型を書き換えないため）．
- （補）List は配列でなく cons セルにした．`[x, ..rest]` のパターンと `[x, ..rest]` の構築が O(1) になる．
- （補）Bool は JS の boolean．enum として持つと `if` と `&&` が遅くなるため．

## エラー（仕様 7章，16.5）

`src/emit/errors.rs`．失敗は戻り値として返し，例外にしない（16.5）．

- （補）`fail e` は `new $Fail(e)` を失敗の行き先へ渡す．行き先は最も内側の escape の対象の中ならその escape，それ以外は関数の呼び出し元（`return`）．ラムダの本体は別の関数なので，ラムダの中の失敗はラムダの呼び出し元へ返る．
- （補）失敗しうる呼び出し（`Expr::Call::fails`）は結果を一時変数に入れ，`instanceof $Fail` なら行き先へ渡す．呼び出し元へ返す位置（`return f(x)`）では確かめずにそのまま返す．`fails` が偽の呼び出しは今までどおり確かめない．E が空の関数だけのプログラムの出力は変わらない．
- （補）失敗しうる呼び出しは文を要する式（`is_simple` が偽）として扱う．左の引数は先に一時変数へ束縛され，評価順が保たれる．
- （補）escape は2重のラベル付きブロック（`$x…: { $e…: { 対象; break $x…; } 腕; 外へ伝える }`）．対象の中の失敗は `{ $t = 失敗; break $e…; }` で腕の側へ移る．対象の中では呼び出し元へ直接返せないので，escape の値はいつも一時変数で受ける．腕は対象の外にあるので，腕の中の失敗は外側の行き先へ伝わる．
- （補）腕は上から `if (エラー.$tag === $err$名前 && パターンの条件) { …; break $x…; }` と並べる．どの腕にも合わなかった失敗は，腕に書かれた error でも書かれていない error でも，そのまま外側へ伝える（7.3，18.1 #20）．ガードのない無条件の腕（パターンが error の構成子だけ）より下の，同じ error の腕は出さない．
- （補）error の値は `$tag` にバリアントの添字でなく error ごとの印 `$err$名前`（`Object.freeze({ show })`）を持つ．型引数の error（`fn raise[E](err: E) { fail err }`）は出力の時点でどの error か分からないので，値の側で見分ける．error は構成子が1つなので，パターンと表示関数は `$tag` を見ない．`$eq` は同じ印を `===` で比べる．
- （補）印はモジュールの先頭に error の数だけ出し，`show` は表示関数（`$show$名前`．関数の宣言の巻き上げで先に使える）．関数型のフィールドを持つ error は表示できない（10.6）ので，`show` は error の名前だけを返す．
- （補）`$Fail` は `Symbol.for("emela.fail")` の鍵の関数を持ち，エラーの値の表示を返す．`emela run` の起動用モジュールとテストランナーは `$Fail` のクラスを読み込まない（ランタイムを埋め込んだ出力でも同じ鍵で見分けられる）．

## エフェクト（仕様 8章，16.5）

`src/emit/effects.rs`．ハンドラは引数で渡して直接呼ぶ（16.5）．

- （補）証拠の表は `{ __proto__: 外側の表, Console_Console: h }` のオブジェクトで，with ごとに新しく作る．外側の表は書き換えないので，外側で取り出した能力の値や外側の表を捕まえたクロージャに影響しない．外側の表がなければ `__proto__: null`．鍵は IR のエフェクトの名前の `.` を `_` にしたもの（大文字で始まるので `Object.prototype` の名前とぶつからない）．
- （補）`use X` は表のプロパティの参照 `ev.Console_Console`，操作の呼び出しは `h.print_line(text)`．操作の名前 `__proto__` は `["__proto__"]` と書く．
- （補）ハンドラの値は，handler ごとの関数（`JsConsole_JsConsole()`）が返すオブジェクトリテラル．操作はアロー関数で，失敗は `$Fail` を返す（ラムダと同じ）．
- （補）証拠の表の鍵は IR のエフェクトの名前を識別子にしたもの．型引数のあるエフェクト（`State.State[Int]`，8.1）は，名前の `[`，`]`，`,` が `_` に潰れて重なりうるので，IR のエフェクトの番号を足す（`State_State_Int_$3`）．型引数のないエフェクトの鍵は今までどおり
- （補）`State.run` の状態の入れ物は，ランタイムの `$cellNew`，`$cellGet`，`$cellSet`（`{ value }` のオブジェクト）

### init と release（仕様 8.4，8.6，14.2，14.4，14.5）

`src/emit/effects.rs` の `with_` とランタイムの `$releaseDefect`，`$retire`，`$Root#interrupt`．

- （補）release のある束縛（`WithBind::release`）ごとに，それより後ろの束縛と本体を `try` で囲み，`finally` で release を評価する．値，失敗（`$Fail` を `return` するか escape へ `break` する），defect（例外）のどれで抜けても `finally` が走る．後ろの束縛（init）の失敗も同じ道で抜けるので，初期化の済んだものだけが逆の順に release される．release のある with を値の位置に書いたときは，本体を `try` の中で評価するため一時変数で受ける
- （補）`catch` で本体の投げたものを覚えておき，release が投げたら `$releaseDefect(本体の投げたもの, release の投げたもの, 名前)` を投げ直す．本体が値かエラーで終わっていた（投げたものがない）なら release の defect が結果になる．本体が defect なら，それを投げ直して結果のままにし，release の defect は `note: the release of handler <名前> also failed: <メッセージ>` の行としてメッセージに足す（14.4）．外側の release も同じ `try` で囲まれているので，残りの release はすべて走り，注記は起きた順に並ぶ
- （補）中断されたまま巻き戻るとき（本体が `$Interrupted` を投げたとき．ランタイムが待っている継続へ投げ込んだものも含む）の release の defect は，`interrupted` でなくその defect を結果にする．release の中で suspend な操作が resume されないまま捨てられた（`$Interrupted`）ときは，本体が defect ならそれを結果のままにし（注記はしない），そうでなければ `interrupted` を結果にする
- （補）`finally` の最後で `$retire(h, 名前)` を呼び，ハンドラの値の操作をすべて defect（`handler <名前> was used after release`）を投げる関数に置き換える（8.6）．操作の名前が `__proto__` でもよいように `Object.defineProperty` で置き換える．suspend fn の操作も `$perform` が呼んだところで defect になり，呼び出しの位置へ投げられる
- （補）中断されたまま巻き戻るのは，起動（`$Root`）が待っている継続へ `$Interrupted` を投げ込んで行う（14.5．alpha.4 では `gen.return()` で閉じていた．fiber 単位にするため投げ込みに替えた．下の「fiber」）．`$step` は resume か Promise か fiber の操作を待っている継続を `root.waits` に入れておき，`$Root#interrupt` が待ち始めた順の逆にすべてへ投げる．`yield*` は投げたものを内側のジェネレータへ伝えるので，release は内側から走る．release の中でまた中断したら，resume されれば巻き戻りを続け，されないまま残れば次の `interrupt` でもう一度投げてその release を打ち切り，外側へ進む．release の最初の defect か `$Interrupted` で起動を終える
- （補）`$start` の返す関数は，起動が終わっていなければ `interrupt` を呼ぶ（今までは後の継続を捨てるだけだった）．起動用モジュール（emela-driver）とテストランナーは，イベントループが空になったときにこれを呼び，release を走らせてから `interrupted`（130 か，そのテストの interrupted）を報告する．release が Promise を待てばループは続き，中断したまま残れば次にループが空になったときにもう一度呼ぶ
- （補）中断しない関数の中の `$sync` も，その場で resume されなければ同じ `interrupt` を待つ継続がなくなるまで繰り返し，`$sync` が進めていたジェネレータの中の with の release を走らせてから `$Interrupted` を投げる．呼んだ側の with は例外として `finally` を抜ける
- （補）起動が値かエラーか defect で終わった後に残った継続（resume の後も走り続ける操作の実装など）の中の with は release しない．alpha.4 の範囲では main の継続が終われば main の with はすべて閉じている

## 外部関数（仕様 12章）

- （補）外部関数は，ホストの関数を呼ぶ包みの関数にする．引数1つの指定（`"console.log"`）はグローバルの参照をそのまま書き（識別子でない区切りは `globalThis["…"]`），引数2つの指定はモジュールの名前を `import { name as $ext0 } from "…"` で読む．同じモジュールと名前は1回だけ import する．
- （補）lowering が絶対パスに直したファイルの指定は `file://` の URL で import する．出力はソースの場所に依存する（出力先へ写さない．ffi のファイルがさらに相対で import するものも動くように）．
- （補）呼び出しを `try` で囲み，投げた例外を `$foreign` で defect にする（12.5）．defect（コールバックの中で起きた Emela の defect）はそのまま投げ直す．それ以外はメッセージに例外の名前を前に付ける（`TypeError: bad`）．Error でない値は `uncaught exception: …`．
- （補）ホストから受け取る String（戻り値，コールバックの引数）は `$wellFormed`（`String.prototype.toWellFormed`，なければ置き換え）で対になっていないサロゲートを U+FFFD にする（12.5）．`()` を返す外部関数はホストの戻り値を捨てて `undefined` を返す．関数型はコールバックとして包み，引数と戻り値を逆向きと同じ向きに写す．
- （補）JS の指定がない外部関数（エントリからたどれないので lowering が誤りにしなかったもの）は，呼ばれたら `$noExternal` で defect にする．

## 名前

- （補）局所変数は `ヒント$番号`，生成コードの一時変数とラベルは `$t0`，`$m0`，`$L0`，ランタイムの関数は `$` で始まる．Emela の識別子は `$` を含まないので衝突しない．
- （補）トップレベル関数は Emela の名前のまま出す．JS の予約語や生成コードが使う大域名（`Math`，`BigInt` など）と重なるときは末尾に `$` を付け，`export { new$ as new }` のように Emela の名前で公開する．
- （補）build，run，test では lowering が単相化した IR を受ける（2026-10-15，emela-core の NOTES の「単相化」）．単相化の写しの名前（`List.map[Int, String, {}, {}]`）は，元の名前の JS の名前，`$`，型引数の英数字の語を `_` でつないだもの（`List_map$Int_String`）にする．集合の要約の `{}` は語を持たないので消える．`$` があるので型引数のない関数とは重ならない．それでも重なったら `$$2`，`$$3`，… を足す（`function_names`．名前は出力の前に全部の関数について決める）．局所変数は `ヒント$数字` でヒントに `$` を含まないので，`$` のない名前，`$` の後が数字でない名前，`$` が 2 つある名前のどれとも重ならない
- （補）defect の文面に出す関数の名前（`$noExternal`，const の評価中の defect）は，写しの型引数を除いた Emela の名前（`source_name`）
- （補）エントリの `pub` の汎用の fn は，lowering が型引数を `Any` にした写しを元の名前で export する．JS の値は型によらず同じ表現なので，どの型の値で呼んでもよい
- （補）emitter は多相な IR も受ける（手で組んだ IR のテスト）．`Type::Param` は enum の表示関数の中でだけ意味を持つのは今までどおり

## 意味

- （補）Int の加減算は `(a + b) | 0`，乗算は `Math.imul`，除算と剰余はランタイムの `$idiv` / `$irem`（ゼロ除算の defect と `| 0` での巻き戻し・`-0` の除去を受け持つ）．
- （補）Int64 の除算 `$ldiv` は `i64::MIN / -1` を `BigInt.asIntN` で巻き戻して `i64::MIN` にする．
- （補）Float の `%` は JS の `%`（fmod，被除数の符号）．仕様 16.1 に反映済み．
- （補）文字列の順序比較はコードポイントの辞書順（`$scmp`）．JS の `<` は UTF-16 の順なので使わない．等値は `===`．仕様 10.6，16.2 に反映済み．
- （補）基本型以外の `==` は `$eq` で構造を比べる．長いリストでもスタックを使わないよう明示的なスタックで辿る．NaN は自分自身とも等しくない．
- （補）補間での表示: Bool は `True` / `False`（enum のバリアント名），Float は整数値で有限なら `1.0` のように `.0` を付ける．それ以外の Float は JS の `String`（`1e+21`，`-2.5e-8`，`Infinity`，`NaN`）．桁数は最短で往復できる桁数（JS の Number#toString と同じ規則）で，WASM 側でもこれに合わせる．Int64 は `n` を付けない．仕様 10.6 に反映済み．
- （補）const（`Function::constant`）は，状態のセル `X$cell`，本体の関数 `X$init`，ランタイムの `$constant` を呼ぶ `X()` の3つにする．`$constant` は最初の呼び出しで `X$init` を呼んで値を覚え，2回目からは覚えた値を返す（5.6）．
- （補）const の評価の途中の defect は，`while evaluating const X: ` を前に付けた defect にして覚え，次の参照からも同じ defect を投げる（評価し直さない）．値は純粋なので評価し直しても同じ defect になり，覚えておくほうが「1回だけ評価する」に合う．入れ子の const はそれぞれ名前を足す（`while evaluating const A: while evaluating const B: division by zero`）．名前は IR の名前で，ほかのモジュールの const は `Config.DEFAULT` のようにモジュール名が付く．
- （補）評価の途中の const を参照すると `const X refers to itself while being evaluated` の defect にする．この defect には名前を足さず，通った const のどれも覚えない（評価していない状態に戻す）．通った const が覚えると，最初にどこから参照したかで次の参照の文面が変わるため．どの const も次の参照でまた評価し，同じ defect になる．
- （補）defect でない例外（JS のスタックのあふれなど）も，const を評価していない状態に戻して投げ直す．
- （補）テストランナーはテストを同じモジュールで続けて呼ぶので，const の値（と覚えた defect）はテストの間で共有される．
- （補）defect は `$Defect`（`name` が `"Defect"` の `Error`）を投げる．メッセージは英語で `division by zero`，`unreachable: no match arm matched`，`panic` は渡したメッセージ．
- （補）match の対象はリテラルか変数でなければ一時変数に1回だけ入れる．腕は `if` の列で，ガードはパターンの束縛の後に評価する．無条件に合う腕があればそれ以降の腕と `$unreachable()` を出さない．
- （補）評価順: 引数の後ろの方に文を要する式（`let`，`match` など）があると，それより左の引数を先に一時変数へ束縛してから文を出す．リテラル・変数・関数・ラムダは束縛しない．
- （補）`Loop` は `let $t… = 初期値; $L: for (;;) { const 変数 = $t…; … }` で，`Recur` は `$t…` へ代入して `continue $L`．
- （補）IR が不正（演算と型の組み合わせ，引数の数，`Loop` の外の `Recur`）なら `emit_module` は panic する．型検査を通った IR だけが来る前提．

## ランタイム

- （補）ランタイムは `src/runtime.mjs` 1つで，`RUNTIME` として埋め込む．`RuntimeMode::Import` は使った名前だけを `./emela_runtime.mjs` から import し，ファイルの書き出しは呼び出し側が行う．`RuntimeMode::Inline` は行頭の `export ` を外して出力の先頭に埋め込む．そのため runtime.mjs では `export` を行頭にだけ書く．
- （補）出力に入る文言は英語で書く．生成コードの先頭の `// Generated by Emela.`（インラインのときはランタイムの後に `// Generated code starts here.`），テストの起動用モジュールの `// Test entry module generated by Emela.`，`runtime.mjs` と `test_runner.mjs` のコメントがそう．どちらも出力にそのまま入るため．Rust のソースのコメントと内部の誤りの文言は日本語のまま
- （補）組み込み関数は `emit/builtins.rs` の `runtime_name` が返す名前のランタイムの関数を呼ぶ．表は網羅的な match なので，組み込み関数を足すと JS の実装を書くまでコンパイルが通らない．`Int.to_float` のような恒等の関数も，生成コードを一様にするため関数として呼ぶ．
- （補）`checked_add` などランタイムが返す `None` は `$None` で，Prelude の Option を持つモジュールは `const Option$None = $None;` として同じ値を共有する．
- （補）シフト（`$ishl` など）は，JS の `<<` がシフト量の下位 5bit しか見ないので，負（defect）と幅以上を先に処理してから演算子を使う．Int64 は BigInt の `<<` / `>>` を使い，`$lushr` は `BigInt.asUintN(64, …)` で符号なしにしてからずらす．
- （補）`String.code_points` は JS の文字列の反復（コードポイントごとに進む）で値を集める．`String.from_code_point` は `$None` か `{ $tag: 0, $0: … }` を返す（`checked_add` と同じ形）．
- （補）`String.length` は `Intl.Segmenter`（ロケールは既定）で書記素クラスタを数える．Segmenter は最初の呼び出しで1つ作って使い回す．書記素の区切りの Unicode の版はホストの ICU に依存する．固定する版は仕様 18.1 #18 で未決．

## 表示（Show の仮実装）

- （補）表示関数は型から作る．基本型とリスト，タプルはランタイムの `$showInt`，`$showList(xs, f)`，`$showTuple(t, [f0, f1])` など．enum ごとに `function $show$Name(v, $a0, …)` を使った分だけモジュールの末尾に出し，型引数の表示関数を `$a0`，`$a1`，… で受ける（辞書渡しの形）．再帰する enum もそのまま自分を呼ぶ．
- （補）表示はソースの書き方に合わせる: `Circle(radius: 1.0)`，`Rect(1.0, 2.0)`，`Empty`，`(1, "a")`，`[1, 2, 3]`，`()`，`Some("a")`．`type` も `Point(x: 1, y: 2)`．
- （補）補間では最上位の String だけを引用符なしで埋め込む．リストや構成子の中の String は `dbg` と同じく引用符で囲んでエスケープする（`"#{["a"]}"` は `["a"]`）．
- （補）String の表示のエスケープは言語のエスケープ `\"`，`\\`，`\n`，`\#` を使い，`\#` は `#{` の前だけに付ける．言語にエスケープのない制御文字は `\t`，`\r`，`\u{…}` と出す（エスケープの一覧は仕様 18.1 #11 で未決）．
- （補）`-0.0` の表示は `0.0`（JS の `String(-0)` が `"0"` のため．10.6 の「JS の Number#toString と同じ規則」に従う）．
- （補）`dbg` は `console.error` で表示を1行出す（ブラウザでも動くように `process.stderr` は使わない）．位置（ファイルと行）はまだ出さない．構文木とつなぐときに足す．
- （補）enum のリストのような深い入れ子の表示はリストの部分だけループで辿り，enum の入れ子は再帰で辿る．深い木の表示はスタックの深さに制限される．
- （補）関数型（`Type::Fn`）と能力の値（`Type::Handler`）の表示を求める IR は panic する（10.6，8.2．型検査で弾く前提）．`Type::Param` も enum の表示関数の外では panic する．

## assert（仕様 13.2）

- （補）比較の assert（`AssertCond::Compare`）は両辺を左から1回ずつ評価して一時変数に入れ，比較が偽のときだけ両辺を表示関数で文字列にして `$assertCmp` で defect を投げる．表示は `dbg` と同じ（String は引用符付き）
- （補）defect のメッセージは `assertion failed: <式の字面>` の後に `\n  left: <左辺>\n right: <右辺>` を続ける（rustc の `assert_eq!` に近い形）．字面が空なら `left == right` のように演算子だけを出す
- （補）比較でない assert（`AssertCond::Bool`）は `assertion failed: <式の字面>`（字面が空なら `assertion failed`）．値は表示しない
- （補）assert は文を要する式（`is_simple` が偽）として扱う．左の引数は先に一時変数へ束縛され，評価順が保たれる

## テストランナー

- （補）テストの起動用モジュール（`emit_test_main`）は出力したモジュールを `import * as $m` で読み，`test_runner.mjs` を `export` を外して埋め込み，`$runTests([[表示名, $m["名前"]], …])` を呼ぶ．関数は Emela の名前（JS の予約語なら `export { new$ as new }` の公開名）で引く
- （補）テスト関数の返り値が `$Fail`（処理されなかったエラー，13.3）なら `outcome: "error"` で，`message` はエラーの値の表示（`NotFound(id: 0)`）．他のテストは続ける
- （補）結果は1テストごとに `\x1eemela-test {"event":"result","name":…,"outcome":"ok"|"defect"|"error"|"interrupted","message":…}`，最後に `{"event":"done"}` を標準出力に書く．頭の制御文字で利用者の出力と見分ける
- （補）中断しうるテスト関数は，出力したモジュールの `$start` で進める．resume されないままイベントループが空になったら（`beforeExit`）`outcome: "interrupted"`．テストの実行中にホストのコールバックの中で投げられた例外（`uncaughtException`）は，そのテストの defect
- （補）`name` が `"Defect"` でない例外（外部関数が投げたもの）も defect として報告し，メッセージに `TypeError: …` のように例外の名前を前に付ける（7.5 の「外部関数が投げた例外」）

## 中断と再開（仕様 8.4，14.5，16.5）

`src/emit/suspend.rs` とランタイムの `$start`．継続はジェネレータで表す（16.5）．CPS にしなかったのは，中断しない関数の出力を変えずに済み，エラーの `$Fail` の戻り値と末尾呼び出しのループ（16.3）がそのまま使えるため．

- （補）中断しうる関数とラムダはジェネレータ（`function*`，ラムダは `(function* (…) { … })`）．アロー関数には yield を書けないため．中断しうる呼び出しは `(yield* f(x))`．関数の値の呼び出しと，印の付いた fn の操作の呼び出しは `(yield* $maybe(g(x)))` で，呼んだ値がジェネレータでなければそのまま返す
- （補）suspend fn の操作の呼び出しは `(yield $perform(h.op, [引数]))`．要求は最も外側の駆動（`$start` の `$step`）まで `yield*` を素通りして届く．駆動が resume を作り，操作の実装を `(引数…, resume)` で呼ぶ
- （補）操作の実装の中でその場で resume したら，実装が戻った後，駆動のループの中でそのまま継続を進める（14.5 の3）．中断も JS のスタックの伸びもない．resume の後に実装の残りがあれば，それを先に走らせてから呼び出し元が続く
- （補）後から resume したら，その resume の呼び出しの中で継続を進める．継続が次に中断するか終わると resume が戻る（値は `()`）
- （補）2回目の resume は defect（``resume called twice in operation `sleep` of handler `JsClock` ``）．その場の2回目は操作の呼び出しの位置の defect，ホストのコールバックの中の2回目はコールバックから投げる（起動用モジュールとテストランナーが拾う）
- （補）継続へ渡すのは，resume の値，実装の失敗（`$Fail`），実装の defect のうち先に起きた1つ．resume せずに失敗した実装の失敗は操作の呼び出しの値になる（呼び出しの `fails` で確かめる）．resume した後で実装が失敗したら，その起動の defect（`… failed after resume: <エラー>`）．resume した後の実装の defect も起動の defect
- （補）中断しうる実装（ジェネレータ）は，同じ起動の中の別の継続として進める．実装が中断した後で resume すれば，その resume の中で呼び出し元が続く
- （補）起動（main かテスト関数）が終わる（値，defect，中断のまま巻き戻る）と，残った継続は進めない．その後の resume は何もしない（2回目でも defect にしない）
- （補）`@external suspend fn` の包みはジェネレータで，ホストの関数を呼んで返った値を `yield $await(値)` で待つ．Promise でない値はそのまま解決した値として扱う（`Promise.resolve`）．解決した値は戻り値と同じ規則で写す．呼んだときの例外と拒否はどちらも `$foreign` で defect（12.5）．拒否は待っていた位置へ投げる
- （補）中断しうる関数を export するモジュールは，ランタイムの `$start` も export する．起動用モジュールとテストランナーはランタイムを読み込まないので，`$start` を出力したモジュールから引く．中断しうる関数がなければ出力は今までと同じ
- （補）中断しない関数（R が Immediate）の中の中断しうる呼び出しは，その場で resume するハンドラの with の中にだけ現れる（8.7）．ジェネレータを `$sync` でその場で最後まで進め，suspend fn の操作は `$syncPerform` で呼ぶ．その場で resume されなければ，その継続は二度と resume されない（resume を後から呼べるハンドラは依存に Async があり，R が Immediate でなくなる）ので，すぐに `$Interrupted`（`name` が `"Interrupted"` の `Error`）を投げて巻き戻る．起動用モジュールとテストランナーは `interrupted` として報告する
- （補）関数の名前の `$`（lowering が作った中断用の写しの `List.map$suspend8`）は JS の名前にそのまま残す（`List_map$suspend8`）．Emela の名前に `$` は現れないので元の関数と重ならない
- （補）`$start(gen, done)` は `done(true, 戻り値)` か `done(false, 例外)` を1回だけ呼び，巻き戻す関数を返す．中断したまま resume されなければ `done` は呼ばれない．それを見分けるのは呼び出し側（イベントループが空になったか）で，そのとき返された関数を呼ぶと，release を走らせてから `done(false, $Interrupted)` になる（上の「init と release」）

## fiber（仕様 11章，14.5）

alpha.5 の第1段（2026-10-13）．`emit/fiber.rs` とランタイムの `$Root`，`$Fiber`，`$Scope`．IR（`Expr::Fiber`）を手で組んで確かめた（`tests/fiber.rs`）．lowering からの接続は後の段．

- （補）起動（`$start`，`$sync`）ごとの `$Root` は fiber のスケジューラ．起動した関数は根の fiber で，fork した子も同じ `$step` で進める．一度に走る fiber は1つで，他の fiber を起こす操作（子の終わり，Deferred の complete，中断）は起こされる側をキューに入れ，走っている fiber が中断してから順に走らせる．キューは入れた順に処理するので，切り替えの順序は実行のたびに同じ
- （補）ホストのコールバックの中の resume も，継続をキューに入れてその場でキューを空になるまで処理する．resume の中で呼び出し元が再開する（14.5 の4）のはこれまでどおり．同じ fiber の中から呼ばれた resume（中断しうる操作の実装が resume したとき）は，これまでどおりその場で継続を進める．別の fiber から呼ばれた resume（resume を値として受け渡したとき）はキューに入れ，呼んだ側の一息の区間に割り込ませない（11.1）
- （補）fork は中断点でない．子はキューに入り，親が次に中断したときに走り始める．走り始める前に中断された子は，f を呼ばずに Interrupt で終わる
- （補）fiber の待つ操作（join，await，interrupt，Deferred.await，スコープが子を待つところ）は `yield` で `$FiberRequest` をスケジューラへ送る．すぐ返せるときは中断せずに続ける．`Fiber.scope` は `yield*` で呼ぶランタイムのジェネレータ `$scope`，`attempt` は `f` がジェネレータを返したときだけジェネレータを返す `$attempt`（関数の値の呼び出しと同じく `$maybe` と `$sync` で包む）
- （補）中断は fiber 単位．中断された fiber には，待っている継続へ `$Interrupted` を投げ込む（alpha.4 の `gen.return()` をやめた）．例外として巻き戻るので，with の `finally` が内側から走り，スコープの枠（`$scope` の `catch`）が途中で受けられる．生成コードの `catch` は with の（投げ直すだけの）ものと attempt だけで，attempt は `$Interrupted` を投げ直す
- （補）中断の要求には的がある．fiber 全体（null）か，その fiber の上で本体が走っているスコープ（子が失敗したスコープは，自分の本体だけを中断する）．`$Interrupted#scope` に的を入れ，スコープの枠は自分宛てのものを受け止める．まだ届けていない要求が複数あれば，最も外側のもの（fiber 全体か，浅いスコープ）を届ける．fiber 全体の中断を届けた後の要求は無視する
- （補）中断が届くのは，その fiber が中断点に達したとき（11.4）．待っているところへの要求はすぐ投げ込み，走っている間の要求は次の `yield` で投げる．キューに入って起こされるのを待っている継続にも，走らせる直前に投げる（join の結果などは捨てる）
- （補）届けた中断は，受け止める枠（fiber 全体なら fiber の終わり，スコープならその枠）に達するまで巻き戻りが続くものとして覚えておく．途中で失敗を受けて普通の処理に戻ったとき（内側のスコープが子の失敗を値で返し，escape がそれを受けたときなど）は，次の中断点でもう一度投げる．巻き戻りの途中の attempt は Die も受けず（release の defect を fiber の結果に残すため，14.4），そのまま投げ直す
- （補）release の実行中は中断を待たせる（11.4）．中断しうる呼び出しを含む release は `$hold()` と `$unhold()` で囲み，その fiber の `holds` を数える．`holds` が 0 でない間は要求を溜めるだけで，release が終わった後の中断点で届ける．スコープが子を待つところも同じく待たせる．中断しうる呼び出しを含まない release は一息に走るので囲まない
- （補）スコープは，本体が終わったら（値，失敗，defect，中断のどれでも）閉じて残りの子を中断し，すべての子が終わってから返る．結果は，本体と子の最初の失敗（Fail，Die）があればそれ，なければ本体の終わり方．子の中断は失敗に数えない．本体が中断で終わった後に子が defect で終われば，その defect が結果になる（14.4 の release の defect と同じ向き）
- （補）最初の失敗の後の失敗（Fail，Die）は，最初のものの表示に `note: another fiber also failed: <Cause.to_string の文面>` を足す．Die はメッセージに足し，Fail は `$Fail#notes` に足してエラーの表示の後ろに出す
- （補）子が失敗した時点でスコープは閉じる（fork と deferred は defect）．11.8 の「本体が値を返した後」に，兄弟の失敗で巻き戻る途中も含めた
- （補）join と await に Fail と Die は届かない（11.5）が，release の実行中の fiber は中断を待たせるので，失敗した子を見ることがある．そのとき join はその fiber を中断として巻き戻し（join の型に E がないので失敗にできない），await は `Failure(原因)` をそのまま返す
- （補）中断された子の join は，中断の起こり方（`Fiber.interrupt`，スコープの終わり，`$sync` の捨てられた継続）によらず defect（`fiber was interrupted`）
- （補）自分自身への `Fiber.interrupt` は，その場で中断として巻き戻る（release の中なら release の後）．自分の巻き戻りを待つと終わらないため
- （補）終わったスコープの使用の defect の文面は `fiber scope was used after it ended`（fork，deferred），`fiber was joined after its scope ended`，`fiber was awaited after its scope ended`，`fiber was interrupted after its scope ended`，`deferred was completed after its scope ended`，`deferred was awaited after its scope ended`，`deferred was polled after its scope ended`
- （補）Exit は `{ $tag: 0, $0: 値 }`（Success）か `{ $tag: 1, $0: 原因 }`（Failure），CauseKind は `{ $tag: 0 | 1 | 2 }`．IR の enum の形はバックエンドが確かめる．原因はランタイムの `$Cause`（種類と，Fail なら `$Fail`，Die なら投げられた例外）．`Cause.raise` は Fail を失敗の値として返し，Die を投げ直し，Interrupt は `$Interrupted` を投げる
- （補）`$sync` の `$Interrupted` は，その fiber の例外として巻き戻り，子ならその子だけが Interrupt で終わる．根の fiber なら今までどおり起動の `interrupted`
- （補）起動の中断（`$start` の返す関数．イベントループが空になったとき）は，すべての fiber に中断を要求し，待っているすべての継続へ `$Interrupted` を投げる．このときだけは release の実行中でも打ち切る（alpha.4 と同じ．もう誰も resume しないため）．子を待つスコープだけは待ち続ける（子も巻き戻るので）．根の fiber が終われば起動が終わる
- （補）中断しうる操作の実装が resume の後も走り続けるとき（14.5 の3，4，NOTES の「決め直すこと」）は，今までどおり同じ fiber の別の継続として進める．fiber の中断はその fiber の本筋の継続だけに届け，実装の継続は止めない．起動の中断ではこれも閉じ，その release の defect は起動の結果にする（`$Root#orphan`）．スコープは実装の継続を待たない

### 譲渡，AbortSignal，仮の時刻（仕様 11.1，12.2，8.8）

alpha.5 の第2段（2026-10-13）．

- （補）公平性の譲渡は JS の出力で挟む（lowering の IR には足さない）．ジェネレータの中の中断しうる呼び出し（`Expr::Call::suspends` が真で，`yield*` か `yield $perform` になるもの）の前に `($spend() && (yield $pause()), 呼び出し)` を書く．`$spend` は走っている fiber の予算を1減らし，0 になったら真．`$pause` の要求を受けたスケジューラは予算を戻し，fiber を待ち行列の末尾へ回す．中断しない関数（`$sync` で進める呼び出しを含む）には何も足さない（11.1 の「Immediate な関数は一息」）．lowering で挟まないのは，印（`suspends`）がそろっていれば出力だけで決まり，並行して作っている fiber-lowering と触る所を分けるため
- （補）予算は fiber ごとに 1000 回，譲渡は起動ごとに数えて 16 回に1回はホストのイベントループを通す（`setImmediate`，なければ `setTimeout(f, 0)`）．タイマーや入出力のコールバックがそこで走る．回数は時間に依らないので，fiber の間の順序は毎回同じ．`$sync` の起動では譲渡はその場で続ける（待てないため）
- （補）fiber の操作の要求（join など）と `Expr::Fiber` の中断しうる操作の前には挟まない．それ自体が中断点で，すぐ返るときも呼び出しは軽いため
- （補）測った結果（Apple M3，node 22，他の作業が並んで走る負荷の高い環境．9回の最小値）: 何もしない中断しうる関数を 1000 万回呼ぶループで，挟まない 404〜495 ms，挟む 280〜316 ms．その場で resume する suspend fn の操作を 200 万回呼ぶループで，挟まない 229〜265 ms，挟む 252〜267 ms．ばらつきが大きく，上乗せは測った誤差の範囲（多くて1割）
- （補）`@external suspend fn` の包みは，`$abortable()`（`AbortController`）の `signal` を最後の引数に足してホストの関数を呼び，`yield $await(promise, controller)` で待つ（12.2）．待っている継続を捨てるとき（fiber の中断，起動の中断）にランタイムが abort する．resume で普通に続くときは abort しない．起動が終わったときは，残った待ちをすべて abort する（ホストがすぐ終われるように）
- （補）suspend fn の操作の実装がジェネレータ（外部関数を待つ SystemClock など）で，resume する前に呼び出し元が中断されたら，実装の継続も `$Interrupted` で巻き戻し，その待ちを abort する（`wait.onDrop`）．12.2 の「この形で待つもの」に，操作の実装の中で待つものを含めた．実装が resume した後は今までどおり止めない
- （補）仮の時刻（TestClock，8.8）の「どの fiber も進めず，ホストの処理も待っていない」は，ホストのイベントループが空になったことで知る．起動用モジュールとテストランナーがループが空になったときに呼ぶ関数（`$start` の返すもの，`$Root#stalled`）は，`$whenStalled` で待っている者がいれば一番早い時刻（同じ時刻なら呼んだ順）の1つを起こし，いなければ今までどおり起動を中断する．起動用モジュールとテストランナーは変えていない．Promise の解決だけでは node はループが空になったことを2度目に知らせないので，起こすのは `setImmediate` を通す
- （補）`$whenStalled(at, signal)` は走っている fiber の起動に登録し，Promise を返す．`signal` が abort されたら（待っている fiber が中断されたら）登録を消す．一度に起こすのは1つで，同じ時刻の次の者は次にループが空になったときに起きる
- （補）ループが空になったことを `beforeExit` で知る仕組みと，`globalThis[Symbol.for("emela.whenStalled")]` での受け渡しは，Node 22.14，Deno 2.7.13，Bun 1.3.8 で同じに動くことを確かめた（2026-10-13．`tests/fiber.rs` の出力の JS を3つのホストで流し，仮の時刻の順，AbortSignal でタイマーが消えてすぐ終わること，譲渡，起動の中断の結果が一致した）．どれも `beforeExit` の後に `setImmediate` で仕事を足せば，次にループが空になったときにもう一度知らせる
