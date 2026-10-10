# emela-cli の補足

仕様にない判断を「（補）」として1行ずつ残す．

- （補）`check`，`build`，`run` の path は省略でき，既定は `.`（`Pome.toml` を上にたどって探すので，プロジェクトの中のどこからでも動く）
- （補）`run` の診断は node を起動する前に出す
- （補）終了コード: 成功 0，診断でエラーがあれば 1，`run` で node まで進んだら node の終了コード（defect は 101．emela-driver の NOTES.md を参照）．警告だけなら 0
- （補）`run` のプログラムへの引数は `--` の後に書く（`emela run -- a b`）
- （補）`build` と `run` は `--out-dir` で出力先を変えられる
- （補）診断は標準エラーに出す．色は，標準エラーが端末で `NO_COLOR` が空か未設定のときだけ付ける
- （補）`Fmt` はまだ `todo!`
- （補）`test [path] [filter]`: filter はテストの表示名の部分一致．結果は標準出力，診断は標準エラー．終了コードは失敗があれば 1．`--out-dir` の既定は `<プロジェクト>/target/emela/test`
- （補）`test` は driver の `WithTests(Resolve)` と `NoJsBackend` を使う．lowering がないので，`@test` の誤り（E0222）がなければ E0905 で止まる
- （補）`check`，`build`，`run` は driver の `Resolve`（構文解析と名前解決）を使う．型検査と lowering がまだないので，`build` と `run` は構文，import，名前に誤りがなければ E0905 で止まる
- （補）`lsp` は引数を取らない．フロントエンドは `check` と同じもの（今は `Resolve`）を渡す．exit を受けたら 0，プロトコルの誤りで止まったら標準エラーに1行出して 1
