# emela-lsp の補足

仕様にない判断を「（補）」として1行ずつ残す．

## 今の段でできること

- `emela lsp` で標準入出力の言語サーバーが起動する（`lsp-server` の同期型）
- 開く，書き換える，保存する，閉じるのたびに，プロジェクト全体を `emela check` と同じフロントエンドで検査し直し，`publishDiagnostics` で送る
- ホバー，補完，コードアクションはまだない（`MethodNotFound` を返す）

## しくみ

- driver の `check` はファイルを `FileSystem` から読む．開いているファイルの未保存の内容は，`OsFs` を包む `Overlay` に重ねて渡す．driver は変えていない
- プロジェクトは開いたファイルのパスから `Input::resolve` で決める（仕様 4.1）．`Pome.toml` がなければ単独ファイル
- （補）単独ファイルは driver の決まり（#126）のとおり，エントリから import でたどれるファイルだけを読む．開いた単独ファイルはどれもエントリとして検査する
- 解析は増分にせず毎回全部やり直す．届いているメッセージを全部読んでから解析し，解析の後にまた書き換えが届いていたら結果を捨てて解析し直す

## 補った判断

- （補）全文の同期（`TextDocumentSyncKind::FULL`）．保存の通知は本文を求めない（`includeText: false`）．本文が来たらそれを使う
- （補）initialize の `workspaceFolders`（なければ `rootUri`）のフォルダで `Pome.toml` が見つかれば，ファイルを開く前からそのプロジェクトを検査して診断を出す．見つからないフォルダは何もしない
- （補）プロジェクトの中のファイルを開いたときは，`emela check <プロジェクト>` と同じく，プロジェクトのディレクトリを入力にする（エントリは `src/main.emel`）．単独ファイルはそのファイルをエントリにする
- （補）同じディレクトリの単独ファイルを2つ開くと，検査を2回して同じ診断が重なるので，ファイルごとに同じ診断を1つにまとめる
- （補）`Input::resolve` の誤り（ソースのルートの外 E0208 など）は，開いたファイルの先頭（0:0）に付ける
- （補）位置がファイル全体（`Location::File`）か，表にないパス（`Location::Path`．ディレクトリ名の誤りなど）の診断は，そのパスの先頭（0:0）に付ける．ディレクトリにも URI のまま送る
- （補）位置のない診断（処理系の内部の誤りなど）は，エントリに，エントリがなければ `Pome.toml` に付ける
- （補）注記はメッセージの末尾に `\nnote: …` として1行ずつ足す．ラベルは `relatedInformation`，コードは `code`（文字列），`source` は `emela`
- （補）`file:` でない URI（`untitled:` など）は扱わない．保存されていないバッファには診断が出ない
- （補）開いているファイルの診断は，エディタから来た URI で送る．開いていないファイルは，ワークスペースのフォルダの下ならエディタから来たフォルダのパスでつなぐ（driver はシンボリックリンクを解いたパスを使うので，そのまま送るとエディタが別のファイルとみなす）
- （補）URI とパスの変換は自前（`%` の復号と符号化だけ）．符号化は英数字と `-._~/` 以外すべて（VS Code と同じ）．Windows のドライブ文字はまだ扱わない
- （補）`publishDiagnostics` の `version` は，開いているファイルなら最後に受けた版，開いていなければ付けない
- （補）毎回の解析で，診断のあるファイルは全部送り直す（変わっていなくても）．前回に送って今回ないファイルには空の配列を送る
- （補）前回に送ったかどうかはパスで比べる．同じファイルで URI の書き方が変わった（開いてエディタの URI になった，閉じてサーバーの URI に戻った）ときは，古い URI に空の配列を送る．空の配列は新しい診断より先に送る（エディタが2つの書き方を同じファイルとみなしても，新しい診断が残るように）

## 手で確かめる

まず `cargo build -p emela-cli` で `target/debug/emela` を作る．

### VS Code（`editors/vscode`）

既存の拡張は `emela.serverPath`（既定は `emela`）を `lsp` の引数で起動するので，0.20 の `emela lsp` でそのまま動く．

1. `cd editors/vscode && npm install && npm run compile`
2. VS Code で `editors/vscode` を開き，F5（Run Extension）
3. 開発用のウィンドウの設定で `emela.serverPath` に `target/debug/emela` の絶対パスを入れる（PATH に置いてあれば不要）
4. `Pome.toml` と `src/main.emel` のあるフォルダを開き，`const Y = "\q"` と打つと `E0107` の波線が出る．直すと消える
5. 「出力」パネルの「Emela Language Server」にサーバーの標準エラーが出る

注意:

- `emela.packageRoots` は空のままにする．0.1x 系の `--package` 引数を足すので，0.20 の `emela lsp`（引数を取らない）は起動に失敗する
- 拡張の README にある補完と `Non-exhaustive match` の波線は 0.1x 系の話で，0.20 のこの段では字句・構文・モジュールと import の診断だけが出る（型検査はまだない）

### Neovim

`editors/nvim` は構文の色付けだけなので，言語サーバーは手で起動する．

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "emela",
  callback = function(args)
    vim.lsp.start({
      name = "emela",
      cmd = { "/path/to/target/debug/emela", "lsp" },
      root_dir = vim.fs.root(args.buf, { "Pome.toml" }),
    })
  end,
})
```

### スクリプトで

標準入出力に `Content-Length` 付きの JSON-RPC を流せばよい．`initialize` → `initialized` → `textDocument/didOpen` を送ると `textDocument/publishDiagnostics` が返る．`shutdown` → `exit` で終了コード 0 で終わる．
