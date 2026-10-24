---
title: 名前で定義を指すコマンド（emela symbol）
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [cmd, cmd.symbol (new), cmd.symbol.path (new), cmd.symbol.json (new), cmd.symbol.show (new), cmd.symbol.refs (new), cmd.symbol.rename (new), lsp.rename, diag.list]
requires: []
related: []
---

# 名前で定義を指すコマンド（emela symbol）

## 要約

定義をファイルと位置ではなく名前（symbol path，`Http.Client.get`）で指す方法を決め，それを引数に取るサブコマンドを `emela symbol` の下に足す．`emela symbol show` は定義のシグネチャと推論された E と R と doc を返し，`emela symbol refs` は参照の一覧を返し，`emela symbol rename` はプロジェクト全体で名前を変えて検査し直す．人間のエディタ操作ではなく，コーディングエージェントが一度の呼び出しで答えを得ることを主な使い手に据える．

## 動機

エージェントが欲しい情報を，正確に，少ない手順で得られるようにする．

- エージェントはカーソルを持たず，名前で考えている．名前から定義の型や参照を得る手段として，今は次のものがある
  - grep でテキストを探す．一致した行から，どれが同じ定義を指すかを読み分ける必要がある．型も E も R も得られない
  - ファイルを開いて読む．欲しいのは一つの定義なのに，ファイル全体を読むことになる
  - `emela lsp` を起動し，initialize してからファイルを開き，名前の行と列（UTF-16）を数えて request を送る．一回の問い合わせに手順が多い
- 型と E と R はコンパイラしか知らない．`fails` と `use` を省いた fn（`[module.pub-signature]`）の E と R は，ソースを読んでも判らない．コンパイラに聞けば一回で正確に得られる
- 名前の変更は，同名の別の定義，import の `{x}`，名前付き引数の省略形（`NotFound(id:)`）を区別する必要がある．LSP の rename（`[lsp.rename]`）はこれを正しく扱うが，位置で指す必要があり，同じスコープの名前との衝突も検査しない．書き換えた結果をコンパイラが検査して返せば，エージェントは確かめる手順を省ける
- 名前で指して，必要な情報だけを構造化して返せば，往復の回数と読む量の両方が減る

## 提案

### symbol path

```text
Http.Client.get            -- モジュール Http.Client の fn get
Http.Client.Client         -- モジュール Http.Client の type Client
App.User.name              -- type User のフィールド name（App はモジュール）
App.Store.Users.find       -- effect Users の操作 find
App.User:Show              -- モジュール App に書いた impl User: Show
App.User:Show.show         -- その impl の fn show
App.Db.PostgresUsers:Users.find   -- handler PostgresUsers の操作 find
App.Db.PostgresUsers:Users.init   -- その init
App.Db.PostgresUsers.conn         -- handler のフィールド conn
```

- モジュールの名前（`[module.name]`），定義の名前，子の名前（フィールド，バリアント，effect の操作，trait の fn，handler の操作）を `.` でつなぐ
- impl は，impl の頭と同じ `型:Trait` の形で指し，その前に impl を書いたモジュールを置く．型引数は書かない．一つの型と trait の組には impl が一つしかない（`[trait.impl]` の E0328）ので，これで一つに決まる
- 末尾が一意に決まるなら，前を省ける（`Client.get`，`get`，`User:Show.show`）．候補が複数なら全部を挙げてエラーにする．path はコードに残らず一回きりの問い合わせに使うので，定義が増えて前は通った path が曖昧になっても害は小さい
- handler の中身（操作，init，release）は，handler の頭と同じ `Handler:Effect` の後に置く（`PostgresUsers:Users.find`）．`[effect.handler]` で `:` は左が右を満たすことを表し，impl と同じ形になる．`:` の後ろは実装の中身，という一つの規則で impl と handler を扱える．handler のフィールドは `:Effect` を付けずに書く（`PostgresUsers.conn`）ので，同じ名前の操作とぶつからない
- layer の要素は指さない．layer の中身は handler の名前の並びで，それぞれの handler を指せば足りる
- 局所変数（fn の中の束縛と引数）は指さない．局所変数の rename は，位置で指す LSP（`[lsp.rename]`）に任せる
- 依存の Pome と bundled core と std の定義も指せる（`List.map`）．ただし rename の対象にはならない
- 一つの symbol path が複数の定義を指すことがある．`[module.resolution]` では名前空間が分かれているので，`Http.Client` はモジュール `Http.Client` も，モジュール `Http` の type `Client` も指しうる
  - 複数を指すときはエラーにし，候補ごとに種類と位置を note に挙げる
  - 全サブコマンドが `--kind <kind>` を取り，その種類の定義だけに絞る．繰り返すと，どれかの種類に当たるものに絞る．note に出す種類の名前は `--kind` に渡す名前と同じ
  - path の文法には，種類を区別する記号を入れない
