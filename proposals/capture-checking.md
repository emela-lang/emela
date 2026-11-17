---
title: Capture checking
authors: [Alliana]
status: draft
pr: 621
issue:
decided:
touches: [effect.escape, effect.use, effect.with, fiber.escape, type.fn, type.rules, type.inference, lsp.hover, diag.list, open.next]
requires: [experimental-features]
related: []
---

# Capture checking

## Summary

値が抱えている capability の集合（capture set）を型と一緒に推論し，capability を供給した with の外へ出る値をコンパイルエラーにする．`[effect.escape]` と `[fiber.escape]` の実行時の検査を静的にし，E0425 を同じ検査にまとめる．R の要素に，effect の名前に加えて capability そのもの（`primary`）を入れる．capture set はソースには書かず，LSP と診断で見せる．

## Motivation

`[effect.escape]` は「capability を捕まえた値は with の外へ出られない」と決めているが，検査は実行時で，「0.22 で静的にする」とだけ書かれている（今は 0.27）．

- 状態を持たない handler は検査の対象外で，外へ出ても動き続ける
- 状態を持つ handler は，外へ出た値を呼んだ時点で defect になる．呼ばれなければ見つからない
- 操作が `self` を捕まえたクロージャを返すと，release の後にフィールドを読める（draft `handler-self-after-release`）
- `Fiber.scope` からの持ち出しは，値の型に Fiber などの名前があるかで止めている（E0425）．型の名前で一律に止める粗い検査である

R が effect の名前の粒度しか持たないことも，ずれを生んでいる．

```emel
with Plain {
  p = use Greeter
  with Shout {
    run_greeter(|| p.greet("b")) # "hello b": the closure reaches Plain
  }
}
```

クロージャの型は `fn() -> String uses { Greeter }` で，`run_greeter` の R の Greeter は Shout で満たされる．実際に届くのは Plain である．R は「名前で供給されていなければならないもの」と「捕まえた実体」を区別していない．

capability を値として扱う提案（`capability-type`，`handler-member`）は，この区別がないと成り立たない．handler member の資源は名前で供給されないので，`copy(src: use ReadFile, ...)` の R に `ReadFile` を入れると，満たす供給者がいない．

## Proposal

### 何を追うか

- capture set の要素は，capability の根（root）である．根は次のどれか
  - with の項目が入れた handler の実体
  - `Fiber.scope` が作った Scope
  - handler の操作の中の `self`（その handler の実体）
- `use E` は，その位置で E を供給する一番内側の with の根を指す．関数の中で with に囲まれていない `use E` は，呼び出し側から渡される根で，effect の名前 E で表す
- 値の capture set は，その値から届きうる根の集合である
  - クロージャとローカル fn: 捕まえた変数の capture set の和
  - データ構造: 要素の capture set の和
  - 関数の呼び出しの結果: 呼ばれた関数の戻り値の capture set に，引数の capture set を当てはめたもの
- capability を含みえない型（関数型，`use E`，型引数，Fiber などの scope に属する型のどれも含まない型）の値は，何も抱えない

### 検査

- with の本体の値の capture set に，その with の根が入っていたらコンパイルエラー
- `Fiber.scope` の値の capture set に，その scope の根（Scope と，それが作った Fiber，Deferred，Semaphore，Queue）が入っていたらコンパイルエラー．E0425 はこの検査に置き換わる
- handler の操作の戻り値の capture set に `self` が入っていれば，その値は handler の with の外へ出られない（上の with の検査で止まる）

### R の要素

- R の要素は，effect の名前（`Db`）と capability の根を指す名前（`primary`，引数の `from`）の2種類になる
  - 名前で引く `use Db` と，それで得た capability の操作は，R に `Db` を入れる（今と同じ）
  - 引数，束縛，捕まえた変数として受け取った capability の操作は，R にその capability を入れる
- `|| p.greet("b")` の型は `fn() -> String uses { p }` になる
- capability の要素は，その effect の名前に広げて扱える．`fn() uses { p }` は `fn() uses { Greeter }` が期待される位置に渡せる（要求が少ない側から多い側へ）．上の `run_greeter` の例は今と同じく通る
- 呼び出しでは，引数の capability の要素を渡した値の要素に置き換えて，呼び出し側の R に足す．capability の要素は，その根を作った with（または束縛）を抜けるときに R から取り除く
- suspend しうるかの判定（E0417，Immediate）は，capability の要素ではその effect で行う

### 推論と表示

