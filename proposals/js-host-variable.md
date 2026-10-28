---
title: Choose the JS host with EMELA_JS_HOST
authors: [Alliana]
status: draft
pr: 472
issue:
decided:
touches: [cmd.build, cmd.test.out-dir, diag.list]
requires: []
related: []
---

# Choose the JS host with EMELA_JS_HOST

## Summary

`emela run` と `emela test` が JS の出力を走らせる host を，環境変数 `EMELA_JS_HOST` で選べるようにする．値は command で，その basename から起動の仕方を決める．`node` と `bun` は今と同じ起動の仕方，`deno` は `deno eval` と `deno run -A` を使う．`EMELA_NODE` は deprecated にし，1 minor version の間だけ警告付きで読む．

## Motivation

- JS の target は Node，Deno，Bun を host として想定し（`[target]`），三つで同じ結果になることを確かめている（`[target.js.host]`）．ところが host を選ぶ変数の名前は `EMELA_NODE` で，Node 以外を指すと名前と中身が食い違う
- x/platform-bun のように，Bun の組み込み機能（`bun:sqlite`）を叩く Pome が出てきた．そのテストは `EMELA_NODE=bun emela test` で走らせている．読んだ人には Node を指しているように見える
- `EMELA_NODE` は「`<値> --input-type=module -e <launcher> -- <entry>` で起動できる command」という前提で使われている．Node と Bun はこの形で動くが，Deno は動かない（`deno` は `-e` を受け付けず，`eval` か `run` が要る）．名前を変えるだけでなく，host ごとに起動の仕方を変える必要がある

他の道具の変数（`EMELA_CC`，`EMELA_WASMTIME`）は道具の名前を付けている．あれは使える道具が 1 つしかないので成り立つ．JS は host を選べるので，役割の名前にする．名前に host を使うのは，仕様が JS を動かすものを一貫して host と呼んでいるため（`[target.js.host]`，host functions）．`RUNTIME` は，emela が出力する `emela_runtime.mjs` と紛らわしい．

## Proposal

### Meaning

- JS の出力を走らせる command は，`EMELA_JS_HOST` が空でなければその値，そうでなく `EMELA_NODE` が空でなければその値，どちらもなければ `node` にする
- command の basename（path の最後の部分．Windows では `.exe` を除く）で起動の仕方を決める

| basename | `emela run`（main） | `emela run`（HTTP サービス） | `emela test` |
|---|---|---|---|
| `deno` | `deno eval --ext=mjs <launcher> -- <entry> <args>` | `deno run -A serve.mjs <args>` | `deno run -A emela_test.mjs` |
| それ以外（`node`，`bun` を含む） | `<cmd> --input-type=module -e <launcher> -- <entry> <args>` | `<cmd> serve.mjs <args>` | `<cmd> emela_test.mjs` |

- それ以外の basename を Node と同じ起動の仕方にするのは，`node22` のような名前の wrapper を今までどおり使えるようにするため
- Deno に `-A`（すべての権限）を付けるのは，Node と Bun が権限の仕組みを持たず，同じ program が同じように動くことを優先するため．`deno eval` は既定ですべての権限を持つ
- `EMELA_NODE` を読んだときは警告 W0902 を出す．`EMELA_JS_HOST` もあるときは `EMELA_JS_HOST` を使い，同じ警告を出す
- `EMELA_NODE` は次の minor version まで読む．その次の minor version で読まなくなる

試作：今の launcher を `deno eval --ext=mjs` で走らせ，suspend する main（`Clock.sleep` の後に出力）が Deno 2.9.7，Node 22.23.4 で同じ結果になった．Deno の `eval` では `process.argv[1]` が eval 用の file になり，entry は `process.argv[2]` に来る．launcher はこの違いを吸収する．`emela test` が書く `emela_test.mjs` は，`deno run -A` でそのまま動いた（x/kv の 4 テスト）．

## Specification changes

### `[cmd.build]`

Replace "The node used is the environment variable `EMELA_NODE` if set, and otherwise `node` on PATH." with:

