# emela-core の補足

仕様にない判断を「（補）」として残す．

- （補）名前はすべて `la_arena::Idx` で持つ．関数は `FnId`，局所変数は `Local`，enum は `EnumId`．`Local` の `hint` と関数の `name` は出力の読みやすさのためだけに使う．関数名はモジュール内で一意とし，公開名にもそのまま使う．
- （補）IR の型はモジュールの表（`Module::types`，`TypeTable`）に1度ずつ登録し，`TypeId` で持つ（hash-consing）．同じ型は同じ ID になるので，型の比較は ID の比較で済む．構造は `Module::ty` で引き，`Type` の部品も `TypeId`．部品のない型（`TypeId::INT`，`TypeId::UNIT` など）は ID が決まっていて，モジュールなしで書ける．型を木（`Box` / `Vec`）のまま複製して持っていたころは，大きなプログラムでメモリが 5〜10% 増えていた．`Type::Param` の置き換えは `TypeTable::subst`（型引数を含まない型はたどらずに返す）．`Module::local` と `Module::define` は `Type` もそのまま受け，その場で登録する（手で IR を組むテストのため）
- （補）局所変数はモジュール全体で一意にする（同じ `Local` を2回束縛しない）．置き換えや変換で遮蔽を考えなくてよくするため．
- （補）match は決定木でなく腕の列で持つ．決定木への変換はバックエンドの最適化として後で入れる．
- （補）`type` は構成子1つの enum として `EnumDef` に入れる．構成子とパターンのフィールドは定義順に並べ，パターンで省いた名前付きフィールドは `Pat::Wild` で埋める．フィールドの参照は名前でなく添字（`Expr::Field { index }`）で持つ．
- （補）Bool は enum `True` / `False` だが，IR では `Lit::Bool` と `OpTy::Bool` の組み込みで持つ．パターンも `Pat::Lit(Lit::Bool(_))` で書く．
- （補）`()` は `Lit::Unit`，`Expr::Tuple` は要素2つ以上だけに使う．
- （補）二項演算と単項演算には対象の型（`OpTy`）を付ける．Int / Int64 / Float で意味が違うため．`==` / `!=` で基本型以外を比べるときは `OpTy::Structural`．順序比較の `Structural` は Trait（Ord）の段まで持たない．
- （補）文字列の補間は `Expr::Concat` で，埋め込む値には IR の型 `Type` を付ける．バックエンドは型から表示関数を作る（Show の仮実装．Trait にするのは 0.21）．型は `Type::Param` を含まない具体的な型に限る．型引数の値を表示するには Show の辞書渡しか単相化が要るので，0.21 まで汎用の関数の中では表示できない．
- （補）ラムダは捕捉する変数を IR に書かない．capture set は WASM の段で求める．
- （補）名前付き引数は `named::in_written_order` で位置の順へ並べ替える．書かれた順と位置の順が食い違うときだけ，リテラル・変数・関数・ラムダ以外の引数を書かれた順に `let` で束縛する．呼び出し先が式のときに先に評価する責任は呼び出し側にある．
- （補）自己末尾呼び出しのループ化は `Expr::Loop` / `Expr::Recur` を作る．ターゲットに依らないので `lower` の最後に1回かけ，どのバックエンドもループ化済みの IR を受け取る．ループの変数は引数とは別の新しい `Local` にして回ごとに束縛し，ラムダが捕捉した値が後の回に書き換わらないようにする．
- （補）末尾位置として辿るのは `let` の本体，`if` の両枝，match の腕の本体，`&&` / `||` の右辺．右辺に自己末尾呼び出しがある `a && b` は `if a then b else false`，`a || b` は `if a then true else b` に脱糖してから `Recur` にする（仕様 6.8 の末尾位置の定義に反映済み）．呼び出しの引数の数が引数の数と違うものは対象外．ラムダの中の呼び出しは対象外．
- （補）エラーとエフェクトを足すとき，escape の対象の中と with の本体の中は末尾位置にしない（仕様 6.8 に反映済み）．
- （補）エラーとエフェクトは下の「エラー」と「エフェクト」の節．

- （補）`assert`（13.2）は `Expr::Assert` で持つ．比較の形（`AssertCond::Compare`: 演算子，`OpTy`，両辺の型 `Type`，両辺）と，比較でない形（`AssertCond::Bool`）に分け，ソースの式の字面 `text` を添える．値は `()`．両辺の型は表示関数を作るのに使う

## 組み込み関数（`intrinsics.rs`）

- （補）組み込み関数は `Builtin` の1値ごとに，モジュール名と関数名，型（`Sig`），純粋かを `Builtin::info` で持つ．実装はターゲットごとに backend が `Builtin` から引く（JS は `emela-codegen-js` の `emit/builtins.rs`）．名前解決は `Builtin::lookup(module, name)`，型推論は `BuiltinInfo::scheme(option)` で引く．Prelude の関数（`panic`，`todo`，`dbg`）は `module` が `None`．
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
- （補）組み込み関数の呼び出しは型引数を `Expr::Builtin` の `ty_args` で持つ（数は `Builtin::info().ty_params`）．`dbg` は表示関数を作るのにも使う．セルの組み込み関数（`$Cell.new` など）は中身の型を1つ受ける．`Sig` ではセルの型を書けないので，検証器と `Typer` が特別に扱う

## 同梱のソース（`lib/`，`sources.rs`）

- （補）同梱のソースは `lib/core/`（core．Prelude と `@intrinsic` を持つモジュール: `Prelude`，`List`，`Option`，`String`，`Int`，`Int64`，`Float`）と `lib/std/`（std．`Std.Duration`，`Std.Instant`，`Std.State`，`Std.Console`，`Std.Random`，`Std.Env`，`Std.Clock`，`Std.Http`，`Std.Path`，`Std.FileSystem`，`Std.Platform`）に分ける（15.2）．`core_sources()` がモジュール名とソースの組を返し，Prelude が先頭．ファイル名はモジュール名の最後の区切り（`std/console.emel` が `Std.Console`）
- （補）std のモジュールは `Std` の下の2区切りの名前で，import して使う（`import Std.Console.{Console, SystemConsole}`）．名前が1区切りでないので，import なしで `.` の左に書く規則と，型の位置の名前をモジュールと同じ名前の型に解決する規則（4.6）は当たらない．std のモジュールどうしも import でつなぐ（`Std.Instant` は `import Std.Duration.{Duration}`）
- （補）std のモジュールを足しても，利用者のファイル名は予約名にならない．予約名は core のモジュール名と `std.emel`，`std/`（4.1．emela-resolve の NOTES）
- （補）同梱の JS のファイルは，それを指すソースと同じディレクトリに置く（`lib/std/clock.mjs`）．`@external(js, "./clock.mjs", …)` はソースのディレクトリ（`lib_dir`．std のモジュールは `std`，それ以外は `core`）からの相対パスとして引く．IR では `lib/` からのパス（`HostModule::Bundled("std/clock.mjs")`）で持ち，出力先のどこに置くかは backend が決める（JS は `emela_core/std/clock.mjs`）
- （補）`@intrinsic` の宣言は `Builtin::lookup_intrinsic(モジュール名, 関数名)` で表を引く．`"Prelude"` は表の `module: None`（修飾せずに呼ぶ関数）に当たる．
- （補）`@intrinsic` の宣言は，引数と戻り値の型，型引数の数を表と一致させる．公開しない補助（`Int.parse` の中身の `parse_or_none`）は `pub` を付けない．表の全行がちょうど1回ずつ宣言されていることもテストで確かめる．引数名（名前付き引数のラベル）はソースが正で，表は持たない．
- （補）今の構文の検査は本体のない fn に `@external` を求める（E0130）ので，同梱のソースの `@intrinsic` の宣言にも E0130 が出る．テストはこれだけを許している．`@intrinsic` を同梱のソースに限って通す検査は名前解決の側で入れる．
- （補）Prelude の `Bool` は `False`，`True` の順に宣言する．導出する Ord で `False < True` にするため．IR では Bool は `Lit::Bool` の組み込みのまま（上の Bool の項）．
- （補）Prelude の enum（Bool，Option，Ordering）には `derive Eq, Ord, Show, Hash` を書く．Option の型引数の制約の扱いは導出の側で決める．
- （補）`dbg` は `dbg[A: Show](value: A) -> A` と書く．表の `ty_params` は制約を持たないので，制約は型推論の側でソースから読む．
- （補）core の関数は対象を第1引数に置く（パイプの左辺が入る）．List は `xs`，Option は `opt`，畳み込みの初期値は `from`（15.3）．
- （補）高階関数の関数の引数名は `f`（位置で渡す）．15.3 の `with` と `where` のラベルは，`with` が予約語（3.1），`where` が将来のための予約（3.3）で仮引数の名前にできないため取りやめになった（2026-10-12 本人の決定．仕様 15.3 と 6.7 も改めた）．畳み込みの初期値の `from` は残す
- （補）繰り返しは自己末尾呼び出しで書き，先頭から結果を積むものは逆順に積んで `reverse` する．補助の関数は非公開の `*_onto`（`reverse_onto` など）．
- （補）15.3 のとおり，2つをつなぐのは `List.append(xs, ys)` と `String.append(a, b)`，平らにするのは `List.flatten(xss)` と `String.concat(parts)`．`String.concat` は Emela で書いた（`join(parts, "")`）
- （補）変換は依存する側のモジュールに行きと帰りをまとめる（15.3）．`Int.to_float` は `Float.from_int` に（Float は `floor` などの Int への変換も持つので），`Int.to_int64` は消して `Int64.from_int` だけにした．`String.from_int` は消した（`Int.to_string`）．`String.code_points` と `String.from_code_point` は String の側のまま
- （補）`Int.parse(s) -> Int fails ParseError` は10進の数字と先頭の `-` か `+` だけを読む．空白，`_`，16進，範囲外は `ParseError`．`Float.parse(s) -> Float fails ParseError` は `Float.to_string` の書く形（数字，`.` と小数部，`e` と指数，先頭の符号，`Infinity`，`-Infinity`，`NaN`）を読む．`1.`，`.5`，`inf` は読まない．どちらも本体は Emela で，中身は公開しない `@intrinsic fn parse_or_none(s) -> Option[...]`（組み込み関数は失敗できないため）
- （補）`ParseError` は Prelude の `pub error ParseError(input: String)` にした．15.3 の例が修飾なしの `fails ParseError` で書いているのと，Int と Float で同じエラーを受けられるようにするため．`input` は入力の全体
- （補）`List.tail([])` は `[]`（17.5 の例 `[x, ..List.tail(xs)]` が List を受けるため Option にしない）．`List.head` と `List.get` は Option．`List.get` の負の添字は `None`．
- （補）`List.range(from, to)` は `from` 以上 `to` 未満．`List.zip` は短い方に合わせる．
- （補）`Option.or_fail[A, E](opt: Option[A], err: E) -> A fails E` の引数名は仕様 7.2 の例に合わせて `err`．本体も 7.2 のとおり（`None -> fail err`）．E が入ったので `option.emel` に同梱した（2026-10-12）．

