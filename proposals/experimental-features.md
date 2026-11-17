---
title: Experimental features
authors: [Alliana]
status: draft
pr: 620
issue:
decided:
touches: [intro.experimental (new), intro.reading, module.pome, pome.manifest, cmd.build, cmd.test, lsp.project, diag.list]
requires: []
related: []
---

# Experimental features

## Summary

仕様に「実験的な機能」の区分を足す．実験的な機能は `Pome.toml` の `[experimental]` で Pome ごとに有効にし，コマンドの `--experimental <name>` でもその実行だけ有効にできる．互換性の保証の外に置く．仕様の中では，その機能の節に「Experimental」の印を付けて書く．最初の機能は capture checking（`capture-checking`）を想定する．

## Motivation

capture checking（`capture-checking`）には非互換がある．状態を持たない handler の capability を with の外へ出すコードは，今は動くが，コンパイルエラーになる．いきなり既定にすると，既存のコードが一度に壊れる．

- 推論と表示を先に入れ，検査を警告の期間を経てエラーにしたい
- 推論の既定（本体のない宣言の扱い）や診断の文面は，実際のコードに当てて直したい．その間に変えても，互換性を破ったことにならない区分が要る

今の仕様には，この区分がない．

- 仕様に入った規則は，すべて互換性の対象として読める
- 実験のためのコマンドの引数や設定がない．`[intro.reading]` は「Informative」の節（`[target.impl-hint]` など）を区別しているが，「まだ変わりうる規範」の区分はない

## Proposal

### Syntax

```toml
# Pome.toml
[experimental]
capture-checking = true
```

- `[experimental]` の各キーは実験的な機能の名前，値は真偽値

```sh
emela run --experimental capture-checking
emela check --experimental capture-checking --experimental other-feature
emela run --experimental capture-checking hello.emel   # single-file mode
```

- `check`，`build`，`run`，`test` は `--experimental <name>` を受け付ける．何度でも書ける

### Meaning

- 実験的な機能は，仕様の `[intro.experimental]` の一覧に名前があるものだけ．それぞれ，導入した版と，何を変えるかを書く
- 有効にした機能は，その Pome のモジュールを検査し，ビルドするときに働く．依存先の Pome では，依存先の `Pome.toml` の `[experimental]` が働く（`[replace]` や `[external]` と違い，依存されたときにも読む）
  - capture checking のように「どの Pome のコードで違反が見つかったか」で警告かエラーかが決まる機能は，これで Pome ごとに決まる
  - Pome の境界をまたいで意味が変わる機能（構文，型の表現，実行時の動作）を実験にするときは，その機能の節で，依存先と依存元の設定が違う場合を決める
- 実験的な機能は互換性の保証の外にある
  - どの版でも，意味を変えたり，取り除いたりしてよい
  - 既定にした（実験を終えた）機能の名前は，しばらく受け付け，効果がないことを警告する
  - 取り除いた機能の名前は，効果がないことを警告する
- 知らない名前（新しいコンパイラにしかない機能など）は警告にする．ビルドは止めない
- 実験的な機能が出す診断は，`= note: experimental feature `capture-checking`` を付ける
- コマンドの `--experimental <name>` は，その実行の間だけ，ビルドする Pome（single-file mode ではそのファイルと，そこから import で届くファイル）でその機能を有効にする
  - `Pome.toml` の `[experimental]` に足す形で働く．`Pome.toml` で有効にした機能をフラグで無効にすることはできない
  - 依存先の Pome には働かない．依存先はそれぞれの `[experimental]` で検査される
  - single-file mode には `Pome.toml` がないので，実験を試す方法はフラグだけになる
  - 言語サーバーはフラグを知らないので，`Pome.toml` の設定で働く．フラグでだけ有効にした機能は，エディタの診断に出ない
- 知らない名前をフラグに書いたときは，`[experimental]` の知らないキーと同じく警告にする
- 仕様の中では，実験的な機能の規則を，ふつうの規則と同じ場所に書き，段落の頭に「Experimental (`capture-checking`).」の印を付ける

### Typing rules

なし．各機能の規則による．

## Specification changes

### `[intro.experimental]` (new)

Add as 1.5:

> ## 1.5 Experimental features `[intro.experimental]`
>
> An experimental feature is a part of the specification that is not yet stable. A Pome enables it in the `[experimental]` table of its `Pome.toml` (`[module.pome]`).
>
> - The rules of an experimental feature are written where they belong, and each paragraph or item that belongs to one starts with "Experimental (`name`)."
> - Experimental features are outside the compatibility guarantees of this document. A later version MAY change or remove any of them.
> - When an experimental feature becomes stable, its rules lose the mark, and the name is accepted in `[experimental]` with a warning (W02xx) for at least one minor version.
>
> | Name | Since | What it changes |
> |---|---|---|
> | `capture-checking` | 0.28 | Values that hold a capability and leave its with are compile errors instead of warnings (`[effect.escape]`) |

