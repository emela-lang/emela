# emela-codegen-c の補足

C のターゲット（仕様 16 章，Tier 2）．Heap IR（`Stage::RefCounted`）を読んで C11 を書き出す．ランタイムは C で書き（`src/emela_runtime.c` と `.h`），`include_str!` で埋め込んで出力の隣に書き出す．設計にない判断を「（補）」として残す．

## パイプライン

- （補）`compile` が Core（lower 済み）から通しでかける．エントリ（main）を根に `emela_core::monomorphize_from` → `emela_lir::from_core` → 対応していない構成の検査（`check.rs`）→ `emela_lir::to_heap` → 出力．単相化の後のモジュールにはエントリからたどれる関数しか残らないので，検査は「たどれるものだけ」になる
- （補）検査は Flow IR の段でかける（Heap IR のパスの前に止める）
- （補）main がなければ，何もしない C の main を出す（JS で main のないエントリを読み込むだけなのと同じ）
- （補）`Entry::Tests` はテスト関数を根に単相化し，C の main でそれらを順に起動（`em_run`）する．各テストの後に保留の defect と失敗を調べて，JS のテストランナーと同じ結果の行（`\x1eemela-test` と JSON．outcome は ok，defect，error，interrupted）を標準出力に書く．テストの間で状態（const の値など）は持ち越す（JS も1つのプロセスで順に呼ぶ）

## 値の表現と RC のヘッダ

- （補）Int と Bool は `int32_t`，Int64 は `int64_t`，Float は `double`，引数のない構成子だけの enum はタグの `int32_t`．それ以外は参照（`em_ref`）で，空のリストだけは NULL
- （補）ヒープのオブジェクトは 16 バイトのヘッダの後に 8 バイトの枠（`em_word`）を並べる

  ```
  offset 0  uint32_t rc     参照カウント．0 は静的なデータの印（数えない）．新しい割り当ては 1 から
  offset 4  uint32_t aux    未使用（詰め物）
  offset 8  em_desc *desc   種類，構成子のタグ，枠の数，どの枠が参照か，error の enum，表示の関数
  offset 16 em_word slots[]
  ```

  調査の結論 8 の Roc 式（0 が静的）にした．`dup` は `rc != 0` なら足すだけ，`drop` は `rc != 0` で減らして 0 なら解放．単一スレッドなので原子的な操作はしない
- （補）記述子（`em_desc`）は形ごとに 1 つ出力する．構成子（enum とタグ），タプル，クロージャ（関数ごと），ハンドラ（ハンドラごと）．どの枠が参照かは，その値を作る `make` などの引数の表現から決める（単相化の後なので型ごとに決まる）．リストのセル，セル（`$Cell`），箱はランタイムが持つ共通の記述子を使う
- （補）文字列は UTF-8 で，枠 0 にバイト数，枠 1 からバイト列と NUL（文字列の一部ではない）．リストは cons のセル（枠 0 が先頭，枠 1 が残り）．クロージャは `{関数ポインタ, 捕獲…}`．ハンドラは `{使用済みの印, 操作のクロージャ…}`．証拠の表は `{個数, (エフェクトの添字, ハンドラ)…}` で，後ろから探す（後の束縛が勝つ）
- （補）文字列のリテラル，引数のない構成子（`None`，`Leaf`，引数のない error），捕獲のないクロージャは `rc = 0` の静的なオブジェクトにする．割り当ても RC もかからない
- （補）`@external type` の値（ホストのポインタ）は箱（`EM_KIND_HOST`）に入れて持つ．Heap IR は参照として `dup` / `drop` をかけるので，生のポインタのままだと数を書き換えてしまう．箱を解放してもホストの値は解放しない（12.3）
- （補）解放は再帰しない（調査の結論 6）．解放するオブジェクトを，生きている間は参照カウントを持っていた先頭の 8 バイトでつないだスタックに積んで回す．余分なメモリを使わずに，長いリストと深い enum の解放でスタックがあふれない（`tests/programs/deep.emel`）

## 呼び出し規約と制御

- （補）IR の変数はすべて関数の頭で宣言する C の局所変数（`v12`）．`yield` と `break` は行き先の変数に代入して文の後のラベルへ `goto`，`recur` はループの変数に並行に代入して頭へ `goto` する．使われないラベルは出さない
- （補）失敗と defect は戻り値の形を変えずに，保留の枠（`em_failure`，`em_defect`）で返す．大域変数1組で足りる．枠に値があるのは呼び出しから戻る間だけで，中断点ではいつも空（着地点は枠を取り出してから cleanup を走らせる）なので，fiber を切り替えても混ざらない．呼んだ側は着地点のある呼び出しの後で枠を調べる（`if (em_defect != NULL) { … }`，`if (em_failure != NULL) { … }`）．`unwind` と `fail` はダミーの値（0）を返す
- （補）クロージャの呼び出しは，レコードの枠 0 の関数ポインタを呼び出しの型にキャストして，レコード自身を最初の引数に渡す．型は呼ぶ値の Core の関数型（操作は `OpDef`）の戻り値と，引数の表現から決める
- （補）ハンドラの操作を呼ぶ前に使用済みの印を調べ，立っていれば `handler X was used after release` の defect にする（8.6）．`retire` が印を立てる
- （補）release が defect を起こしながら別の defect が巻き戻っているときの注記（`note: the release of handler X also failed: …`）のハンドラの名前は，最後に `retire` したハンドラの名前を使う．Heap IR の cleanup は release の呼び出しの直後に必ず `retire` を置くので，注記を作る `defect_resume` の時点でその名前が分かる
- （補）const はランタイムの `em_const` の状態（未評価，評価中，値，defect）で 1 回だけ評価する．文言は JS の `$constant` と同じ（`while evaluating const X: …`，`const X refers to itself while being evaluated`）．終了のときに値と defect を解放する（漏れの検査のため）
- （補）エントリに届いた失敗の表示（`error: NotFound(id: 1)`）には，error の値の記述子の `show` を使う．`from_core` が，関数の中で作る error の enum のうち表示できるものに `show$E` を作る（`emela_lir::Module::error_shows`）