## platform のハンドラと Platform（`std/console.emel`，`std/clock.emel`，`std/random.emel`，`std/env.emel`，`std/platform.emel`）

alpha.2〜alpha.3 で JS 用のハンドラ（`JsConsole` など）を別のモジュールに置いて同梱し，alpha.4 で仕様 15.2，18.1 #33 の形に移した（2026-10-12）．

- （補）ホストを使うハンドラは `System*`（`SystemConsole`，`SystemClock`，`SystemRandom`，`SystemEnv`），テスト用は `Test*`（`TestConsole`，`TestClock`）で，どちらもエフェクトと同じモジュールに置く．外部関数はハンドラと同じモジュールの非公開の fn．今の外部関数は JS の `@external` だけを持つ（WASM の注釈は WASM の出力を入れる段で並べる，12.1）
- （補）`Std.Platform` の layer `Platform` は `SystemConsole`，`SystemClock`，`SystemRandom`，`SystemEnv`，`SystemHttp`，`SystemFileSystem` の6つ（`SystemHttp` と `SystemFileSystem` は alpha.5 で足した）．並び順は意味を持たない（8.5）．SystemClock，SystemHttp，SystemFileSystem が Async に依存するので，`with Platform { ... }` の R には Async が残る（main とテスト関数ではランタイムが供給する）
- （補）エフェクト `Console` の操作は `print_line(text: String) -> ()`（標準出力）と `print_error_line(text: String) -> ()`（標準エラー）の2つ．どちらも改行を付けて書き，失敗しない．`SystemConsole` は `console.log` と `console.error` を呼ぶ．状態を持たない
- （補）ホストの関数は `use` を書かない `@external` の fn（ホストの副作用を持つので，ハンドラの操作の本体からしか呼べない．12.2）

## FileSystem と Path（`std/file_system.emel`，`std/file_system.mjs`，`std/path.emel`）

alpha.5 で同梱した（2026-10-13）．仕様 15.2 の宣言と規則に従い，仕様にない点をここで決めた．

- （補）長い `fails` は次の行に折り返す（2.5 の規則4，`emela fmt` の折り返し）．規則4が入るまでは1行に書いていた
- （補）error 7つは `pub error`（利用者が escape の腕に名前で書くため）
- （補）外部関数は List も失敗も返せない（12.4，12.5）ので，SystemFileSystem の外部関数（`@external suspend fn host_*`）はホストの結果を `@external type Outcome` で返す．中身は `{ error, message, text, names }` で，`error` は成功なら `""`，失敗なら fail する error の名前．取り出す関数（`outcome_error` など）は `use {}` の純粋な外部関数にした（不変のオブジェクトを読むだけなので）．非公開の `check_*`（操作ごとの `fails` と同じ集合）が名前を fail に変える．名前のリストは件数と添字で取り出し，末尾から積む
- （補）ホストのエラーの写し方は操作ごとに `file_system.mjs` で決める．`ENOENT` と `ENOTDIR`（途中にファイルがある）は FileNotFound，`EACCES` と `EPERM` は FileAccessDenied，それ以外は FileSystemFailure（`message` はホストの例外のメッセージ）．例外: read_text，write_text，append_text の `EISDIR` は IsADirectory．list_directory の `ENOTDIR`（そのパスか途中がファイル）は NotADirectory．create_directory の `ENOTDIR` と `EEXIST`（そのパスがファイル）は NotADirectory．remove の `ENOTEMPTY`（と `EEXIST`）は DirectoryNotEmpty
- （補）read_text は `TextDecoder("utf-8", { fatal: true, ignoreBOM: true })` で読む．`ignoreBOM` で先頭の BOM（U+FEFF）を本文に残す．読んで書き戻したときに中身が変わらないようにするため（15.2 の InvalidText と同じ考え方）
- （補）exists と is_directory は `stat`（シンボリックリンクを辿る）で見て，どのエラーでも False．remove は `lstat` で見て，シンボリックリンクはリンクそのものを消す
- （補）list_directory の「名前の順」はコードポイントの辞書順（String の `<`，10.6）．JS の既定の `sort`（UTF-16 の単位の順）と違うので比較の関数を書いた
- （補）create_directory は `mkdir(path, { recursive: true })` の後に `stat` で確かめる．そのパスがファイルのとき `recursive` の mkdir がエラーにしないホストがあるため
- （補）FileSystem の操作は中断（AbortSignal）で止めない（読み書きは短いため．TASK_BRIEF の範囲）
- （補）TestFileSystem: パスは文字列のまま比べ，正規化しない（`a/b` と `./a/b` は別）．ディレクトリは `directories` にあるか，`files` か `directories` のパスの祖先（`Path.parent` を辿ったもの）．`/` はいつもディレクトリ．親のないパス（`a.txt`）の親は問わない（いつも書ける）
- （補）TestFileSystem の create_directory は，まだディレクトリでない祖先とそのパスを `directories` に足す（既にあれば状態を変えない）．remove は消したパスの祖先を `directories` に足す．祖先が「ファイルの祖先」としてだけあったとき，最後のファイルを消しても本物のファイルシステムと同じくディレクトリが残るようにするため．`directories` も名前の順に保つ
- （補）TestFileSystem のエラーは SystemFileSystem の写し方にそろえる: 祖先にファイルがあるパスは，read_text，write_text，append_text，remove では FileNotFound，list_directory と create_directory では NotADirectory
- （補）`Path` は要素（`/` で分けた空でない部分）で考える．末尾と重なった `/` は無視する（`parent("a//b/")` は `Some("a")`）．`join(a, b)` は `b` が `/` で始まれば `b`（Rust の `Path::join`，Python と同じ．Node の `path.join` は単につなぐ），片方が空ならもう片方，`a` が `/` で終わればそのままつなぐ．`file_name` は最後の要素が `.` か `..` なら `None`．`extension` はファイル名の最後の `.` より後で，`.` がない，先頭の `.` だけ（`.bashrc`），`.` で終わる（`b.`）ものは `None`（Rust は `b.` に `Some("")` を返す）

## Instant と Duration（`std/instant.emel`，`std/duration.emel`）

alpha.3 で同梱した（2026-10-12）．仕様は「中身が Int64 の opaque な型」だけを決めている（15.2）ので，単位と関数の名前と数はここで決めた．

- （補）どちらも中身はミリ秒の Int64（`pub opaque type Instant(milliseconds: Int64)`，`Duration` も同じ形）．JS の `Date.now()` の単位に合わせた．Instant は Unix エポック（1970-01-01T00:00:00Z）からのミリ秒で，閏秒は数えない．Duration は負の長さも表す．
- （補）Duration は `milliseconds` / `seconds` / `minutes` / `hours`（引数は Int64．リテラルは文脈で Int64 になる），`to_milliseconds`，`to_seconds`（0 の方向へ切り捨て），`add`，`subtract`，`compare`（`Ordering`），`to_string`（`1500ms`）．
- （補）Instant は `to_unix_milliseconds`，`add` / `subtract`（Duration だけずらす），`since(t, earlier)`（`earlier` から `t` までの Duration．`now |> Instant.since(start)`），`compare`，`to_string`．
- （補）`Instant.to_string` は UTC の ISO 8601 で，JS の `Date.prototype.toISOString` と同じ形（`2025-10-12T03:04:05.678Z`．年が 0〜9999 の外なら `+010000` / `-000001` のように符号と6桁）．日付は proleptic Gregorian で，Emela で書いた（Hinnant の civil_from_days）．JS の Date の範囲（±8.64e15 ミリ秒）の外でも同じ式で書く．
- （補）単位の換算（`Duration.seconds(n)` の `n * 1000`）と足し算，引き算のあふれは Int64 の規則（16.1）で巻き戻る．検出しない．
- （補）現在の時点を取る関数は置かない（Clock のエフェクトの `now`）．`Instant.from_unix_milliseconds(ms: Int64) -> Instant` は公開する．Instant に守る不変条件はなく，作れる場所を Clock に絞っても `Instant.add` で任意の時点を作れるので保証にならない．テストで時刻を固定する，利用者が Clock のハンドラを書く（8.4），自前のホストの時刻を受ける，のいずれにも要る（2026-10-12 本人と決定．いったん `@internal`（core の中からだけ参照できる．emela-driver の NOTES）で絞ったが外した．仕組みは残す）．opaque は Int64 との取り違えを防ぎ，単位を隠す役を残す．
- （補）大小比較の演算子は Ord の段（Trait）まで opaque な型に使えないので，`compare` を関数で持つ．`==` は構造の比較で使える．`dbg` と補間では中身がそのまま見える（`Instant(milliseconds: …)`．Show を Trait にする 0.21 で `to_string` に揃える）．
- （補）`Std.Instant` は `import Std.Duration.{Duration}` で Duration を取り込む．

## Random（`std/random.emel`，`std/random.mjs`）

alpha.3 で同梱した（2026-10-12）．

