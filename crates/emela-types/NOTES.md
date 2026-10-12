# emela-types の補足

仕様にない判断を「（補）」として残す．

- （補）名前付き型の表 `TyCons`（`TyConId = la_arena::Idx<TyConData>`）は当面 emela-types が持つ．名前解決（emela-resolve）ができたら持ち主を移すか ID を写す．
- （補）`Int` などの基本型，`List`，タプル，関数は `TyCons` に登録せず `Ty` の構造で持つ．`List` は prelude の名前付き型にもできるが，表示と単一化を単純にするため組み込みにした．
- （補）単一化では `Never` は `Never` としか一致しない．`Never` を他の型として受け入れるのは `join`（分岐の合流）と `coerce`（期待した型の位置に置くとき）の最上位だけ．`List[Never]` と `List[Int]` は一致しない．
- （補）`join(a, b)` で片方が未束縛の型変数，もう片方が `Never` のとき，型変数を `Never` に束縛せず型変数のまま返す．
- （補）`join` が失敗したときは，1つ目の分岐を期待した型，2つ目を実際の型として報告する．`join_or_report` は1つ目の型を全体の型として推論を続ける．
- （補）単一化が途中で失敗したら，ena のスナップショットで途中の束縛を戻す．エラーの型は戻した状態で辿った形にする．
- （補）`TypeError` の `expected` / `actual` は単一化に渡した最上位の型，`kind` の `Mismatch` は実際に食い違った部分の型を持つ．
- （補）型エラーはまだ位置（span）を持たない．構文木とつなぐときに呼び出し側で付ける．
- （補）汎化で付ける型引数の名前は現れた順に `A`，`B`，…，`Z`，その後は `T26`，`T27`，…．汎化した型引数に制約は付けない．
- （補）スキームの表示は `∀ A: Ord + Show, B. T`．量化しないスキームは型だけを出す．スキームの外にある `Ty::Bound(i)` は `'i` と出す．
- （補）`generalize` は環境の型変数を呼び出し側から受け取る（`env_vars`）．トップレベルの関数だけ汎化する方針なので，レベル付きの汎化は入れていない．
- （補）網羅性の検査（`exhaustiveness.rs`）は `Ty` に依存しない．入力は検査専用の `Pat` と，型から構成子の形（`TyShape`）を引く `CtorTable` で，構成子は `TyShape::Enum` の並びの添字で指す．`Bool` と `()` も呼び出し側が `TyShape` に写す．
- （補）変数パターンは `Pat::Wild` として渡す．定数パターンは値に置き換えて `Pat::Lit` として渡すので，witness に出るときは定数名でなく値になる．
- （補）`Float` のリテラルは値で比べる．`0.0` の後の `-0.0` は冗長．NaN の定数どうしは同じ値とみなす（NaN の腕が常に冗長と報告されないように）．
- （補）名前付きフィールドは（宣言順の位置，パターン）の組で渡し，書いていないフィールドは `_` とみなす．位置で書いた構成子パターンも同じ形にする（`Pat::variant`）．
- （補）witness は，列に構成子パターンが1つもなければ構成子を並べず `_` とだけ出す．構成子パターンがある列では足りない構成子を全部並べる．
- （補）リテラルしか書いていない列で足りない値は `_`（現れたリテラル以外の値）と出す．
- （補）witness は `WITNESS_LIMIT`（64個）で打ち切る．
- （補）構成子のない enum は腕がなくても網羅しているとみなす．
- （補）リストのパターンは前置部と残り（`[x, ..rest]`）だけを扱う．`[..rest, x]` のような後置部は仕様にないので扱わない．
- （補）ガード付きの腕も，上のガードなしの腕で覆われていれば冗長と報告する．
- （補）検査は `MatchCheck`（足りない値の例と冗長な腕の添字）を返すだけで，`TypeError` にはしない．位置（span）を付けてエラーにするのは型推論側でつなぐときに行う．
- （補）`..` を書かずにフィールドを省いた構成子パターンは仕様（6.5，17.6）ではエラーになる．網羅性の検査は省いたフィールドを `_` とみなすだけなので，そのエラーは型検査側で出す．
- （補）witness の `[_, ..]` は文法（17.6）では書けない形（残りに名前が要る）だが，足りない値の説明としてそのまま出す．
- （補）escape（7.3）の腕は一部だけ受けてよいので，診断には `MatchCheck` の `redundant` だけを使う．`missing` は，E からタグを消すかどうか（腕がそのエラーの値をすべて受けるか）の判定にだけ使う．
- （補）opaque な型（4.4）は定義モジュールの外では構成子パターンを書けないので，列に構成子がなく witness は `_` になり，隠れた構成子は出ない．呼び出し側は `TyShape` をそのまま渡してよい．
- （補）17.6，6.5 改訂済み（`emela-0.20-spec.md`）: リストのパターンで名前なしの `[x, ..]` を許す．`[..]` と `[..rest]` は書けない．網羅性の検査は名前の有無を区別しないので変更は要らない．
- （補）Bool は Prelude では enum（`True` / `False`）だが，型推論では `Prim::Bool` で持つ．Bool の enum の `DefId` は `TyCons` に登録せず `Ty::BOOL` に写し，`True` / `False` の構成子の型も `Bool` にする．網羅性の検査には `TyShape::Enum`（`False`，`True` の順．prelude.emel の宣言順）で渡す．仕様が enum とするのは利用者から見える意味で，表現は実装依存（16.4）．
- （補）読めなかった型（`TypeKind::Missing`）と解決できなかった名前（`Res::Err`）は `Ty::Error` にする．単一化ではどの型とも黙って一致し，型変数は `Ty::Error` に束縛される．エラーが1件でもあれば lowering は呼ばれないので，lowering は `Ty::Error` を見ない．表示は `{unknown}`．
- （補）`error` の宣言は `TyConKind::Error` として `TyCons` に登録する（7.1 で同じ名前の型を定義するため）．E（fails の集合）はエラーを `DefId` で持ち，型の表に `DefId → TyConId` の対応を持つ．
- （補）型引数の数の検査（`List[Int, Int]`，型引数のない `Option`）は名前解決ではなく型推論で行う（E0304）．組み込みの型の型引数の数を知っているのが型推論のため．
- （補）Prelude の `panic`，`todo`，`dbg` は同梱の `prelude.emel` の `@intrinsic` の fn なので，宣言の型は他の fn と同じく `Signatures` が作る．特別扱いしない．
- （補）呼び出しは，引数を仮引数の位置に並べ直してから型を照合する．位置引数は前から，名前付き引数はラベル（仮引数名，名前付きフィールド名）で置く．ラベルを引けるのは，呼ばれる側が定義（fn，構成子）を名前で指しているときだけ．局所変数やラムダの値を名前付き引数で呼ぶと E0306．
- （補）呼び出しのエラー: 引数の数が合わない E0305，そのラベルの引数がない E0306，同じ引数を2回渡した E0307．ラベルの誤りを出した呼び出しには，引数の数の誤りを重ねない．
- （補）呼ばれる側の型がまだ関数型と分からない（型変数）か関数でない型なら，引数の数だけ型変数を並べた関数型と単一化する．関数でなければ型が合わないエラーになる．
- （補）戻り値の型を書いていない fn と，型を書いていない const の参照は，呼び出しの順の推論（ステップ4）が入るまで「まだ扱えない」の診断にする．
- （補）パターンの照合は「来た値の型」を期待する側，パターンの型を実際の側に置く（`expected` が値の型）．タプルとリストのパターンは，要素の型変数を並べた型と来た値の型を単一化してから要素に渡す．
- （補）構成子のパターンのフィールドは，呼び出しの引数と同じく位置とラベルで並べ直す．名前付きフィールドの構成子を位置で書いてもよい（6.2）．名前付きフィールドの構成子で `..` なしにフィールドを省けば E0308（省いたフィールドを全部持つ），フィールドより多い，または位置だけの構成子で `..` なしに足りなければ E0309．ラベルの誤りは E0306，E0307．置けなかったパターンには `Ty::Error` を渡す．
- （補）束縛のパターンが必ず成功するかどうか（6.3）は，網羅性の検査をつなぐステップ5で見る．それまではリテラルや構成子のパターンも束縛に書ける．

