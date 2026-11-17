---
title: Handlers that implement several effects
authors: [Alliana]
status: draft
pr: 625
issue:
decided:
touches: [effect.handler, effect.with, effect.layer, effect.label, effect.use, type.rules, grammar.decl, grammar.type, diag.list]
requires: [capability-type, handler-member]
related: []
---

# Handlers that implement several effects

## Summary

1つの handler が複数の effect を実装できるようにする（`handler H: ReadFile + WriteFile`）．実装した effect はすべて同じ `self` を共有する．capability の型を `use ReadFile + WriteFile` と書けるようにし，effect の集合が含まれる capability は，少ない effect の型として渡せる．layer とは別の概念として残す．

## Motivation

1つの資源に複数の面があり，面どうしが状態を共有しなければならない場面がある．

- ロックを取ったファイル，一時ファイル: 読む面と書く面が同じファイルハンドルを使う
- Db のトランザクション: 読むだけの関数を `use Query`，書く関数を `use Execute` と分けて書きたいが，トランザクションの中では両方が同じ接続，同じトランザクションを使わなければならない

handler は effect をちょうど1つ実装する（`[effect.handler]`）ので，今は全部の操作を持つ effect（`Tx`）の handler を作り，面ごとに `Tx` へ転送するだけの handler を書く．

```emel
with Tx by db.begin(), Query by TxQuery, Execute by TxExecute { ... }
```

面の数だけ転送の handler とラベルの組み合わせが要る．

handler member（`handler-member`）と組み合わせると，原点の例（読むだけ，書くだけ，ロック，一時ファイル）がそのまま書ける．

```emel
pub effect TemporaryFile {
  fn path() -> String
  suspend fn persist(to: String) -> ()
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
}

pub effect Files {
  handler Open(path: String): ReadFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Create(path: String): WriteFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Lock(path: String): ReadFile + WriteFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Temporary(): ReadFile + WriteFile + TemporaryFile fails FileAccessDenied | FileSystemFailure
}

fn save(path: String, text: String) -> ()
  fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure uses { Files } {
  with guard = Files.Lock("#{path}.lock"), tmp = Files.Temporary() {
    tmp.write(Bytes.encode_utf8(text))
    tmp.sync()
    tmp.persist(path)
  }
}

with src = Files.Open("in.txt"), tmp = Files.Temporary() {
  copy(src, tmp) # tmp は `use WriteFile` として渡せる
}
```

| 欲しかった型 | capability の型 |
|---|---|
| ReadableFile | `use ReadFile` |
| WritableFile | `use WriteFile` |
| LockedFile | `use ReadFile + WriteFile` |
| TemporaryFile | `use ReadFile + WriteFile + TemporaryFile` |

## Proposal

### Syntax

```emel
handler SystemTx(conn: Connection): Query + Execute {
  init(url: String) { ... }
  release { ... }
  suspend fn query(sql, resume) { ... }
  suspend fn execute(sql, resume) { ... }
}
```

- handler の頭のコロンの右に，effect を `+` でつないで書ける
- capability の型に `use E1 + E2` と書ける

### Meaning

- handler は列挙した effect の操作をすべて，それぞれちょうど1回実装する．同じ名前の操作を持つ effect を2つ列挙するのはコンパイルエラー（どちらの実装か決まらないため）
- 実装したすべての effect の操作は，同じ `self` を共有する
- with に書くと，列挙した effect をすべて供給する．init と release は1回ずつ走る
- 内部で1つの effect の実装が別の effect の操作を使うとき，with を通らず自分の実装に届くわけではない．操作の本体の `use E` は今と同じく requirement になり，自分の effect への requirement は外側で満たされる（`[effect.with]`）．状態の共有は `self` を通して行う
- layer とは別の概念として残す．layer は別々の handler を with で結線し，member の requirement を with の時点で解決する（`with AppLive, FixedClock` で Clock を差し替えると，PostgresUsers も FixedClock を使う）．複数の effect を実装する handler は1つの `self` を共有し，差し替えの単位は handler 全体になる

### Typing rules

