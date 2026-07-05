# リリース手順（release-plz）

安定版リリースは release-plz が自動化する。設定は `release-plz.toml`、動作は
`.github/workflows/release-plz.yml`。`dev` の nightly（`nightly.yml`）と
タグ起点の tarball リリース（`release.yml`）はそのまま。

## セットアップ（初回のみ）

1. `RELEASE_PLZ_TOKEN` を Secret 登録する。
   デフォルトの `GITHUB_TOKEN` では release-plz が打ったタグが `release.yml` を
   起動できないため、GitHub App トークンか PAT を使う。
   - 権限: `contents: write` と `pull-requests: write`
   - 登録先: Settings → Secrets and variables → Actions → New repository secret
2. `release-plz.toml` をリポジトリルートに置く。
3. `.github/workflows/release-plz.yml` を置く。

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