- （補）ラムダとリストのリテラルも，期待する型があれば中へ渡す．ラムダは期待する型が同じ数の引数の関数型なら，その引数の型を（型を書いていない）ラムダの引数に先に付けてから本体を推論する．`List.map(users, fn(u) { u.name })` の `u` が `User` に決まってからフィールドを引けるようにするため．
- （補）フィールドの参照 `x.name` は，`x` の型がその時点で type か error の名前付き型に決まっている必要がある．型変数のままなら E0314（型注釈が要る）．フィールドの名前から型を逆算することはしない（行多相を持たない）．
- （補）フィールドの型は，その型の構成子の宣言の型（ラベルと引数の型）から引き，型引数を代入する．enum とフィールドのない `@external` の type はフィールドを持たない（E0313）．opaque な型のフィールドは定義モジュールの外では引けない（E0315，4.4）．
- （補）部分更新 `C(f: e, ..base)` は構成子の型を2回具体化する．1つ目が作る値，2つ目が `base` の型．書いたフィールドは1つ目の型で検査し，書いていないフィールドは2つの型のそのフィールドの型を単一化する（引き継ぐ）．型引数は変わってよい（5.3）．ないフィールドは E0312，2回書いたフィールドは E0307．
- （補）補間の中の式は推論するだけで，Show を満たすかは Trait の段で見る．`assert e` の `e` は Bool で，全体は `()`．
- （補）プログラム全体の検査（`check_program`）は，本体のある fn と const を，本体の中で参照する定義への辺で結び，強連結成分（互いに参照し合うまとまり）ごとに，参照される側から順に推論する（Tarjan）．まとまりごとに `InferCtx` を1つ使う．
- （補）宣言の型のない定義（戻り値の型を書いていない fn，型を書いていない const）は，まとまりの中では単相の型で参照し，まとまりを推論し終えてから汎化して宣言の型の表に足す．書いた型引数（`fn f[A](...)`）は汎化した型引数の前に並べ，汎化した型引数には書いたものと重ならない名前を付ける．宣言の型のない定義どうしの再帰は多相にならない（同じまとまりの中で別々の型に使うと型が合わない）．
- （補）宣言の型のない定義に書いた型引数があるとき，同じまとまりの他の定義からの参照は，型引数を新しい型変数に置き換え，まだ決まっていない部分も新しい型変数にして，まとまりを推論し終えてから定義の最終的な型と照合する．その定義の型引数が他の定義へ漏れないようにするため．
- （補）const も汎化する（`const EMPTY = []` は `∀ A. List[A]`）．ただし alpha.1 では const の値は純粋な式に限る（5.6）ので，汎化して困る場面はない．
- （補）トップレベルの fn の引数に型を書いていなければ E0316（6.1）．構文と名前解決では検査していない．型は `Ty::Error` にして続ける．
- （補）同梱の Prelude と core のソースは，型の診断なしに検査を通る（テスト `core_sources_typecheck`）．
- （補）網羅性の検査は，定義のまとまりの推論が終わって型が決まった後に行う（`coverage.rs`）．match は E0303（足りない値の例を全部）と W0301（冗長な腕ごと），束縛は E0317（6.3．直し方が match への書き換えなので E0303 と分ける）．対象の型が `Ty::Error` なら確かめない．
- （補）パターンは対象の型に沿って検査用の形に変換する．型に合わない部分（型検査が誤りを出している）はワイルドカードにして，検査が崩れないようにする．
- （補）const のパターンは，値がリテラル（か負の数値リテラル）ならその値として検査する．それ以外の式はコンパイル時の評価がまだないので，その const だけに合う値として扱う（網羅したとは見なさず，同じ const を2回書いたときだけ冗長）．
- （補）足りない値の例では，名前付きフィールドの構成子も位置で出す（`Circle(_)`）．
- （補）式の種類は全部扱っている（`ExprKind` の `match` に `_` の腕はない）．`use` と `with` は効果の段で入れた（下の「効果の集合 R」）．
- （補）仕様の例（2.4，5章，6章，9章）は `tests/spec_examples.rs` で型検査にかける．仕様の例は断片が多いので，足りない定義を補って完全なプログラムにしてある（補った行に「補」と書く）．

