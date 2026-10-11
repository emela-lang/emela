# emela-core の補足

仕様にない判断を「（補）」として残す．

- （補）名前はすべて `la_arena::Idx` で持つ．関数は `FnId`，局所変数は `Local`，型は `EnumId`．`Local` の `hint` と関数の `name` は出力の読みやすさのためだけに使う．関数名はモジュール内で一意とし，公開名にもそのまま使う．
- （補）局所変数はモジュール全体で一意にする（同じ `Local` を2回束縛しない）．置き換えや変換で遮蔽を考えなくてよくするため．
- （補）match は決定木でなく腕の列で持つ．決定木への変換はバックエンドの最適化として後で入れる．
- （補）`type` は構成子1つの enum として `EnumDef` に入れる．構成子とパターンのフィールドは定義順に並べ，パターンで省いた名前付きフィールドは `Pat::Wild` で埋める．フィールドの参照は名前でなく添字（`Expr::Field { index }`）で持つ．
- （補）Bool は enum `True` / `False` だが，IR では `Lit::Bool` と `OpTy::Bool` の組み込みで持つ．パターンも `Pat::Lit(Lit::Bool(_))` で書く．
- （補）`()` は `Lit::Unit`，`Expr::Tuple` は要素2つ以上だけに使う．
- （補）二項演算と単項演算には対象の型（`OpTy`）を付ける．Int / Int64 / Float で意味が違うため．`==` / `!=` で基本型以外を比べるときは `OpTy::Structural`．順序比較の `Structural` は Trait（Ord）の段まで持たない．
- （補）文字列の補間は `Expr::Concat` で，埋め込む値には IR の型 `Type` を付ける．バックエンドは型から表示関数を作る（Show の仮実装．Trait にするのは 0.21）．型は `Type::Param` を含まない具体的な型に限る．型引数の値を表示するには Show の辞書渡しか単相化が要るので，0.21 まで汎用の関数の中では表示できない．
- （補）ラムダは捕捉する変数を IR に書かない．capture set は WASM の段で求める．
- （補）名前付き引数は `named::in_written_order` で位置の順へ並べ替える．書かれた順と位置の順が食い違うときだけ，リテラル・変数・関数・ラムダ以外の引数を書かれた順に `let` で束縛する．呼び出し先が式のときに先に評価する責任は呼び出し側にある．
- （補）自己末尾呼び出しのループ化は `Expr::Loop` / `Expr::Recur` を作る．ループの変数は引数とは別の新しい `Local` にして回ごとに束縛し，ラムダが捕捉した値が後の回に書き換わらないようにする．
- （補）末尾位置として辿るのは `let` の本体，`if` の両枝，match の腕の本体，`&&` / `||` の右辺．右辺に自己末尾呼び出しがある `a && b` は `if a then b else false`，`a || b` は `if a then true else b` に脱糖してから `Recur` にする（仕様 6.8 の末尾位置の定義に反映済み）．呼び出しの引数の数が引数の数と違うものは対象外．ラムダの中の呼び出しは対象外．
- （補）エラーとエフェクトを足すとき，escape の対象の中と with の本体の中は末尾位置にしない（仕様 6.8 に反映済み）．
- （補）エラー（16.5 の戻り値方式）とエフェクトは後で `Expr` のバリアントを足して入れる．バックエンドは値の行き先（return / 代入）で式を出す作りにしてあるので，「エラーなら return」の分岐を足せる．

- （補）`assert`（13.2）は `Expr::Assert` で持つ．比較の形（`AssertCond::Compare`: 演算子，`OpTy`，両辺の型 `Type`，両辺）と，比較でない形（`AssertCond::Bool`）に分け，ソースの式の字面 `text` を添える．値は `()`．両辺の型は表示関数を作るのに使う

## 組み込み関数（`intrinsics.rs`）