- 種類の名前は宣言のキーワードと同じにし，キーワードのない子には名前を足す

| kind | 指すもの |
|---|---|
| `module` | モジュール |
| `fn`，`const`，`type`，`enum`，`variant`，`error`，`effect`，`trait`，`handler`，`layer` | その宣言．`fn` は trait と impl の fn も含む |
| `field` | type，error，variant，handler のフィールド |
| `operation` | effect の操作 |
| `impl` | impl |
| `init`，`release` | handler の init と release |

- type とそのコンストラクタ，error とそのコンストラクタのように，同じ位置にある定義は一つの symbol として扱う（`[lsp.navigation]`，`[lsp.rename]` と同じ）．`--kind type` で type とコンストラクタの両方が当たる

```sh
$ emela symbol show Http.Client
error[E????]: `Http.Client` matches 2 symbols
  = note: module Http.Client   src/http/client.emel
  = note: type Http.Client     src/http.emel:3:10

$ emela symbol show --kind type Http.Client
pub type Client(base: Url)
  src/http.emel:3:10
```

### 出力

- 全サブコマンドが `--json` を取る．仕様が約束するのは `--json` の形だけで，既定のテキストの出力は人間向けで，実装が変えてよい（`[diag]` のメッセージと同じ扱い）
- エージェントは `--json` を使う

`--json` の形:

```jsonc
// emela symbol show --json Http.Client.get
{
  "symbol": {
    "path": "Http.Client.get",
    "kind": "fn",
    "signature": "pub fn get(url: Url) -> Response fails HttpError use Http",
    "location": { "file": "src/http/client.emel", "line": 12, "column": 8 },
    "doc": "Sends a GET request."
  }
}

// emela symbol refs --json Http.Client.get
{
  "references": [
    { "location": { "file": "src/main.emel", "line": 8, "column": 15 }, "text": "  res = Client.get(url)" }
  ]
}

// 曖昧なとき（exit 1）
{
  "error": {
    "code": "E????",
    "message": "`Http.Client` matches 2 symbols",
    "candidates": [
      { "path": "Http.Client", "kind": "module", "location": { "file": "src/http/client.emel" } },
      { "path": "Http.Client", "kind": "type", "location": { "file": "src/http.emel", "line": 3, "column": 10 } }
    ]
  }
}
```

- `--json` のとき，エラーも JSON にして標準出力に出す．`candidates` の `kind` は，そのまま `--kind` に渡せる．`--json` でないときのエラーは `[diag]` に従い標準エラーに出す
- 型と E と R は構造化せず，`signature` に表示形式（`[type.inference]`）の文字列で入れる．E には隠れた集合（`E(f)`）や行変数が入り，構造化すると約束する形が重くなる
- 位置は1始まりの行と，1始まりで code point 単位の列．`file` は診断と同じ相対パス．モジュールそのものの位置は `file` だけ
- refs の各参照には，参照のある行のソース（`text`，改行を除く）を付ける

### 共通のオプション

- `--project <path>`（既定は `.`）でプロジェクトを指す．探し方は `emela check` と同じ（`[cmd.build]`）．他のコマンドは位置引数でパスを取るが，`emela symbol` では位置引数が symbol path なので，オプションにする
- `--kind <kind>` と `--json` は上に書いたとおり
- 終了コードは，成功で 0，エラーで 1，引数の誤りで 2

### emela symbol の範囲

