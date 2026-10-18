# emela-pome の補足

仕様にない判断を「（補）」として1行ずつ残す．

## Pome.lock

- （補）形式は TOML で，`version = 1` と `[[pome]]` の表の列．表は `path`，`version`，`commit`（40桁の16進），`hash`（中身の `h1:`），`manifest`（`Pome.toml` の `h1:`）を持つ．4.7 は書く項目だけを決め，形式を決めていないため
- （補）lock には依存の木の全部（MVS が選んだ build list）を書く．直接の依存だけでは，推移的な依存を取ってハッシュを確かめられないため
- （補）表は source path の順に並べる．同じ source path は1つだけ（`/vN` の違う Pome は別の source path なので並べてよい）

## Pome.toml

- （補）`[pome].version` の major は `[pome].name` の `/vN` と合わなければならない．`[dependencies]` の version の major も鍵の `/vN` と合わなければならない．4.7 の「合わない tag はその Pome の版とみなさない」を，ファイルの段で先に誤りにする
- （補）`[build]` はこの crate では読まない．`[build].target` は emela-driver が読む（E0232）
- `[pome].emela` は書かれたままの文字列で持つ．コンパイラの版との比較は，依存を読む段（driver）で行う

## sumdb の検証

- （補）inclusion proof と consistency proof を受け取らず，tile から署名済みの checkpoint の根を組み立て直して比べる．tile の中身は，組み立てた根が署名済みの根と一致して初めて信じる
  - inclusion: 記録の位置の葉を記録から計算し直した木の根
  - consistency: 新しい木の tile から，古い大きさの根と新しい大きさの根の両方を作る
- （補）完全な部分木の hash は，高さ 8L の節を 256 個ずつ並べた tile の1つから畳んで作る．1回の証明で読む tile は level ごとに1〜2枚
- 署名の検証は `verify_strict`（Ed25519 の正規でない署名と弱い鍵を拒む）．sumdb の署名は正規なので通る

## テストの値

- `tests/data/hash-vectors.json` と `tests/data/tlog-vectors.json` は pome-registry（`packages/protocol/testdata`）と同じもの．TypeScript の実装と同じ値を通す
