# emela-lsp の補足

仕様にない判断を「（補）」として1行ずつ残す．

## 今の段でできること

- `emela lsp` で標準入出力の言語サーバーが起動する（`lsp-server` の同期型）
- 開く，書き換える，保存する，閉じるのたびに，プロジェクト全体を `emela check` と同じフロントエンドで検査し直し，`publishDiagnostics` で送る
- `textDocument/formatting` で文書全体を `emela fmt` と同じく整形する
- `textDocument/hover` でカーソルの下の名前や式の型と E（エラーの集合）を出す（仕様 9.3「E と R は LSP が表示する」）
- `textDocument/definition` で名前の定義へ，`textDocument/references` で名前の参照へ移動する
- `textDocument/documentSymbol` でファイルの中の宣言を入れ子で返す
- `textDocument/documentHighlight` で同じ名前の同一ファイル内の出現を示す
- `workspace/symbol` でワークスペースの宣言を名前の部分一致で探す
- `textDocument/foldingRange` と `textDocument/selectionRange` で構文木から折り畳みと選択の拡大範囲を返す
- `textDocument/typeDefinition` で型の定義へ，`textDocument/implementation` で Trait の明示的な impl とそのメソッドへ移動する
- `textDocument/completion` で文脈に合う名前を出す（`.` で起動する）
- `textDocument/signatureHelp` で呼び出しの型とラベルを見せる（`(` と `,` で起動する）
- `textDocument/rename` と `textDocument/prepareRename` で名前をプロジェクト全体で変える
- `textDocument/inlayHint` で，ソースに書かない型と E，R を行の中に出す（9.3）
- `textDocument/codeAction` で，トップレベルの fn の省いた注釈に推論結果を挿入する（4.5）．診断に結び付く quickfix で，match の不足アーム，impl の不足メソッド，import と fails/use の不足，名前の綴り間違い，前の形の impl と handler の頭（E0144，E0145）を直す
- `textDocument/semanticTokens/full` と `range` で，能力の値（`x = use X`．8.2）に印を付ける

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
- （補）パイプの右辺の呼び出し（`x |> f(a)`）では，シグネチャのヘルプも補完のラベルも左辺が入る第1引数を除く（6.7．判定は driver の `signature.rs`，名前付き引数との対応は driver の NOTES）
- （補）シグネチャのヘルプは1つだけ返す（多重定義はない）．引数の範囲は `ParameterInformation` のラベルの中の UTF-16 の位置．カーソルの引数が引数の数を超えれば `activeParameter` を付けない
- （補）名前の変更は `WorkspaceEdit` の `changes`（URI ごとの編集）で返す．開いていないファイルも同じく編集に入れる．変えられない名前と字句クラスの合わない新しい名前は `RequestFailed` の誤りの応答にし，文面（`this name cannot be renamed`，`` `figure` is not a valid type name ``）をエディタに出す．`prepareRename` は変えられなければ `null`
- （補）省いた注釈を挿入するコードアクションは `refactor.rewrite` のまま診断に結び付けない（省いた注釈の linter の警告はまだない）．診断を直すものは `quickfix` で，`diagnostics` に対象診断を載せる．編集は `WorkspaceEdit.changes` でそのファイルだけ
- （補）quickfix は要求のたびに検査したスナップショットから作り，`context.diagnostics` には依存しない．診断の E 番号とファイル・主位置の範囲で選び，code のない診断は扱わない．空の要求範囲はカーソルを含む診断（終端を除く）を選び，`context.only` があれば種類を絞る
- （補）E0303 の quickfix は網羅性検査が示した不足パターンの例（6.5）を末尾に挿入し，本体は Prelude の `todo()`（Never）にする．構成子は必要なら import 済みモジュールで修飾する．名前を一意に描画できない場合は `_ -> todo()`，例の件数が上限に達した場合は末尾に同じ wildcard を補う
- （補）impl の default のない不足メソッドは，共通の型検査フロントエンドで E0325 として全件報告する（10.2）．主位置は `impl` キーワードから Trait の名前の末尾までのヘッダー（`impl User: Show`．本体の `{` とその前の空白を除く）とし，quickfix の `diagnostics` に E0325 を載せ，宣言順に末尾へ一括挿入する．カーソルがヘッダー内にあるときだけ提示し，`{` や本体内では提示しない（要求範囲が空でなければ，診断範囲との重なりで選ぶ）．ヘッダーの `refactor.rewrite` は同じ編集と重複するので廃止し，quickfix に一本化する．引数名だけを trait から取り，型・fails・use は書かない．本体は既存の仮実装に倣い `todo()` にする
- （補）E0211 の import 候補はモジュール表と検査済みの公開定義から選ぶ．`Io.print` などの修飾名には `import Io`，未 import の effect などには `import Std.Console.{Console}` を足す．effect の操作を `Effect.op` で呼べるようにはしない（4.6，8.2）．候補ごとに別のアクションを返す
- （補）E0318/E0402 の quickfix は，関数本体の E/R と書いた節の和を既存の注釈挿入と同じ規則で描画する．既存の要素は消さず，見えない集合や修飾なしで書けない名前は省いた注釈の挿入と同じく扱う．E0402 の main・test・const・handler 操作には伝播する use を提案しない
- （補）E0211/E0212 の名前置換は，その位置の補完候補から予約語とラベル，大文字小文字のクラスの違う名前を除き，resolver と同じ編集距離（名前の長さの3分の1以内，最低1）で最も近いものを選ぶ．同距離なら内側のスコープを優先する
- （補）E0144 の quickfix は頭（`impl` から頭の終わりまで）を書き換えた頭で置き換え，E0145 の quickfix は名前（フィールドがあればその並び）の後ろから `implements` までを `:` に置き換える．書き換えは emela-syntax の `rewrite` モジュールが診断の文面と共通で作る
- 未使用 import の診断はまだないので，削除する quickfix は出さない
- （補）インレイヒントの種類は，型と省いた注釈を `Type`，呼び出しの E と R を `Parameter` にした．型（`: Int`）は名前に続けて出し，ほかは左に空白を入れる．範囲の終わりがファイルの外なら，ファイルの終わりまでとみなす
- （補）semantic tokens は能力の値だけを返す．束縛した名前と，それを参照する名前（クロージャの中も）の両方．関数や型などほかのトークンは返さない（エディタの正規表現の色付けと二重にならないように）．判定は driver の `Frontend::semantic_tokens`
- （補）legend は型が `variable` の1つ，修飾子が独自の `capability` の1つ．Neovim では `@lsp.typemod.variable.capability.emela` で色を付けられる（`editors/nvim` は下線を既定にする）
- （補）`full` と `range` に答え，差分（`full/delta`）には答えない．`resultId` は付けない．範囲の終わりがファイルの外なら，ファイルの終わりまでとみなす（インレイヒントと同じ）．検査できないファイルは `null`
- （補）整形の `options`（`tabSize`，`insertSpaces`）は見ない．字下げは空白2つ，幅は 100 のまま（仕様の整形の規則に揃える）
- （補）`publishDiagnostics` の `version` は，開いているファイルなら最後に受けた版，開いていなければ付けない
- （補）毎回の解析で，診断のあるファイルは全部送り直す（変わっていなくても）．前回に送って今回ないファイルには空の配列を送る
- （補）前回に送ったかどうかはパスで比べる．同じファイルで URI の書き方が変わった（開いてエディタの URI になった，閉じてサーバーの URI に戻った）ときは，古い URI に空の配列を送る．空の配列は新しい診断より先に送る（エディタが2つの書き方を同じファイルとみなしても，新しい診断が残るように）
- （補）documentHighlight は参照の検索を同一ファイルに絞り，定義の名前も含める．変数は不変（6.3）なので種類はすべて Text．名前がなければ空配列
- （補）workspace/symbol は workspaceFolders（または rootUri）のプロジェクトと開いたファイルのプロジェクトを検査し，documentSymbol の入れ子を平らにして返す．未開封のモジュールも含め，依存 Pome と同梱の core/std は含めない．単独ファイルは import でたどれる範囲だけ．query は名前の大文字小文字を無視した部分一致（空なら全件），同じ位置は重複させず，containerName は親の宣言名かモジュール名
- （補）foldingRange と selectionRange は検査せず，未保存の内容の構文木から作るので，構文・import・型の誤りがあっても動く．読めない文書は null
- （補）foldingRange は複数行のブロック，match/escape の腕，enum のバリアント，type/error/handler のフィールド，trait/impl/effect/handler/layer の本体と，隣接行の import・コメント（# と ## を区別しない）の連続を返す．空行や間のコードで連続を切り，1行だけの範囲は返さない．種類は region/imports/comment，範囲の終わりは閉じ括弧の直後
- （補）selectionRange は各位置についてトークンから親の構文ノード，ファイル全体までの鎖を返し，同じ範囲は重ねない．トークンの境界では右側を優先するが，片側だけが空白やコメントなら他方を選ぶ．文書の外の位置を含む要求は null
- （補）typeDefinition は変数・式の最外の名前付き型（型引数はたどらない）と能力のエフェクトの定義へ移動する．型・Trait 自体の名前はその定義へ移動する．組み込み・構造型・未決定型・同梱 core/std など移動先がなければ null．既存の検査器が型を記録しない Trait の impl 本体も，型からは移動先を求めない
- （補）implementation は Trait の名前なら明示的な impl の全体，Trait のメソッドなら対応する impl のメソッド名を返す．既定のメソッドを省いた impl はブロック全体を指す．derive や組み込みの実装は返さず，該当なしは空配列
- （補）documentHighlight・typeDefinition・implementation は hover と同じく要求ごとに検査し直し，誤りがあっても残った名前解決・型検査結果から答える