## エラーの集合 E（7章，9.2〜9.4）

表現と解き方は `row.rs` と `body/errors.rs` の冒頭に書いた．ここには仕様にない判断を残す．

- （補）集合（`Row`）はタグ（error の `DefId`），集合の型引数（`RowElem::Bound`），1つのエラーの型（`RowElem::Of`），推論の集合変数（`RowElem::Var`）の和で表す．効果の集合 R も同じ `Row` で持つ（タグは effect の `DefId`）．関数型は `FnTy::fails` に E，`FnTy::uses` に R を持つ．
- （補）関数型の E は単一化で等しくせず，包含の制約 `sub ⊆ sup` にして溜める．期待する型の位置に置いた関数の E は，期待する型の E に含まれればよい（E についてだけの部分型）．向きは，関数型の引数の中で逆になり，名前付き型の型引数の中は両方向（型引数がどの位置に使われるかを見ないため），リストとタプルの要素は外側と同じ（値は不変なので）．
- （補）制約は定義のまとまりを推論し終えてから解く．集合変数ごとに下界を集め，最小の解を求めてから全部の制約を確かめる．`sub ⊆ sup` の `sup` に集合変数があれば，`sub` の要素のうち `sup` にないものを `sup` の最初の集合変数の下界にする．`sup` に集合変数が2つ以上あるとき（`fails E1 | E2` の引数に渡す）に最初のものを選ぶのは近似で，後の上界の検査で誤りになることがある．
- （補）下界には `sup` のタグを除いて渡す．`fails Timeout | E` の引数に関数を渡すと，`E` には渡した関数のエラーから Timeout を除いたものが入る（7.4）．
- （補）関数の値の参照（定義と局所変数）の型は，E に新しい集合変数を足す．E の小さい関数を，E の大きい関数として使ってよいので．分岐の合流（`if c { f } else { g }`）とリストの要素（`[f, g]`）で E の違う関数を並べられる．分岐の合流で両方が関数型なら，E を新しい集合変数にして両方の E を含める．
- （補）関数型の `fails` を省略したときの意味（9.2）: トップレベルの fn の引数の型の中は見えない集合の型引数（`ParamKind::Set`），戻り値の型・束縛の型注釈・ラムダの引数の型注釈は新しい集合変数（本体から推論），宣言の中（type・enum・error のフィールド，const の型注釈）は空．ラムダの引数は「引数の型の中」だが，ラムダは汎化しないので推論の集合変数にした．
- （補）見えない集合の型引数は，書いた型引数の後ろに並べる．名前は引数の名前から `E(f)`，同じ引数に2つ目からは `E(f, 2)`．外側の関数型から振る（`f: fn(fn() -> A) -> B` の f 自身の E が `E(f)`）．書けない名前なので書いた型引数と重ならない．
- （補）戻り値の型を書いた fn の宣言の E は，書いた `fails` 節（なければ空）と，引数の型の中の正の位置（本体が呼べる関数．引数の関数型の E と，その戻り値の関数型の E）の見えない集合の和にする．見えない集合は書けないので，引数の関数を呼んだときのエラーはいつも伝えてよい（`List.map` が f のエラーを引き継ぐ，9.2）．負の位置（引数の関数が受け取る関数）の見えない集合は含めない．本体から見ると中身の分からない集合で，そこに失敗する関数を渡すと E0321．
- （補）戻り値の型の中に `fails` を省略した関数型がある fn（`compose`）は，宣言の型を本体から推論する（戻り値の型を書いていない fn と同じく，まとまりの中では単相）．この fn 自身の E も推論し，宣言の E（上の和）に含まれることを確かめる．引数の関数を呼ぶ関数を返すだけなら，この fn 自身は失敗しない．
- （補）型引数の種類は使われた位置から決める．`fails` にだけ書いた型引数は集合，値の型に書いた型引数はふつうの型で，`fails` にも書いていれば1つのエラーの型（`RowElem::Of(Ty::Bound(i))`，7.2）．`use` にだけ書いた型引数も集合（`ParamKind::Set`）で，エラーの集合か効果の集合かは書いた位置で決まる．
- （補）type・enum の宣言の型引数を `fails` と `use` に書く形（9.2 の `type Handler[E, R]`）は，下の「宣言の型引数の集合」に書いた．
- （補）`fail e` の E は `e` の型のタグ．`e` の型が型引数か推論の型変数なら1つのエラーの型として E に入れ，汎化で値と `fails` の両方に出る型引数になる（`fn raise[E](err: E) { fail err }` は `∀ E, A. fn(E) -> A fails E`）．`Never` と壊れた型は何も足さない．それ以外の型は E0319．呼ぶ側で1つのエラーの型の型引数をエラーでない型に具体化したときも，呼び出しの位置に E0319．
- （補）escape（7.3，18.1 #20）は，元の式の E から，腕がそのエラーの値をすべて受けるタグだけを消す．エラーごとに腕を集め，W0301 と同じ網羅性の検査（`Shapes::check_arms`）で足りない値がなければ消す．ガード付きの腕は網羅に数えない．`Flag(on: True)` と `Flag(on: False)` のように，ガードのない腕を合わせて網羅していれば，1本の `Flag(_)` でなくても消す．一部の値しか受けないエラーは E に残るので，`-> T` の関数なら E0318．エラーは型引数を持たず形が宣言だけで決まるので，この判定は E を求めるときに行う（`body/errors.rs` の `escaped_tags`）．W0302（起きないエラーの腕）と W0301（冗長な腕）は今のまま，一部しか受けない腕でも確かめる．推論の集合変数（同じまとまりの中で決まる E や，関数の値の参照に足した集合変数）は，消すタグを除いた解を持つ新しい集合変数に置き換えるので，解が決まった後でも正確に消える．消さないのは，集合の型引数（`fails E` の `E`，見えない集合）と1つのエラーの型の型引数と，汎化する集合変数．7.3 の「中身の分からない集合変数」をこの3つと読んだ．
- （補）escape の腕のパターンがエラーの構成子でない（`_`，変数など）ときは E0320．名前解決が誤りを出すエラーでない構成子は型検査では重ねない．誤りのある escape は，元の式のエラーを全部受けたとみなす（E0318 を重ねない）．
- （補）escape の全体の型は，元の式の型と各腕の型を順に `join` したもの．腕は元の式の型か Never．元の式が Never なら腕の型になる．
- （補）escape の腕が受けるエラーが元の式から起きないときは W0302．元の式の E にタグしかない（中身の分からない集合がない）ときだけ確かめる．同じエラーの腕が上の腕で覆われていれば W0301（エラーごとに網羅性の検査の冗長な腕を使う）．
- （補）汎化する定義（宣言の型のない定義）の型で，負の位置（引数の関数型の E）にあって下界も上界もない集合変数は，空にせず集合の型引数として汎化する（`fn caller() { fn(g) { g() } }` は `∀ A, E. fn() -> fn(fn() -> A fails E) -> A fails E`）．名前は `E`，`E1`，… で，型の型引数の後ろに並べる．それ以外の集合変数は最小の解にする．`fn(g) { if c { g } else { f } }` のように負の位置の集合変数に下界があると，最も一般な型にはならない（近似）．
- （補）const の値の E は空でなければならない（5.6）．空でなければ E0318（`declared` は偽）．
- （補）E0318 は，宣言の E に含まれないエラーごとに，それを起こした最初の式（E にそのエラーを含む部分式を書かれた順に辿って，それ以上辿れない式．`fail` か呼び出し）に出す．同じ式から起きた複数のエラーは1つにまとめる．
- （補）main とテスト関数の E は特別扱いしない．戻り値の型を書かなければ E は推論で，空でなくてよい（13.3，14.3）．R は空でなければならない（下の「効果の集合 R」）．
- （補）型の表示では空の E を省く．ただし関数型の引数の型の中では，省くと「何でもよい」と読めるので `fails {}` と出す．戻り値が関数型で外側に `fails` 節があるときは戻り値を括弧で囲む（5.5）．型の食い違いの診断（E0301）では，まだ解いていない集合変数を出さない．
- （補）`ProgramTypes::errors` に各式の E を持つ（集合変数は解に置き換えたもの）．lowering はまだ使わない．

