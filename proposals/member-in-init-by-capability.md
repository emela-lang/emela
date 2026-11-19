---
title: Handler members inside an init are opened through a capability
authors: [somnicattus]
status: draft
pr: 702
issue:
decided:
touches: [effect.handler, type.rules, module.resolution, grammar.expr, diag.list]
requires: []
related: [handler-member, handler-member-gaps, handler-member-gaps-2]
---

# Handler members inside an init are opened through a capability

## Summary

init の中の式で，effect（または label）の名前から handler member を開く形（`Files.Open(path)`，`L.Open(path)`）をやめ，capability を通す形（`(use Files).Open(path)`，`fs.Open(path)`）だけにする．with の項目 `x = Files.Open(path)` は変えない．

これで，式の中で effect の名前が供給者に届くことはなくなり，式から供給者に届くのは `use` だけになる．`[effect.use]` の「operation は capability の値を通してしか呼べない」と同じ考えを，handler member にも当てはめる．

## Motivation

### 式の中で effect の名前が供給者に届く，唯一の形

`[effect.use]` は，operation を effect の名前で呼ぶ形を持たない（「There is no form that calls them directly by effect name (`Logger.info(...)`)」）．供給者に届くには，`use E` で capability の値を取り，それを通す．

`handler-member`（#624）は，init の中で handler member を式として開く形を2つ入れた（`[effect.handler]`）．

- `E.M(arguments)`: その位置で E を供給する一番内側の handler の，member M の init を走らせる．E は handler の requirement になる
- `c.M(arguments)`: c（E の capability）を供給する handler の，member M の init を走らせる

`E.M(arguments)` は，意味の上では `(use E).M(arguments)` と同じで，その略記である．`handler-member-gaps`（#667）で E は label でもよくなった（`L.M(arguments)`）．

effect の名前が供給者に届く書き方は，ほかには with の項目（handler `H`，label の `L` と `L by E`，`x = E.M(arguments)`）にしかない．with の項目は，供給者を名前で並べる位置である．ふつうの式で `use` を書かずに effect の名前から供給者に届くのは，init の中の `E.M(arguments)` だけである．

### `E.op()` を止める理由が弱くなる

`E.M()` を許すと，「なぜ `E.op()` は書けないのか」に意味の上で答えられなくなる．今の理由は，`handler-member-gaps-2`（#670）の名前の衝突だけである．モジュールと effect は同じ名前を持てる（`State`）ので，`.` の左の effect は handler member を指すときだけ読み，member は大文字で始まるのでモジュールの関数（小文字）とぶつからない．operation は小文字なので，`State.get` のようにぶつかる．

これは書き方の都合で，`use` を書く原則の説明ではない．`E.M()` が例外として残ると，`use` を省ける形を広げる圧力が残る．

### 読みやすさ

`Files.Open(path)` は，モジュールの関数の呼び出し（`Json.parse(s)`）と見分けがつかない．それなのに init を走らせ，handler に requirement を足す．`use` を書けば，そこで供給者に届くことが見える．

### 失うものはない

`(use Files).Open(path)` と，`fs = use Files` の後の `fs.Open(path)` は今も書け，同じ意味になる．`E.M(arguments)` は v0.28.0（2026-11-18）で出たばかりで，main で使っているのはテストだけである（Compatibility）．

## Proposal

### Syntax

構文は変えない．`postfix` の `"." type_name "(" [ args ] ")"` は，`c.M(arguments)` のために残る．

```emel
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) {
      Self(inner: (use Files).Open(path)) # the outer Files
    }

    suspend fn read(max, resume) { resume(self.inner.read(max)) }
  }
}

handler Pair(first: use ReadFile, second: use ReadFile): Logger {
  init(path: String) {
    fs = use Files
    Self(first: fs.Open("#{path}.1"), second: fs.Open("#{path}.2"))
  }

  fn info(msg) { ... }
}
```

### Meaning

- init の中では，handler member を `c.M(arguments)` の形でだけ開く．c は E の capability で，`(use E)` と書いても，`use E` を束縛した変数でもよい
- init の中の式で，`.` の左に effect または label を書いて handler member を開く（`Files.Open(path)`，`L.Open(path)`）のは誤り（E0448）
- 開いたものの寿命，requirement，E は変わらない．`(use E).M(arguments)` の R は `use E` の R，つまり E で，今の `E.M(arguments)` と同じく handler の requirement になる
- 自分の effect への `use E` は，今と同じく外側の供給者を指す（requirement は外で供給される）．なので `(use Files).Open(path)` は，`Files` の handler の init の中でも外側の handler の member を開く
- with の項目 `x = E.M(arguments)`，`x = L.M(arguments)`，`x = c.M(arguments)` は変わらない

### Typing rules

- `[type.rules]` の init の中の行は `c.M(a)` だけになる．R は `{ c } ∪ Ra` で，c が `use E` のときは `use E` の R（`{ E }`）

## Specification changes

### `[effect.handler]`

In the item "Inside an init, a handler member can be written as an expression. ...", replace "Inside an init, a handler member can be written as an expression. `E.M(arguments)` runs the init of the handler member M of the innermost handler that provides E, and `c.M(arguments)`, where c is a capability of E, runs that of the handler that provides c." with:

> Inside an init, a handler member can be written as an expression `c.M(arguments)`, where c is a capability of E (`(use Files).Open(path)`, or `fs.Open(path)` after `fs = use Files`). It runs the init of the handler member M of the handler that provides c. An effect or a label is not written on the left of `.` in an expression: `Files.Open(path)` inside an init is a compile error (E0448), and is written only as an item of a with (8.3).

In the same item, replace "A handler member of the handler's own effect refers to the outer handler." with:

> `use E` of the handler's own effect refers to the outer handler (8.3), so a handler member opened through it is that of the outer handler.