- capture set と R の capability の要素は推論する．ソースには書かない（`[effect.escape]` のまま）
- 関数の戻り値の capture set は，本体から推論する．再帰のグループの中は単相に扱う（`[type.inference]` の R と同じ）
- 本体がない宣言（effect の操作，trait の関数，`@external` の関数，引数の関数型）は，次の既定で埋める
  - 戻り値は，引数の capture set の和を抱えうる
  - effect の操作の戻り値は，さらにその handler の根を抱えうる
  - 宣言の中の関数型は今と同じく純（`[type.fn]`）なので，何も抱えない
- LSP の hover は，戻り値の capture set を `holds:` の行で見せる

```text
fn pick(primary: use Db, replica: use Db, write: Bool) -> use Db uses {}
  holds: primary, replica
```

```text
f: fn() -> Rows uses { primary }
  holds: primary (with PrimaryDb, line 4)
```

### 例

```emel
fn first_line(path: String) -> use ReadFile fails ... uses { Files } {
  with f = Files.Open(path) {
    f # error: the value of the body holds `f`, which is released at the end of this with
  }
}

fn make_greeter() {
  with ConsoleLogger {
    log = use Logger
    || log.info("hi") # error: holds `log` (now: a compile error only when the handler has state, at run time)
  }
}

with PrimaryDb {
  p = use Db
  with ReplicaDb {
    r = use Db
    db = pick(p, r, False) # holds p, r
    [db] # error: holds `r`, which is released at the end of `with ReplicaDb`
  }
}
```

```text
error[E04xx]: a value that holds `f` cannot leave the `with` that provides it
   ╭─[ src/main.emel:3:5 ]
   │
 2 │   with f = Files.Open(path) {
   │        ────────┬────────
   │                ╰── `f` is released at the end of this `with`
 3 │     f
   │     ┬
   │     ╰── the value of the body holds `f`
───╯
```

### 段階的な導入（案）

- 0.28: 推論はいつも行い，`holds:` を LSP で見せる．検査は既定では警告，`Pome.toml` の `[experimental] capture-checking = true` でコンパイルエラーにする．実行時の検査は残す
- 警告の期間に，状態を持たない handler の capability を外へ出しているコードを見つけて直してもらう（Compatibility の非互換の範囲）
- 次の版: 既定でコンパイルエラーにし，`[experimental]` の項目を外す
- R の capability の要素（`uses { p }`）は広げる規則で今の R の検査と互換なので，0.28 から入れてよい．capability-type と handler-member はこれに依存する

## Specification changes

### `[effect.escape]`

Replace the items from "The compiler tracks which capabilities a value holds ..." through "Until the capture set check is added (0.22), ..." with:

> - The compiler infers, together with its type, which capabilities a value holds (its capture set). The elements of a capture set are roots: the handler of a with item, the Scope of a `Fiber.scope`, and `self` inside the operations of a handler. `use E` refers to the root of the innermost with that provides E at that position.
> - A closure or a local fn holds what its captured variables hold. A data structure holds what its elements hold. The result of a call holds what the return value of the callee holds, with its parameters replaced by the arguments. A value whose type cannot contain a capability holds nothing.
> - If the value of a with body holds a root of that with, it is a compile error (code to be assigned).
> - The capture set of the return value of a function is inferred from its body. For a declaration without a body (an effect operation, a trait function, an `@external` fn, a function type of a parameter), the return value holds what its arguments hold, and the return value of an operation also holds the handler.
> - The capture set is not written in source. The language server shows it (`[lsp.hover]`).

### `[effect.use]`

Replace "Calling an operation of a capability value adds its effect to R" with:

> - Calling an operation of a capability value obtained by `use E` adds E to R. Calling an operation of a capability received as a parameter, bound by a with item, or captured, adds that capability to R (`uses { from }`). A capability in R can be widened to its effect: a function whose R contains `p` can be used where R contains the effect of p. Whether a call may suspend (E0417) is decided by the effect of the capability.

### `[fiber.escape]`

Replace the item on E0425 and the paragraph and table on 0.22 with:

> - The Scope of a `Fiber.scope`, and the Fibers, Deferreds, Semaphores and Queues it makes, are roots of that scope (8.6). If the value of `Fiber.scope` holds one of them, it is a compile error (the code of 8.6).

### `[type.fn]`

Add:

> - The `uses` of a function type can contain capabilities as well as effects (`fn() -> Rows uses { primary }`). They are inferred and shown by the language server, and are not written in source.

### `[type.rules]`

Replace the rows of `use X` and the closure:

> | `use X` | The capability of X | {} | { X } |
> | `\|x\| e` | `fn(..) -> T fails Ee uses { Re }`, where Re keeps the captured capabilities as elements | {} | {} |

Add:

> - A call `f(a)` adds to R the elements of the R of f, with each capability parameter replaced by the elements of the corresponding argument.

### `[lsp.hover]`

> - On a function or a value that may hold capabilities, the hover shows a line `holds: ...` with the roots it may hold, and for each root the with that provides it.

### `[diag.list]`

> | E04xx | A value that holds a root leaves the with or the `Fiber.scope` that provides it (8.6) | ``a value that holds `f` cannot leave the `with` that provides it`` |
> | E0425 | Unused (was: the value of `Fiber.scope` contains `Fiber`, `Deferred`, `Semaphore` or `Queue`; replaced by capture checking) | |

### `[open.next]`

Remove the item "0.22: capture set".

## Diagnostics

- [ ] A value that holds a root leaves its with or `Fiber.scope`: code to be assigned. The labels show where the root is provided and where the value leaves
- [ ] E0425: retired
- [ ] The run-time defect `handler X was used after release`: kept for values that reach a released handler through the host (callbacks of `@external` fns), or removed (open question)

## Compatibility

- 今，実行時の defect になるプログラム（状態を持つ handler の capability を外へ出して呼ぶもの）は，コンパイルエラーになる．`[fiber.escape]` が予告していた移行と同じ
- 状態を持たない handler の capability を外へ出すプログラムは，今は動くが，コンパイルエラーになる．これは非互換である
- 外へ出しても呼ばないプログラム（今は defect にもならない）も，コンパイルエラーになる
- R の capability の要素は effect の名前に広げられるので，今通る R の検査は通る

## Implementation

- emela-types: capture set の推論（型と一緒に持つ），with と Fiber.scope の出口での検査，R の capability の要素と広げる規則，関数の境界での置き換え
- emela-lsp: `holds:` の表示
- 実行時の検査（release 後の使用）は，静的な検査が入ったあとに外せるか確かめる

## Alternatives

- 実行時の検査のままにする: 状態を持たない handler と，呼ばれない値を見逃す
- capture set をソースに書く（`^{c}` の形）: 書く負担が増える．Emela は capability の種類が決まっていて（`use E` と Scope），変数が不変で，寿命が入れ子なので，推論と保守的な既定で足りる見込み．足りない宣言が出てきたら，書く構文を後から足す
- 型の名前で止める（E0425 の形を広げる）: `Option[Fiber[User]]` のように型を全部たどる必要があり，同じ型の別の with の値を区別できない
- R を effect の名前だけに保つ: handler member の資源を引数で渡す関数の R が満たせない．クロージャが捕まえた実体と，呼ばれた場所の実体の区別がつかない

## Prior art

- Capture checking: tracking the capabilities a value captures as part of its type, so that a value cannot outlive the capabilities it holds. Scala 3 has it, with capture sets written in types (`T^{c}`, `A ->{c} B`) (Odersky, Boruch-Gruszecki et al., "Capturing Types").
- Effects as capabilities, and capture sets for effect handlers: System C tracks the capabilities a computation uses as capture sets, with second-class capabilities by default and boxes for first-class use (Brachthäuser, Schuster, Lee, Boruch-Gruszecki, "Effects, Capabilities, and Boxes", OOPSLA 2022). Effekt is built on it.
- Region-based escape analysis: rejecting values that refer to a region after it ends.

## Open questions

- 関数の境界で capture set が本体の変更によって変わると，呼び出し側が壊れる．pub な関数について，Pome の互換性の検査で拾うか，表示だけにするか
- 保守的な既定（戻り値は引数の capture set の和を抱えうる）で通らない正しいプログラムが出たときに，書く構文を足すか
- 実行時の検査（`handler X was used after release`）を残すか．`@external` のコールバックを通ってホストから届く場合は静的に追えない
- R の capability の要素の表示の名前．引数名（`from`）でよいか．引数のない場所（クロージャを返す関数の戻り値の型）では何と書くか
- `[effect.escape]` の「状態を持たない handler は外へ出ても動き続ける」に頼っている既存のコードがあるか（非互換の範囲）
- 実験的な機能を有効にする仕組み．今の仕様には実験的な機能の概念がない．`Pome.toml` の `[experimental]`（Pome ごとに決まり，依存先にも LSP にも同じ設定が見える）にするか，コマンドの `--experimental-capture-check` にするか．後者は `emela check` と `emela build` と LSP で設定がずれうる．実験的な機能の扱い（互換性の保証の外にあること，診断の文面）は別の proposal にするか
- 依存先の Pome が実験を有効にしていないとき．推論はいつも行うので，依存先の関数の capture set は使える．依存先の中の違反は依存先の設定で警告かエラーかが決まる

## Decision
