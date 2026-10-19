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

## 取得（`fetch`）

- （補）キャッシュの並び: `<cache>/<エスケープした source path>@<version>/` に中身，`<cache>/download/<エスケープした source path>/@v/<version>.toml` に確かめた `Pome.toml`，`<cache>/sumdb/<鍵の名前>/latest` に見た最大の checkpoint（署名付き）．書くときは一時的な場所に書いてから rename する
- （補）中身のディレクトリはファイルを 0444，ディレクトリを 0555 にする（4.7 の「読み取り専用」）
- （補）`Pome.toml` だけを取るとき（MVS で依存の木をたどるとき）も sumdb に照らす．照らすのは記録の2行目（`Pome.toml` の `h1:`）
- （補）`direct` は利用者の git の設定（資格情報，`url.<base>.insteadOf`）をそのまま使う．`EMELA_POMEPRIVATE` の非公開のリポジトリを取れるようにするため．ingester とは違い，プロトコルも絞らない
- （補）`direct` の `@latest` は，prerelease でない最大の版（なければ prerelease を含めた最大）．proxy と違い，取り込めない版を飛ばさない（取り込みの検査を手元でしないため）
- （補）proxy の URL の version はエスケープしない（pome-registry と同じ）
- （補）`direct` でシンボリックリンクや submodule を含む版は「ない」（NotFound）として扱う．proxy が 410 を返すのと揃える
- `EMELA_POMESUMDB` の既定（`config::DEFAULT_SUMDB`）は `sum.emela.ab2m.link` の公開鍵と URL `https://sum.emela.dev`．ホストを `emela.dev` に移したときも鍵は替えなかったので，鍵の名前とホスト名が違う．鍵を替えると，古い鍵を埋め込んだコンパイラはすべて検証に落ちる
- tile はディスクにキャッシュしない．1回の証明で読むのは数枚なので，まず足りる

## emela pome のコマンド（`project`）

- （補）どのコマンドも，新しい `Pome.toml` と lock を先に作り，依存を全部解決できてから2つを書く．`add` などが途中で失敗したときは，どちらのファイルも変わらない
- （補）`Pome.toml` は toml_edit で書き換え，コメント，並び，表の形（`{ version, module }`）を残す
- （補）`add <src>@<version>` は今の要求より小さい version も書く（下げられる）．`@latest` と version の省略は proxy の `@latest`
- （補）`update` は要求を上げるだけで，下げない．`[replace]` した依存の要求も上げる（`[replace]` はそのまま）
- （補）`remove` は `[dependencies]` か `[dev-dependencies]` から除き，`[replace]` は残す
- （補）`install` は，lock にある版を lock のハッシュで確かめて取り（sumdb に問い合わせない），lock にない版だけ sumdb と照らす．依存の木をたどるときの `Pome.toml` も同じ
- （補）`[replace]` した Pome の依存は，置き換え先のディレクトリの `Pome.toml` から読む．その `[pome].name` が置き換えた source path と違えばエラー
- （補）`list` の1行は「source path と要求」で，選ばれた版が大きければ `-> <版>`，置き換えなら `=> <ディレクトリ>` を足す．循環はたどらない
- （補）`verify` はキャッシュにある依存だけを照らす．ない依存は問題にしない

## リポジトリの中のディレクトリの Pome

- github.com の source path は `github.com/<owner>/<repo>` までがリポジトリ，その後ろ（`/vN` を除く）がディレクトリ（4.7）．ほかの host は `/vN` を外した全体がリポジトリ
- ディレクトリの Pome の tag は `<ディレクトリ>/<version>`．`version_of_tag` は，ディレクトリが合わない tag と major が合わない tag を除く
- （補）`direct` はディレクトリの tree（`<commit>:<ディレクトリ>`）を読む．tag の commit にディレクトリがなければ「ない」（NotFound）
- （補）ルート名の既定は，リポジトリでなく Pome の場所の最後の区切りから作る（`.../x/platform-cloudflare` は `PlatformCloudflare`）

## @external（4.7，付録 B）

- （補）`@external` を含むファイルは，`test/` の下を除く `.emel` のうち，行の先頭（空白を除く）が `@external` の行を持つもの．構文木は作らずに行で調べる（`#` で始まる注釈の行は数えない）
- （補）lock の `external` は，lock にある版を使い回すときも中身から数え直す．このフィールドを持たない古い lock から `install` しても正しい一覧になる
- （補）`add --allow-external` が許すのは，足す Pome と，この `add` で新しく lock に入った Pome のうち `@external` を使うもの．前から lock にあって許されていない Pome は，許さずにエラーのまま（`[external] allow` に手で足す）
- （補）許されていない Pome があれば，`add`，`install`，`update`，`remove` のどれも何も書かずに止まる．`update` で `@external` のファイルが増えても，許しは source path に出しているので止めずに出力で知らせる

## テストの依存（4.7）

- `[dev-dependencies]` は依存されたときに読まない．依存の木をたどるとき（`requirements_of`）は `[dependencies]` だけを返す
- MVS は `[dependencies]` と `[dev-dependencies]` を合わせて1回だけ解く．テストの依存が上げた版は通常のビルドも使う
- （補）lock の `dev = true` は，「`[dependencies]` だけの build list にない」かつ「`[dependencies]` から，選ばれた版の要求をたどって届かない」Pome に付ける．テストの依存が共通の依存の版を上げ，その版が新しい Pome を要求したとき，その Pome は通常のビルドにも要るので `dev` にしない（`mvs::reached`）
- （補）`add --dev` は `[dependencies]` にある Pome を指すとエラー．`--dev` のない `add` が `[dev-dependencies]` にある Pome を指すと，書いた形のまま `[dependencies]` へ移してから version を書く
- （補）`[dev-dependencies]` が空になったら表ごと消す．`[dependencies]` は空でも残す
- （補）`update` は両方の表の要求を上げる．`list` は `[dev-dependencies]` と lock の `dev` の Pome の行に `(dev)` を付ける