- （補）組み込み関数は `Builtin` の1値ごとに，モジュール名と関数名，型（`Sig`），純粋か，JS の実装名を `Builtin::info` で持つ．名前解決は `Builtin::lookup(module, name)`，型推論は `BuiltinInfo::scheme(option)` で引く．Prelude の関数（`panic`，`todo`，`dbg`）は `module` が `None`．
- （補）型は `emela_types::Ty` を直接持たず，`Sig` で持つ．`Option` の `TyConId` は型検査側の表で決まるので，`Sig::to_ty` に渡してもらう．
- （補）同じ働きの別名を1行ずつ持つ: `String.from_int` と `Int.to_string`，`Int.to_int64` と `Int64.from_int`（16.1 の例）．
- （補）`String.join(parts, sep)` は区切りを後ろに置く（パイプ `parts |> String.join(", ")` で書けるように）．`String.split(s, sep)` と `String.contains(s, sub)` なども対象の文字列が先．
- （補）`dbg` は標準エラーに書くが，デバッグ用の例外として純粋に数える（Prelude は純粋なものだけ，15.1）．
- （補）`Float.floor` / `ceil` / `round` / `truncate` は Int を返す．丸めた結果が Int の範囲外か，NaN や ±Infinity なら defect（巻き戻しも飽和もしない）．WASM の `i32.trunc_f64_s` の trap と同じ振る舞いで，NaN が黙って 0 になるのを防ぐ．
- （補）`Float.round` の 0.5 は 0 から遠い方へ丸める（`2.5` → 3，`-2.5` → -3）．JS の `Math.round`（正の向き）や WASM の `f64.nearest`（偶数へ）とは違う．
- （補）`Int64.to_int` は下位 32bit に巻き戻す（整数のあふれの規則 16.1 にそろえる）．あふれを検出する版は持たない．
- （補）`String.split(s, "")` は書記素ごとに分ける（`String.chars` と同じ．`split("", "")` は `[]`）．空でない区切りでは `split("", ",")` は `[""]`．どちらでも `String.join(String.split(s, sep), sep) == s` が成り立つ．
- （補）`String.contains` / `starts_with` / `ends_with` / `split` の照合はコードポイントの列で行い，書記素の境界は見ない（`contains("e\u{301}", "e")` は True）．
- （補）ビット演算（`Int.bit_and` など，6.6）のシフト量は Int64 の版でも Int．シフト量が負なら defect（`negative shift amount: -1`）．型の幅（32 か 64）以上なら全部のビットを押し出した値にする．`shift_left` と `shift_right_unsigned` は 0，`shift_right` は符号に応じて 0 か -1．JS の `<<` や WASM の `i32.shl` のように幅で剰余を取らない（`1 << 32` が 1 になる驚きを避ける）．
- （補）`shift_right_unsigned` の結果は符号付きとして読む（`Int.shift_right_unsigned(-1, 0)` は -1）．
- （補）`String.code_points` はコードポイントの値のリスト（書記素クラスタ1つが複数の値になりうる）．`String.from_code_point(n)` は `n` が Unicode のスカラー値（0〜0x10FFFF でサロゲート 0xD800〜0xDFFF を除く）でなければ `None`．
- （補）`String.trim` が取り除く空白は Unicode の White_Space 特性の 25 文字（U+0009〜U+000D，U+0020，U+0085，U+00A0，U+1680，U+2000〜U+200A，U+2028，U+2029，U+202F，U+205F，U+3000）．U+FEFF と U+200B は空白でない．表を固定して，ホストの Unicode の版に依存しないようにする．

## Option

- （補）Prelude の `Option` は `Module::option_enum` でモジュールに1つだけ入れる `EnumDef`（`Some(A)` が添字 0，`None` が添字 1．5.4 の宣言の順）．組み込み関数の返す Option と利用者の書く `Some` / `None` が同じ定義を指す．`List.get` も core を Emela で書くときにこの定義を返す．
- （補）enum は型引数の数（`EnumDef::params`）と，フィールドの型（`VariantDef::tys`，`Type::Param(i)` で型引数を指す）を持つ．表示関数を型から作るのに使う．
- （補）型引数を取る組み込み関数（`dbg`）は，具体的な型を `Expr::Builtin` の `ty_args` で受ける．

## 同梱のソース（`lib/`，`sources.rs`）

