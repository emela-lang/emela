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
- （補）`String.trim` が取り除く空白は Unicode の White_Space 特性の 25 文字（U+0009〜U+000D，U+0020，U+0085，U+00A0，U+1680，U+2000〜U+200A，U+2028，U+2029，U+202F，U+205F，U+3000）．U+FEFF と U+200B は空白でない．表を固定して，ホストの Unicode の版に依存しないようにする．

## Option

- （補）Prelude の `Option` は `Module::option_enum` でモジュールに1つだけ入れる `EnumDef`（`Some(A)` が添字 0，`None` が添字 1．5.4 の宣言の順）．組み込み関数の返す Option と利用者の書く `Some` / `None` が同じ定義を指す．`List.get` も core を Emela で書くときにこの定義を返す．
- （補）enum は型引数の数（`EnumDef::params`）と，フィールドの型（`VariantDef::tys`，`Type::Param(i)` で型引数を指す）を持つ．表示関数を型から作るのに使う．
- （補）型引数を取る組み込み関数（`dbg`）は，具体的な型を `Expr::Builtin` の `ty_args` で受ける．