- `emela symbol` の下のサブコマンドは，symbol path を引数に取るか，symbol path を返す
- この proposal は `show`，`refs`，`rename` を足す．型や E や R で探す `find`，モジュールの公開 API を並べる `outline` は別の proposal で足す

### emela symbol show

```sh
$ emela symbol show Http.Client.get
pub fn get(url: Url) -> Response fails HttpError use Http
  src/http/client.emel:12:8

Sends a GET request.
```

- JSON は `symbol` を一つ返す．曖昧ならエラーになるので，返るのは常に一つ
- 1行目は宣言の形．省いた `-> T`，`fails E`，`use R` は推論の結果で埋める（`[lsp.inlay-hint]` と同じ値）
- 2行目は定義の位置，その後に doc（`[lsp.doc]`）

### emela symbol refs

```sh
$ emela symbol refs Http.Client.get
src/main.emel:8:15
src/sync.emel:21:9
```

- 1行に1つ，ファイルと位置の順．`[lsp.navigation]` の find references と同じ集合で，定義そのものは含めない（定義は `show` で引ける）
- 探すのはプロジェクトのソースルートとテストルート．依存の Pome と bundled core の中は探さない．依存の定義を指したときは，プロジェクトの中の参照を返す（依存はプロジェクトに依存しないので，それで全部になる）

### emela symbol rename

```sh
$ emela symbol rename Http.Client.get fetch
src/http/client.emel:12:8
src/main.emel:8:15
src/sync.emel:21:9
```

- 名前を変える範囲と，一緒に変えるものは `[lsp.rename]` と同じ．trait の fn とその impl の fn も一緒に変わる．なので `rename User:Show.show` と `rename Show.show` は同じ編集になる
- 書き換えた後のプロジェクトを検査し，書き換える前に無かった診断が出るなら，何も書かずに失敗する（衝突の検査を，同じスコープの名前の表ではなく，検査し直した結果で行う）
  - 書き換える前と後の診断の差を見る．前から有った診断は判断に使わない．エラーの有るプロジェクトでも rename できる
  - 差は，診断のコードと位置で比べる．書き換えの前の位置は，書き換えによるずれを直してから比べる．メッセージは比べない（前から有った診断のメッセージに古い名前が出ていると，書き換えで変わってしまう）
- `--dry-run` は書かずに，書き換える位置を出す．検証はする

```jsonc
// 成功（--dry-run では written が false）
{ "written": true,
  "edits": [ { "location": { "file": "src/main.emel", "line": 8, "column": 15 }, "old": "get", "new": "fetch" } ] }

// 検証で失敗（exit 1，何も書かない）
{ "error": { "code": "E09xx", "message": "renaming `get` to `fetch` would introduce 1 diagnostic",
    "diagnostics": [ { "code": "E0213", "message": "...", "location": { "file": "src/main.emel", "line": 3, "column": 4 } } ] } }
```

## Specification changes

### `[cmd]`

Add to the list at the start of the appendix:

> `emela symbol` is in `[cmd.symbol]`.

### `[cmd.symbol]` (new)

```markdown
## emela symbol `[cmd.symbol]`

Commands that refer to definitions by name rather than by file and position are placed under `emela symbol`. Each of them takes a symbol path (`[cmd.symbol.path]`) or returns symbol paths.

| Command | What it does |
|---|---|
| `emela symbol show <path>` | Prints the declaration, location and doc of a definition (`[cmd.symbol.show]`) |
| `emela symbol refs <path>` | Prints the references to a definition (`[cmd.symbol.refs]`) |
| `emela symbol rename <path> <new-name>` | Renames a definition across the project and checks the result (`[cmd.symbol.rename]`) |

- The project is found from `--project <path>`, which defaults to `.`, by the same rules as `emela check` (`[cmd.build]`). The test root and `[dev-dependencies]` are read.
- Every command takes `--kind <kind>` (`[cmd.symbol.path]`) and `--json` (`[cmd.symbol.json]`).
- The exit code is 0 on success, 1 on an error, and 2 on an argument error.
- Without `--json`, the output is meant for people. Its form is not part of the specification, and an implementation MAY change it. Errors are reported as diagnostics (`[diag]`).
```

