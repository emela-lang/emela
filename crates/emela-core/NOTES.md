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
- （補）エラーとエフェクトは下の「エラー」と「エフェクト」の節．

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

- （補）Prelude と core のソースは `lib/*.emel` で，`core_sources()` がモジュール名とソースの組を返す．モジュール名は `List`，`Option`，`String`，`Int`，`Int64`，`Float`，`Duration`，`Instant`，`Console`，`JsConsole` と，Prelude の `PRELUDE`（`"Prelude"`）．Prelude が先頭．
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
- （補）`Option.or_fail[A, E](opt: Option[A], err: E) -> A fails E` の引数名は仕様 7.2 の例に合わせて `err`．本体も 7.2 のとおり（`None -> fail err`）．E が入ったので `option.emel` に同梱した（2026-10-12）．

## Console（`console.emel`，`js_console.emel`）

alpha.2 で同梱した（2026-10-12）．仕様は構成だけを決めている（15.2）ので，名前と数はここで決めた．

- （補）エフェクト `Console` の操作は `print_line(text: String) -> ()`（標準出力）と `print_error_line(text: String) -> ()`（標準エラー）の2つ．どちらも改行を付けて書く．`fails` は書かない（失敗しない）．改行なしの `print` と入力の読み取りは，要るときに足す．
- （補）エフェクトはモジュール `Console`（`console.emel`），JS 用のハンドラはモジュール `JsConsole`（`js_console.emel`）に置き，ハンドラの名前もモジュールと同じ `JsConsole` にする．ターゲットの名前を前に付ける（WASI 用は `WasiConsole` にする）．名前が1区切りの core のモジュールは，型の位置の名前をモジュールと同じ名前の型に解決する（4.6）ので，利用者は import なしで `with JsConsole { (use Console).print_line("hi") }` と書ける．ターゲットごとの layer（15.2）は，layer を入れるときに別のモジュールで決める．
- （補）`JsConsole` は `import Console.{Console}` でエフェクトを取り込む．core のモジュールが別の core のモジュールを import する初めての例．
- （補）`Console` と `JsConsole` を同梱したので，ソースのルート直下の `console.emel` と `js_console.emel` は予約名（E0203）になる（仕様 4.1，15.2．本人の決定）．
- （補）WASM のビルドで `JsConsole` を読んでも，使わなければ誤りにならない（12.1 の「たどれる fn だけ検査」．下の「外部関数」）．
- （補）ホストの関数（`console_log` は `console.log`，`console_error` は `console.error`）は `JsConsole` の中の非公開の `@external(js, ...)` の fn にし，`use` を書かない（ホストの副作用を持つので，ハンドラの操作の本体からしか呼べない．12.2）．ハンドラは状態を持たない（フィールド，init，release なし）．

## Instant と Duration（`instant.emel`，`duration.emel`）

alpha.3 で同梱した（2026-10-12）．仕様は「中身が Int64 の opaque な型」だけを決めている（15.2）ので，単位と関数の名前と数はここで決めた．