- （補）エフェクト `Random` の操作は `int() -> Int`（Int の全域で一様）と `float() -> Float`（0 以上 1 未満で一様）の2つ．0.1x の spec 0054 の `bytes(len)` は，0.20 に Bytes がないので入れない．
- （補）範囲つきの `between(min, max)` は操作にしない．入れるなら `int()` の上に書く普通の関数にする（ハンドラが実装する操作を少なく保ち，テスト用のハンドラを書きやすくするため）．種を渡せる PRNG（spec 0054 の Part B）はエフェクトでなく純粋な値と関数にする．どちらも alpha.3 には入れない．
- （補）`SystemRandom` は `crypto.getRandomValues` で引く（暗号論的に安全）．`Math.random` は使わない．関数のパスひとつで書けないので補助の `lib/std/random.mjs` を置く（#155）．
- （補）`int` は `Int32Array` で受ける．符号付き 32 ビットになるので，Int（16.1）へそのまま渡せる．
- （補）`float` は 32 ビットを2つ引き，上位 27 ビットと 26 ビットから 53 ビットの整数を作って 2^53 で割る．0 以上 1 未満で，Float で表せる 2^-53 刻みの値が一様に出る．

## Http（`std/http.emel`，`std/http.mjs`）

alpha.5 で同梱した（2026-10-13）．宣言とシグネチャは仕様 15.2 のとおり．

- （補）error 4つ（`InvalidUrl`，`ConnectionFailed`，`ResponseNotText`，`UnexpectedStatus`）は `pub error` にした．仕様 15.2 の宣言は `pub` を省いているが，利用者が escape で受けるには import できなければならない
- （補）`header` と `headers` の名前の比べ方は ASCII の A〜Z だけを小文字にして比べる．HTTP のヘッダの名前は ASCII のトークンなので足りる．core の String に小文字にする関数がないので，非公開の `ascii_lower` を `String.code_points` の上に書いた
- （補）`SystemHttp` は `@external type` の `HostRequest` と `HostResponse` で境界を越える（12.4）．要求は `newRequest(method, url, body)` に `withHeader` を畳み込んで作り（ヘッダは足すたびに写しを返すので，`List.fold` で純粋な形に書ける），応答は種類（0 が応答，1〜3 が失敗の種類）と，status，本文，添字で引くヘッダの名前と値を関数で取り出す．`send` は Promise を拒否せず，失敗も値で返す（拒否は defect になる．12.5）
- （補）`http.mjs` の `send(req, signal)` は最後の引数の `signal` を `fetch` に渡す．今のランタイムは signal を足さないので `undefined` が来て，そのまま動く（12.2 の規則は js-fiber-runtime の2本目が入れる）．中断で abort したときの結果は ConnectionFailed（`This operation was aborted`）の値になるが，中断した fiber はその結果を捨てる
- （補）InvalidUrl は `new URL(url)` で読めないときと，scheme が `http:` と `https:` 以外のときにした．fetch は `ftp:` や `file:` を URL としては読めても送れず，ConnectionFailed にすると「送ってみて届かなかった」と区別できないため．利用者が書いた URL の誤りとして扱う
- （補）ConnectionFailed の message は fetch の例外の message に，`cause` があれば `: ` の後に cause の `code` と `message` を空白でつないで足す（Node では `fetch failed: ECONNREFUSED connect ECONNREFUSED 127.0.0.1:1234`）．応答の本文を読む途中の失敗も ConnectionFailed．GET と HEAD に空でない本文を書いたとき（fetch が受け付けない）と，ヘッダの名前や値が正しくないとき（`Headers.append` の例外）も ConnectionFailed にした．どちらも利用者の誤りだが，要求の値は組み立てられてしまうので defect にはしない．GET と HEAD の空の本文は本文なしで送る
- （補）content-type の既定は fetch に任せる．文字列の本文には fetch が `text/plain;charset=UTF-8` を付けるので仕様の既定と一致する．GET と HEAD（本文なし）には付かない
- （補）応答のヘッダは fetch の Headers を回して `set-cookie` 以外を取り（名前は小文字，同じ名前は `, ` でつながれている），`set-cookie` は `getSetCookie()`（ない古い Node では `get("set-cookie")` の1つ）で1つずつ足し，名前で安定に並べ替える．set-cookie どうしは受けた順のまま
- （補）本文は `TextDecoder("utf-8", { fatal: true })` で読む．先頭の BOM は TextDecoder の既定のとおり取り除く
- （補）リダイレクトは fetch の既定（20 回まで）で辿り，超えたら ConnectionFailed．Cookie は fetch が覚えない．プロキシの環境変数は Node の fetch が既定で読まない（Node 24 の `NODE_USE_ENV_PROXY` を設定したときだけ読むが，それは利用者が選んだものとして止めない）
- （補）TestHttp は，状態の `sent` に要求を足してから `respond(request, n)` を呼ぶ（`n` は足す前の `sent` の長さ）．`None` の ConnectionFailed の message は `TestServer returned no response`．`respond` が `sent` を見られないので，要求を足す順と呼ぶ順はどちらでも結果は同じ
- （補）`TestServer` の `respond` の型 `fn(Request, Int) -> Option[Response]` は型宣言の中の関数型なので純粋（9.2）

## Env（`std/env.emel`，`std/env.mjs`）

alpha.3 で同梱した（2026-10-12）．

- （補）エフェクト `Env` の操作は `get(name: String) -> Option[String]` だけ．全部の変数を返す操作（`List[(String, String)]`）は，境界の変換が増えるので入れない．
- （補）`Option[String]` は外部関数の境界を越えられない（12.4）ので，ホストの関数は有無の `has(name) -> Bool` と値の `get(name) -> String` の2つに分け，`SystemEnv` の操作の本体で Option に組み立てる．`@external type` で値と有無をまとめる案より，ホストの側が単純になる．
- （補）空の値（`NAME=`）は `Some("")`，未設定は `None`．`has` は `Object.hasOwn(process.env, name)` で見る（`process.env[name] !== undefined` でも同じだが，プロトタイプの名前を拾わないことを明示する）．
- （補）`has` と `get` の間に環境変数が変わることは考えない（JS のプログラムの中から `process.env` を書き換える手段を core は持たない）．

## Clock（`std/clock.emel`，`std/clock.mjs`）

alpha.3 で同梱した（2026-10-12）．

- （補）エフェクト `Clock` の操作は仕様 8.1 のとおり `suspend fn sleep(ms: Int) -> ()` と `fn now() -> Instant` の2つ（宣言の順も 8.1 に合わせた）．
- （補）`SystemClock` は状態を持たないハンドラ．`now` は `Date.now()` を `BigInt` にして返す同梱の `std/clock.mjs` の `nowMilliseconds` を呼び，`Instant.from_unix_milliseconds` で包む．Int64 は JS では BigInt なので，境界（12.4）はそのまま渡る．`Date.now()` を直接 `@external` にしないのは Number を返すため．
- （補）`SystemClock` の `sleep` は，`std/clock.mjs` の `sleep(ms, signal)`（Promise を返す `setTimeout`）を `@external suspend fn sleep_milliseconds` として呼んで待ち，その後で `resume(())` を呼ぶ（alpha.5，12.2）．待っている fiber が中断されると，ランタイムが最後の引数の AbortSignal を abort し，タイマーを `clearTimeout` で消す．alpha.3 からのコールバックの形（`after(ms, f)` から resume）は中断で止まらないのでやめた．`@external suspend fn` を呼ぶので SystemClock は Async に依存し，`with SystemClock { ... }` の R に Async が残る（8.7）．main とテスト関数ではランタイムが供給する．
- （補）`sleep` の負の `ms` は 0 と同じ（8.8）．defect にしない．`clock.mjs` の `sleep` が `setTimeout` に渡す前に 0 に丸める（Node 24 は負の値に TimeoutNegativeWarning を標準エラーに出すため）．待つ時間はタイマーの精度に従い，正確さは保証しない．
- （補）Clock が中断しうるので，Clock を使う関数はすべて中断しうる関数になる（`now` しか呼ばなくても．印は R で決まる，下の「lowering の印」）．
- （補）Clock を実装する利用者のハンドラは，`now` で `Instant.from_unix_milliseconds` を呼ぶか（#172 で公開），外側の Clock の `now` を呼べる（#173．fn の操作の本体から fn の操作を呼んでも E0417 にならない）．テスト用の時計（15.2 の TestClock）は，時刻を State に置くハンドラとして同梱した（下の「State，TestConsole，TestClock」）．

## State，TestConsole，TestClock（`std/state.emel`，`std/console.emel`，`std/clock.emel`）

alpha.4 の第2段（#183）で宣言を置き，第3段の後半（2026-10-12）で同梱した．

- （補）操作は仕様 8.8 のとおり `get`，`set`，`update` の3つ（宣言の順も合わせた）．どれも `fn`（suspend fn でない）なので State は Immediate
- （補）`State.run` は `@intrinsic` の本体のない宣言のまま，組み込み関数の表には載せない（`intrinsics::RUNTIME_HANDLERS`）．body を呼び，証拠の表に State のハンドラを足すので，関数1つの実装にならないため．lowering がその場で写す（下の「エフェクト」の `State.run`）
- （補）TestConsole は `Std.Console`，TestClock は `Std.Clock` に，エフェクトと一緒に置く（15.2）
- （補）TestConsole の `print_line` は，行を `State[List[String]]` に書いた順に足す（`List.append`）．仕様 8.8 の例は先頭に足す（`[text, ..st.get()]`）ので，2行以上だと逆順になる．読み出したリストをそのまま比べられるよう，書いた順にした．`print_error_line` は仕様 8.8 の例のとおり捨てる
- （補）TestClock は仮の時刻で動く（8.8，alpha.5）．`sleep(ms, resume)` は起きる時刻（`State[Instant]` の今の時刻 + `ms`）を求め，`std/clock.mjs` の `whenStalled(at, signal)` を `@external suspend fn when_stalled` として呼んで待つ．起きたら `State[Instant]` をその時刻まで進め（今の時刻の方が後なら進めない），resume する．負の `ms` は SystemClock と同じく 0 とみなす．`now` は `State[Instant]` を返す．`@external suspend fn` を呼ぶので TestClock は Async に依存する
- （補）「どの fiber も進めず，ホストの処理も待っていない」を知る仕組みはランタイムに置いた（emela-codegen-js の NOTES の「仮の時刻」）．Emela から呼ぶ形は `@intrinsic` でなく，ランタイムを指す `@external suspend fn`（`whenStalled` はランタイムの関数を `globalThis[Symbol.for("emela.whenStalled")]` から呼ぶだけ）にした．中断する `@intrinsic` は lowering と型検査に新しい道が要り，並行して作っている fiber-lowering とぶつかるため．また `@external suspend fn` なら，待っている fiber が中断されたとき AbortSignal で待ちの登録を消せる（12.2 と同じ道）