### `[cmd.symbol.path]` (new)

```markdown
## Symbol paths `[cmd.symbol.path]`

A symbol path names a definition with the names of the module, the definition and its children joined by `.`.

| Path | What it names |
|---|---|
| `Http.Client` | The module `Http.Client` |
| `Http.Client.get` | The fn `get` of the module `Http.Client` |
| `App.User.name` | The field `name` of the type `User` of the module `App` |
| `App.Shape.Circle` | The variant `Circle` of the enum `Shape` |
| `App.Store.Users.find` | The operation `find` of the effect `Users` |
| `App.User:Show` | The impl `impl User: Show` written in the module `App` |
| `App.User:Show.show` | The fn `show` of that impl |
| `App.Db.PostgresUsers:Users.find` | The operation `find` implemented by the handler `PostgresUsers` |
| `App.Db.PostgresUsers:Users.init` | The init of that handler |
| `App.Db.PostgresUsers.conn` | The field `conn` of that handler |

- The module name is as in `[module.name]`. Definitions of dependency Pomes and of the bundled core and std can be named (`List.map`).
- The children of a definition are the fields of a type, error, variant and handler, the variants of an enum, the operations of an effect, and the fns of a trait.
- An impl is named by its head without type arguments, `Type:Trait`, after the module the impl is written in. One impl at most exists for a pair of a type and a trait (`[trait.impl]`), so this names one impl. The fns of the impl follow it.
- The operations, init and release of a handler follow `Handler:Effect`, the form of its head (`[effect.handler]`). The fields of a handler follow the handler's name without `:Effect`.
- Local variables (bindings and parameters inside a fn) and the items of a layer cannot be named.
- Leading segments MAY be omitted (`Client.get`, `get`, `User:Show.show`). The path then names every definition whose full path ends with the given segments.
- Definitions at the same position (a type and its constructor, an error and its constructor) are one symbol (`[lsp.navigation]`).
- The kinds of symbols are as follows.

| Kind | Symbols |
|---|---|
| `module` | Modules |
| `fn`, `const`, `type`, `enum`, `variant`, `error`, `effect`, `trait`, `handler`, `layer` | Definitions declared with that keyword. `fn` includes the fns of a trait and of an impl |
| `field` | Fields of a type, error, variant and handler |
| `operation` | Operations of an effect, and their implementations in a handler |
| `impl` | Impls |
| `init`, `release` | The init and release of a handler |

- `--kind <kind>` keeps only the symbols of that kind. It MAY be given more than once, and then keeps the symbols of any of the kinds given.
- A path that names no symbol is an error (E09xx). A path that names more than one symbol is an error (E09xx), and each candidate is listed as a note with its kind, full path and location (`= note: type Http.Client  src/http.emel:3:10`).
```

### `[cmd.symbol.json]` (new)

```markdown
## JSON output `[cmd.symbol.json]`

With `--json`, a command writes one JSON object to standard output, and nothing else. The form of this object is part of the specification. A later version MAY add members to an object, but does not remove or change members.

- A location is an object with `file`, `line` and `column`. `file` is the path relative to the project directory, the same as in diagnostics. `line` and `column` start at 1, and `column` counts Unicode code points. The location of a module has only `file`.
- An error is written to standard output as `{"error": {...}}`, and the exit code is 1. The members are `code` (the diagnostic code), `message`, and, depending on the error, `candidates` (`[cmd.symbol.path]`) or `diagnostics` (`[cmd.symbol.rename]`).
- A candidate is an object with `path` (the full path), `kind` and `location`. `kind` is a value that `--kind` accepts.
- A diagnostic is an object with `code`, `message` and `location`. A diagnostic without a code has `code: null`.

    {
      "error": {
        "code": "E09xx",
        "message": "`Http.Client` matches 2 symbols",
        "candidates": [
          { "path": "Http.Client", "kind": "module", "location": { "file": "src/http/client.emel" } },
          { "path": "Http.Client", "kind": "type", "location": { "file": "src/http.emel", "line": 3, "column": 10 } }
        ]
      }
    }
```

### `[cmd.symbol.show]` (new)