## 整数と Float

- （補）Int と Int64 の `+ - *` と単項の `-` は符号なしの演算で巻き戻す（16.1．C の符号付きのあふれは未定義なので）．割る数が 0 でないことは Heap IR が先に調べる．`INT_MIN / -1` は割る数が -1 のときに符号反転にして巻き戻す（剰余は 0）
- （補）Float の表示は JS の `Number.prototype.toString` と同じにする．`%.{1..17}e` を桁の少ない順に試して読み戻しで同じ値になる最短の桁を求め，ECMAScript の規則で小数点の位置と指数の形（`1e+21`，`1.5e-7`）を決め，有限の整数値には `.0` を足す．`%.Ne` は正しく丸めるので，最短の桁のうち一番近いものになる（JS と同じ）
- （補）Float の `%` は `fmod`．`floor` などの変換はリンクに `-lm` が要る

## 組み込み関数

- 引数は借用（Heap IR の規則），結果は所有．JS のランタイムと同じ結果と defect の文言にした
- （補）Unicode の表が要るもので C にまだないもの（書記素クラスタ）は，コンパイル時のエラーにする．`String.length`，`String.chars`，区切りが定数の空文字列の `String.split`．区切りが実行時に空になった `String.split` は，ASCII の文字列なら正確に分けられる（ASCII の書記素は CR LF を除いて 1 文字ずつ）のでそうし，ASCII でなければ `emela: … is not supported on target c yet` で終了コード 70 で止まる（defect ではない．Heap IR は split を defect しうる組み込み関数に数えないため）
- `String.trim` の空白は White_Space の 25 文字の固定の表（16.2）で，C でも書ける
- 文字列の比較は UTF-8 のバイトの辞書順（コードポイントの順と同じ）

## ホストの境界（12.4）

- （補）外部関数ごとに包みの関数を出し，String を `const char *` に写して（U+0000 を含めば defect），戻り値の `char *` を UTF-8 として検査して写してから `free` する（正しくない並びは U+FFFD．maximal subpart ごとに置き換える）．NULL は `external function X returned NULL` の defect
- （補）ホストの関数のプロトタイプは 12.4 の表から書く．生成した `main.c` はランタイムのヘッダしか include しないので，C の標準ヘッダの宣言とぶつからない
- （補）`@external(c, "file", "name")` の file は，同梱のもの（`HostModule::Bundled`）を `COutput::bundled`，プロジェクトのもの（`HostModule::File`）を `COutput::host_files` で返し，呼ぶ側が一緒にコンパイルする
- （補）c の注釈のない外部関数は，呼ばれたときに JS と同じ文言（`external function X is not available on this target`）の defect にする．たどれるものは lowering の E0412 で先に止まる

## suspend と fiber（仕様 11 章，14.5）