## fiber（`std/fiber.emel`，`std/cause.emel`，`std/deferred.emel`，11章）

- （補）ランタイムが持つ型のうち，`Fiber[A]` と `Deferred[A]` は `@external pub type`（12.3 のホストの型）で宣言する．Emela からは持ち回るだけで，中身はランタイムが決める
- （補）`Cause[E]` は `pub opaque type Cause[E](raise: fn() -> Never fails E)` と書く．フィールドのない型では E の種が決まらず（9.2 は使われた位置で決める），ふつうの型になると `Exit[A, E]` と `attempt` の E が1つのエラーの型（7.2）になってしまうため．フィールドは E をエラーの集合にするためだけのもので，読まない（opaque なので外から読めない）．値はランタイムが作る
- （補）Scope は `@intrinsic pub effect Scope[E, R]` と書く．`@intrinsic` が組み込みのエフェクトの印（emela-types の NOTES の「fiber」）
- （補）`scope`，`join`，`await`，`interrupt`，`attempt`，`Cause.kind`，`Cause.to_string`，`Cause.raise`，`Deferred.complete`，`Deferred.await`，`Deferred.poll` は `@intrinsic` だが，組み込み関数の表（`intrinsics.rs`）に行を持たない．lowering が `Expr::Fiber` に写す（`lower/fiber.rs` の `FiberFn`．下の「並行性」の「lowering からの接続」）．`sources.rs` のテストは `FIBER_INTRINSICS` を表との突き合わせから外す
- （補）`fork`，`deferred`，`all`，`race`，`both`，`map`，`timeout` は純 Emela．`all` と `map` は `Fiber.fork` で子を起動して順に join する．`race` は 11.3 の例のとおり．`both` は2つを fork して組にする．`timeout` は Deferred に `Some(f())` を入れる子と，Clock で ms 待ってから `None` を入れる子を fork して，先に入った方を返す（race と同じ形）
- （補）`timeout` が Clock を使うので，`Std.Fiber` は `Std.Clock` を import する
- （補）SystemClock の `sleep` の負の `ms`（8.8 では 0 とみなす）は，`clock.mjs` の `sleep` で 0 に丸めてから `setTimeout` に渡す（#201 で `after` に入れた丸めを，#199 で `sleep` の形に移した）．Node 24 は負の値に `TimeoutNegativeWarning` を標準エラーへ出すため（`Fiber.map` のテストで見つかった）
- （補）`await` は 3.3 の予約語から外した（進行役と合意．`Fiber.await` と `Deferred.await` の名前のため．emela-syntax の NOTES）

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
- （補）構成子のフィールドの型は型検査の構成子の宣言の型（`sigs`）から引く．構成子の型引数は作る型の型引数と同じ順なので，`Ty::Bound(i)` を `Type::Param(i)` に写せばよい．型の変換では，写している定義で汎化された `Ty::Var` はその型引数（`Type::Param`，`generalized_tys`）に，決まらずに残った `Ty::Var` は `Type::Unit` にする．関数型は `Type::Fn(FnType)` で，引数と戻り値の型に加え，呼び方を決める fails，suspends，証拠の表を取るか（ev）を持つ（それぞれ `Expr::Call` の印と同じ判定）．能力の値の型（`Ty::Cap`）は `Type::Handler(EffectId)`（表示も比較もできない値）にする．
- （補）JS で export するのは，エントリのモジュールの `pub` の fn と `main`（`emela run` の起動用モジュールが `entry.main` を呼ぶ）．
- （補）呼び出しの引数は，型検査が並べ直した仮引数の順（`ProgramTypes::calls`）から各引数の位置を引き，`named::in_written_order` で書かれた順に評価する．呼ばれる側が変数か関数でない式なら，引数より先に `let` で束縛する．パイプ `x |> f(a)` は左辺を先頭の引数とする呼び出しとして写す．
- （補）`@intrinsic` の fn の呼び出しは `Expr::Builtin` にする．型引数は `ProgramTypes::type_args` から引く．`@intrinsic` の fn とフィールドのある構成子を値として使うとき（`List.map(xs, String.length)`，`List.map(xs, Some)`）は，それを呼ぶだけのラムダで包む（`fn(x) { Some(x) }`）．引数の数は参照の式の型（関数型）から取る．
- （補）ブロックの文は `let` の入れ子にする．値を使わない文と `_ = e` は，捨てる用の新しい局所変数（`_`）に束縛する．変数と `_` でないパターンの束縛（`(a, b) = pair`）は腕1つの match にして，残りの文を腕の本体に入れる．必ず成功することは型検査が確かめてある（E0317）．
- （補）パターンの `True` / `False` は `Pat::Lit(Lit::Bool)`，`()` は `Pat::Lit(Lit::Unit)`．構成子のパターンのフィールドは `ProgramTypes::pat_fields`（位置の順，省いたものは `None`）から引き，`None` を `Pat::Wild` で埋める．括弧のない構成子のパターン（`Empty`）は表にないので，フィールドの数だけ `Wild` を並べる．const のパターンは腕のガードにする（下の項）．
- （補）演算子の `OpTy` は左辺の型から決める．`==` / `!=` で基本型でなければ `Structural`．大小比較で基本型でなければ（Ord）`Unsupported`．
- （補）フィールドの参照は，値の型（type か error．構成子は1つ）の構成子と，型検査が引いた添字（`ProgramTypes::fields`）で `Expr::Field` にする．表にないフィールドの参照は能力の操作（`log.info`，`ProgramTypes::ops`）で，下の「エフェクト」の節．
- （補）部分更新 `C(f: e, ..base)` は，書いたフィールドを書かれた順に評価して（リテラル・変数・関数・ラムダ以外は `let` で束縛して），`..base` を最後に評価して束縛する（14.1）．書いていないフィールドは `base` の `Expr::Field` で埋めて `Expr::Ctor` を作る．
- （補）補間は `Expr::Concat` にし，埋め込む値に IR の型を付ける．型が型引数（`Type::Param`）か関数を含むと表示関数を作れないので `Unsupported`（型引数は Show の辞書渡しか単相化が要るので 0.21，関数は表示できない（10.6）が Show の検査は Trait の段まで型検査にない）．
- （補）const は引数のない関数にして，参照のたびに呼ぶ（`LIMIT` → `LIMIT()`）．関数に `Function::constant` の印を付け，出力段が最初の呼び出しで1回だけ本体を評価して値を覚える（5.6）．使う場所に式を埋め込まないのは，const の式の中の局所変数を2回束縛することになるため．const は export しない．
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

alpha.2 の lowering（2026-10-12）．ハンドラは引数で渡して直接呼ぶ（evidence passing，16.5）．状態を持つ handler（フィールド，init，release），layer，suspend，capture set は範囲外で，型検査の「まだ扱えない」のまま（lowering に来ても `Unsupported` にして panic しない）．alpha.4 の第1段で型検査は状態を持つ handler，layer，`self` を通すようになり，第3段で lowering がそれらを IR にする（下の「状態のあるハンドラと layer の lowering」）．
alpha.3 の第1段（型検査）で suspend の実装と `@external suspend fn` は型検査を通るようになった．lowering はまだ扱わず，suspend の実装を持つ handler を with に書いたときと，`@external suspend fn` を出力に含めるときに `Unsupported` にする．