```markdown
## emela symbol show `[cmd.symbol.show]`

- Prints the declaration, location and doc of the symbol.
- The declaration is the head of the definition (for a fn, up to the `{` of its body), with omitted annotations filled in by inference: the return type, `fails E` and `use R`, with the same values as `[lsp.inlay-hint]`. Hidden sets are shown as `E(f)` (`[type.inference]`).
- The doc is taken by the rules of `[lsp.doc]`. It is shown whether the definition is public or not.

    {
      "symbol": {
        "path": "Http.Client.get",
        "kind": "fn",
        "signature": "pub fn get(url: Url) -> Response fails HttpError use Http",
        "location": { "file": "src/http/client.emel", "line": 12, "column": 8 },
        "doc": "Sends a GET request."
      }
    }

- `doc` is `null` for a definition without a doc. For a definition in the bundled core, `location` is `null`.
```

### `[cmd.symbol.refs]` (new)

```markdown
## emela symbol refs `[cmd.symbol.refs]`

- Prints the references to the symbol, in order of file and position. The set is the same as find references in `[lsp.navigation]` without the declaration.
- References are searched in the source root and the test root of the project. The sources of dependency Pomes and of the bundled core are not searched. For a definition of a dependency, the references in the project are printed.
- Each reference carries the text of the line it is on, without the line break.

    {
      "references": [
        { "location": { "file": "src/main.emel", "line": 8, "column": 15 }, "text": "  res = Client.get(url)" }
      ]
    }

- A symbol without references prints an empty array and exits with 0.
```

### `[cmd.symbol.rename]` (new)

```markdown
## emela symbol rename `[cmd.symbol.rename]`

- Renames the symbol across the project and writes the files.
- The edits are those of `[lsp.rename]`: the definition and every reference, the constructor of a type and of an error, the implementations of an effect operation in handlers, the fns of a trait in impls, and the `{x}` of an import. A punned named argument or field pattern (`NotFound(id:)`) becomes `id: new_name`.
- The following cannot be renamed (E09xx): modules, impls, the init and release of a handler, and definitions of dependency Pomes and of the bundled core and std. A new name that is not a single token of the same lexical class (`[lex.name]`) as the old one is an error (E09xx).
- After computing the edits, the command checks the project with the edits applied, in the same way as `emela check`. If the check reports a diagnostic that the check before the edits did not, the command writes nothing and fails (E09xx), listing the new diagnostics. Diagnostics are compared by code and location, with the locations before the edits shifted by the edits. A project with errors before the edits can still be renamed.
- `--dry-run` computes and checks the edits, and writes nothing.

    {
      "written": true,
      "edits": [
        { "location": { "file": "src/main.emel", "line": 8, "column": 15 }, "old": "get", "new": "fetch" }
      ]
    }

- `written` is `false` with `--dry-run`. The edits are in order of file and position.
```

### `[lsp.rename]`

Replace the bullet "A type and its constructor, an effect operation and its handler implementations, and the `{x}` of an import are renamed together." with:

> A type and its constructor, an effect operation and its handler implementations, a fn of a trait and its implementations in impls, and the `{x}` of an import are renamed together. The same set of edits is used by `emela symbol rename` (`[cmd.symbol.rename]`).

### `[diag.list]`

Add rows in the E09xx band (codes to be assigned):

| Code | Meaning | Example |
|---|---|---|
| E09xx | A symbol path names no symbol (`[cmd.symbol.path]`) | ``no symbol matches `Http.Clinet.get` `` |
| E09xx | A symbol path names more than one symbol (`[cmd.symbol.path]`) | ``` `Http.Client` matches 2 symbols ``` |
| E09xx | The symbol cannot be renamed (`[cmd.symbol.rename]`) | ``module `Http.Client` cannot be renamed`` |
| E09xx | The new name is not of the same lexical class (`[cmd.symbol.rename]`) | `` `figure` is not a valid type name `` |
| E09xx | The rename would introduce diagnostics (`[cmd.symbol.rename]`) | `renaming `get` to `fetch` would introduce 1 diagnostic` |

## Diagnostics

E09xx（ツール）の帯に足す．コードは未定．

- [ ] symbol path が何も指さない
- [ ] symbol path が複数を指す．候補を種類と完全な path と位置とともに note に挙げる
- [ ] rename の対象にできない（モジュール，impl，init と release，依存と bundled core の定義）
- [ ] 新しい名前の字句の種類が違う（LSP では `RequestFailed` の応答で，コードが無かった）
- [ ] rename の後に新しい診断が出る

## Compatibility

コマンドを足すだけで，今のプログラムの意味は変わらない．`emela` のサブコマンド名 `symbol` を使う．

## Implementation

- `emela-cli` にサブコマンド `symbol` と，その下のサブコマンドを足す（`emela pome` と同じ構成）
- 名前解決と型検査の結果から，`emela-lsp` が持つ型インデックス（hover と codeAction が共有しているもの）を引く．rename の編集の集合は LSP の実装を共有する
- symbol path の解決は新しく書く
- 試作: `emela lsp`（0.24.0）で trait の fn を rename すると，trait の宣言，impl の fn，`Greet.greet(u)` の呼び出しの3か所が変わる．impl の fn の位置から rename しても同じ3か所になる．impl の fn の名前は trait の fn への参照として記録されている（`[lsp.navigation]`）ため．`[lsp.rename]` はこれを書いていないので，`[cmd.symbol.rename]` に明記する

## Alternatives

- `[lsp.symbol]` の LSP の kind を `--kind` に使う．type と error がどちらも Struct，effect と trait がどちらも Interface で，区別できない
- impl を `User[Show].show` で指す．`[` は型引数の記法と読み違え（`Pair[Eq].eq`），zsh ではグロブの文字なのでクォートしないと失敗する
- impl を `User.show` で指す．User のフィールド `show` とぶつかり，impl が User と別のモジュールにあることも表せない
- handler の操作を `PostgresUsers.find` で指す．handler のフィールドとぶつかる
- エラーの有るプロジェクトでは rename を拒む．エージェントは直している途中のプロジェクトで rename することが多い
- 型と E と R を JSON で構造化する．隠れた集合と行変数の表し方まで約束することになる
- 位置を LSP と同じ UTF-16 の列で表す．エージェントには数えにくい
- テキストの出力の形を約束する．doc の表示や折り返しを改善できなくなる
- 位置で指す CLI（`emela symbol refs src/main.emel:8:15`）．LSP の request をそのまま CLI にしたもの．実装は少ないが，エージェントが位置を数える手間は残る
- LSP だけを使う．エージェントのツールが LSP を持つ場合はこれで足りるが，持たない場合が多く，一回の問い合わせに initialize からの手順が要る
- トップに `emela show`，`emela refs`，`emela rename` を置く．トップのコマンドはプロジェクト全体を対象にするもの（check，build，fmt など）で，名前で定義を指すコマンドとは性質が違う．後から `find`，`outline` と増えるとトップが散らかる
- `emela ide`，`emela code` の下にまとめる．`ide` はエディタ向けの印象を与え，エディタ向けの入口の `emela lsp` と役割が重なって見える．`code` は何でも当てはまる．どちらも対象を言っていない．`symbol` は全サブコマンドが symbol path を取るか返すことを表し，LSP の `workspace/symbol` とも用語が合う
- symbol path の文法で種類を区別する（`type:Http.Client`，`Http::Client`）．エージェントが毎回正しく書く必要があり，曖昧になるのはまれなので，曖昧なときだけ `--kind` で絞るほうが軽い
- 曖昧なとき，名前空間の優先順位で一つに決める．どれが選ばれたかが出力から読めず，rename で別の定義を書き換える危険がある
- MCP サーバ．CLI があれば，シェルを持つエージェントは呼べる．MCP は CLI の上に後から足せる

## Prior art

- 位置を使わない symbol の識別子: SCIP の symbol 文字列，LSIF の moniker．索引を作る側の仕組みで，人やエージェントが打つための記法ではない
- パッケージと名前で定義の doc を引くコマンド: Go の `go doc http.Client.Get`
- コマンドラインからの rename: `gopls rename` は `file.go:#offset` か `file.go:line:col` で位置を指す

## Open questions

## Decision