- （補）Prelude と core のソースは `lib/*.emel` で，`core_sources()` がモジュール名とソースの組を返す．モジュール名は `List`，`Option`，`String`，`Int`，`Int64`，`Float` と，Prelude の `PRELUDE`（`"Prelude"`）．Prelude が先頭．
- （補）`@intrinsic` の宣言は `Builtin::lookup_intrinsic(モジュール名, 関数名)` で表を引く．`"Prelude"` は表の `module: None`（修飾せずに呼ぶ関数）に当たる．
- （補）`@intrinsic` の宣言は `pub` で，引数と戻り値の型，型引数の数を表と一致させる．表の全行がちょうど1回ずつ宣言されていることもテストで確かめる．引数名（名前付き引数のラベル）はソースが正で，表は持たない．
- （補）今の構文の検査は本体のない fn に `@external` を求める（E0130）ので，同梱のソースの `@intrinsic` の宣言にも E0130 が出る．テストはこれだけを許している．`@intrinsic` を同梱のソースに限って通す検査は名前解決の側で入れる．
- （補）Prelude の `Bool` は `False`，`True` の順に宣言する．導出する Ord で `False < True` にするため．IR では Bool は `Lit::Bool` の組み込みのまま（上の Bool の項）．
- （補）Prelude の enum（Bool，Option，Ordering）には `derive Eq, Ord, Show, Hash` を書く．Option の型引数の制約の扱いは導出の側で決める．
- （補）`dbg` は `dbg[A: Show](value: A) -> A` と書く．表の `ty_params` は制約を持たないので，制約は型推論の側でソースから読む．
- （補）core の関数は対象を第1引数に置く（パイプの左辺が入る）．List は `xs`，Option は `opt`，関数の引数は `f`，畳み込みの初期値は `init`（6.7 の `List.fold(init: 0, f: add)`）．
- （補）繰り返しは自己末尾呼び出しで書き，先頭から結果を積むものは逆順に積んで `reverse` する．補助の関数は非公開の `*_onto`（`reverse_onto` など）．
- （補）`List.append(xs, ys)` は2つのリストをつなぎ，`List.concat(xss)` はリストのリストを平らにする（Elm，Gleam と同じ）．`String.concat(a, b)` とは引数の形が違う．
- （補）`List.tail([])` は `[]`（17.5 の例 `[x, ..List.tail(xs)]` が List を受けるため Option にしない）．`List.head` と `List.get` は Option．`List.get` の負の添字は `None`．
- （補）`List.range(from, to)` は `from` 以上 `to` 未満．`List.zip` は短い方に合わせる．
- （補）`Option.or_fail` は fail が要るので alpha.2 で足す（`option.emel` にコメントで残してある）．

## lowering（`lower.rs`）

本人と相談して決めた（2026-10-11）．型検査の結果（emela-driver の `TypedProgram`．HIR と `ProgramTypes` の組）から Core IR を作る．