- （補）どちらも中身はミリ秒の Int64（`pub opaque type Instant(milliseconds: Int64)`，`Duration` も同じ形）．JS の `Date.now()` の単位に合わせた．Instant は Unix エポック（1970-01-01T00:00:00Z）からのミリ秒で，閏秒は数えない．Duration は負の長さも表す．
- （補）Duration は `milliseconds` / `seconds` / `minutes` / `hours`（引数は Int64．リテラルは文脈で Int64 になる），`to_milliseconds`，`to_seconds`（0 の方向へ切り捨て），`add`，`subtract`，`compare`（`Ordering`），`to_string`（`1500ms`）．
- （補）Instant は `to_unix_milliseconds`，`add` / `subtract`（Duration だけずらす），`since(t, earlier)`（`earlier` から `t` までの Duration．`now |> Instant.since(start)`），`compare`，`to_string`．
- （補）`Instant.to_string` は UTC の ISO 8601 で，JS の `Date.prototype.toISOString` と同じ形（`2025-10-12T03:04:05.678Z`．年が 0〜9999 の外なら `+010000` / `-000001` のように符号と6桁）．日付は proleptic Gregorian で，Emela で書いた（Hinnant の civil_from_days）．JS の Date の範囲（±8.64e15 ミリ秒）の外でも同じ式で書く．
- （補）単位の換算（`Duration.seconds(n)` の `n * 1000`）と足し算，引き算のあふれは Int64 の規則（16.1）で巻き戻る．検出しない．
- （補）現在の時点を取る関数は置かない（Clock のエフェクトの `now`，次の段）．Instant を作れるのは `Instant.from_unix_milliseconds(ms: Int64) -> Instant` だけで，これは `@internal`（同梱の core の中からだけ参照できる．E0228，emela-driver の NOTES）．Clock のハンドラ（`JsClock`，`TestClock`）が使う．利用者のコードは Clock から Instant を受け取るだけで，時点を作り出せない．
- （補）大小比較の演算子は Ord の段（Trait）まで opaque な型に使えないので，`compare` を関数で持つ．`==` は構造の比較で使える．`dbg` と補間では中身がそのまま見える（`Instant(milliseconds: …)`．Show を Trait にする 0.21 で `to_string` に揃える）．
- （補）Instant は Duration を import せずに使う（名前が1区切りの core のモジュールなので `.` の左と型の位置で見える，4.6）．
- （補）`Instant` と `Duration` を同梱したので，ソースのルート直下の `instant.emel` と `duration.emel` は予約名（E0203）になる（4.1，15.2．本人の決定）．

## Random（`random.emel`，`js_random.emel`，`js_random.mjs`）

alpha.3 で同梱した（2026-10-12）．形は Console と同じ（エフェクトとターゲットごとのハンドラを別のモジュールに置く）．

- （補）エフェクト `Random` の操作は `int() -> Int`（Int の全域で一様）と `float() -> Float`（0 以上 1 未満で一様）の2つ．0.1x の spec 0054 の `bytes(len)` は，0.20 に Bytes がないので入れない．
- （補）範囲つきの `between(min, max)` は操作にしない．入れるなら `int()` の上に書く普通の関数にする（ハンドラが実装する操作を少なく保ち，テスト用のハンドラを書きやすくするため）．種を渡せる PRNG（spec 0054 の Part B）はエフェクトでなく純粋な値と関数にする．どちらも alpha.3 には入れない．
- （補）`JsRandom` は `crypto.getRandomValues` で引く（暗号論的に安全）．`Math.random` は使わない．関数のパスひとつで書けないので補助の `lib/js_random.mjs` を置く（#155）．
- （補）`int` は `Int32Array` で受ける．符号付き 32 ビットになるので，Int（16.1）へそのまま渡せる．
- （補）`float` は 32 ビットを2つ引き，上位 27 ビットと 26 ビットから 53 ビットの整数を作って 2^53 で割る．0 以上 1 未満で，Float で表せる 2^-53 刻みの値が一様に出る．
- （補）`Random` と `JsRandom` を同梱したので，ソースのルート直下の `random.emel` と `js_random.emel` は予約名（E0203）になる（Console と同じ）．

## Env（`env.emel`，`js_env.emel`，`js_env.mjs`）

alpha.3 で同梱した（2026-10-12）．形は Console と同じ．