- The JS host is the command in the environment variable `EMELA_JS_HOST` if it is set and not empty, otherwise the command in `EMELA_NODE` if it is set and not empty, otherwise `node` on PATH.
- The basename of the command (the last component of its path, without `.exe` on Windows) decides how a program is started. With `deno`, `run` starts main as `deno eval --ext=mjs <launcher> -- <entry> <args>`, and an HTTP service as `deno run -A serve.mjs <args>`. With any other basename, including `node` and `bun`, `run` starts main as `<command> --input-type=module -e <launcher> -- <entry> <args>`, and an HTTP service as `<command> serve.mjs <args>`.
- `EMELA_NODE` is deprecated. When it is read, W0902 is reported, also when `EMELA_JS_HOST` is set and takes precedence. It is read until the minor version after the one that deprecates it.

In "Running the program (JS)", replace "node" with "the JS host" in "For an HTTP service, `run` runs `node serve.mjs` ..." and "Arguments to the program are passed after `--` as node arguments", and in the description of `<command>` ("node with its options for `js`" becomes "the JS host with its options for `js`"). The example ``running `node --input-type=module -e <launcher> -- target/emela/js/main.mjs` `` stays, as the line for `node`.

### `[cmd.test.out-dir]`

Replace "`emela test` runs the launcher module with node." with: "`emela test` runs the launcher module with the JS host (`[cmd.build]`): `deno run -A emela_test.mjs` with `deno`, and `<command> emela_test.mjs` otherwise."

### `[diag.list]`

- Change E0904 to "The JS host is not found or cannot be started", with the example ``cannot start `node` `` kept. The note becomes ``= note: install Node.js and add it to PATH, or set EMELA_JS_HOST to the command of a JS host``.
- Add W0902: "`EMELA_NODE` is set (Appendix B)". Message: ``EMELA_NODE is deprecated; use EMELA_JS_HOST``.

## Diagnostics

- [ ] W0902（新規）：`EMELA_NODE` を読んだ
- [ ] E0904 の意味と note を JS host に合わせて直す

## Compatibility

- `EMELA_NODE` だけを設定している環境は，1 minor version の間，警告が出るだけで同じように動く
- `EMELA_NODE=deno` は今は起動に失敗するが，`deno` の起動の仕方になるので動くようになる
- compiler の tests（`crates/emela-driver/tests/wasm_*.rs`，`crates/emela-codegen-wasm/tests/unicode.rs`，`crates/emela-cli/tests/`）が `EMELA_NODE` を読んでいる．`EMELA_JS_HOST` に移す

## Implementation

- emela-driver：`NODE_ENV` を `EMELA_JS_HOST` と `EMELA_NODE` の 2 つにし，basename から起動の仕方（Node 形式か Deno 形式か）を決める．`node_command`，`node_script_command` と test の起動を，その起動の仕方で組み立てる．launcher は entry を `process.argv[1]` でなく，Deno の上では `process.argv[2]` から読む
- emela-cli：W0902 を出す．E0904 の note を直す
- CI：Deno と Bun でも `emela run` と `emela test` を走らせる job を足すかを決める

## Alternatives

- `EMELA_JS_RUNTIME`．仕様の用語（host）とずれ，`emela_runtime.mjs` と紛らわしい
- 値を host の名前（`node`，`bun`，`deno`）に限り，command は別の変数で渡す．変数が 2 つになり，wrapper や path を指したいときに不便になる
- basename でなく，値に起動の形を丸ごと書かせる（`EMELA_JS_HOST="deno run -A"`）．shell の分割の規則を仕様に持ち込むことになる
- `EMELA_NODE` を残したまま `EMELA_JS_HOST` を足す．同じことをする変数が 2 つ残り続ける
- `Pome.toml` に host を書く（`[build] js-host = "bun"`）．Bun でしか動かない Pome のテストを付け忘れなく走らせられる．別の proposal にする

## Prior art

- `EMELA_CC`，`EMELA_WASMTIME`：使える道具が 1 つの場合の命名

## Open questions

- Deno の権限を `-A` にするか，Std が使う分（`--allow-read`，`--allow-write`，`--allow-net`，`--allow-env`）に絞るか．絞ると，`@external` の host の file が使う権限を emela が知る手段がない
- CI で Deno と Bun の job を足すか．足さないと，Deno の起動の仕方が壊れても気づけない

## Decision