- （補）lowering は emela-core に置く．emela-core はもともと emela-types に依存しているので，別のクレートに分けても依存は軽くならない．
- （補）プログラム全体（利用者のモジュール，Prelude，core）を IR の `Module` 1つにする．
- （補）enum（type，enum，error）は先に全部に `EnumId` を割り振り，構成子を入れる．fn は根（`Roots`）からたどれるものだけを写す．通常のビルドの根はエントリのモジュールの export する fn，`Roots::All` は本体のある全部の fn．fn の `FnId` は参照に出会ったときに `Lowerer::fn_id` が割り振って作業リストに積み，作業リストが尽きるまで本体を変換する．相互再帰でも ID は参照した時点で決まる．`fns` の表に足すのは `fn_id` だけにする（参照の拾い漏れを防ぐため）．
- （補）たどれない fn は写さないので，その中の lowering の誤り（`Unsupported`）は呼ばれるまで出ない．型の誤りは型検査が全部の fn について出すので，ずれるのは lowering の誤りだけ．使われない core の関数も写せることは，テスト `every_core_function_lowers`（`Roots::All`）で確かめる．
- （補）関数と enum の名前にはモジュール名を付ける（`List.map`，`Http.Client.User`）．エントリのモジュールの定義だけは付けない（`main`）．JS では `.` が `_` になるが，モジュール名は `_` を含まず（4.1），関数名は小文字で始まる（2.3）のでぶつからない．enum にも付けるのは，表示関数（`$show$User`）と値のない構成子の JS の名前を enum の名前から作るため．
- （補）HIR の `LocalId` はプログラム全体で一意なので，IR の `Local` へ表で1対1に写す．添字は使い回さない．IR の側では名前付き引数の並べ替えやループ化で `Local` が増える．
- （補）`lower` は `LowerInput`（HIR，型検査の結果，モジュール名の表，モジュールのソース，エントリ）と根（`Roots`）を受け，`Result<Module, Vec<LowerError>>` を返す．型検査を通っても IR にできない場面（汎用の fn の中での型引数の値の補間など）を「まだ扱えない」の診断にする．見つけた分を全部集める．driver が `Diagnostic` に写す．ソースは assert の失敗に出す式の字面を切り出すのに使う．
- （補）`error` の宣言も IR の enum にする（構成子1つ，型引数なし）．type と同じく同じ名前の型と構成子を定義し，普通の値として扱える（7.1）．error の構成子は宣言の `DefId` そのもの．
- （補）構成子のフィールドの型は型検査の構成子の宣言の型（`sigs`）から引く．構成子の型引数は作る型の型引数と同じ順なので，`Ty::Bound(i)` を `Type::Param(i)` に写せばよい．型の変換で `Ty::Var`（決まらずに残った型変数）は `Type::Unit` にする．
- （補）JS で export するのは，エントリのモジュールの `pub` の fn と `main`（`emela run` の起動用モジュールが `entry.main` を呼ぶ）．
- （補）呼び出しの引数は，型検査が並べ直した仮引数の順（`ProgramTypes::calls`）から各引数の位置を引き，`named::in_written_order` で書かれた順に評価する．呼ばれる側が変数か関数でない式なら，引数より先に `let` で束縛する．パイプ `x |> f(a)` は左辺を先頭の引数とする呼び出しとして写す．
- （補）`@intrinsic` の fn の呼び出しは `Expr::Builtin` にする．型引数は `ProgramTypes::type_args` から引く．`@intrinsic` の fn とフィールドのある構成子を値として使うとき（`List.map(xs, String.length)`，`List.map(xs, Some)`）は，それを呼ぶだけのラムダで包む（`fn(x) { Some(x) }`）．引数の数は参照の式の型（関数型）から取る．
- （補）ブロックの文は `let` の入れ子にする．値を使わない文と `_ = e` は，捨てる用の新しい局所変数（`_`）に束縛する．変数と `_` でないパターンの束縛（`(a, b) = pair`）は腕1つの match にして，残りの文を腕の本体に入れる．必ず成功することは型検査が確かめてある（E0317）．
- （補）パターンの `True` / `False` は `Pat::Lit(Lit::Bool)`，`()` は `Pat::Lit(Lit::Unit)`．構成子のパターンのフィールドは `ProgramTypes::pat_fields`（位置の順，省いたものは `None`）から引き，`None` を `Pat::Wild` で埋める．括弧のない構成子のパターン（`Empty`）は表にないので，フィールドの数だけ `Wild` を並べる．const のパターンは const の式を扱うまで `Unsupported`．
- （補）演算子の `OpTy` は左辺の型から決める．`==` / `!=` で基本型でなければ `Structural`．大小比較で基本型でなければ（Ord）`Unsupported`．
- （補）フィールドの参照は，値の型（type か error．構成子は1つ）の構成子と，型検査が引いた添字（`ProgramTypes::fields`）で `Expr::Field` にする．表にないフィールドの参照は能力の操作（`log.info`）で，エフェクトの段まで `Unsupported`．
- （補）部分更新 `C(f: e, ..base)` は，書いたフィールドを書かれた順に評価して（リテラル・変数・関数・ラムダ以外は `let` で束縛して），`..base` を最後に評価して束縛する（14.1）．書いていないフィールドは `base` の `Expr::Field` で埋めて `Expr::Ctor` を作る．
- （補）補間は `Expr::Concat` にし，埋め込む値に IR の型を付ける．型が型引数（`Type::Param`）か関数を含むと表示関数を作れないので `Unsupported`（型引数は Show の辞書渡しか単相化が要るので 0.21，関数は表示できない（10.6）が Show の検査は Trait の段まで型検査にない）．
- （補）const は引数のない関数にして，参照のたびに呼ぶ（`LIMIT` → `LIMIT()`）．値は純粋な式（5.6）なので結果は同じ．使う場所に式を埋め込まないのは，const の式の中の局所変数を2回束縛することになるため．1回だけ計算する形は，IR に「モジュールの値」を足すときに最適化として入れる．const は export しない．
- （補）const のパターンは新しい変数で受け，`x == LIMIT()` を腕のガードにする．構成子の中など入れ子の const のパターンも同じで，比較は書いたガードより先に評価する（`&&` でつなぐ）．`OpTy` はパターンの型が基本型ならその型，そうでなければ `Structural`．
- （補）`assert` は，式が比較なら `Assert::compare`（両辺の値を表示する），それ以外は `Assert::bool`．比較でも両辺の型が表示できない（型引数や関数を含む）ときは `Assert::bool` に落とす（基本型でない値の大小比較は，式の側で `Unsupported` になる）．`text` はソースから切り出した式の字面．
- （補）`Roots::Tests` はテスト関数（`LowerInput::tests`）を根にする．`lower` は `Lowered { module, tests }` を返し，`tests` は `LowerInput::tests` と同じ順の `FnId`（`Roots::Tests` のときだけ埋める）．通常のビルドではテスト関数を export しない．