- （補）証拠の表（evidence）は，エフェクトからハンドラの値への表．宣言の型の R が空でない fn と，型の R が空でないラムダは，証拠の表を最後の引数（`ev`）に受ける．R が空の関数の IR と出力は変わらない．R が見えない集合（`R(f)`）や汎化した集合だけの関数（`List.map`）も受ける（引数の関数へ渡すため）．
- （補）表は1つの値で渡す（エフェクトごとの引数にしない）．`List.map` のように中身の分からない R の関数を呼ぶ側は，どのエフェクトが要るか分からないので，今の表をまるごと渡す．
- （補）直接の呼び出しは，呼ばれる fn が表を受けるなら今の表を渡す．関数の値の呼び出しは，呼ばれる側の式の型の R が空でなければ渡す．R が空の関数の値に表を渡しても無視される（JS）．今の表がない所（R が空の関数の中）では `()` を渡す．型検査が R を確かめてあるので，そのとき呼ばれる側は表を使わない．
- （補）`use X` は，同じ関数（トップレベルの fn か handler の操作）の中で X を供給する最も内側の with のハンドラを直接指す（8.3）．なければ今の証拠の表から引く（`Expr::Use`）．
- （補）ラムダの中の `use X` も，ラムダを囲む with（同じ関数の中のもの）を先に見る．なければラムダの呼び出し元から渡された表から引く．仕様 8.3 の「同じ関数に該当する with がなければ呼び出し元から」の「関数」を，囲む with についてはトップレベルの fn，表についてはラムダと読んだ．型検査はラムダの本体の R をラムダの型に閉じ込める（囲む関数の R に足さない）ので，囲む関数の表は X を持っているとは限らず，ラムダの呼び出し元から受けるしかない．ラムダの中で使う表は，受けた表に囲む with の束縛を足したもの．
- （補）`with H1, ..., Hn { e }` は `Expr::With`．ハンドラの値を型検査の決めた初期化の順（`ProgramTypes::with_handlers`）に作り（ハンドラごとの関数を呼ぶ），今の表に供給するエフェクトを足した新しい表（`Expr::Provide`）で本体を評価する．同じエフェクトは後ろが優先で，上書きされたハンドラは作らない（8.3）．本体が表を使わないとき（`use` が with のハンドラを直接指すだけ）は表を作らない．`With` の本体は末尾位置でない（6.8，`tail.rs` は辿らない）．
- （補）ハンドラの依存（`ProgramTypes::handler_uses`）は，同じ with の別のエフェクトを供給するハンドラ（`with_handlers` で先に初期化されたもの）が満たし，残りと自分のエフェクトへの依存は外側の表が満たす（8.3，14.2）．依存のあるハンドラの関数は，それを足した表を引数に受ける．循環は型検査が E0411 にしてある．
- （補）状態を持たない handler は，ハンドラの値（`Expr::Handler`．操作は宣言の順のラムダ）を返す IR の関数にする．名前は handler の IR の名前（`Std.Console.SystemConsole`）．操作の本体は証拠の表を受けない（効果は with の位置で決まっている）．操作の本体の失敗は，ラムダの失敗と同じく操作の呼び出し元へ戻り値で返る．
- （補）能力の値はハンドラの値そのもの．外側で取り出した能力の値は変数に入っているので，内側の with で上書きされても外側のハンドラを指す（8.3）．能力を捕まえたクロージャ（8.2 の `copy`）も変数を捕まえるだけ．
- （補）操作の呼び出し `log.info(x)` は `Callee::Op`（ハンドラの値，エフェクト，操作の添字，操作の型引数）．`fails` は操作の型の E で決める．操作を呼ばずに値として使う（`f = log.info`）ときは，能力の値をその場で評価して束縛し，操作を呼ぶだけのラムダにする．
- （補）IR のエフェクト（`EffectDef`）は使ったものだけを入れる．名前は IR の名前（`Console.Console`）で，証拠の表の鍵に使う．
- （補）証拠の表の鍵は，エフェクトと型引数の組（`EffectKey`）．型引数の違うエフェクト（`State[Int]` と `State[String]`，8.1）は別の IR のエフェクト（`EffectDef`）で，名前は `State.State[Int]` の形．型引数のないエフェクトの名前は今までどおり
- （補）鍵に書く型は，名前付き型を IR の名前，型引数（`Bound(i)`）を持ち主の定義の IR の名前と添字（`$modify.0`）にする．推論した定義の本体に残る汎化した型変数は，`ProgramTypes::generalized_tys` で型引数の添字に直す．汎化しなかった決まらない型変数は `_`（`()` として扱うので，どれも同じ鍵）．関数型の E と R は鍵に入れない（近似．型引数に関数型を書いた State で，E や R だけが違うものを区別しない）
- （補）汎用の fn の本体は，型引数の鍵（`State[$modify.0]`）でハンドラを引く．型引数を含むタグを宣言の R に持つ fn を呼ぶときは，呼ぶ側で具体化した鍵（`State[Int]`．呼ぶ側も汎用ならその型引数の鍵）のハンドラを，呼ばれる側の鍵で引けるように今の表に足して渡す（`generic_ev`）．まとまりの中の単相の参照（再帰）は具体化しないので足さない．そうした fn を値として使うとき（`f = modify`）は，表を受けて同じく足すラムダで包む
- （補）`State.run(initial, body)` はランタイムが供給するハンドラで，その場で写す: 初期値の入れ物を作り（内部の組み込み関数 `$cellNew`，`$cellGet`，`$cellSet`），それを読み書きする操作のハンドラの値を `State[S]` の鍵で今の表に足して body を呼び，body の値と最後の状態の組を返す．body の失敗はその呼び出しの失敗としてそのまま伝わり，状態は捨てる．body の中断も呼び出しの中断として扱う（状態は入れ物にあるので再開の後も引き継ぐ）．`update` は読んで `f` を呼んで書くまでを中断しない呼び出しで行う（`f` は純粋）．`State.run` を値として使うときも，表を受けるラムダで包む
- （補）入れ物の組み込み関数は `Builtin` に足したが `Builtin::ALL` には載せない（ソースから呼べず，表の照合にも入らない）．入れ物の型は IR の型で書けないので，型引数の型で代わりに書いた

### init と release（8.4，8.6，14.2，14.4，14.5）

alpha.4 の第1段（2026-10-12）．IR とバックエンドとランタイムを先に作り，手で組んだ IR で確かめた．lowering からの接続は第3段（下の「状態のあるハンドラと layer の lowering」）．

- （補）`Expr::With` の束縛は `WithBind`（変数，値，`release: Option<Release>`）．release のある束縛は init と release のあるハンドラの値で，束縛の値が init にあたる（失敗も中断もしてよい）．`Release` はハンドラの名前と，with を抜けるときに評価する式．式は同じ with のその束縛まで（自身を含む）の変数を参照してよい．init がハンドラの値でなく状態（`self`）を返す形なら，lowering は状態を release のない束縛に，状態を捕まえたハンドラの値を release のある束縛にして，release の式で状態を使う
- （補）依存の順の初期化は束縛の順，解放はその逆．束縛の値が失敗したら，後ろの束縛と本体は評価せず，それまでの release だけが逆の順に走る．失敗した束縛自身の release は走らない（14.2）．init を束縛の値にすると，この規則は「release のある束縛から後ろを1つの区間として囲む」だけで決まる
- （補）release の式は失敗しない（release の E は空，8.4）．IR では確かめない．中断はしてよい
- （補）release 後の使用の検査（8.6）は release のある束縛のハンドラの値にだけ付ける．release の式を評価した後（release が defect になっても），そのハンドラの値の操作はすべて defect（`handler <名前> was used after release`）になる．release のない束縛（状態を持たないハンドラ）は持ち出しても今までどおり動く．lowering は init か release かフィールドのあるハンドラを release のある束縛にする想定（init だけのハンドラは `release` を `()` にする）
- （補）ダンプでは release のある束縛を `let h = 値 release 名前 { 式 }` と出す

### 状態のあるハンドラと layer の lowering（8.3，8.4，8.5，8.6）

alpha.4 の第3段の前半（2026-10-12）．

- （補）状態のあるハンドラは，フィールド，init，release のどれかを持つ handler（仕様 8.6 の検査の対象と同じ）．状態を持たないハンドラの IR と出力は変えない
- （補）状態のあるハンドラの値（`self`）は，handler の名前の enum で，構成子は handler の構成子1つ（`Pool(url: …)`）．フィールドのない handler で init を書いたものは値の構成子（`Spare`），構成子のないもの（release だけ）は enum に構成子がなく，状態は `()`．フィールドの参照と部分更新は type と同じ `Expr::Field` と構成子の呼び出しになる
- （補）状態のあるハンドラは3つの IR の関数にする．init の関数 `名前$init` は状態を返す（init のないハンドラは構成子の値か `()`）．ハンドラの値を返す関数（handler の IR の名前）は状態を最初の引数に受け，操作のラムダが捕まえる．操作ごとの暗黙の `self` はどれもこの引数を指す．release の関数 `名前$release` は状態を受けて release の本体を評価する．名前の `$` は中断用の写しと同じく Emela の名前と重ならない
- （補）3つの関数は，依存(H)（`handler_uses`）に Async 以外があれば同じ証拠の表を最後の引数に受ける．依存(H) は init，release，操作の本体の R の和なので，3つに分けずに1つの表を渡す．with で表を作る式は変数に束縛して3回使う
- （補）init と release の関数が中断しうるかは，本体の式の R に Immediate でないエフェクトがあるかで決める．init の呼び出しの `fails` は init の E（`handler_fails`）が空でないか．release の呼び出しは失敗しない（E0420）
- （補）with では，状態のあるハンドラごとに，init の値を release のない束縛（`self`）にし，状態を受けて作ったハンドラの値を release のある束縛にする．release は release の関数の呼び出しで，release を書いていないハンドラは `()`．init が失敗したら，後ろの束縛と本体を飛ばし，先に初期化したハンドラだけが逆順に release される（`WithBind` の規則のとおり）
- （補）layer は lowering では何も作らない．with に書いた layer は型検査が展開して `with_handlers` に入れてあるので，lowering はその順に並べるだけ．同じエフェクトのハンドラのうち上書きされたものは `with_handlers` にないので，init も走らない
- （補）状態を持たない handler の操作の中の `self` は値を持たないので `()` にする（型は `()`）

## 外部関数（12章）

