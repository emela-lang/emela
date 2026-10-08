# emela-cli の補足

仕様にない判断を「（補）」として1行ずつ残す．

- （補）`check`，`build`，`run` の path は省略でき，既定は `.`
- （補）終了コード: 成功 0，診断でエラーがあれば 1，`run` で node まで進んだら node の終了コード（defect は 101．emela-driver の NOTES.md を参照）．警告だけなら 0
- （補）`run` のプログラムへの引数は `--` の後に書く（`emela run -- a b`）
- （補）`build` と `run` は `--out-dir` で出力先を変えられる
- （補）診断は標準エラーに出す．色は，標準エラーが端末で `NO_COLOR` が空か未設定のときだけ付ける
- （補）`Fmt`，`Test`，`Lsp` はまだ `todo!`