- （補）`##`（2.2）は公開・非公開を問わず定義の名前の位置で引き，ホバーでは型のコードブロックの後に水平線で区切った Markdown として出す．型・trait と，型検査がまだ型を持たないメソッドには宣言の表示を補う．impl・handler の実装の名前では，宣言側ではなく実装自身の doc を出す
- （補）名前のない impl と handler の init・release の doc は，ホバーでそのヘッダーにだけ出す．本体の式や束縛には引き継がない
- （補）シグネチャのヘルプと補完の `documentation` は Markdown の `MarkupContent`．引数説明の規約はまだないので関数の doc を引数ごとに分割しない．構成子の名前付き引数の補完には，対応するフィールドの doc を出す
- （補）doc は解析に使ったソース（未保存の内容・依存 Pome・同梱の core/std を含む）を再パースし，定義の名前の span と結び付ける．DefId のないフィールドと impl・handler のメソッドも名前の span で引く．`emela doc` と抽出を共用するが，そちらは従来どおり pub だけを出す
- （補）補完の doc は最初の応答に含め，`completionItem/resolve` は増やさない．1つの要求の中では参照したモジュールを1回だけ再パースして使い回し，編集をまたぐキャッシュは持たない
- （補）doc の行から `##` と直後の空白1つだけを取り除き，改行とそれ以上の字下げを残す．空か空白だけの doc は LSP に付けず，doc のない候補・局所変数・予約語に別の定義の doc を引き継がせない

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