- （補）エフェクト `Env` の操作は `get(name: String) -> Option[String]` だけ．全部の変数を返す操作（`List[(String, String)]`）は，境界の変換が増えるので入れない．
- （補）`Option[String]` は外部関数の境界を越えられない（12.4）ので，ホストの関数は有無の `has(name) -> Bool` と値の `get(name) -> String` の2つに分け，`JsEnv` の操作の本体で Option に組み立てる．`@external type` で値と有無をまとめる案より，ホストの側が単純になる．
- （補）空の値（`NAME=`）は `Some("")`，未設定は `None`．`has` は `Object.hasOwn(process.env, name)` で見る（`process.env[name] !== undefined` でも同じだが，プロトタイプの名前を拾わないことを明示する）．
- （補）`has` と `get` の間に環境変数が変わることは考えない（JS のプログラムの中から `process.env` を書き換える手段を core は持たない）．
- （補）`Env` と `JsEnv` を同梱したので，ソースのルート直下の `env.emel` と `js_env.emel` は予約名（E0203）になる（Console と同じ）．

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
- （補）構成子のフィールドの型は型検査の構成子の宣言の型（`sigs`）から引く．構成子の型引数は作る型の型引数と同じ順なので，`Ty::Bound(i)` を `Type::Param(i)` に写せばよい．型の変換で `Ty::Var`（決まらずに残った型変数）は `Type::Unit` にする．能力の値の型（`Ty::Cap`）は `Type::Fn`（表示も比較もできない値）にする．
- （補）JS で export するのは，エントリのモジュールの `pub` の fn と `main`（`emela run` の起動用モジュールが `entry.main` を呼ぶ）．
- （補）呼び出しの引数は，型検査が並べ直した仮引数の順（`ProgramTypes::calls`）から各引数の位置を引き，`named::in_written_order` で書かれた順に評価する．呼ばれる側が変数か関数でない式なら，引数より先に `let` で束縛する．パイプ `x |> f(a)` は左辺を先頭の引数とする呼び出しとして写す．
- （補）`@intrinsic` の fn の呼び出しは `Expr::Builtin` にする．型引数は `ProgramTypes::type_args` から引く．`@intrinsic` の fn とフィールドのある構成子を値として使うとき（`List.map(xs, String.length)`，`List.map(xs, Some)`）は，それを呼ぶだけのラムダで包む（`fn(x) { Some(x) }`）．引数の数は参照の式の型（関数型）から取る．
- （補）ブロックの文は `let` の入れ子にする．値を使わない文と `_ = e` は，捨てる用の新しい局所変数（`_`）に束縛する．変数と `_` でないパターンの束縛（`(a, b) = pair`）は腕1つの match にして，残りの文を腕の本体に入れる．必ず成功することは型検査が確かめてある（E0317）．
- （補）パターンの `True` / `False` は `Pat::Lit(Lit::Bool)`，`()` は `Pat::Lit(Lit::Unit)`．構成子のパターンのフィールドは `ProgramTypes::pat_fields`（位置の順，省いたものは `None`）から引き，`None` を `Pat::Wild` で埋める．括弧のない構成子のパターン（`Empty`）は表にないので，フィールドの数だけ `Wild` を並べる．const のパターンは腕のガードにする（下の項）．
- （補）演算子の `OpTy` は左辺の型から決める．`==` / `!=` で基本型でなければ `Structural`．大小比較で基本型でなければ（Ord）`Unsupported`．
- （補）フィールドの参照は，値の型（type か error．構成子は1つ）の構成子と，型検査が引いた添字（`ProgramTypes::fields`）で `Expr::Field` にする．表にないフィールドの参照は能力の操作（`log.info`，`ProgramTypes::ops`）で，下の「エフェクト」の節．
- （補）部分更新 `C(f: e, ..base)` は，書いたフィールドを書かれた順に評価して（リテラル・変数・関数・ラムダ以外は `let` で束縛して），`..base` を最後に評価して束縛する（14.1）．書いていないフィールドは `base` の `Expr::Field` で埋めて `Expr::Ctor` を作る．
- （補）補間は `Expr::Concat` にし，埋め込む値に IR の型を付ける．型が型引数（`Type::Param`）か関数を含むと表示関数を作れないので `Unsupported`（型引数は Show の辞書渡しか単相化が要るので 0.21，関数は表示できない（10.6）が Show の検査は Trait の段まで型検査にない）．
- （補）const は引数のない関数にして，参照のたびに呼ぶ（`LIMIT` → `LIMIT()`）．値は純粋な式（5.6）なので結果は同じ．使う場所に式を埋め込まないのは，const の式の中の局所変数を2回束縛することになるため．1回だけ計算する形は，IR に「モジュールの値」を足すときに最適化として入れる．const は export しない．
- （補）const のパターンは新しい変数で受け，`x == LIMIT()` を腕のガードにする．構成子の中など入れ子の const のパターンも同じで，比較は書いたガードより先に評価する（`&&` でつなぐ）．`OpTy` はパターンの型が基本型ならその型，そうでなければ `Structural`．
- （補）`assert` は，式が比較なら `Assert::compare`（両辺の値を表示する），それ以外は `Assert::bool`．比較でも両辺の型が表示できない（型引数や関数を含む）ときは `Assert::bool` に落とす（基本型でない値の大小比較は，式の側で `Unsupported` になる）．`text` はソースから切り出した式の字面．
- （補）`fail e` は `Expr::Fail`，`式 escape { 腕 }` は `Expr::Escape` にする（下の「エラー」の節）．腕は match の腕と同じに写す（const のパターンの比較はガードの先頭に置く）．
- （補）`Roots::Tests` はテスト関数（`LowerInput::tests`）を根にする．`lower` は `Lowered { module, tests }` を返し，`tests` は `LowerInput::tests` と同じ順の `FnId`（`Roots::Tests` のときだけ埋める）．通常のビルドではテスト関数を export しない．