## 効果の集合 R（8章，9.2〜9.5，12.2）

表現と解き方は `body/effects.rs` と `body/capabilities.rs` の冒頭に書いた．R の制約は E と同じ表（`InferCtx::subsets`，種類は `SetKind`）に溜めて `body/errors.rs` で一緒に解く．ここには仕様にない判断を残す．

- （補）能力の値の型は `Ty::Cap(effect)`．ソースには書けない（E0221）．単一化では同じエフェクトの `Cap` どうしだけが一致する．表示はそれを作る式の形で `use Logger`（ホバーで `log: use Logger`）．
- （補）能力の値の操作 `log.info` の型は，エフェクトの操作の宣言の型に，R としてそのエフェクトを足した関数型．呼べば R にそのエフェクトが載る．能力を捕まえたクロージャの型の R にもそのエフェクトが載るので，8.2 の `copy(read: fn() -> Rows use { Db }, ...)` に渡せる．操作の `fails` はその呼び出しの E に足す（8.1）．名前付き引数は操作の宣言の引数の名前で引ける．ない操作は E0401．
- （補）エフェクトの操作の宣言は「宣言の中」なので，引数の関数型の `fails` と `use` を省略すれば空（9.2）．戻り値の型を省略すれば `()`．操作の `use` 節は構文の段の E0137（8.1）なので，型推論では読まない．
- （補）`use` 節を書いていない本体のある fn は，戻り値の型を書いていても宣言の型のない定義として扱う（9.3 の「常に R を推論する」）．まとまりの中では単相の型で参照し，推論し終えてから汎化する．T と E は宣言のとおりで，R だけが集合変数．そのため同じまとまりの中では多相再帰にならない（書いた型引数のある fn を，まとまりの中の他の定義から参照するときは，既存の `deferred` の照合を使う）．
- （補）`use` 節を書いた fn の宣言の R は，書いた集合と，引数の型の中の正の位置の見えない効果の集合の和（E の 7.4 の規則にそろえた）．本体の R がそれに含まれなければ E0402．
- （補）本体のない fn（`@external`，`@intrinsic`）で `use` を省略すると，R は空（と正の位置の見えない効果の集合）．`@external` のホストの副作用は R に載せず，呼べる場所の制限（12.2）で扱う．`@external suspend fn` の R には Async を足す（下の「suspend，Async，Immediate」）．
- （補）引数の関数型で `use` を省略したときの見えない効果の集合の名前は `R(f)`，`R(f, 2)`．E の `E(f)` と別に数える．汎化した効果の集合の型引数の名前は `R`，`R1`，…．
- （補）効果の集合にエフェクトと集合の型引数を混ぜて書く（`use { Quiet, R }`，8.3，17.4）のは，E の `fails Timeout | E` と同じ `Row`（タグと `RowElem::Bound` の和）で持つ．with はタグの Quiet を消し，`R` は残す．引数の関数型に書けば，渡した関数の R から Quiet を除いたものが `R` の下界になる（E の 7.4 と同じ規則をそのまま使う）．`use { Quiet, R1, R2 }` のように集合の型引数が2つ以上ある引数に渡すときの近似も E と同じ．
- （補）効果の集合の表示は書いた順でなく，タグを先，集合の型引数を後に並べる（`use { R, Quiet }` は `use { Quiet, R }` と出す）．要素の順に意味はない．
- （補）`with H1, ..., Hn { e }` の R は，`e` の R から供給したエフェクトを消し，各ハンドラの依存を足したもの．ハンドラの依存のうち，同じ with の別のエフェクトを供給するハンドラが満たすものは足さない（14.2 の「同じ with で先に初期化されたハンドラ」）．自分と同じエフェクトへの依存は外側のハンドラが満たす．同じ with のハンドラが互いの効果に依存して循環すれば E0411．
- （補）with で消すのはタグだけ．推論の集合変数は，消すエフェクトを除いた解を持つ新しい集合変数に置き換える（escape と同じ）．集合の型引数（見えない `R(f)` など）からは消さない．`fn run(f: fn() -> ()) { with Quiet { f() } }` の R は `R(f)` のまま（安全側）．
- （補）handler の依存（9.4 の「依存(H)」）は，操作の本体の R の和．handler もまとまりの中の定義（グラフの頂点）で，with に書いた handler へ辺を張る．まとまりの中では集合変数で，推論し終えてから `ProgramTypes::handler_uses` に入れる．
- （補）handler の検査: 1つのエフェクトの全部の操作を実装する（足りなければ E0404），同じ操作を2回実装しない（E0405），引数の数はエフェクトの操作の宣言と同じ（E0406）．エフェクトにない操作は名前解決が E0212 を出す．操作の本体の E は操作の宣言の `fails` に含まれる（E0318．文面は fn と同じ）．
- （補）状態を持つ handler（フィールド，init，release）は「まだ扱えない」．その handler は推論せず，with に書いても依存は空として扱う．状態のない handler の `self` も「まだ扱えない」．with に layer を書くのも「まだ扱えない」．
- （補）`use` を省略した `@external` の fn と `@external suspend fn` は，handler の操作の本体の中でしか使えない（E0407，12.2）．`suspend fn` は `use {}` を書いていても同じ．呼び出しに限らず参照で確かめる（関数の値として外へ持ち出せないように）．操作の本体の中の無名関数の中も「中」とみなす．
- （補）能力の値の制約（8.2）は，型が決まった後に値の最上位の型だけを見る．呼び出しの引数は E0408（構成子の引数はデータ構造に入れることなので E0409），リスト・タプル・部分更新のフィールドは E0409，fn・handler の操作・無名関数の本体の値は E0410．無名関数の引数をそのまま返す無名関数は E0410 にしない（能力の値は引数として渡ってきたもので，渡した側が E0408 になる）．with の本体の値は見ない（capture set，8.6 は 0.22）．
- （補）main（エントリのモジュールの `main` という名前の fn）とテスト関数の本体の R は，Async を除いて空でなければならない（E0402，8.3，13.3）．`use` 節を書いていても同じ．どれが main とテスト関数かは driver が決めて `CheckOptions` で渡す．
- （補）const の値の R は空（5.6）．空でなければ E0402．
- （補）E0402 は E0318 と同じく，宣言の R に含まれないエフェクトごとに，それを要求した最初の式（`use` か呼び出し）に出す．
- （補）型の表示: 関数型の空の R は省く．引数の型の中では `use {}` と出す（E の `fails {}` と同じ理由）．効果の集合は要素が1つならそのまま，2つ以上なら `{ Logger, Clock }`（17.4），空なら `{}`．`fails` 節の後に `use` 節を出す．
- （補）`ProgramTypes::effects` に各式の R，`ProgramTypes::ops` に能力の値の操作の参照（`Field` の式 → 操作の定義）を持つ．lowering はまだ使わない（次の段の evidence passing で使う）．