### `[intro.reading]`

Add:

> - A section or item marked "Experimental" applies only when its feature is enabled (`[intro.experimental]`).

### `[module.pome]`

Add to the example:

> ```toml
> [experimental]
> capture-checking = true                          # experimental features (1.5)
> ```

Add:

> - `[experimental]` enables experimental features (`[intro.experimental]`) for the modules of that Pome. Unlike `[replace]` and `[external]`, it is also read when the Pome is depended on: each Pome in the dependency graph is checked with its own `[experimental]`.

### `[pome.manifest]`

Add:

> - The keys of `[experimental]` are names of experimental features, and the values are booleans. An unknown name, or a name of a feature that is stable or removed, is a warning (W02xx) at the key, when building that Pome. A value that is not a boolean is a warning (W02xx), and the key has no effect.

### `[cmd.build]`

Add:

> - `check`, `build` and `run` take `--experimental <name>`, any number of times. It enables the experimental feature `name` (`[intro.experimental]`) for that run, for the Pome being built (in single-file mode, for the files read), in addition to the `[experimental]` of its `Pome.toml`. It does not apply to dependencies, and it cannot disable a feature enabled in `Pome.toml`. An unknown, stable or removed name is the same warning as for a key of `[experimental]` (W02xx).

### `[cmd.test]`

Add:

> - `test` takes `--experimental <name>`, as `[cmd.build]`.

### `[lsp.project]`

Add:

> - The language server applies the `[experimental]` of each Pome, as the commands do.

### `[diag.list]`

> | W02xx | A key of `[experimental]` is not a known experimental feature, or the feature is stable or removed (`[intro.experimental]`) | ``unknown experimental feature `capture-check` `` / ``experimental feature `capture-checking` is now stable and always on`` |
> | W02xx | A value of `[experimental]` is not a boolean | ```[experimental].capture-checking` must be `true` or `false` `` |

## Diagnostics

- [ ] An unknown, stable or removed experimental feature: W02xx
- [ ] A value that is not a boolean: W02xx
- [ ] The note `= note: experimental feature `name`` on diagnostics produced by an experimental feature

## Compatibility

追加だけ．`[experimental]` は今は読まれないので，書いてある既存の `Pome.toml` はない見込み．

## Implementation

- emela-pome: `[experimental]` の読み込み，警告
- emela-cli，emela-lsp: Pome ごとの設定を検査に渡す
- emela-types など: 機能ごとの分岐

## Alternatives

- コマンドの引数だけにする: `check`，`build`，`test` と言語サーバーで設定がずれる．言語サーバーはコマンドの引数を知らない．依存先の Pome の設定も表せない．`Pome.toml` を正にし，フラグはその実行だけ足す形にした
- 機能ごとのフラグ（`--experimental-capture-check`）: 機能が増えるたびにコマンドの引数が増える．`--experimental <name>` の1つで足りる
- 環境変数（`EMELA_EXPERIMENTAL`）: 同じ理由でずれる．Pome ごとの設定を表せない
- ソースの属性（`@experimental(capture_checking)` をモジュールに付ける）: モジュールごとに違う設定になり，境界をまたぐ機能で扱いが増える．Pome 単位で足りる
- 実験の区分を作らず，警告の期間だけを仕様に書く: 推論の既定や診断の文面を変えるたびに，非互換の変更として記録することになる
- edition（`[open.list]` #12）で切り替える: edition は予約語のような大きな切り替えのためのもので，機能ごとに試す用途には粗い

## Prior art

- Opt-in unstable features per crate or per compilation unit, outside stability guarantees: Rust (`#![feature(...)]`, nightly only), GHC language extensions (`{-# LANGUAGE ... #-}`), Scala 3 (`import language.experimental.captureChecking`).
- Experimental flags declared in a project manifest: Kotlin compiler options in the build script, TypeScript `compilerOptions`.

## Open questions

- 依存先の Pome が実験的な機能を有効にしているとき，依存元に知らせるか（`emela pome` の一覧や警告）．実験的な機能は互換性の保証の外にあるので，依存先がコンパイラの更新で壊れうる
- 実験的な機能を使う Pome の `[pome].emela`（最小のコンパイラの版）．機能の意味が版ごとに変わりうるので，最小の版だけでは足りない．上限も書けるようにするか
- 1つの機能の中を段階に分ける（capture checking の「推論と表示はいつも，検査のエラーは実験」）とき，常に働く部分と実験の部分を，機能の節の中でどう書き分けるか
- 機能の名前の書き方（`capture-checking` のように `-` でつなぐ）
- `emela doctor` に，有効になっている実験的な機能を出すか
- フラグでだけ有効にした機能が言語サーバーの診断に出ないことを，どこかで知らせるか（`run` の出力の1行目など）
- フラグを `fmt`，`doc`，`pome` のコマンドにも持たせるか．構文を変える実験的な機能が出たときに要る

## Decision