## エラー（7章，16.5）

alpha.2 の lowering（2026-10-12）．失敗は戻り値として返す（16.5）．defect は今のまま例外．

- （補）IR は `Expr::Fail(e)` と `Expr::Escape { body, arms }` を持つ．エラーの値は error の enum（`EnumDef::error` が真．構成子1つ，型引数なし）の値で，普通の値として作り，比べ，表示する（7.1）．
- （補）呼び出しには失敗しうるかの印 `Expr::Call::fails` を付ける．lowering は呼ばれる側の式の型（型検査が E の解を入れた関数型）の E が空でなければ真にする．集合の型引数，見えない集合，1つのエラーの型の型引数，汎化した集合変数のように中身の分からない集合も，空でないので真．偽の呼び出しの結果はバックエンドが確かめない．E が空の関数だけのプログラムは今までと同じ IR になる．
- （補）組み込み関数，構成子，const の呼び出しの `fails` はいつも偽（型の E が空，const は 5.6）．組み込み関数は関数を引数に取らないので，渡された関数の失敗を受けることもない．
- （補）escape の腕は match と同じ `Arm` で，パターンは error の構成子（`Pat::Ctor`）．失敗したエラーと同じ error の腕を上から試し，パターンとガードが合った最初の腕を選ぶ．腕に書かれていない error はそのまま外側へ伝える．
- （補）腕に書かれた error なのに，どの腕のパターンとガードにも合わない（`NotFound(id: 1)` の腕に `NotFound(id: 2)` が来た）ときも，腕に書かれていない error と同じく外側へ伝える（7.3，18.1 #20）．型検査は腕がその error の値をすべて受けるときだけ E から消すので，合わない値が来うる error は E に残っている．網羅している error では起きない．IR には網羅したかどうかを持たせない．
- （補）escape の対象は末尾位置でない（6.8）．`tail.rs` は escape の中（対象も腕も）を辿らない．失敗しうる自己末尾呼び出し（`fails` が真）はループにする．末尾位置の呼び出しの失敗はそのまま呼び出し元へ伝わるので，ループの次の回で失敗しても同じ結果になる．
- （補）ダンプでは失敗しうる呼び出しの後ろに `?`，error の enum の前に `error` を付ける（`error enum NotFound { … }`）．

## エフェクト（8章，16.5）

alpha.2 の lowering（2026-10-12）．ハンドラは引数で渡して直接呼ぶ（evidence passing，16.5）．状態を持つ handler（フィールド，init，release），layer，suspend，capture set は範囲外で，型検査の「まだ扱えない」のまま（lowering に来ても `Unsupported` にして panic しない）．
alpha.3 の第1段（型検査）で suspend の実装と `@external suspend fn` は型検査を通るようになった．lowering はまだ扱わず，suspend の実装を持つ handler を with に書いたときと，`@external suspend fn` を出力に含めるときに `Unsupported` にする．