- （補）`@external` の fn は `Function::external`（`External`）を持つ IR の関数にする．ターゲットごとの指定（`ExternalBinding`）を全部持ち，選ぶのはバックエンド．引数の数で `HostRef` に分ける（1つは `Global`，2つは `Import`）．`Import` のモジュールは，`./` か `../` で始まらなければ `HostModule::Named`，利用者のソースの隣のファイルなら `File`（絶対パス），同梱のソースなら `Bundled`．名前の意味（JS のグローバル参照，WASM の import の名前）はバックエンドが決める．呼び出しは普通の直接の呼び出しで，`fails` はいつも偽（12.5），表も渡さない（R は空，12.2）．
- （補）ファイルを指すモジュール（`./` か `../` で始まる，引数が2つのときの1つ目）は，宣言を含むソースのディレクトリからの絶対パスに直す（12.1）．`.` と `..` は字面で畳む．パスは `LowerInput::paths`（driver がソースの表から渡す）．
- （補）引数と戻り値の境界での扱い（12.4，12.5）は IR に書かない．引数の局所変数の型（`LocalData::ty`）と `Function::ret` から，各 backend が決める．JS では，String はホストから受け取るときにサロゲートを直し，`()` はホストの返した値を捨て，関数型はコールバックとして包む．
- （補）IR の局所変数はすべて型を持つ（`LocalData::ty`）．関数は戻り値の型（`Function::ret`），失敗しうるか（`Function::fails`．宣言の E が空でないか．集合の型引数は空でないとみなす）と型引数の数（`Function::type_params`．型検査の型スキームの引数の数で，集合の引数も数える）を持つ．引数の型は引数の局所変数の型．lowering が作る一時変数のうち，型検査の型に対応しないものには IR だけの型を使う．証拠の表は `Type::Evidence`，`State.run` のセルは `Type::Cell(S)`，fiber のスコープは `Type::FiberScope`，Scope のハンドラの中で fork が受け取る子の関数の戻り値は `Type::Any`（`fork[A]` の A は呼ぶたびに違い，多相な操作を持てるのは組み込みのエフェクトだけ．8.1）
- （補）IR だけで式の型が決まるように，導けないものを IR に持たせる（2026-10-15，ir-type-args）．直接の呼び出し（`Callee::Direct`）と関数の値（`Expr::Fn`）は型引数 `ty_args`（数は `Function::type_params`），構成子（`Expr::Ctor`）は enum の型引数，リスト（`Expr::List`）は要素の型，ラムダ（`Expr::Lambda::ty`）は関数型そのもの．型引数は型検査の `ProgramTypes::type_args` から引く．まとまりの中で具体化されない参照（自分自身の再帰，型がまだ決まっていない定義）は，汎化した型変数を今の定義から見た型にする（自分自身なら自分の型引数をそのまま渡す）．const のパターンは参照の式がないので，const の宣言の型をパターンの型に突き合わせて読む
- （補）集合の型引数（`R(f)` など）は値の型を持たないので，汎用の関数の中の関数型の `fails` / `suspends` / `ev` は具体化より広め（真）に出る．汎用の関数から出てくる値は汎用の印のまま．単相化で呼び方を正確にするため，集合は要約（`Type::Set`）として IR に残す（下の「集合の要約」）．はじめは `ty_args` のその位置に `()` を置いていた
- （補）式の型は `Typer::type_of`（`typing.rs`）で求める．`let` の値，引数，ガード，ラムダの本体はたどらず，型を決める所（`let` / `with` / `loop` の本体，最初の `Never` でない枝，間接呼び出しの呼ばれる側，フィールドの元，タプルの要素）だけを見る．`let` の連なりは端の型を節ごとに覚えるので，全部の節で聞いても連なりの長さに比例する．型の表にない型（どの局所変数も持たないタプルの型など）は，初めて要るときに表を複製して足す（ID はモジュールの表の続き）．`&Module` だけで使える
- （補）分岐の型は `Never` でない最初の枝の型．操作の呼び出し（`Callee::Op`）の値は，`OpDef::ret` に `Callee::Op::ty_args` を代入した型．`FiberOp::Fork` と `FiberOp::Deferred` の返すハンドルは `Any`（型が呼ぶたびに違う）
- （補）検証器 `verify`（`verify.rs`）は，各関数の本体の型を下から1回たどって求め，値が型の決まった所へ流れ込む所を全部確かめる（引数，`let`，戻り値，構成子のフィールド，分岐どうし，`loop` / `recur`，パターン，組み込み関数，fiber の操作，操作の呼び出しの引数と値，ハンドラの値の操作の実装，証拠の表に足すハンドラ）．`Never` はどこへでも，`Any` は型の一番上の段だけで何とでも，`()` は `Type::Evidence` の所へ（表がない所で渡す `()`）流してよい．関数型どうしは引数と戻り値だけ比べ，印（`fails` / `suspends` / `ev`）は比べない（上の集合の型引数の理由）．呼び出し方を決める印は使う所で確かめる（`Expr::Call` の印と呼ばれる側の型，ラムダの引数と型の `ev`，直接の呼び出しの `suspends`）
- （補）検証器は，lowering のテストで作った全部のモジュール（`Roots::All` の core と std を含む．ループ化の前と後）と，emela-codegen-js のテストで手で組んだ IR にかける．driver は debug ビルドで lowering の直後にかけ，誤りがあれば panic する（コンパイラの誤り）．emela-codegen-js の出力の中ではかけない
- （補）検証器が見つけた lowering の誤り：release の関数の戻り値を `()` にしていた（本体の値をそのまま返すので本体の型にした）．init もフィールドもないハンドラの release で，`self` の型がハンドラの型のままだった（状態の型 `()` に揃えた）．型を付けるときに見つけたもの：能力の操作 fork を値として使ったときのラムダが，型は表を受けるのに表の引数を持っていなかった（使わない表の引数を足した）．同じエフェクトの別の具体化のハンドラを足す `generic_ev` は，初め `EffectDef` が何を具体化したかを持たなかったので，検証器が名前（`State[..]`）で突き合わせていた．今は `EffectDef::decl` と `args` で突き合わせる（下の「操作の型」）
- （補）操作の型（2026-10-15，ir-op-types）．`OpDef` は引数の型 `params`，戻り値の型 `ret`，失敗しうるか `fails`，自分の型引数の数 `type_params` を持つ．型は具体化したもの（`State[Int]` の `get` は `fn() -> Int`）で，宣言の型スキーム（エフェクトの型引数，操作の型引数の順）の前の段をエフェクトの型引数で置き換え，後の段を 0 から番号を振り直して作る．操作の型の中の集合は中断しうるとみなす（汎用の関数の宣言と同じ近似．フラグは検証器が比べない）
- （補）`EffectDef` は具体化した宣言の IR の名前 `decl` と型引数 `args` を持つ．鍵 `State[$modify.0]` の `args` は `Type::Param(0)` で，`modify` の型引数を指す．この鍵の `EffectDef` を使うのは `modify` の本体だけなので，局所変数の型の `Param` と同じ読み方になる．型引数を作るには鍵を作った定義（`tag_key` の `ctx`）から見た型が要るので，`EffectKey` はタグと `ctx` も持つ（等しさと hash は今までどおりエフェクトと文字列だけで見る）
- （補）検証器は，証拠の表に足すハンドラ（`Expr::Provide`）が鍵のエフェクトと同じ `decl` を持ち，鍵の `args` の `Param` をそれぞれ1つの型に置き換えるとハンドラの `args` になる（`instance_of`）ことを確かめる．`generic_ev` が呼ばれる側の鍵（`State[$modify.0]`）に呼ぶ側のハンドラ（`State[Int]`）を足すのはこの形
- （補）Scope の `fork[A]` と `deferred[A]`（8.1，組み込みのエフェクトだけが操作の型引数を持てる）は，操作の型引数として持ち，呼び出しごとの具体化を `Callee::Op::ty_args` に入れる（`s.fork[Int](..)`）．型検査が操作の参照の式に型引数を記録している（`ProgramTypes::type_args`）ので，直接の呼び出しと同じ手間で済む．`Type::Any` にしなかったのは，呼び出しの値の型（`Fiber[Int]`，`Deferred[Int]`）を IR だけで決めるため．Scope の型引数は集合で `()` になるので，操作の型の `Param` は操作の型引数だけを指す
- （補）ハンドラの実装は全部の具体化を1つで受けるので，検証器は実装を操作の型引数を `Any` に置き換えた `OpDef` と比べる．lowering が作る Scope のハンドラの fork と deferred の実装は，今までどおり `Any` を受けて返す
- （補）IR の fork は利用者の `f: fn() -> A use { Scope[E, R], R }` をそのまま受けず，表を普通の引数で受ける形に包んだもの（`fn(evidence) -> A`，`ForkCall`）を受ける．`OpDef` の fork の引数の型もこの形にする（`wrapped_child`）
- （補）検証器が操作の実装で確かめること：表を受けない（型の `ev` が偽），操作が失敗しないなら実装の型も失敗しない，引数の数，操作の引数が実装の引数へ流れ込む，実装の値が操作の値へ流れ込む．suspend fn の操作では，最後の引数が resume（`fn(ret) -> ()`）で，実装の値は `()`（8.4：本体の値は捨てる）．操作の呼び出しでは，型引数の数，引数，`fails` の印（操作が失敗しうるなら真）を確かめる．型を付けた後，lowering のテストの全部のモジュールと driver の debug ビルドで誤りは出なかった
- （補）`EnumDef::external` は `@external type`（12.3）の印．HIR では構成子のない `type`（`TypeItem::ctor` が `None`）．バックエンドはホストのハンドルと，値を持たない構成子のない enum をこれで見分ける．`Std.Fiber.Fiber` と `Std.Deferred.Deferred` も `@external type` なので印が付く．ダンプでは `external enum`
- （補）ターゲットの検査（12.1）は lowering の後で IR をたどって行う（`LowerInput::target`）．根は `Roots::Exported` なら main，`Roots::Tests` ならテスト関数．with に書いたハンドラの関数を呼ぶので，init と release から呼ぶ関数はたどれる側に入る．操作の本体（`Expr::Handler` のラムダ）は，その操作がたどれるコードのどこかで呼ばれる（`Callee::Op`．値として使う場合もラムダの中の呼び出しになる）まで保留し，呼ばれたらたどる（`unannotated_reachable`）．操作は `EffectDef::decl` と添字で同一視し，型引数の違うエフェクトの同じ操作は同じものとして扱う．たどれる外部関数にそのターゲットの `@external` がなければ `LowerError::MissingExternal`（driver が E0412 にする）．main のないエントリでは検査しない．`Roots::All` は検査しない．
- （補）たどれない外部関数でも，エントリの `pub` の fn から写されることはある．そのターゲットの指定がなければ，バックエンドは呼ばれたら defect にする関数を出す．
- （補）`Roots::All` は本体のある fn と const に加えて，`@external` の fn と handler も根にする（同梱の `SystemConsole` を写せることを `every_core_function_lowers` で見る）．
- （補）同梱の core のソースはディスク上のパスを持たないので，core の `@external(js, "./x.mjs", …)` はソースの隣でなく，同梱のホストのファイル（`lib/` の `.mjs` を `include_str!` で埋め込んだ `bundled_files()`，`LowerInput::bundled_files`）から引く．ターゲットは問わない．指定はソースを置いた `lib/` の中のディレクトリ（`lib_dir`）からの相対パスとみなし，`.` と `..` を畳んだ `lib/` からのパスで引く．`lib/` の外へ出る指定と表にない指定は `LowerError::MissingCoreFile`（同梱のソースの誤り．driver は内部の誤りの診断にする）．
- （補）引いた指定は `HostModule::Bundled`（`lib/` からのパス）にする．出力先の配置は IR に書かない．JS 出力は `emela_core/`（`emela_codegen_js::BUNDLED_DIR`）の下に置いたファイルを，エントリの出力からの相対パス（`./emela_core/x.mjs`）で import する．利用者のソースのファイルの指定（`HostModule::File`，絶対パス）とは扱いを分ける．
- （補）`lib/` の `.mjs` は出力先へそのまま書き出すので，コメントを英語で書く（`.emel` のコメントは出力に入らないので日本語のまま）
- （補）写した外部関数が同梱のファイルを1つでも指したら，同梱のホストのファイルを全部 `Module::bundled_files` に入れる（どのファイルがどれを import するかを JS を読まずに済ませるため，`lib/` と同じ並びで全部書き出す）．1つも指さなければ空で，何も書き出さない．
- （補）core の WASM の `@external` のファイルの指定は直さない（WASM は範囲外）．