- provides(H) は列挙したすべての effect
- `use E1 + ... + En` の capability は，`use F1 + ... + Fm` の引数に渡せる．ただし {F1, ..., Fm} ⊆ {E1, ..., En}．渡した先で呼べるのは F の操作だけ
- capability の型を広げる手段（型の検査，match，Dynamic への変換）は設けない．狭めた capability から元の effect の操作を呼ぶことはできない
- `use E1 + E2` は capability-type の `use E` の拡張で，同じく型を書けるところならどこにでも書ける．操作を呼ぶとその effect が R に入る

## Specification changes

### `[effect.handler]`

Replace "A handler is a declaration, and it implements each operation of one effect exactly once." with:

> - A handler is a declaration, and it implements each operation of the effects after its colon exactly once (`handler SystemTx(conn: Connection): Query + Execute`). All of them share the same `self`. Listing two effects that have an operation of the same name is a compile error (code to be assigned).

### `[effect.with]`

> - A handler that implements several effects provides all of them. Its init and release run once.

### `[effect.layer]`

> - A handler that implements several effects counts as providing each of them for E0421.

### `[effect.label]`

> - In `L by H`, where H implements several effects, H serves L through the effect that is the underlying effect of L. If none of them is, it is E0431.

### `[effect.use]`

> - The type of a capability of several effects is written `use E1 + ... + En`. A capability can be passed to a parameter of type `use F1 + ... + Fm` when every Fi is one of the Ej. Only the operations of the Fi can be called through the parameter. There is no way to recover the other effects from it.

### `[type.rules]`

> - provides(H) is all the effects that H implements.

### `[grammar.decl]`

```ebnf
handler_decl = "handler" type_name [ "(" [ fields ] ")" ]
               ":" effect_ref { "+" effect_ref } "{" { handler_item NL } "}"
effect_ref   = type_ref [ "[" type { "," type } "]" ]
member_sig   = "handler" type_name "(" [ params ] ")" ":" effect_ref { "+" effect_ref }
               [ "fails" error_set ]
```

### `[grammar.type]`

```ebnf
type       = ...
           | "use" effect_ref { "+" effect_ref }
```

## Diagnostics

- [ ] Two effects listed on a handler have an operation of the same name: code to be assigned
- [ ] The same effect listed twice on a handler: code to be assigned
- [ ] A capability passed to `use F...` that lacks some Fi: E0403 or a new code (to be assigned)

## Compatibility

追加だけ．

## Implementation

- emela-syntax: `+` で続く effect の列
- emela-types: provides(H) の集合化，操作名の重複の検査，capability の型の包含
- emela-core: 1つの handler の値から複数の effect の操作表を作る

## Alternatives

- 面ごとに転送する handler を書く（今の仕様で書ける形）: 面の数だけ handler とラベルが要る
- 委譲の糖衣（残りの操作を外側へ転送する構文）: 転送の手間は減るが，面どうしの状態の共有は解決しない
- layer を，複数の effect を実装する handler で置き換える: 結線の時期が逆になり，テストで一部の effect だけを差し替えても，handler の内部では元の実装が使われる
- capability の型の包含を入れない: LockedFile や TemporaryFile を `use ReadFile` の引数に渡せず，面ごとに関数を書き分けることになる

## Prior art

- Interface subsumption in object-capability systems: attenuating authority by passing a reference through a narrower interface.
- Handlers for several effects at once: Koka and Effekt let one handler handle several operations of different effects.

## Open questions

- 同じ操作名を持つ2つの effect を列挙できないのは厳しすぎないか（操作を effect の名前で修飾して実装する構文を用意するか）
- ラベルの `L by H` で，H がラベルの元の effect を2つ以上の経路で持つ場合（ありえるか）
- 同じ capability を別の面として2回渡す別名（`copy(tmp, tmp)`）は型では止めず，同じ変数を2つの引数に渡したときだけ lint で警告する．lint の draft（`linter`）に入れるか，こちらに書くか
- 性質（中身が変わらない，排他）を保証したい型は，包含関係のない別の effect として宣言する，という指針を仕様のどこに書くか

## Decision