- （補）証拠の表（evidence）は，エフェクトからハンドラの値への表．宣言の型の R が空でない fn と，型の R が空でないラムダは，証拠の表を最後の引数（`ev`）に受ける．R が空の関数の IR と出力は変わらない．R が見えない集合（`R(f)`）や汎化した集合だけの関数（`List.map`）も受ける（引数の関数へ渡すため）．
- （補）表は1つの値で渡す（エフェクトごとの引数にしない）．`List.map` のように中身の分からない R の関数を呼ぶ側は，どのエフェクトが要るか分からないので，今の表をまるごと渡す．
- （補）直接の呼び出しは，呼ばれる fn が表を受けるなら今の表を渡す．関数の値の呼び出しは，呼ばれる側の式の型の R が空でなければ渡す．R が空の関数の値に表を渡しても無視される（JS）．今の表がない所（R が空の関数の中）では `()` を渡す．型検査が R を確かめてあるので，そのとき呼ばれる側は表を使わない．
- （補）`use X` は，同じ関数（トップレベルの fn か handler の操作）の中で X を供給する最も内側の with のハンドラを直接指す（8.3）．なければ今の証拠の表から引く（`Expr::Use`）．
- （補）ラムダの中の `use X` も，ラムダを囲む with（同じ関数の中のもの）を先に見る．なければラムダの呼び出し元から渡された表から引く．仕様 8.3 の「同じ関数に該当する with がなければ呼び出し元から」の「関数」を，囲む with についてはトップレベルの fn，表についてはラムダと読んだ．型検査はラムダの本体の R をラムダの型に閉じ込める（囲む関数の R に足さない）ので，囲む関数の表は X を持っているとは限らず，ラムダの呼び出し元から受けるしかない．ラムダの中で使う表は，受けた表に囲む with の束縛を足したもの．
- （補）`with H1, ..., Hn { e }` は `Expr::With`．ハンドラの値を依存の順に作り（ハンドラごとの関数を呼ぶ），今の表に供給するエフェクトを足した新しい表（`Expr::Provide`）で本体を評価する．同じエフェクトは後ろが優先．本体が表を使わないとき（`use` が with のハンドラを直接指すだけ）は表を作らない．`With` の本体は末尾位置でない（6.8，`tail.rs` は辿らない）．
- （補）ハンドラの依存（`ProgramTypes::handler_uses`）は，同じ with の別のエフェクトを供給するハンドラ（同じエフェクトが複数なら後ろのもの）が満たし，残りと自分のエフェクトへの依存は外側の表が満たす（8.3，14.2）．依存のあるハンドラの関数は，それを足した表を引数に受ける．循環は型検査が E0411 にしてある．
- （補）状態を持たない handler は，ハンドラの値（`Expr::Handler`．操作は宣言の順のラムダ）を返す IR の関数にする．名前は handler の IR の名前（`JsConsole.JsConsole`）．操作の本体は証拠の表を受けない（効果は with の位置で決まっている）．操作の本体の失敗は，ラムダの失敗と同じく操作の呼び出し元へ戻り値で返る．
- （補）能力の値はハンドラの値そのもの．外側で取り出した能力の値は変数に入っているので，内側の with で上書きされても外側のハンドラを指す（8.3）．能力を捕まえたクロージャ（8.2 の `copy`）も変数を捕まえるだけ．
- （補）操作の呼び出し `log.info(x)` は `Callee::Op`（ハンドラの値，エフェクト，操作の添字）．`fails` は操作の型の E で決める．操作を呼ばずに値として使う（`f = log.info`）ときは，能力の値をその場で評価して束縛し，操作を呼ぶだけのラムダにする．
- （補）IR のエフェクト（`EffectDef`）は使ったものだけを入れる．名前は IR の名前（`Console.Console`）で，証拠の表の鍵に使う．

## 外部関数（12章）