## suspend，Async，Immediate（8.1，8.4，8.7，9.5，12.2）

表現と判定は `body/suspend.rs` の冒頭に書いた．ここには仕様にない判断を残す．

- （補）suspend は関数型に持たない（9.2 の関数型に印がない）．中断しうることは R の中のエフェクト（suspend fn を持つエフェクトか Async）で表す．操作の型（`Signatures`）も普通の操作と同じで，suspend の有無は HIR の `OpItem::sig.is_suspend` を見る．呼ぶ側の形は普通の操作と同じ（8.2）．
- （補）エフェクトが Immediate かは宣言から決める（9.5）: suspend fn を1つも持たず，Async でない．名前解決が誤りを出したエフェクトなど，宣言の分からないものは Immediate とみなす．
- （補）handler の suspend fn の実装の引数は，操作の引数と，最後の `resume`．`resume` の名前は問わない（最後の引数がそれ）．型は `fn(T) -> ()`（T は操作の戻り値の型，E と R は空）．本体の値は捨てるので `()` を求める（E0301）．本体の E は操作の `fails` に含まれる（普通の操作と同じ）．
- （補）handler の操作の `suspend` の有無がエフェクトの宣言と違えば E0415．引数の数と本体の検査は実装の書き方に従う（誤りを重ねないため）．引数の数の誤り（E0406）の数には `resume` を含める．
- （補）resume を後から呼びうるか（8.7）の判定: 操作の本体で，無名関数の外の `resume(v)` と `v |> resume` だけを直接の呼び出しとする．それ以外の参照（束縛の右辺，引数，無名関数の中の参照．無名関数の中の `resume(v)` も含む）が1つでもあれば，その操作の本体の R に Async を足す（ハンドラの依存に載る）．
- （補）Async は Prelude のエフェクト（emela-resolve が定義する）．handler の `implements Async` は E0413 で，その handler は推論せず，with に書いても何も供給しない（Async を消さない）．`use Async` は E0414 で，R に足さず型は `{unknown}`．use 節には書ける（`fn f() use Async`）．
- （補）`@external suspend fn` の型の R は，書いた use 節（と見えない集合）と Async の和．呼べる場所はハンドラの中だけ（E0407）．呼んだ側の R に Async が載るので，ハンドラの依存になる（8.7 の「Async を足す」を型で行う）．無名関数の中で呼んでも，その無名関数を呼ぶかコールバックに渡したところで R か Immediate の検査に現れる．
- （補）型引数の制約は Immediate だけを型に持つ（`TyParam::bounds`）．Immediate は `use` 節に書いた型引数（効果の集合）にだけ付けられる．それ以外（ふつうの型，エラーの集合）に付けたら E0322．効果の集合に Trait の制約を付けたら「まだ扱えない」．ふつうの型の型引数の Trait の制約（Prelude の `dbg[A: Show]`）は今までどおり検査も表示もしない（Trait は 0.21）．
- （補）`R: Immediate` の制約のある定義を参照したら，具体化した集合変数を覚え，集合を解いた後に要素を確かめる（E0416）．エフェクトは Immediate か，集合の型引数（参照した側の定義のもの）は Immediate の制約を持つか．中身の分からない見えない集合 `R(f)` は制約を持たないので通らない（`R: Immediate` の名前付きの型引数にする）．同じまとまりの中でまだ汎化していない集合変数は，汎化するときにその型引数に Immediate の制約を付ける（`fn inferred() { fn(g) { update(1, g) } }` は `∀ E, R: Immediate. ...`）．
- （補）外部関数の引数の型の中で `use` を省略した見えない効果の集合（`R(f)`）は Immediate の制約を持つ（9.5 の「外部関数に渡すコールバック」）．渡したコールバックは上と同じ仕組みで確かめ，文面だけを変える．外部関数の宣言の引数の関数型に書いた `use` 節も，Immediate でないエフェクトか制約のない集合の型引数があれば宣言の位置に E0416．
- （補）E0416 は参照の位置（呼ばれる側の名前）に出す．同じ呼び出しの中の要素はまとめて1つにする．
- （補）型の表示: `R: Immediate` はスキームの表示に `∀ A, R: Immediate. ...` と出す．ホバーでは suspend fn（操作と `@external suspend fn`）の型の前に `suspend` を付け，handler の名前には `handler H implements E` と依存（`use Async` など）を出す．
- （補）fn の操作（suspend fn でないもの）の実装の本体は，R が Immediate であるか，中断しうる呼び出しを含まないかのどちらかを満たす（E0417，8.1）．fn の操作は「ハンドラが値を返すと呼び出し元はすぐ続きを実行する」もので，呼ぶ側は中断しない前提で動く．どちらの判定もそれだけで本体が中断しないことを保証するので，OR にしても健全さは崩れない．R が Immediate なら呼び出しを見ずに通す（その場で resume するハンドラの with の中で suspend fn の操作を呼ぶ本体）．そうでなければ呼び出しごとに判定する: 操作の呼び出し（`clock.now()`，`(use Clock).now()`）は操作の宣言が `suspend fn` か，`@external suspend fn` の呼び出しはいつも，関数と関数の値の呼び出しは呼ばれる側の型の R が Immediate でないか．本体の R に suspend fn を持つ Clock が載っても，fn の操作しか呼ばなければ通る（R はそのままハンドラの依存になる）．どのハンドラの fn の操作の実装も同じ規則で縛られるので，fn の操作の呼び出しはどのハンドラに当たっても中断しない
- （補）E0417 の呼び出しは，本体の部分式のうち無名関数の本体を除いたものから探す（R が合流するのと同じ範囲，`sub_exprs`）．無名関数の中の呼び出しは，その無名関数を呼んだところで関数の値の呼び出しとして数える．with の中の呼び出しも，with がそのエフェクトを供給していても数える．そのため外側の Clock の now と，内側の `with InstantClock` の sleep を同じ本体に混ぜると，R も呼び出しも満たさず E0417 になる（取りこぼし．拾うには呼び出しを最も内側で供給する with まで見る必要があり，規則が重くなるので見送った）．関数の本体から中断しないことは推論しない（`fn stamp() use { Clock }` を挟むと E0417）．値として取り出した操作（`g = clock.now; g()`）も関数の値の呼び出し
- （補）E0417 は，本体の R が Immediate でないときに，中断しうる呼び出しの式の位置に呼び出しごとに出す．理由は `SuspendingCall` で持つ（suspend fn の操作，`@external suspend fn`，型の R が Immediate でない関数か関数の値．関数の値は局所変数の名前で呼んだらその名前）
- （補）lowering が中断しうるかの印を具体化ごとに決められるよう，`ProgramTypes::type_rows`（参照の式 → 集合の型引数を具体化した集合）と `ProgramTypes::generalized_rows`（推論した定義の本体に残る汎化した集合変数 → 型引数の添字）を返す（emela-core の NOTES の「lowering の印」）．
- （補）type の効果の集合の型引数にも `R: Immediate` を書ける（`type Ticker[R: Immediate](tick: fn() -> () use R)`）．制約は構成子のスキームに写り，構成子を参照したところで確かめる（E0416）．型の書き方（`Ticker[Clock]` と書いた引数の型など）では確かめない．値は構成子からしか作れないので，作ったところで確かめれば足りる．