## 中断と再開（8.4，8.7，14.5，16.5）

alpha.3 の第2段（2026-10-12）．前半で IR とバックエンドを作り，後半で lowering から印を付けた（`lower/suspend.rs`）．

- （補）中断しうる関数は `Function::suspends`，中断しうるラムダは `Expr::Lambda::suspends`，中断しうる呼び出しは `Expr::Call::suspends` の印で持つ．中断しない関数の本体の中の真の呼び出しは，その場で resume するハンドラ（8.7 の InstantClock）の with の中にだけ現れる（R が Immediate のまま）．バックエンドはその場で最後まで進める
- （補）`Callee::Direct` の呼び出しの印は，呼ばれる関数の印と同じでなければならない．`Callee::Indirect` は呼ばれる側の式の型で決め，真でも実際の関数の値は中断しないものでよい（逆は不可）．`List.map` のように中身の分からない R の関数を呼ぶ側も真にする．`Callee::Op` は suspend fn の操作なら真
- （補）エフェクトの操作は `OpDef`（名前と `suspend`．型は上の「外部関数」の節の「操作の型」）．suspend fn の操作のハンドラの実装（`Expr::Handler::ops` のラムダ）は，宣言の引数の後ろに resume を1つ多く受ける．resume は `fn(A) -> ()` の普通の関数の値で，それを呼ぶ式は中断も失敗もしない（`Callee::Indirect`，`fails` も `suspends` も偽）
- （補）`Expr::Handler::name` はハンドラの名前（ソースの名前）．2回目の resume の defect の文面に使う（14.5）
- （補）`@external suspend fn` は `Function::suspends` が真の外部関数（`External` に印は足さない）
- （補）ダンプでは中断しうる呼び出しの後ろに `!`（失敗の `?` の前），中断しうる関数とラムダと suspend fn の操作の前に `suspend`，ハンドラの値は `handler 名前: エフェクト { … }`

### lowering の印（`lower/suspend.rs`）

- （補）印は型の効果の集合 R が中断しうるか（Immediate でないか，9.5）で決める．R の要素のエフェクトが suspend fn を持つか Async なら中断しうる．関数は宣言の型の R，ラムダは式の型の R，関数の値の呼び出しは呼ばれる側の式の型の R，直接の呼び出しは呼ぶ関数の印．操作の呼び出しは型の R でなく操作の宣言で決め，suspend fn の操作ならいつも真，fn の操作ならいつも偽
- （補）集合の型引数（`List.map` の `R(f)`，汎化した集合）は，具体化しだいで中断しうるかが変わる．そういう関数は，中断しうる集合に具体化した呼び出し用に「中断用の写し」を別の IR の関数として作る（コンパイラの用語では特殊化．コードでは `Specialization`）．どの写しかは，効果の集合として使われ Immediate の制約のない型引数のうち，中断しうる集合に具体化したもののビットで表す．0（全部 Immediate）は元の関数そのもので，今までと同じ名前と出力．中断用の写しは `名前$suspendビット`（`List.map$suspend8`）．公開するのは元の関数だけ．中断する関数を一度も渡さないプログラムには写しができない
- （補）参照ごとにどれを呼ぶかは，型検査が記録した具体化した集合（`ProgramTypes::type_rows`）から決める．中断用の写しの本体の中では，集合の型引数（`RowElem::Bound`）と，推論で汎化した集合変数（`ProgramTypes::generalized_rows` で型引数に対応づける）が中断しうるかが決まっている．自分自身の参照は今写している関数と同じ．同じまとまりの中のまだ汎化していない定義の参照は具体化されないので，共有している集合変数から決める
- （補）中断用の写しの中で作ったラムダは，外へ出ても具体化した型の R が中断しうるので，呼ぶ側の印とずれない．写しを作らずに「中断しうるかもしれない関数は全部ジェネレータ」にすると，Immediate に具体化した呼び出しの側が中断しない形で呼んでしまう（逆も同じ）ため
- （補）引数の型にだけ現れる効果の集合の型引数もビットに数えるので，それ自身は中断しない写し（中でラムダを作るだけの関数など）もできる．出力は正しく，コードが大きくなるだけ．ラムダも関数の値も外へ出ない関数を写さないようにするのは，WASM の段の単相化と合わせて見直す
- （補）関数ごとに HIR の局所変数との対応を作り直す（元の関数と中断用の写しが局所変数を共有しないように）
- （補）証拠の表は，R に Async 以外の要素があるときだけ受け渡す．Async はランタイムが供給し（8.3，13.3），ハンドラの値を持たないので表に入れるものがない．R が Async だけの main とテスト関数は引数なしで呼べる
- （補）suspend fn の操作の実装は，引数（最後が resume）をそのままラムダの引数にする．本体の式の R が中断しうれば（`@external suspend fn` を待つなど）ラムダもジェネレータにする．resume を後から呼ぶだけの Async（8.7）はハンドラの依存に載るが本体の式の R には載らないので，ジェネレータにしない
- （補）fn の操作の実装のラムダは，型検査の E0417 と同じく，本体の式の R が Immediate か，写した本体が中断しうる呼び出し（`Expr::Call::suspends` が真）を無名関数の外に含まない（`has_suspending_call` が偽）ならジェネレータにしない．本体の R に suspend fn を持つ Clock があっても，`clock.now()` しか呼ばなければジェネレータにしない．R が Immediate で中に真の呼び出しがある（その場で resume するハンドラの with の中の suspend fn の操作）ときは，中断しない関数の本体と同じ扱いで，バックエンドがその場で最後まで進める（上の「中断と再開」）．E0417 を通った本体では，どちらかが必ず成り立つのでジェネレータにならない
- （補）suspend fn の操作の実装は，今までどおり本体の式の R で決める．中断しうる呼び出しがなくても R に suspend fn を持つエフェクトがあればジェネレータになるが，suspend fn の操作の呼び出しは中断しうる形で呼ぶので出力は正しい
- （補）`@external suspend fn` は，宣言の型の R に Async があるので印の付いた外部関数になる

## 並行性（11章）

alpha.5 の第1段（2026-10-13）．IR とバックエンドとランタイムを先に作り，手で組んだ IR で確かめた（emela-codegen-js の `tests/fiber.rs`）．lowering からの接続は，型検査と同梱の `std/fiber.emel` などがそろってからの別の段．

- （補）11章の操作は `Expr::Fiber { op: FiberOp, args }` の1つの形で持つ．操作ごとの引数の数（`FiberOp::arity`），中断しうるか（`suspends`），失敗しうるか（`fails`）は `FiberOp` が決める．呼び出しのように印を式に持たせないのは，ほとんどの操作で印が操作から決まるため．印が変わるのは `Scope { fails }`（本体の E）と `Attempt { suspends }`（`f` の R）だけ
- （補）`Fiber.scope(body)` の `body` は，スコープの能力の値を1つ受ける関数の値．lowering は利用者の本体を呼ぶラムダで包み，受けた能力の値を Scope の鍵で証拠の表に足して渡す想定（`Expr::Provide`）．Scope の能力の値と fiber のハンドルと Deferred と原因は，バックエンドが決める不透明な値．Scope の操作（fork，deferred）は `Callee::Op` でなく `FiberOp::Fork`，`FiberOp::Deferred` にして，能力の値を最初の引数に渡す
- （補）`s.fork(f)` の `f` は引数のない関数の値．子が使う証拠の表は lowering がラムダで閉じ込める（11.2 の「Fiber.scope を呼んだ時点のハンドラ，ただし Scope は子の属すスコープ」）．`f` の R は多相なので，中断用の写し（上の「lowering の印」）がそのまま使える．中断しない関数を渡せば，子の fiber として一息に走る
- （補）Exit と CauseKind は利用者の enum と同じ `EnumDef` で，lowering が同梱の定義から入れる．構成子の並びは `EXIT_SUCCESS`，`EXIT_FAILURE`，`CAUSE_FAIL`，`CAUSE_DIE`，`CAUSE_INTERRUPT` に固定する．`FiberOp::Await`，`Attempt`，`CauseKind` はその `EnumId` を持つ（バックエンドが形を確かめる）．`Deferred.poll` の値は Prelude の Option
- （補）中断しうる呼び出しを含むか（`lower/suspend.rs` の `has_suspending_call`）は，中断しうる `Expr::Fiber` も数える
- （補）ダンプでは `fiber#join(h)!` のように `fiber#` の後ろに操作の名前を書き，Exit などの enum を `[Exit]` と添える．中断しうれば `!`，失敗しうれば `?` を後ろに付ける

### lowering からの接続（`lower/fiber.rs`）

alpha.5 の第2段（2026-10-13）．型検査（`ProgramTypes`）から上の IR を作る．

