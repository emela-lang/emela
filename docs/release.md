# リリース手順（release-plz）

安定版リリースは release-plz が自動化する。設定は `release-plz.toml`、動作は
`.github/workflows/release-plz.yml`。`dev` の nightly（`nightly.yml`）と
タグ起点の tarball リリース（`release.yml`）はそのまま。

## セットアップ（初回のみ）

デフォルトの `GITHUB_TOKEN` では release-plz が打ったタグが `release.yml` を
起動できない（GITHUB_TOKEN 由来のイベントは他ワークフローを起こさない）。
そのため GitHub App で認証し、実行ごとに一時トークンを発行する。

1. GitHub App を作る（Settings → Developer settings → GitHub Apps → New）。
   - Repository permissions で **Contents: Read and write** と
     **Pull requests: Read and write** を付与（タグ保護を使う場合のみ
     Administration: Read and write も）。
   - Webhook は無効でよい。
2. App の設定画面で **Private key** を生成し（Generate a private key）、
   ダウンロードした `.pem` を控える。**App ID** もメモする。
3. App を **このリポジトリに Install** する（Install App）。
4. リポジトリに Secret を2つ登録する
   （Settings → Secrets and variables → Actions → New repository secret）。
   - `APP_ID`: App ID
   - `APP_PRIVATE_KEY`: `.pem` の中身を `-----BEGIN` から `-----END` まで
     そのまま貼る。
   ワークフローは各実行で `actions/create-github-app-token` を使い、この2つから
   一時トークンを発行して release-plz に渡す。
5. `release-plz.toml` と `.github/workflows/release-plz.yml` をリポジトリに置く。

## リリース手順（毎回）

1. `dev` で開発する。push すると `nightly.yml` が `x.y.z-dev.<TZ>` を発行する。
2. リリースする区切りで `dev` → `main` を PR にしてマージする。
   コミット/PR タイトルは conventional commits（`feat:` `fix:` など）に従う。
   次バージョンはこの履歴から決まる（0.y 系では `feat:`→minor、`fix:`→patch）。
3. `main` への push で release-plz が `release/vX.Y.Z` PR を自動で開く。
   中身は Cargo.toml/Cargo.lock の bump と CHANGELOG の追記。内容を確認する。
4. その `release/vX.Y.Z` PR をマージする。
5. release-plz が `vX.Y.Z` タグを打ち、`release.yml` が tarball 付きの
   GitHub Release を発行する。

## 補足

- crates.io へは publish しない（`publish = false`）。配布物は `emela` バイナリの
  tarball のみ。
- GitHub Release は `release.yml` が作る。release-plz は作らない
  （`git_release_enable = false`）。
- リリース後、`main` の bump を `dev` に取り込むため `main` → `dev` をマージする
  （`dev` と `main` のバージョンを揃える）。
</parameter>
</invoke>