## 宣言の型引数の集合（9.2 の `type Handler[E, R]`）

- （補）名前付き型の型引数に渡す集合は `Ty::Set(SetKind, Row)` で持つ．`Ty::Named` の型引数の位置にだけ現れ，値の型ではない．`SetKind` で表示の形（エラーの集合は `A | B`，効果の集合は `{ A, B }`）を決める．単一化では同じ種類の `Set` どうしを包含の制約にする（名前付き型の型引数の中なので両方向，つまり一致）．置き換え（`substitute`，`substitute_bound`），汎化，集合を解いた後の辿り方は関数型の E と R と同じに扱う．汎化では名前付き型の型引数の中の集合変数を負の位置とみなす（両方の位置）．
- （補）type・enum の型引数の種類は，フィールドの型の中で使われた位置から決める（9.2）．`fails` にだけ書けばエラーの集合，`use` にだけ書けば効果の集合，値の型にも書けばふつうの型．別の名前付き型の集合の型引数の位置に渡した型引数（`type Server[E](inner: Handler[E, Db])` の `E`）も集合なので，宣言全体で変わらなくなるまで繰り返して決める（`lower.rs` の `decl_sets`）．`fails` と `use` の両方に書いた型引数は，先に見つけた方の種類にする．fn の型引数の種類も，名前付き型の集合の型引数の位置に書けば集合（`fn serve[E, R](h: Handler[E, R])`）．
- （補）集合の型引数の位置には集合を書ける（17.4 に足した `type_arg`）．エラーの集合は `|` でつなぎ（`Handler[NotFound | Timeout, R]`），効果の集合は波括弧で囲む（`Handler[E, { Db, Log }]`）．要素1つなら囲まずに書ける（`Handler[NotFound, Db]`）．空の `{}` はどちらの位置にも書ける（エラーの集合の空も `fails {}` と同じ書き方なので）．要素に書けるのは，その種類のタグ（error か effect の名前）と集合の型引数で，それ以外（ふつうの型，違う種類のタグや集合の書き方，ふつうの型の型引数）は E0323 で，その要素を除いて続ける．
- （補）集合でない型引数の位置（`List[{}]`，ふつうの型の型引数）に集合の書き方を書いたら E0324．
- （補）名前解決は，名前付き型の型引数の位置ではエフェクトの名前も通す（emela-resolve の `type_arg`）．集合でない型引数の位置（`List[Db]` など）にエフェクトを書いたときは，型推論が名前解決と同じ E0221 を出す．
- （補）宣言の中なので，フィールドの関数型で `fails` と `use` を省略すれば空のまま（9.2）．効果を持たせたいときに型引数で明示するのは仕様どおり．
- （補）構成子 `Handler(run: f)` は，フィールドの関数型の E と R を集合変数に具体化し，渡した関数の E と R がそれに含まれる制約から推論する（`fn db_handler()` は `fn() -> Handler[{}, Db]`）．フィールドの関数を呼ぶ（`h.run(req)`）と，その E と R が呼んだ側に載る．
- （補）lowering では，集合の型引数は実行時の表現を持たない．IR の型引数の位置をそろえるため `()` にする．evidence passing はフィールドの関数の型の R で決まるので，構成子に渡した関数と，フィールドから取り出して呼ぶ側とで R が一致する（名前付き型の型引数は一致の制約）．

