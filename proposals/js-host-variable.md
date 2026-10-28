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

`emela run` と `emela test` が JS の出力を走らせる host を，環境変数 `EMELA_JS_HOST` で選べるようにする．値は command で，起動の仕方は今の `EMELA_NODE` と同じ．`EMELA_NODE` は deprecated にし，1 minor version の間だけ警告付きで読む．

## Motivation

- JS の target は Node，Deno，Bun を host として想定し（`[target]`），三つで同じ結果になることを確かめている（`[target.js.host]`）．ところが host を選ぶ変数の名前は `EMELA_NODE` で，Node 以外を指すと名前と中身が食い違う
- x/platform-bun のように，Bun の組み込み機能（`bun:sqlite`）を叩く Pome が出てきた．そのテストは `EMELA_NODE=bun emela test` で走らせている．読んだ人には Node を指しているように見える

他の道具の変数（`EMELA_CC`，`EMELA_WASMTIME`）は道具の名前を付けている．あれは使える道具が 1 つしかないので成り立つ．JS は host を選べるので，役割の名前にする．名前に host を使うのは，仕様が JS を動かすものを一貫して host と呼んでいるため（`[target.js.host]`，host functions）．`RUNTIME` は，emela が出力する `emela_runtime.mjs` と紛らわしい．

## Proposal

### Meaning

- JS の出力を走らせる command は，`EMELA_JS_HOST` が空でなければその値，そうでなく `EMELA_NODE` が空でなければその値，どちらもなければ `node` にする
- 起動の仕方は今と同じ．main は `<cmd> --input-type=module -e <launcher> -- <entry> <args>`，HTTP サービスは `<cmd> serve.mjs <args>`，テストは `<cmd> emela_test.mjs`．Node と Bun はこの形で動く
- `EMELA_NODE` を読んだときは警告 W0902 を出す．`EMELA_JS_HOST` もあるときは `EMELA_JS_HOST` を使い，同じ警告を出す
- `EMELA_NODE` は次の minor version まで読む．その次の minor version で読まなくなる

## Specification changes

### `[cmd.build]`

Replace "The node used is the environment variable `EMELA_NODE` if set, and otherwise `node` on PATH." with:

- The JS host is the command in the environment variable `EMELA_JS_HOST` if it is set and not empty, otherwise the command in `EMELA_NODE` if it is set and not empty, otherwise `node` on PATH. It MUST accept the options `--input-type=module -e` as node does; Node and Bun do.
- `EMELA_NODE` is deprecated. When it is read, W0902 is reported, also when `EMELA_JS_HOST` is set and takes precedence. It is read until the minor version after the one that deprecates it.

In "Running the program (JS)", replace "node" with "the JS host" in "For an HTTP service, `run` runs `node serve.mjs` ..." and "Arguments to the program are passed after `--` as node arguments", and in the description of `<command>` ("node with its options for `js`" becomes "the JS host with its options for `js`"). The example ``running `node --input-type=module -e <launcher> -- target/emela/js/main.mjs` `` stays, as the line for `node`.

### `[cmd.test.out-dir]`

Replace "`emela test` runs the launcher module with node." with: "`emela test` runs the launcher module with the JS host (`[cmd.build]`)."

### `[diag.list]`

- Change E0904 to "The JS host is not found or cannot be started", with the example ``cannot start `node` `` kept. The note becomes ``= note: install Node.js and add it to PATH, or set EMELA_JS_HOST to the command of a JS host``.
- Add W0902: "`EMELA_NODE` is set (Appendix B)". Message: ``EMELA_NODE is deprecated; use EMELA_JS_HOST``.

## Diagnostics

- [ ] W0902（新規）：`EMELA_NODE` を読んだ
- [ ] E0904 の意味と note を JS host に合わせて直す

## Compatibility

- `EMELA_NODE` だけを設定している環境は，1 minor version の間，警告が出るだけで同じように動く
- compiler の tests（`crates/emela-driver/tests/wasm_*.rs`，`crates/emela-codegen-wasm/tests/unicode.rs`，`crates/emela-cli/tests/`）が `EMELA_NODE` を読んでいる．`EMELA_JS_HOST` に移す

## Implementation

- emela-driver：`NODE_ENV` を `EMELA_JS_HOST` と `EMELA_NODE` の 2 つにする．起動の組み立て（`node_command`，`node_script_command`）は変えない
- emela-cli：W0902 を出す．E0904 の note を直す

## Alternatives

- `EMELA_JS_RUNTIME`．仕様の用語（host）とずれ，`emela_runtime.mjs` と紛らわしい
- 値を host の名前（`node`，`bun`）に限り，command は別の変数で渡す．変数が 2 つになり，wrapper や path を指したいときに不便になる
- `EMELA_NODE` を残したまま `EMELA_JS_HOST` を足す．同じことをする変数が 2 つ残り続ける
- `Pome.toml` に host を書く（`[build] js-host = "bun"`）．Bun でしか動かない Pome のテストを付け忘れなく走らせられる．別の proposal にする

## Prior art

- `EMELA_CC`，`EMELA_WASMTIME`：使える道具が 1 つの場合の命名

## Open questions

- CI に Bun で `emela run` と `emela test` を走らせる job を足すか．足さないと，Bun で動かなくなっても気づけない

## Decision