- （補）suspend する関数のうち中断点（中断の辺）のあるものを状態機械として出す．本体の C の関数（`e3_nap_sm`）は `(em_ref em_fr, em_word em_in, 引数…)` を取り，フレームなしで入れば頭から，フレームで入ればフレームの状態の再開の地点へ `goto` する．関数そのもの（`e3_nap`）はフレームなしで本体を呼ぶだけの包みで，クロージャのレコードもこれを指す．駆動のループが呼ぶ再開の関数（`e3_nap_resume`）は本体の値を `em_word` にして返す
- （補）中断の辺は，値の直後の `if (em_suspending) { フレームを作って saved を移し，em_frame_push(fr); return; }` と，その後ろのラベル `em_rN`．着地点の検査はラベルの後なので，再開の入力の失敗と defect も値の着地点が受ける
- （補）フレームは `3 + n` 枠のオブジェクト（枠 0 再開の関数，枠 1 状態，枠 2 外側のフレーム，枠 3 から saved）．記述子は関数と saved の形ごと．フレームを指すのは継続だけなので，再開では枠を変数に移してから殻だけを解放する（`em_frame_free`）．走らせずに捨てるとき（起動が終わった後に残った継続）は `em_drop` で中身も解放する
- （補）ランタイムのスケジューラは JS の `$Root`，`$Fiber`，`$Scope`，`$performOp` を1つずつ写した（`emela_runtime.c` の「Suspension and fibers」）．ジョブの待ち行列の順，公平性の予算（fiber ごとに 1000 回）と 16 回に1回ホストを通す譲渡（`later` の待ち行列．JS の `setImmediate`），中断の的（fiber 全体かスコープ），release の間の保留（holds），子の失敗による兄弟の中断と注記まで同じにしたので，切り替えの順と出力は JS と一致する（`tests/programs/fiber_*.emel`）
- （補）JS の継続（ジェネレータ）と待ち（`wait`）は，フレームの並びと `em_wait`（手で数える）にした．要求（譲渡，その場で resume されなかった操作，待つ並行の操作）が `em_wait` を作って `em_suspending` を立て，フレームが積まれて駆動のループ（`em_advance`）まで戻ったところで，継続を待ちに付ける．待ちを起こす（`em_cont`）と，同じ fiber の中ならその場で，そうでなければジョブとして続ける（14.5 の4）
- （補）中断は保留の defect の一種（`em_desc_interrupted` のオブジェクト．枠 0 に的のスコープ）で届ける．Heap IR の defect の着地点がそのまま release を走らせて巻き戻る．捨てられた継続（起動が止まった，fiber が中断された）もこれを入力にして再開する．`em_defect_resume` は，中断の巻き戻り中の release の defect を結果にし，defect の巻き戻り中の中断は注記にしない（14.4）．main の終了コードは 130，テストの結果は interrupted
- （補）suspend fn の操作の呼び出しは，`em_resume_new` で resume の関数値（`fn(T) -> ()` のクロージャのレコード．T の表現ごとの関数）を作って実装の最後の引数に渡し，実装が戻ったら `em_perform_end` が続ける値を返す．その場で resume されていればそれを返し，フレームも待ちも作らない（8.7）．されていなければ待ちを作って中断を始める．実装自身が中断したら，同じ fiber の別の継続として待たせる．2回目の resume，resume の後の実装の失敗と defect の文言と扱いは JS と同じ
- （補）suspend しない関数の中の中断しうる呼び出しは `em_sync_enter()` と `em_sync_leave()` で囲む．中断して戻ったら，その継続はもう resume されないので，その場で中断を入力にして最後まで回し（release を内側から走らせる），中断を保留の defect にする（14.5）
- （補）main とテスト関数も起動（`em_run`）の根の fiber として走らせる．値を `em_word` にして返す包み（`e0_main_entry`）を渡す
- （補）並行の操作には値の表現（`EM_REPR_*`）を渡す．子の値は `Any`（Flow IR が箱に入れる）なので，join と await はランタイムが表現に合わせて箱から出す．Exit，Some，Cause はランタイムの記述子で作る
- （補）後の失敗の注記（11.2）は，JS では `$Fail` の包みに持つ．C には失敗の包みがないので，起動の間だけ error の値ごとの表（`em_note_errors`）に持つ．静的な error の値は写してから載せる
- （補）イベントループはまだない．待ち行列と `later` が空になったら，JS でホストのイベントループが空になったときと同じく起動が止まったとみなし，すべての fiber を中断する（`em_root_interrupt`）．JS と同じく，その巻き戻しが `later` に仕事を足さなければそこで起動を終える
- （補）12.6 のイベントループは `em_run` のこの地点（「No job is left」）に入る．待ち行列と `later` が空のとき，タイマー，ファイル記述子，別のスレッドからの完了（`em_complete_*`）を待って準備のできたコールバックを走らせ，待つものが何もなければ止まったとみなす．完了は待ち（`em_wait`）を `em_cont` で起こせばよい．別のスレッドからの完了はロックのある受け箱に積んでループを起こし，ランタイムのスレッドで `em_cont` する（`em_cont` も待ち行列もスレッドに安全ではない）．中断で待ちを捨てるとき（`em_drop_wait`）に `em_on_cancel` のコールバックを呼ぶ

## 対応していないもの

- コールバックを取る外部関数（E0427）と suspend fn の外部関数（E0428）．イベントループと一緒に入れる

## テスト

- `tests/programs/*.emel` を C にして cc で作って動かし，終了コード，標準出力，標準エラーを JS の出力（`*.expected`．`emela run` で作ったもの）と比べる．実行では `EMELA_LEAK_CHECK` を立て，終了のときに生きている割り当てがあれば終了コード 99 で知らせる（調査の結論 10）．環境変数を渡せない所（driver を通す実行）では，ランタイムを `-DEMELA_LEAK_CHECK` でコンパイルしても同じ検査になる．cc が `-fsanitize=address,undefined` を受け付ければ，それでも作って同じ結果になることを確かめる
- cc は `EMELA_CC` か `cc`．起動できなければ理由を出して飛ばす
- `tests/snapshots.rs` は小さなプログラムの C を insta で見る
- cc に渡すフラグ：`-std=c11 -fno-strict-aliasing`（静的なオブジェクトを別の構造体の型で読むため）と `-lm`