## 結果の形（lowering に渡すもの）

本人と相談して決めた（2026-10-11）．lowering は HIR を直接辿り，必要な情報を `ProgramTypes` の表から引く．型付きの木（THIR）は作らない．

- （補）型が付いた木を別に持つと HIR とほぼ同じ形の木を二重に保守することになるので，HIR の番号（`ExprId`，`PatId`，`LocalId`，`DefId`）をキーにした表を並べる．LSP の hover も同じ表を引く．
- （補）`exprs`，`pats`，`locals`: 各式，パターン，局所変数の型．`local_owner`: 局所変数がどの定義の本体のものか．
- （補）`sigs`: fn，構成子，const の宣言の型．宣言の型のなかった定義は推論して汎化した型．`ty_table`: `TyConId` から型の定義（`TyTable::def`）を引く．
- （補）`fields`: フィールドの参照の式 → フィールドの添字（構成子の宣言の順）．IR の `Expr::Field { index }` に使う．
- （補）`calls`: 呼び出し（`Call`），パイプ（`Binary { op: Pipe }`），部分更新（`Update`）の式 → 仮引数（フィールド）の位置の順に並べ直した引数．パイプは左辺を先頭に含み，呼ばれる側は右辺が呼び出しならその呼ばれる側，そうでなければ右辺そのもの．部分更新は書いたフィールドだけが `Some` で，`None` は `..` の後の値から引き継ぐ．書かれた順で評価するかどうか（`named::in_written_order`）は lowering が決める．
- （補）`type_args`: 汎用の定義（`Some`，`dbg`，型引数のある fn）の参照の式 → 具体化した型引数（宣言の型引数の順）．組み込み関数の `ty_args` に使う．
- （補）`pat_fields`: 構成子のパターン → フィールドの位置の順に並べ直したパターン．省いたフィールド（`..`）は `None`．IR では `Pat::Wild` で埋める．
- （補）決まらずに残った型変数（使われない `None` の `Option[?]` など）は型変数のまま表に残る．lowering は `()` として扱う．どの表現でも振る舞いは変わらない．
- （補）汎用の fn の本体の中の型には，その fn の型引数が `Ty::Bound(i)` で現れる．
- （補）エラーが1件でもあれば lowering は呼ばれない前提．表は誤りのある式に対しては欠けることがある（たとえば型の誤りのある呼び出しの `calls`）．

## driver に渡すときの文面

driver の `Diagnostic::from_type`（emela-driver の `diagnostic.rs`）がこの文面で写す．

付録 A の文面に合わせる．文面は仕様の一部ではないが，ここに置いておく．

