# emela-cli の補足

仕様にない判断を「（補）」として1行ずつ残す．

- （補）`check`，`build`，`run` の path は省略でき，既定は `.`（`Pome.toml` を上にたどって探すので，プロジェクトの中のどこからでも動く）
- （補）`run` の診断は node を起動する前に出す
- （補）終了コード: 成功 0，診断でエラーがあれば 1，`run` で node まで進んだら node の終了コード（defect は 101．emela-driver の NOTES.md を参照）．警告だけなら 0
- （補）`run` のプログラムへの引数は `--` の後に書く（`emela run -- a b`）
- （補）`build` と `run` は `--out-dir` で出力先を変えられる
- （補）診断は標準エラーに出す．色は，標準エラーが端末で `NO_COLOR` が空か未設定のときだけ付ける
- （補）`fmt [paths...]`: 対象の決め方と整形は driver の `fmt`（driver の NOTES.md の「整形」）．paths を省いたら `.`．書き込みは cli が `std::fs::write` でする．成功しても何も出さない
- （補）`fmt --check`: 書き換えず，整形が要るファイルを標準出力に1行ずつ（診断と同じ相対パスで）並べる．1つでもあれば終了コード 1．差分は出さない
- （補）`fmt` の終了コード: 構文エラーなど診断のエラーが1つでもあれば 1（ほかのファイルの整形は続ける）．`--check` で整形が要るファイルがあっても 1
- （補）`fmt -`: 標準入力を整形して標準出力に書く（整っていれば入力のまま）．構文エラーなら標準出力には何も書かず，診断を `<stdin>` のパスで標準エラーに出して 1．`--check -` は整形が要れば `<stdin>` を出して 1．`-` とほかのパスを混ぜたら引数の誤り（終了コード 2）．`-` という名前のファイルは `./-` と書く
- （補）`test [path] [filter]`: filter はテストの表示名の部分一致．結果は標準出力，診断は標準エラー．終了コードは失敗があれば 1．`--out-dir` の既定は `<プロジェクト>/target/emela/test`
- （補）`test` は driver の `LowerTests` と `CoreJs` を使う．`build` と `run` は `Lower` と `CoreJs`
- （補）`build`，`run`，`test` は `--target js|c|wasm` を取る．省けば `Pome.toml` の `[build].target`，それもなければ js（driver の NOTES.md の「C のターゲット」「WASM のターゲット」）．`c` では `Lower` / `LowerTests` を `with_target(Target::C)` にして `CoreC` を，`wasm` では `with_target(Target::Wasm)` にして `CoreWasm` を使う．`--out-dir` の既定はターゲットごと（`target/emela/c`，`target/emela/test-c`，`target/emela/wasm`，`target/emela/test-wasm`）．`run` は実行ファイルの終了コードをそのまま返す
- （補）`check` は driver の `Typecheck`（構文解析，名前解決，型検査）を使う．`build` と `run` は `Lower`（型検査の後に lowering）と `CoreJs`（JS の出力）を使う
- （補）`run` はエントリに `main` の fn がなければ E0230 で止まり，node を起動しない（終了コード 1）．`build` は main がなくても出力する（8.3）．ただしディレクトリ指定で `src/lib.emel` だけがあるライブラリ専用プロジェクトでは，`build` は検査だけで出力せず，`run` は lib の fn `main` の有無にかかわらず E0230 にする．明示的ファイル指定は従来どおりそのファイルを入口にする．判定は driver が持つ
- （補）`lsp` は引数を取らない．フロントエンドは `check` と同じもの（今は `Typecheck`）を渡す．exit を受けたら 0，プロトコルの誤りで止まったら標準エラーに1行出して 1
- （補）`--help` の文言（サブコマンドと引数の説明）は英語で書く．README の Commands の表と同じ言葉を使う．既定値は clap の `[default: …]` の形を説明の末尾に手で書く（`Option<PathBuf>` の既定は main.rs で決めるため）．テスト `help_is_english` が ASCII だけであることを確かめる

## emela pome

- （補）`emela pome` はカレントディレクトリから祖先へ `Pome.toml` を探す（`--path` は持たない）
- （補）出力は `Pome.toml:` と `Pome.lock:` の見出しの下に変わった依存を1行ずつ（`+` 足した，`-` 除いた，`a -> b` 変えた）．lock が変わらなければ `Pome.lock is up to date`
- （補）誤りは `error: ` に続けて標準エラーに出し，1 で終わる．`verify` で食い違いがあったときも 1

## 組み込みC

- （補）`build --target c-embedded`はdriverの`CoreEmbedded`へ接続し，board未選択ならCとheaderだけを出力する．`run`と`test`はこのtargetを診断して停止する．既存の`c`とは別targetである
- （補）`--board`は`[embedded].board`に優先する．boardを選びtargetが未設定なら`c-embedded`を使い，明示またはproject設定の別targetとは併用できない
- （補）`--embedded-config`は`[embedded].config`に優先し，前者の相対pathは実行directory，後者はproject directoryを基準にする．設定ファイルの自動探索はしない
- （補）`check --board`と組み込みbuildは`[embedded.modules]`の読取り専用置換を共有する．LSP・doc・host実行には適用しない
- （補）board buildは`generated/`・`bsp/`・`firmware/`を出力し，成功時の入力と成果物を記録する．`flash`は同じproject・board・設定とhashを照合し，再buildせず書き込みtoolへ渡す．`--dry-run`も照合するがデバイスは開かない
- （補）`flash --port`はESP32-C3のserial port，STM32F4ではST-Link adapter serialを表す．実装範囲・設定・toolの準備と確認範囲は[組み込みCLI](../emela-codegen-emb/docs/cli-embedded.md)へまとめる
