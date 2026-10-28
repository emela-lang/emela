# リリース手順

リリースは2系統ある。

- 正式版（`vX.Y.Z`）: `main` で release-plz が自動化する。設定は `release-plz.toml`、
  動作は `.github/workflows/release-plz.yml`。
- プレリリース（`vX.Y.Z-<pre>`、例 `v0.26.0-alpha.1`）: タグを手で打つと
  `.github/workflows/prerelease.yml` が動く。

どちらも配布物は `emela` バイナリの tarball
`emela-<tag>-<target>.tar.gz`（`x86_64-unknown-linux-gnu` と `aarch64-apple-darwin`）
で、`install.sh` がこの名前を引く。

## 正式版（release-plz）

### セットアップ（初回のみ）

`release-plz.yml` の `notify-nix` は GitHub App のトークンで emela.nix に
`repository_dispatch` を送る。

1. GitHub App を作る（Settings → Developer settings → GitHub Apps → New）。
   - Repository permissions で **Contents: Read and write** を付与（`repository_dispatch` に要る）。
   - Webhook は無効でよい。
2. App の設定画面で **Private key** を生成し（Generate a private key）、
   ダウンロードした `.pem` を控える。**App ID** もメモする。
3. App を emela.nix に Install する（Install App）。
4. このリポジトリに Secret を2つ登録する
   （Settings → Secrets and variables → Actions → New repository secret）。
   - `APP_ID`: App ID
   - `APP_PRIVATE_KEY`: `.pem` の中身を `-----BEGIN` から `-----END` まで
     そのまま貼る。

### 手順（毎回）

1. `main` にマージする。コミット/PR タイトルは conventional commits
   （`feat:` `fix:` など）に従う。次バージョンはこの履歴から決まる
   （0.y 系では `feat:`→minor、`fix:`→patch）。
2. `main` への push で release-plz が `release/vX.Y.Z` PR を自動で開く。
   中身は Cargo.toml/Cargo.lock の bump と CHANGELOG の追記。内容を確認する。
3. その `release/vX.Y.Z` PR をマージする。
4. `release-plz.yml` が続けて動く。
   1. release-plz が `vX.Y.Z` タグを打ち、下書きの GitHub Release を作る
      （タグを打つのは `emela-cli` パッケージだけ）。
   2. `build` が Linux と macOS で tarball を作り、Release に載せる。
   3. `publish-release` が下書きを公開する。
   4. `notify-nix` が emela.nix に新しい版を知らせる。

## プレリリース（prerelease.yml）

`prerelease.yml` はプレリリースのタグ（`v*-*`，例 `v0.26.0-alpha.1`）の push で動く。
ハイフンのない `vX.Y.Z` では動かないので、release-plz の正式版と二重にならない。
`workflow_dispatch`（入力にタグ名）は、既存のタグで作り直すための予備。

ワークフローがやること:

1. タグ（先頭の `v` を除く）と `emela-cli` の version（workspace の version）が
   一致するか確かめる。違えば失敗する。ビルド後に `emela --version` も確かめる。
2. Linux（`x86_64-unknown-linux-gnu`）と macOS（`aarch64-apple-darwin`）で
   `cargo build --release --bin emela` を走らせ、tarball に固める。
3. GitHub Release を prerelease として作り（本文は `--generate-notes`）、
   2つの tarball を載せる。同じタグの Release が既にあれば tarball を差し替える。

emela.nix には知らせない（正式版だけ）。

### 出し方

1. version を上げる PR を出す。`Cargo.toml` の `[workspace.package]` の `version` と
   `[workspace.dependencies]` の `emela-*` の `version` を `X.Y.Z-alpha.N` にし、
   `cargo build` で `Cargo.lock` も更新する。
2. その PR をマージし、マージしたコミットでタグを打って push する。

   ```sh
   git tag vX.Y.Z-alpha.N
   git push origin vX.Y.Z-alpha.N
   ```

3. `prerelease.yml` が prerelease を作る。Actions の実行と Releases を確認する。

タグと version がずれていたら、ワークフローはビルドの前に失敗する。そのときは
タグを消して（`git push origin :refs/tags/vX.Y.Z-alpha.N`）、version を直してから
打ち直す。

## インストール

```sh
# 最新の正式版（prerelease は選ばれない）
curl -fsSL https://emela.dev/install.sh | sh

# 版を指定する（prerelease も可、先頭の v は省略可）
curl -fsSL https://emela.dev/install.sh | EMELA_VERSION=v0.24.0 sh
```

`EMELA_CHANNEL=nightly` は最新の prerelease を選ぶ。

## 補足

- crates.io へは publish しない（`release-plz.toml` の `publish = false`）。配布物は
  `emela` バイナリの tarball のみ。Cargo.toml には `publish = false` を書かない。
  書くと release-plz がそのパッケージを丸ごと飛ばし、`emela-cli` のタグも打たれない。
- release-plz（`git_only`）は最新の正式版のタグのコミットで `cargo package` を走らせる。
  そのため workspace の内部の依存には `version` を付け、循環になる dev 依存
  （emela-types → emela-core）は path だけにする。
- 全部のクレートが `release-plz.toml` で同じ `version_group` に入っている。release-plz は
  コミットが触ったディレクトリのクレートだけを「変更あり」と見るので、これがないと
  `emela-cli` 以外だけを触ったコミットでは version が上がらず、タグも出ない。
  CHANGELOG は `emela-cli` の `changelog_include` でほかのクレートのコミットを拾う。
- 前のリリースは、どのクレートも `vX.Y.Z` のタグで探す（workspace の `git_tag_name`）。
  打つのは `emela-cli` だけ（`git_tag_enable`）。
- release PR の version を手で変えるとき（例: 0.21.0 を 0.20.1 にする）は、PR のブランチで
  次の3つをそろえる。マージすると release-plz はその version のタグを打つ。
  1. Cargo.toml の `[workspace.package]` の `version` と、`[workspace.dependencies]` の
     `emela-*` の `version` すべて
  2. Cargo.lock の `emela-*` すべての `version`（`cargo metadata --locked` が通ること）
  3. CHANGELOG.md の見出し `## [0.20.1](…/compare/v0.20.0...v0.20.1)`
  直した後にほかの PR が `main` に入ると、release-plz は手のコミットが入った PR を閉じて、
  release PR を開き直す（version も元に戻る）。直したらすぐマージする。
- `release-plz-pr` は `release-plz-release` の後に動く。version を手で上げて `main` に
  入れたときに、先にタグを打たせるため。
- 新しいクレートを足すときは `release-plz.toml` の `[[package]]` に `version_group = "emela"`
  で加え、`emela-cli` の `changelog_include` にも足す。最初のリリースのタグにそのクレートが
  入るまでは `git_tag_name = "<crate>-v{{ version }}"` を付ける（理由は `release-plz.toml` の
  コメント）。タグが打たれたら外す。
