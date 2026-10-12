# emela-lsp の補足

仕様にない判断を「（補）」として1行ずつ残す．

## 今の段でできること

- `emela lsp` で標準入出力の言語サーバーが起動する（`lsp-server` の同期型）
- 開く，書き換える，保存する，閉じるのたびに，プロジェクト全体を `emela check` と同じフロントエンドで検査し直し，`publishDiagnostics` で送る
- `textDocument/formatting` で文書全体を `emela fmt` と同じく整形する
- `textDocument/hover` でカーソルの下の名前や式の型と E（エラーの集合）を出す（仕様 9.3「E と R は LSP が表示する」）
- `textDocument/definition` で名前の定義へ，`textDocument/references` で名前の参照へ移動する
- `textDocument/documentSymbol` でファイルの中の宣言を入れ子で返す
- `textDocument/completion` で文脈に合う名前を出す（`.` で起動する）
- `textDocument/signatureHelp` で呼び出しの型とラベルを見せる（`(` と `,` で起動する）
- `textDocument/rename` と `textDocument/prepareRename` で名前をプロジェクト全体で変える
- `textDocument/inlayHint` で，ソースに書かない型と E，R を行の中に出す（9.3）
- `textDocument/codeAction` で，トップレベルの fn の省いた注釈に推論結果を挿入する（4.5）

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
- （補）整形は driver の `format_source`．開いている文書は未保存の内容を，開いていなければディスクの内容を整形する．文書全体を置き換える `TextEdit` を1つ返し，整っていれば空の配列を返す
- （補）整形できない文書（構文エラー，formatter の検証の失敗，読めないファイル，`file:` でない URI）は `null` を返し，誤りの応答にはしない．構文エラーは publishDiagnostics で出ている
- （補）ホバーは要求のたびに，そのファイルを含むプロジェクト（単独ファイルならそのファイル）を検査し直して引く．診断の解析の結果は持ち回らない．位置を引くのは driver の `Frontend::hover`（`Typecheck` だけが返す．型を持たないフロントエンドでは `null`）
- （補）ホバーの中身は Markdown の `emela` のコードブロック．定義の名前と定義の参照は `名前: 型`（宣言の型．推論したものは汎化した型），局所変数（束縛した位置と参照）は `名前: 型`，それ以外の式は型と，E が空でなければ次の行に `fails E`．位置を含むもののうち一番内側（範囲の短いもの）を選ぶ．範囲の終わりは含めない
- （補）定義へ移動と参照の検索は，ホバーと同じく要求のたびに検査し直して引く（driver の `Frontend::definition` と `Frontend::references`）．定義へ移動は位置を1つ（`Location`）返し，移動先がなければ `null`
- （補）同梱の core の定義（`List.map` など）は仮のパス（`<core>/list.emel`）にしかないので，定義へ移動は `null` にする．参照の検索も core の中の参照を返さない
- （補）参照の検索は検査したプロジェクトの中だけ（単独ファイルならエントリから import でたどれるファイル）．参照が1つもなければ `null`．`includeDeclaration` が立っていれば定義の名前も含める
- （補）文書のシンボルは検査せず，開いていれば未保存の内容を，開いていなければディスクの内容を構文解析して作る（`DocumentSymbol` の入れ子）．読めないファイルは `null`
- （補）シンボルの種類は fn → Function，type と error → Struct，enum → Enum，バリアント → EnumMember，const → Constant，effect と trait → Interface，effect の操作と trait・impl・handler の fn → Method，handler → Class，layer と impl → Namespace，フィールド → Field
- （補）補完はホバーと同じく要求のたびに検査し直して引く（driver の `Frontend::complete`）．書きかけの名前での絞り込みはエディタに任せ，候補を全部返す．並びは driver の順で，`sortText` に連番を入れて保つ．起動の文字は `.` だけ
- （補）補完の種類は局所変数 → Variable，fn → Function，const → Constant，構成子 → Constructor，type と error → Struct，enum → Enum，effect と trait → Interface，handler と layer → Class，effect の操作と trait の関数 → Method，フィールド → Field，モジュール → Module，予約語 → Keyword，名前付き引数のラベル → Property（`insertText` に `x: ` を入れる）
- （補）シグネチャのヘルプは1つだけ返す（多重定義はない）．引数の範囲は `ParameterInformation` のラベルの中の UTF-16 の位置．カーソルの引数が引数の数を超えれば `activeParameter` を付けない
- （補）名前の変更は `WorkspaceEdit` の `changes`（URI ごとの編集）で返す．開いていないファイルも同じく編集に入れる．変えられない名前と字句クラスの合わない新しい名前は `RequestFailed` の誤りの応答にし，文面（`this name cannot be renamed`，`` `figure` is not a valid type name ``）をエディタに出す．`prepareRename` は変えられなければ `null`
- （補）コードアクションの種類は `refactor.rewrite`．診断に結び付けない（省いた注釈の linter の警告はまだない）．編集は `WorkspaceEdit` の `changes` で，そのファイルだけ
- （補）インレイヒントの種類は，型と省いた注釈を `Type`，呼び出しの E と R を `Parameter` にした．型（`: Int`）は名前に続けて出し，ほかは左に空白を入れる．範囲の終わりがファイルの外なら，ファイルの終わりまでとみなす
- （補）整形の `options`（`tabSize`，`insertSpaces`）は見ない．字下げは空白2つ，幅は 100 のまま（仕様の整形の規則に揃える）
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