- （補）第1段の想定（Scope の操作を `FiberOp::Fork`，`FiberOp::Deferred` にして能力の値を渡す）を改めた．Scope の能力の値は，`Fiber.scope` を呼んだところで lowering が作るハンドラの値（`Expr::Handler`．名前は `Scope`）にする．操作 fork と deferred の実装は，ランタイムのスコープ（`fiber#scope` が本体に渡す値）を捕まえて `fiber#fork` と `fiber#deferred` を呼ぶラムダ．利用者の `s.fork(f)` と `s.deferred()` は普通の操作の呼び出し（`Callee::Op`）になる．子の証拠の表を作るのに，`Fiber.scope` を呼んだ位置の表を能力の値に結びつけておく要があり，ハンドラの値ならラムダが捕まえるだけで済むため
- （補）子の証拠の表は「`Fiber.scope` を呼んだ位置の表 + Scope の鍵に fork した能力の値」（11.2）．fork の実装は，呼ぶ側が包んだ関数 `g` を，`Fiber.scope` を呼んだ位置の表で呼ぶ．呼ぶ側は `fn(ev) { f(ev + { Scope: 能力の値 }) }` に包む．`f` の R が表を要らなければ表を渡さない（`f` を受けるかは呼ぶ位置の `f` の型で決まり，スコープの R では決まらないため．スコープの R は本体のエフェクトも含む）
- （補）スコープの本体の中の with は子に届かない．with の中で作ったクロージャを fork すれば，そのクロージャが自分で with のハンドラを表に足すので内側のハンドラを使う（11.2 の例外のとおり）
- （補）fork の実装の中の子の関数は，常に中断しうるラムダにして `g` を中断しうる呼び出しで呼ぶ（`g` が中断しない関数でもバックエンドがそのまま進める）．子の関数の失敗の印はスコープの E で決める
- （補）Scope の鍵は型引数が集合なので，いつも `Std.Fiber.Scope[_, _]`（`key_ty` が集合を `_` にする）．入れ子のスコープでは `use Scope` が最も内側のハンドラを引き，外側で取り出した能力の値は外側のスコープへ fork する
- （補）`Fiber.attempt(f)` は，`f` の R が表を要るなら今の表で `f` を呼ぶ引数のない関数に包む（ランタイムは `f` を引数なしで呼ぶ）．Exit と CauseKind の `EnumId` は具体化した型の戻り値の型から引く
- （補）11.2 の「子は兄弟を fork できる」は，型検査（#196 の修正）が fork に渡す関数の型の R に `Scope[E, R]` を載せる．lowering は子の表の Scope を fork した能力の値（子の属すスコープ）にするので，入れ子のスコープでも内側の子の fork は内側のスコープに入る．最初は型検査が E0402 にしていたのを進行役に報告して直してもらった

## 単相化（`mono.rs`）

2026-10-15，mono．WASM（後に C）の経路だけで使う Core IR → Core IR のパス．JS は今の多相な出力のままで，このパスを通らない．

### 集合の要約

- （補）集合の型引数の具体化は，呼び方を決める 3 つの印に縮めて `Type::Set(SetSummary)` で持つ．失敗しうるか（E として），中断しうるか（R として），証拠の表が要るか（R として）と，集合に入っている集合の型引数（`Type::Param`）の並び．E か R かは集合を見ても決まらないので 3 つとも求め，使う位置で要る印だけを読む．エラーのタグは失敗だけ，エフェクトのタグは表と中断だけを立てる．決まらずに残った集合変数は失敗と表を立てる（今までの `row_needs_ev` と同じ）
- （補）要約を置く所は，直接の呼び出しと関数の値の `ty_args`（`type_rows` から），enum の型引数（`Handler[E, R]` の集合の位置），集合の型引数を含む関数型の E と R（`FnType::sets`），関数自身の宣言の E と R（`Function::sets`）．エフェクトの型引数（証拠の表の鍵）は今までどおり `()` で，関数型は印だけを残す（`TypeTable::erase_sets`．鍵が E と R を見ないのに合わせる）
- （補）`FnType::sets` がある関数型の印は，今までどおり汎用の近似（集合の型引数は失敗と表を立て，中断は lowering している写しで決める）．型の置き換え（`subst_type`）で E と R の集合の型引数がすべて要約に置き換わると，印は要約から正確に決まり，`sets` は消える．一部が残るときは近似の印のまま（分かった分だけ立てる）．多相な IR の検証器と JS はこの近似の印を見るので，出力は変わらない
- （補）`Typer::fn_value` の印は関数自身の印（汎用の関数では近似）のまま．多相な IR では関数の値の実際の引数の数を表すため
- （補）中断用の写し（`$suspend<bits>`）とは重なるが，そのまま残す．写しを選ぶビットと要約の中断の印は同じ集合から求めるので食い違わない．単相化の鍵には要約が入るので，写しごとに別の具体化になる

### パス

- （補）`monomorphize(&Module) -> Module` は公開された型引数のない関数から，`monomorphize_from(&Module, roots)` は与えた根から始める．型引数のある根は飛ばす（具体化が 1 つに決まらない）．作業リストで，鍵（関数と，呼ぶ側の型引数を置き換え済みの `ty_args`）ごとに 1 回だけ写す．`TypeId` は登録済みなので鍵の比較は安い．多相再帰がない（9.3）ので止まる
- （補）各式で 2 つの段を分けている．(1) 型の具体化（`Subst`）：局所変数，戻り値，`ty_args`，ラムダとリストの型，能力の型と証拠の表の鍵のエフェクト．(2) 呼び先の書き換え（`Mono::target`）：直接の呼び出しと関数の値を具体化の写しに替える．0.21 の Trait は辞書渡しをせず，型を置き換えた後に (2) で `(trait, 型の頭) → impl の fn` の表を引いて直接の呼び出しに替える想定で，ここが拡張の場所．鍵と作業リストと型の置き換えは Core の木の形に依らないように書いた（後で Flow IR にかけたり，JS も単相化したりするときのため）
- （補）enum は写さない．定義は汎用のまま，使う所が具体的な型引数を持つ（`Type::Enum(id, args)`）．低い IR の `repr_of` は外側の構成子だけで表現を決め，フィールドは `repr_of_subst` で型引数を置き換えて求めるので，定義を写す必要がない．フィールドの型の `Param` は enum の型引数を指すので，単相化の後も残る
- （補）エフェクトは作り直す．出力には使ったエフェクトの具体化だけを，宣言の名前と（E と R を除いた）型引数ごとに 1 つずつ入れる．型引数に `Param` を含むエフェクト（`State[$modify.0]`）はその型引数の持ち主の関数の具体化で置き換え，同じ鍵の既存のもの（`State[Int]`）と同じエフェクトになる．汎用の関数を呼ぶときに足す鍵（`generic_ev`）は呼ばれる側のものなので，呼び出しの型引数で置き換える．置き換えた結果，足す鍵が足す値（同じ鍵で引いた値）と同じになったら足さない
- （補）`Type::Any`（Scope のハンドラの中）と，組み込みのエフェクトの操作自身の型引数（`fork[A]`）は残す
- （補）名前は `List.map[Int, String, {}, {}]` のように，元の名前に型引数を dump の書き方で付ける．同じ名前になったら `#2` を足す．公開するのは型引数のない根だけ

### 呼び方を正確にする

- （補）写しの中では集合の型引数が分かるので，関数型の `ev` は値が表を取るときにだけ真になる．写しの宣言の R が表を要らなければ表の引数を落とし，呼ぶ側も引数を落とす．ラムダも同じ（型の `ev` が偽になったら最後の引数を落とす）．関数の値の呼び出しは，呼ばれる側の型の `ev` が偽になったら表を渡さない．落とした引数を使う所は `()` になる
- （補）`fails` は写しの宣言の E から決め，直接の呼び出しの印も写しに合わせる
- （補）型検査は E だけでなく R も小さい関数を大きい関数として使わせる（ラムダの E と R は期待する型の部分集合，関数の値の参照は `open_fails` で集合変数を足す）．そのため表を取らない関数の値が，表を取る関数型の所へ流れ込む．JS では余分な引数が無視されて動くが，WASM の `call_indirect` は引数の数が違うと止まる．単相化はこの所に小さな包み（表を受けて捨てるラムダ）を挟む（`Mono::coerce`）．値が流れ込む所（`let`，`with`，直接・関数の値・操作の呼び出しの引数，構成子のフィールド，リストの要素，戻り値，ラムダの本体，`loop` / `recur`）で，値の型と所の型を比べる．分岐と `let` の本体は枝ごとに見る．引数と戻り値が関数で食い違うときも同じように包む（引数は向きが逆）
- （補）表がない所で渡す `()` は，表を取る所へ流れ込むなら空の表（`provide(;)`）にする．WASM では `()` は値を持たず，表は参照なので
- （補）enum の型引数やリストの要素の型の中で関数の印が食い違うもの（`List[fn() -> Int]` を `List[fn() -> Int ev]` として使う）は包めない．型検査は名前付きの型の型引数を一致で比べ，リストの要素の関数は要素ごとに包むので，今のところ起きない．起きたら検証器が見つける
- （補）lowering が手で作る関数型（fiber の包み，State.run の操作，ハンドラの操作の実装）は `sets` を持たず，印は汎用の近似のまま．どれも `ev` が偽で引数の数は変わらないので，呼び方には響かない

### 検証

- （補）`verify_monomorphic` は `verify` に次を足す．関数が型引数も `Function::sets` も持たない．局所変数，戻り値，`ty_args`，ラムダ，リスト，埋め込む値，assert の型と，使うエフェクトの型引数に，`Param` も集合の型引数もない（`has_param`）．関数の値は `ev` の一致する関数型の所へだけ流れ込む．`()` を表の所へ流さない．失敗しうる関数の直接の呼び出しは `fails` の印を持つ
- （補）テストは lowering と単相化の dump のスナップショット（`tests/mono.rs`）と，同梱の core と std の全関数を根にした単相化（`Roots::All`）を `verify_monomorphic` にかけるもの．計測は `EMELA_MONO_BENCH` にファイルを渡して `measure` を手で走らせる（時間を確かめるテストにはしない）
- （補）計測（2026-10-15，release，関数 4000 個の生成したプログラム）．main から届く分（`Roots::Exported`）は関数 1005 個（汎用 4）→ 1005 個，単相化 6 ms．全部を根にすると（`Roots::All`）4139 個（汎用 35）→ 4153 個，単相化 23 ms，`verify_monomorphic` 8 ms．型検査と lowering と検証は 1.3〜1.5 秒．core と std だけなら 139 個（汎用 35）→ 149 個
