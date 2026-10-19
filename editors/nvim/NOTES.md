# Vim と Neovim の構文の補足

仕様（0.20 草案）と字句解析（`crates/emela-syntax/src/lexer*`）に書かれていない，色付けの判断を残す．どれもエディタの表示だけの判断で，言語の意味は変えない．

# 字句をそのまま写したもの

- （補）名前のクラスは綴りだけで決める（2.3）．どのクラスにも合わない名前（`Http_client`）は Error にする
- （補）数の直後に英字が続くもの（`1e9` `0XFF` `3px`）は Error にする（E0104 と同じ範囲）．`1..2` は数と `..`，`1.x` は数と `.` と名前
- （補）普通の文字列は行末で終える．閉じていない文字列（E0101）が次の行まで色を引きずらないようにするため．補間の中の改行は継続なので，補間が開いている間は行末で終えない
- （補）補間の中のコメントは行末まで続き，`}` も含む（字句解析と同じ）．`"a #{ x # }⏎}"` は2行目の `}` で補間を閉じる
- （補）7つのエスケープと `\u{...}`（1〜6桁）以外の `\` は Error にする．サロゲートや 10FFFF を超える値は桁数しか見ないので，色では分からない
- （補）`as` と 3.3 の将来の予約語は，構文を持たず，書けば必ず構文エラーになるので Error にする．`await` は 3.3 から外れた（`Fiber.await`，`Deferred.await` の関数名）ので普通の名前として扱い，`(` が続けば関数呼び出しの色になる
- （補）宣言のキーワード（`type` `enum` `error` `trait` `effect` `handler` `layer`）と修飾子（`pub` `opaque` `suspend`）は `Keyword` に結び，`fn` と同じ色にする．`Structure` と `StorageClass` は配色によって色が付かない（`handler` が地の色になる）ため．グループ（`emelaStructure` `emelaEffectDecl` `emelaStorageClass`）は分けたままなので，結び直せば色を分けられる
- （補）match は後置（6.5）になったが，`match` は位置によらず予約語なので色付けは変えない．`x match {`，`f(x) match {`，パイプの後ろの `match` をテストで確かめる

# 位置で見分けるもの（正規表現による近似）

- （補）文脈依存の語（3.2）は，それぞれの位置の形で見分ける．`derive` は後ろに型名が続くとき，`fails` は後ろに型名，大文字名，`{` が続くとき，`init` と `release` は後ろに `{` が続くとき．どの形も，普通の名前としては構文エラーになる並びなので，名前として使った `fails = 2` は色が付かない
- （補）型名の直後が `(` なら構成子として色を分ける．型の位置の型引数は `[...]` なので，`(` が続く型名は構成子か宣言（`type Todo(`）のどちらかで，宣言の名前は宣言の色が優先する．`(` の付かない構成子（`None`，`Red`）は型と同じ色になる
- （補）大文字名のうち，1文字（数字が続いてもよい: `A` `T1`）は型引数，2文字以上（`MAX_SIZE`）は const として色を分ける．宣言の直後の `[...]`（`fn map[A, B]`，`type Pair[A, B: Eq]`）では，`[` か `,` の直後の名前を型引数の宣言にする（`:` と `+` の後ろは制約で，普通の型の色）．型名で書いた型引数（`Item`）の使用箇所は型と同じ色になる
- （補）`Some` と `None` は Prelude の構成子として，`True` と `False` は Boolean として，場所によらず色を付ける．`panic` `todo` `dbg` は関数なので特別扱いしない
- （補）`()` は直前が名前でないときだけ Unit とする（`f()` の空の引数は Unit にしない）
- （補）注釈は 3.5 の5つ（`@external` `@test` `@intrinsic` `@internal` `@deprecated`）とそれ以外の `@名前` でグループを分ける．フィールド注釈の名前が未定（18章）なので，それ以外も Error にはしない
- （補）複数行の文字列と補間はファイルの先頭から同期する（`syntax sync fromstart`）

# ftplugin

- （補）インデントは空白2つ（emela fmt の `INDENT`）
- （補）`gf` で import のモジュールのパスを開く．`Http.Client` を `http/client.emel` に，`JsonCodec` を `json_codec.emel` にする（4.1 の逆）．`Http2` のように `_` の後ろの数字はどちらとも取れるので `http2` にする．`Pome.toml` が見つかれば，その隣の `src/` を `path` の先頭に足す

# 能力の値の下線（Neovim）

- （補）能力の値（`x = use X`．8.2）かどうかは名前の解決と型検査が要るので，正規表現の色付けでは扱わない．`emela lsp` の semantic tokens（型 `variable`，独自の修飾子 `capability`）に任せる
- （補）`plugin/emela.vim` で `@lsp.typemod.variable.capability.emela` を `highlight default` で下線にする．配色を読み込むと消えるので，`ColorScheme` の自動コマンドで掛け直す．`default` なので，利用者や配色が先に決めていればそちらが勝つ
- （補）Vim には semantic tokens がないので，`plugin/emela.vim` は Neovim でだけ働く