- （補）`@external` の fn は `Function::external`（`External`）を持つ IR の関数にする．ターゲットごとの指定（`ExternalBinding`）を全部持ち，選ぶのはバックエンド．呼び出しは普通の直接の呼び出しで，`fails` はいつも偽（12.5），表も渡さない（R は空，12.2）．
- （補）ファイルを指すモジュール（`./` か `../` で始まる，引数が2つのときの1つ目）は，宣言を含むソースのディレクトリからの絶対パスに直す（12.1）．`.` と `..` は字面で畳む．パスは `LowerInput::paths`（driver がソースの表から渡す）．
- （補）引数と戻り値の境界での扱い（`Boundary`）は型から決める．String はホストから受け取るときにサロゲートを直し，`()` はホストの返した値を捨て，関数型はコールバックとして包む（12.4，12.5）．
- （補）ターゲットの検査（12.1）は lowering の後で IR をたどって行う（`LowerInput::target`）．根は `Roots::Exported` なら main，`Roots::Tests` ならテスト関数．with に書いたハンドラの関数を呼ぶので，操作の本体から呼ぶ関数もたどれる側に入る．たどれる外部関数にそのターゲットの `@external` がなければ `LowerError::MissingExternal`（driver が E0412 にする）．main のないエントリでは検査しない．`Roots::All` は検査しない．
- （補）たどれない外部関数でも，エントリの `pub` の fn から写されることはある．そのターゲットの指定がなければ，バックエンドは呼ばれたら defect にする関数を出す．
- （補）`Roots::All` は本体のある fn と const に加えて，`@external` の fn と handler も根にする（同梱の `JsConsole` を写せることを `every_core_function_lowers` で見る）．
- （補）同梱の core のソースはディスク上のパスを持たないので，core の `@external(js, "./x.mjs", …)` はソースの隣でなく，同梱の JS のファイル（`lib/` の `.mjs` を `include_str!` で埋め込んだ `core_js_files()`，`LowerInput::core_js_files`）から引く．core のソースは `lib/` の直下にあるとみなし，指定の `.` と `..` を畳んだ `lib/` からのパスで引く．`lib/` の外へ出る指定と表にない指定は `LowerError::MissingCoreFile`（同梱のソースの誤り．driver は内部の誤りの診断にする）．
- （補）引いた指定は，出力先の `emela_core/`（`CORE_JS_DIR`）に書き出したファイルへの，エントリの出力からの相対パス（`./emela_core/x.mjs`）に直す．JS 出力は `./` で始まる指定を書かれたまま import する．利用者のソースのファイルの指定（絶対パスにして `file://` で読む）とは扱いを分ける．
- （補）`lib/` の `.mjs` は出力先へそのまま書き出すので，コメントを英語で書く（`.emel` のコメントは出力に入らないので日本語のまま）
- （補）写した外部関数が同梱のファイルを1つでも指したら，同梱の JS のファイルを全部 `Module::js_files` に入れる（どのファイルがどれを import するかを JS を読まずに済ませるため，`lib/` と同じ並びで全部書き出す）．1つも指さなければ空で，何も書き出さない．
- （補）core の WASM の `@external` のファイルの指定は直さない（WASM は範囲外）．

## 中断と再開（8.4，8.7，14.5，16.5）

alpha.3 の第2段の前半（2026-10-12）．IR とバックエンドだけを先に作り，手で組んだ IR で確かめた．lowering はまだ印を付けない（全部偽．suspend fn の操作の実装は今までどおり `Unsupported`）．

- （補）中断しうる関数は `Function::suspends`，中断しうるラムダは `Expr::Lambda::suspends`，中断しうる呼び出しは `Expr::Call::suspends` の印で持つ．真の呼び出しは真の関数かラムダの本体の中にだけ書ける．lowering は型（どの関数が中断しうるか）から付ける
- （補）`Callee::Direct` の呼び出しの印は，呼ばれる関数の印と同じでなければならない．`Callee::Indirect` は呼ばれる側の式の型で決め，真でも実際の関数の値は中断しないものでよい（逆は不可）．`List.map` のように中身の分からない R の関数を呼ぶ側も真にする．`Callee::Op` は suspend fn の操作なら真
- （補）エフェクトの操作は `OpDef`（名前と `suspend`）．suspend fn の操作のハンドラの実装（`Expr::Handler::ops` のラムダ）は，宣言の引数の後ろに resume を1つ多く受ける．resume は `fn(A) -> ()` の普通の関数の値で，それを呼ぶ式は中断も失敗もしない（`Callee::Indirect`，`fails` も `suspends` も偽）
- （補）`Expr::Handler::name` はハンドラの名前（ソースの名前）．2回目の resume の defect の文面に使う（14.5）
- （補）`@external suspend fn` は `Function::suspends` が真の外部関数（`External` に印は足さない）
- （補）ダンプでは中断しうる呼び出しの後ろに `!`（失敗の `?` の前），中断しうる関数とラムダと suspend fn の操作の前に `suspend`，ハンドラの値は `handler 名前: エフェクト { … }`