- E0301 `Mismatch`: ``type mismatch: expected `Int`, found `String` ``
- E0302 `Mismatch`（`TypeErrorKind::InfiniteType`）: ``infinite type: `?0` occurs in `List[?0]` ``
- E0304 `WrongTypeArgCount`: ``wrong number of type arguments: `List` takes 1, found 2``
- E0305 `WrongArgCount`: ``wrong number of arguments: `connect` takes 2, found 3``．`callee` が `None`（関数値の呼び出し）なら ``this function takes 2 arguments, found 3``
- E0306 `UnknownLabel`: ``no parameter named `prot` in `connect` ``．`LabelOnUnnamedCallee`: ``no parameter named `x`: a function value has no parameter names``
- E0307 `DuplicateArg`: ``argument `port` given more than once``
- E0308 `MissingPatternFields`: ``pattern `User` does not mention fields `id`, `email` ``（1つなら ``field `id` ``）．4つ以上なら先頭の3つと ``and N more``．注記 ``= help: add `..` to ignore the remaining fields``
- E0309 `PatternFieldCount`: ``pattern `Rect` has 2 fields, found 1``
- E0310 `NotNumeric`: ``expected a number (`Int`, `Int64` or `Float`), found `String` ``．`integer` なら ``expected an integer (`Int` or `Int64`), found `Float` ``
- E0311 `IntLiteralOutOfRange`: ``integer literal `3000000000` is out of range for `Int` ``
- E0312 `NoField`: ``no field `nmae` on type `User` ``
- E0313 `NoFields`: ``type `Int` has no fields``
- E0314 `FieldOnUnknownType`: ``the type of this value must be known to access field `name` ``．注記 ``= help: add a type annotation``
- E0315 `OpaqueField`: ``cannot access field `value` of opaque type `Email` outside its module``
- E0316 `MissingParamType`: ``parameter `x` of a top-level function needs a type``
- E0303 `NonExhaustiveMatch`: ``non-exhaustive match: `Rect(_, _)` not covered``（複数なら ``patterns `A`, `B` not covered``）
- E0317 `RefutableBinding`: ``refutable pattern in binding: `None` not covered``．注記 ``= help: use `match` to handle the other cases``
- W0301 `UnreachableArm`: ``unreachable match arm``
- `Unsupported`: コードなしの ``error: not supported yet: {what}``（扱えるようになるまでの間だけ）
- E0318 `UnhandledErrors`: fails 節を書いていなければ ``unhandled error `NotFound` ``（複数なら ``unhandled errors `A`, `B` ``）．注記 ``= help: handle it with `escape`, or declare it with `fails` ``．書いていれば ``error `DbError` is not declared in `fails` ``（複数なら ``errors `A`, `B` are not declared in `fails` ``）．注記 ``= help: add it to `fails` or handle it with `escape` ``
- E0319 `NotAnError`: ``expected an error value, found `Int` ``
- E0320 `EscapeArmNotError`: ``escape arms must match an error constructor``
- E0321 `ErrorSetMismatch`: ``error set mismatch: expected `fails {}`, found `fails Timeout` ``
- W0302 `UnraisedError`: ``error `Timeout` is never raised here``
- E0401 `UnknownOperation`: ``effect `Logger` has no operation `infoo` ``
- E0402 `UnhandledEffects`: `context` で変える．`Declared` は ``effect `Logger` is not declared in `use` ``（複数なら ``effects `A`, `B` are ...``）．注記 ``= help: add it to `use` or provide it with `with` ``．`Const` は ``constant values cannot use effect `Logger` ``．`Main` は ``effect `Logger` is not provided in `main` ``，`Test` は ``effect `Logger` is not provided in test function `logs` ``．注記 ``= help: provide it with `with` ``
- E0403 `EffectSetMismatch`: ``effect set mismatch: expected `use {}`, found `use Logger` ``
- E0404 `MissingHandlerOps`: ``handler `Half` does not implement operation `warn` ``（複数なら ``operations `a`, `b` ``）
- E0405 `DuplicateHandlerOp`: ``operation `info` is implemented more than once``
- E0406 `HandlerOpArgCount`: ``wrong number of parameters: operation `info` takes 1, found 2``
- E0407 `ExternalOutsideHandler`: ``external function `console_log` can only be used in handlers``．注記 ``= help: declare it with `use {}` if it has no side effects``（`suspend fn` では注記なし）
- E0408 `CapabilityArgument`: ``a capability cannot be passed as an argument``．注記 ``= help: pass a closure that uses the capability instead``
- E0409 `CapabilityInData`: ``a capability cannot be stored in a data structure``
- E0410 `CapabilityReturned`: ``a capability cannot be returned from a function``
- E0411 `HandlerCycle`: ``handlers depend on each other: `Stamped` -> `Ticking` -> `Stamped` ``
- E0413 `AsyncHandler`: ``handler `MyAsync` cannot implement `Async`: only the runtime provides it``
- E0414 `UseAsync`: ``` `Async` has no operations and cannot be used with `use` ```
- E0415 `SuspendMismatch`: ``operation `sleep` is a `suspend fn` in effect `Clock` ``．注記 ``= help: implement it with `suspend fn sleep(..., resume)` ``．逆は ``operation `now` is not a `suspend fn` in effect `Clock` ``．注記 ``= help: remove `suspend` ``
- E0417 `SuspendingFnOp`: 呼び出しの理由ごとに ``operation `sleep` of `Clock` is a `suspend fn` and may suspend, but operation `now` is not a `suspend fn` ``，``external function `fetch_text` may suspend, but ...``，``calling `stamp` may suspend because it uses effect `Clock`, but ...``（名前のない呼ばれる側は ``this call may suspend because ...``）．注記 ``= help: declare `now` as a `suspend fn` in the effect ``．関数の呼び出しで，R に Async 以外の中断しうるエフェクトがあるときだけ ``, or call the `fn` operations of `Clock` directly instead of through a function`` を足す
- E0416 `NotImmediate`: ``effect `Clock` may suspend, but `R` of `update` must be `Immediate` ``．集合の型引数なら ``effect set `R(f)` may suspend, ...``，注記 ``= help: constrain the effect set with `: Immediate` ``．外部関数のコールバックは ``effect `Async` may suspend, but callbacks passed to external function `set_timeout` must be `Immediate` ``
- E0323 `SetArgMismatch`: ``` `E` of `Handler` is an error set, found type `Int` ```（`found` は ``effect `Clock` ``，``error `NotFound` ``，``type parameter `A` `` など）．注記 ``= help: write an error or an error set type parameter``（効果の集合なら ``an effect or an effect set type parameter``）
- E0324 `SetAsType`: ``expected a type, found a set: sets can only be passed to set type parameters``
- E0221 `EffectAsType`（型推論から）: ``effect `Db` cannot be used as a type``．注記は名前解決の E0221 と同じ
- E0322 `ImmediateOnNonEffect`: ``` `Immediate` can only constrain effect sets, but `A` is not used in `use` ```
- （補）期待する型がある位置（戻り値の型，束縛の型注釈，引数）にブロック・if・match を置いたときは，期待する型を最後の式と各分岐まで渡して照合する（双方向の検査）．型が合わないときに一番内側の食い違った式を指すため．その式の型は期待する型として記録する．期待する型がなければ，分岐の型を `join` で合わせる．
- （補）整数リテラルの型は「整数リテラルの型変数」（`InferCtx::new_int_var`）にする．Int か Int64 にしか束縛できず，他の型と単一化すると，その場で型が合わないエラーになる（エラーには `Int` と出す）．他の型変数と単一化すると，その型変数も同じ制約を持つ．本体を推論し終えても決まらなければ Int にする（2.4）．
- （補）整数リテラルの範囲は，本体を推論し終えて型が決まった後に確かめる（E0311）．負の数は単項マイナスの式なので，リテラルは正の数として見る．そのため Int の `-2147483648` は書けない（`2147483648` が範囲外）．2の補数として読み替えない（`0xFFFF_FFFF` は Int に収まらない）．
- （補）算術演算子と単項マイナスのオペランドは Int，Int64，Float のどれか（6.6）．型変数のまま決まらないこともあるので，本体を推論し終えたときに確かめる（E0310）．決まらなければ Int にする．