### `[type.rules]`

Replace the row "| `E.M(a)`, `c.M(a)` inside an init | `use F` (F is the effect of M) | Ea ∪ (E of the init of M) | { E } or { c }, ∪ Ra |" with:

> | `c.M(a)` inside an init | `use F` (F is the effect of M) | Ea ∪ (E of the init of M) | { c } ∪ Ra (for `(use E).M(a)`, the R of `use E`, { E }) |

### `[module.resolution]`

Replace "On the left of `.`, modules and Traits are looked up, and effects, only to name their handler members (`Files.Open`, 8.3)." with:

> On the left of `.`, modules and Traits are looked up, and effects, only to name their handler members as an item of a with (`x = Files.Open(p)`, 8.3).

Replace "If both a module and an effect are found, `X.M` names the handler member M of the effect if it has one, and otherwise the definition M of the module." with:

> If both a module and an effect are found, `X.M` as an item of a with names the handler member M of the effect if it has one, and otherwise the definition M of the module. In an expression, `X.M` names the definition M of the module.

### `[grammar.expr]`

In the comment of `postfix`, replace "a handler member, inside an init only; 8.4" with "a handler member of a capability, inside an init only; 8.4".

### `[diag.list]`

Replace the row of E0448 with:

> | E0448 | A handler member is opened outside an item of with and an init (`f = Files.Open(p)` in a fn, 8.2, 8.4), or, inside an init, by the name of an effect or a label (`Files.Open(p)`, 8.4). The help is ``= help: open it as an item of `with`: `with x = Files.Open(...) { ... }` `` outside an init, and ``= help: open it through a capability: `(use Files).Open(...)` `` inside an init | ``handler member `Open` can only be opened in `with` or called in an `init` `` / ``handler member `Open` is opened through a capability inside an `init` `` |

## Diagnostics

- [ ] `E.M(arguments)` or `L.M(arguments)` inside an init: E0448 (extended; it opens the member now), with the help `(use E).M(...)`
- [ ] `E.M(arguments)` outside a with item and an init: E0448 (unchanged)

## Compatibility

- 互換性を壊す．init の中の `E.M(arguments)` と `L.M(arguments)` は v0.28.0 で書けるようになった形で，`(use E).M(arguments)` に書き換えが要る．意味は同じなので，書き換えは機械的にできる
- main で init の中の `E.M(arguments)` を使っているのはテストだけ:
  - `crates/emela-codegen-c/tests/programs/handler_members.emel`，`handler_member_failure.emel`，`handler_member_suspend.emel`
  - `crates/emela-core/tests/lower.rs`，`crates/emela-driver/tests/wasm_parity.rs`，`crates/emela-types/tests/handler_member.rs`，`crates/emela-resolve/tests/resolve.rs`
  - `crates/emela-fmt/test_data/handler_member.emel`
- spec と std と examples に，init の中の `E.M(arguments)` はない．`spec/history.md` と採択済みの proposal（`handler-member.md` の `TracedFiles`）は記録なので書き換えない
- 審議中の `region-member`（#660）の例 `Self(stage: Engine.Wave)` は，`Self(stage: (use Engine).Wave)` になる

## Implementation

- emela-types: init の中の `E.M(arguments)` と `L.M(arguments)`（`check_member_open` と `MemberProvider::Effect` の経路）を E0448 にする．`c.M(arguments)` の経路は変えない
- emela-resolve: 式の中の `X.M` で，effect の handler member を引かない（モジュールと effect が同じ名前のとき，式ではモジュールの M を引く）．with の項目は変えない
- emela-core: init の中の `E.M` の lowering（`member_provider` の `Effect` の経路を init で使う所）は with の項目でだけ使う
- emela-driver: E0448 の文と help．LSP の補完で，init の中の `Files.` に member を出さない
- テストを `(use E).M(...)` に書き換える（Compatibility の一覧）

## Alternatives

- **今のまま `E.M(arguments)` を残し，`uses { E }` を持つ組み込みの関数として位置づける**: 短く書けて，with の項目と同じ見た目になり，互換性も壊さない．`uses { E }` を持つ関数を呼ぶとき，呼ぶ側は `use` を書かないので，それと同じだと説明できる．ただ，そういう関数は宣言の側に `uses` と `use` が書いてあり，呼ぶ側はふつうの関数の呼び出しである．`E.M` にはその宣言にあたるものがなく，effect の名前そのものが供給者に届く．`E.op()` を止める理由は名前の衝突だけになる
- **`E.op()` も許し，operation でも `use` を省けるようにする**: `E.M()` と揃う．ただ，`[effect.use]` の原則を捨てることになり，#670 が解いたモジュールとの名前の衝突も戻る
- **with の項目の `x = E.M(arguments)` もやめ，`x = (use E).M(arguments)` か `x = c.M(arguments)` だけにする**: 全部の位置で同じ規則になる．ただ，with の項目は供給者を名前で並べる位置で（handler `H`，label の `L` と `L by E`），そこで effect の名前を書くのは一貫している．`x = E.M(arguments)` は E を requirement にし，handler の項目と同じく with の R に出る

## Prior art

- Capability-passing style: effects are reached only through capabilities that are values in scope, so where a provider is reached is visible in the code.

## Open questions

- 移行: 1つのリリースだけ警告（W04xx）にしてから誤りにするか，すぐ誤りにするか．v0.28.0 で出たばかりで，使っているのはテストだけなので，すぐ誤りにしてよいと考える．LSP の code action で `(use E).M(...)` に書き換えられるようにするか
- `(use Files).Open(path)` の括弧が冗長に見える．`use Files.Open(path)` のような短い形を足すか（足すなら，`use` の後に member を書ける形として別に決める）

## Decision
