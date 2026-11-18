---
title: Regions, scopes that open and close handler members at any time
authors: [somnicattus]
status: draft
pr:
issue: 616
decided:
touches: [effect.region (new), effect.handler, effect.escape, intro.experimental, type.rules, lib.layers, api.std.region (new), lsp.hover, diag.list, open.next]
requires: [handler-member, capability-type, capability-trait]
related: [capture-checking, scope-handle-values, semaphore-queue]
---

# Regions, scopes that open and close handler members at any time

## Summary

領域（region）は，with を書かずに handler member を開き（init を呼び），with の終わりを待たずに閉じる（release を呼ぶ）ための機能．その代わり，開いたものの寿命の上限は領域が持ち，領域が終わるときに残りをすべて release する．

| 開く位置 | 寿命の上限 | 閉じる時点 |
|---|---|---|
| with の項目（`handler-member`） | その with | with の終わり |
| init の中（`handler-member`） | 開いた handler | その handler の release |
| 領域（この提案） | 領域 | `close` を呼んだとき，または領域の終わり |

`Region.scope` が供給者の capability を受け取って領域を作り，本体に渡す．領域は，その供給者の handler member を自分を通して開ける．

```emel
Region.scope(use Engine, |enemies| {
  a = enemies.Spawn(kind: Small) # opens Engine's handler member Spawn
  enemies.close(a)
})
```

## Motivation

数と寿命が実行中に決まる資源がある．ゲームエンジンの側のオブジェクトは，プレイ中に任意の数だけ作られ，任意の順に消える．接続ごとに開くハンドルや，足したり外したりする購読の集まりも同じ形をしている．

```emel
pub enum Kind {
  Small
  Large
}

pub effect Enemy {
  fn move_to(x: Int, y: Int) -> ()
  fn damage(amount: Int) -> Int # the HP left, kept in the engine's object
}

pub effect Engine {
  handler Spawn(kind: Kind): Enemy
}
```

`Engine` の本物の供給者（`SystemEngine`）は，`Spawn` の init でエンジンにオブジェクトを作らせ，release で消させる．テスト用の供給者（`TestEngine`）は，State に記録する．

`handler-member` は，資源について次の (a) から (d) を with の項目で満たす．

- (a) 資源の状態は，その資源の handler が持つ
- (b) 資源の handler の実装は供給者が選ぶ．供給者を差し替えると，資源の handler も替わる
- (c) 資源を指す値も，閉じる操作も，effect の操作として出てこない
- (d) 資源を使うコードはふつうの式で，effect，エラー，値に制限がない

この提案は，次の2つを足す．

- (e) 項目を任意の時点で開き，任意の時点で閉じられる．数と順は実行時に決まる
- (f) すべての項目の寿命の上限が1つの範囲に結び付いていて，範囲が終わるとき残りが release される

`handler-member` と `capability-type` が入った後の言語での書き方は，どれも少なくとも1つを欠く．

| 書き方 | 欠けるもの |
|---|---|
| with の項目で1つずつ開く | (e)．後から開いたものが先に閉じる |
| プールの handler の init で開き，フィールドに持つ | (e)．数は init の時点で決まり，プールより先に1つだけ release することもできない |
| `Engine` に `spawn(kind: Kind) -> EnemyId` と `despawn(id: EnemyId)` を置く | (a) と (c)．`EnemyId` は普通の値なので，capture set で持ち出しを止められない |
| 項目ごとに fiber を作り，その fiber の中の with で開く | (d)．下のコード |
| 全体を値で記述し，供給者が差分を当てる | (d)．操作は記述の変更になり，`damage` が返す HP を受け取れない |

項目ごとに fiber を作ると，項目の capability はその fiber の中の with から出せないので，ほかの fiber からは命令を Queue で送ることになる（`Std.Queue` は仕様にあり実装はまだなので，このコードは動かしていない）．

```emel
enum Command {
  MoveTo(x: Int, y: Int)
  Damage(amount: Int, reply: Deferred[Int])
  Despawn
}

fn enemy(kind: Kind, commands: Queue[Command]) {
  with e = Engine.Spawn(kind:) {
    serve(e, commands)
  }
}

fn serve(e: use Enemy, commands: Queue[Command]) {
  commands.take() match {
    MoveTo(x, y) -> {
      e.move_to(x, y)
      serve(e, commands)
    }
    Damage(amount, reply) -> {
      reply.complete(e.damage(amount))
      serve(e, commands)
    }
    Despawn -> ()
  }
}
```

操作ごとに `Command` の変種が要り，値を返す操作には返事の Deferred が要る．操作のエラーも，`Command` と返事の型に入れて運ぶことになる．

## Proposal

### Syntax

構文は足さない．`Region.scope` と `close` はふつうの宣言で書け（`capability-trait`），領域を通して handler member を開く `r.M(args)` だけが組み込みの形である．

```emel
import Std.Region.{Region}

fn play() {
  Region.scope(use Engine, |enemies| {
    a = enemies.Spawn(kind: Small)
    b = enemies.Spawn(kind: Large)
    if a.damage(10) <= 0 {
      enemies.close(a) # A is released now, before B
    }
    c = enemies.Spawn(kind: Small)
    b.move_to(10, 0)
    c.move_to(20, 0)
  }) # C, then B are released here
}

fn main() {
  with SystemEngine {
    play()
  }
}
```

```emel
# Std.Region
@intrinsic
pub effect Region[P] {
  suspend fn close[C: Capability](item: C) -> ()
}

@intrinsic
pub fn scope[P: Capability, A, E, R](provider: P, body: fn(use Region[P]) -> A fails E uses { Async, R }) -> A
  fails E
  uses { Async, R }
```

### Meaning

- `Region.scope(p, body)` は，供給者 `p` に結び付いた領域を作り，その capability（型は `use Region[P]`）を引数として `body` を呼び，`body` の値を返す
- Region は組み込みの effect．名前で供給されることはなく，Region の handler は書けない（E0413）．ラベルの元の effect にもできない（E0429）
- `r.M(args)` の M は，`p` の effect の handler member でなければならない．`args` を評価した後，`p` の M の init を呼び，init が値を返したら，その handler を領域に登録して capability を返す（型は M が供給する effect の `use G`）．init がエラーで終わるか中断されたら，何も登録せず，同じように終わる
- `r.M(args)` は組み込みの形で，effect の操作ではない．`r.M` を関数の値にはできない．init を走らせるので中断しうる
- `close(c)` は，`c` の handler をその場で release し，領域から外す．release は `close` を呼んだ fiber で走る．`close` はふつうの操作なので，`enemies.close` を関数の値にできる
- `Region.scope` が終わるとき，閉じていない項目を登録の逆の順に release する．値を返したとき，エラー，defect，中断のどれでも同じ．release の中の defect は `[dyn.defect]` に従い，残りの release も走る
- 領域は `p` を持つ．領域は `Region.scope` の呼び出しの中で終わり，その間 `p` は使えるので，項目が供給者より長生きすることはない
- 寿命のレキシカルでない部分: `capability-type` は寿命をすべてレキシカルな with の入れ子で決める．領域で開いた handler も，寿命の上限は `Region.scope` でレキシカルに決まるが，終わる時点は `close` で決まる．この提案で寿命がレキシカルでなくなるのはここだけで，その分を実行時に検査する
- 次は defect
  - 閉じた項目の操作を呼ぶ: `handler Spawn was used after release`
  - 閉じた項目を `close` する: `handler Spawn was closed twice`
  - その領域で開いていない capability を `close` する: `handler Spawn was not opened in this region`
  - 終わった領域，または終わりの release の途中にある領域に `r.M(args)` か `close` を使う: `region was used after it ended`

### Capture sets

- 根の種類に2つを足す: `Region.scope` の領域と，領域で開いた handler
- 領域の capability の capture set は，領域の根と `p` が持つ根．`r.M(args)` の値は，開いた handler の根，領域の根，`p` と `args` が持つ根を持つ
- 開いた handler の操作を呼ぶと，その根が R に入り，effect の名前（`Enemy`）に広げられる．`r.M(args)` と `close` は，領域の根を R に入れる
- これらの根は，`Region.scope` が終わるところで R から外す．`Region.scope` の値がこれらの根を持てば W0402（`capture-checking` を有効にすると E0444）
- 閉じた項目の使用は静的には止めない（どの項目をいつ閉じるかは実行時に決まる）．上の defect の検査は，0.29 で `[effect.escape]` の実行時の検査を外した後も残す

### Typing rules

- `Region.scope` と `close` の型は，上の宣言のとおり
- `r.M(a)`: `r` の型は `use Region[P]` で，P は effect F の capability の型 `use F`．M は F の handler member．型は `use G`（G は M が供給する effect）．E は a の E と M の init の E の和．R は a の R と `r` の要素の和
- Region は suspend fn を持つので Immediate ではない（`[type.effect-bound]`）

## Specification changes

### `[effect.region]` (new)

Add as 8.10, after `[effect.label]`:

> ## 8.10 Regions `[effect.region]`
>
> A region opens handlers from the handler members of one provider, and closes them at any time. When the region ends, the handlers it still holds are released.
>
> ```emel
> import Std.Region.{Region}
>
> Region.scope(use Engine, |enemies| {
>   a = enemies.Spawn(kind: Small)
>   b = enemies.Spawn(kind: Large)
>   enemies.close(a)
>   b.move_to(10, 0)
> }) # B is released here
> ```
>
> - `Region.scope(p, body)` makes a region bound to the provider `p`, calls `body` with the capability of the region, of type `use Region[P]`, and returns the value of `body` (`[api.std.region]`).
> - Region is a built-in effect. It is never provided by name. A handler for Region is a compile error (E0413). Region cannot be the underlying effect of a label (E0429).
> - `r.M(args)` is a built-in form written on the capability of a region. It is not an operation, and `r.M` cannot be made into a function value (code to be assigned). M MUST be a handler member of the effect of `p` (code to be assigned). `r.M(args)` evaluates `args`, then runs the init of the handler member M of `p`. When the init returns, the handler is registered in the region, and the value is its capability, of type `use G`, where G is the effect that M provides. When the init fails or is interrupted, nothing is registered, and `r.M(args)` ends in the same way. It may suspend.
> - `close(c)` releases the handler of `c` at that point, in the fiber that calls `close`, and removes it from the region.
> - When `Region.scope` ends, the handlers that the region still holds are released in the reverse order of registration. This holds when the body returns a value, fails, ends with a defect, or is interrupted. A defect inside one of these releases follows 14.4, and the releases of the remaining handlers run.
> - A region ends within the call of `Region.scope`, while `p` can be used. A handler opened in a region therefore never outlives its provider.
> - The upper bound of the lifetime of a handler opened in a region is the `Region.scope`, which is lexical. The point where it ends is the call of `close`, which is not. The following are defects, checked at run time:
>   - calling an operation of a handler after it has been closed (`handler Spawn was used after release`)
>   - closing a handler that has already been closed (`handler Spawn was closed twice`)
>   - closing a capability that was not opened in that region (`handler Spawn was not opened in this region`)
>   - `r.M(args)` or `close` on a region that has ended, or whose releases at its end are running (`region was used after it ended`)

### `[effect.handler]`

Add to the items on handler members (from `handler-member`):

> - A handler member of an effect F can also be opened by `r.M(args)` on a region bound to a provider of F (8.10).

### `[effect.escape]`

In the item that lists the roots (from `capture-checking`), replace "The elements of a capture set are roots: the handler of a with item, the Scope of a `Fiber.scope`, and `self` inside the operations of a handler." with:

> The elements of a capture set are roots: the handler of a with item, the Scope of a `Fiber.scope`, the region of a `Region.scope`, a handler opened in a region (8.10), and `self` inside the operations of a handler.

Add:

> - The capability of a region holds the root of the region and the roots that its provider holds. The value of `r.M(args)` holds the root of the handler it opened, the root of its region, and the roots that the provider and `args` hold. Calling an operation of that handler adds its root to R. `r.M(args)` and `close` add the root of the region to R. These roots are removed from R where the `Region.scope` ends.
> - If the value of a `Region.scope` holds the root of its region, or the root of a handler opened in it, it is a warning (W0402).
> - Experimental (`capture-checking`). It is a compile error (E0444) instead.

### `[intro.experimental]`

In the row of `capture-checking` (from `capture-checking`), replace "leaves the with or `Fiber.scope` that provides it" with "leaves the with, `Fiber.scope` or `Region.scope` that provides it".

### `[type.rules]`

Add a row:

> | `r.M(a)` (r: `use Region[use F]`, M a handler member of F) | `use G` (G is the effect of M) | Ea ∪ (E of the init of M) | Ra ∪ {r} |

### `[lib.layers]`

In the row of the effect layer, replace "State, Fiber, Exit, Cause, Deferred (0.20), Semaphore, Queue" with "State, Fiber, Exit, Cause, Deferred (0.20), Semaphore, Queue, Region".

### `[api.std.region]` (new)

Add after `[api.std.semaphore]`:

> ### Std.Region (`std/region.emel`) `[api.std.region]`
>
> Regions (`[effect.region]`). Used with `import Std.Region.{Region}`.
>
> | Declaration | Signature | Implementation | Description |
> |---|---|---|---|
> | `Region` | `@intrinsic pub effect Region[P] { suspend fn close[C: Capability](item: C) -> () }` | intrinsic | The capability of a region bound to a provider whose capability has type P. `close` releases a handler opened in the region. Handler members are opened with the built-in form `r.M(args)` (8.10) |
> | `scope` | `scope[P: Capability, A, E, R](provider: P, body: fn(use Region[P]) -> A fails E uses { Async, R }) -> A fails E uses { Async, R }` | intrinsic | Makes a region bound to `provider`, runs the body with it, releases the handlers the region still holds, and returns the value of the body |

### `[lsp.hover]`

Add (after the item on `holds:` from `capture-checking`):

> - The root of a handler opened in a region is shown by the variable that the value of `r.M(args)` is bound to. The root of a region is shown by the parameter of the body of `Region.scope`.

### `[diag.list]`

Add rows:

> | E04xx | In `r.M(args)` on a region, M is not a handler member of the effect of its provider (8.10) | ``the region of `Engine` has no handler member `Spwan` `` |
> | E04xx | `r.M` on a region is used as a function value (8.10) | ```Spawn` of a region cannot be used as a value; call it`` |

Change the row of E0413 to cover Region: "A handler written for Async, Scope or Region (8.7, 11.2, 8.10)", with the message ``handler `MyRegion` cannot implement `Region`: only `Region.scope` provides it``.

In the rows of W0402 and E0444 (from `capture-checking`), replace "leaves the with or the `Fiber.scope` that provides it (8.6, 11.8)" with "leaves the with, the `Fiber.scope` or the `Region.scope` that provides it (8.6, 11.8, 8.10)".

### `[open.next]`

In the item "0.29: make E0444 the default, retire W0402 and E0425, and remove the run-time check of 8.6" (from `capture-checking`), add:

> The run-time checks of regions (8.10) are not removed.

## Diagnostics

- [ ] In `r.M(args)`, M is not a handler member of the effect of the provider: code to be assigned
- [ ] `r.M` used as a function value: code to be assigned
- [ ] The argument of `Region.scope` or `close` is not a capability: E0326 (`capability-trait`)
- [ ] A handler for Region: E0413
- [ ] Region as the underlying effect of a label: E0429
- [ ] The value of `Region.scope` holds the root of its region or of a handler opened in it: W0402, or E0444 with `capture-checking`
- [ ] Defects: `handler X was used after release` (the existing message), `handler X was closed twice`, `handler X was not opened in this region`, `region was used after it ended`

## Compatibility

追加だけである．`Std.Region` は新しいモジュール．

## Implementation

- emela-types: `r.M(args)` の形の検査（M が供給者の effect の handler member であること，関数の値にさせないこと）と型，領域と開いた handler を根に加えること
- emela-core: 領域が持つ供給者，項目の表（登録の順），`close` での release，領域の終わりでの逆順の release，項目ごとの閉じた印
- ランタイム（JS，C，WASM）: 閉じた項目の操作の検査は，今の `[effect.escape]` の実行時の検査（release した handler の印）をそのまま使う
- std: `Std.Region`

## Alternatives

- **Motivation の表の書き方**: どれも (a) から (f) の少なくとも1つを欠く
- **開いた handler の寿命の持ち主を一般化する**: handler member を開く場所ごとに，寿命の持ち主が決まっている（with の項目なら with，init なら handler，この提案なら領域）．持ち主を指定する一般の形（開く式に持ち主を渡す）を先に足せば，`r.M(args)` はその使い方の1つになる．ただ，一般化して得をするのは今のところ領域だけで，with と init は今の形で足りている．4つ目の持ち主が要る場面が出たときに考える
- **領域を供給者に結び付けず，開く式で供給者を選ぶ（`r.open(Engine.Spawn(kind: Small))`）**: 1つの領域に，いくつもの供給者の member を入れられる．代わりに，`open` の引数にだけ handler member の式を書ける形が要る．`open` の位置から見て一番内側の供給者を使うので，領域の中の with で供給した供給者（`with TestEngine { r.open(...) }`）が先に終わりうる
- **項目を専用の型（`Item[F]`）で返す**: `close` に渡せるものを領域の項目に絞れる．代わりに，項目の操作を呼ぶたびに capability を取り出す形が要り，`use G` として引数に渡したりデータに入れたりする形から外れる
- **項目を番号（位置と世代の組）で返す**: 番号はただの値なので，capture set で持ち出しを止められない
- **届かなくなった項目を release する**: 領域は開いた項目をすべて持つので，領域が生きている間に項目が届かなくなることはない．届かなくなったことで解放するには，領域が項目を弱く持つ必要があり，領域の終わりにまとめて release できなくなる．解放の時期もプログラムから決まらなくなり，release の副作用がほかの処理の途中に割り込む

## Prior art

- Region-based memory management: MLKit (Tofte and Talpin), Cyclone.
- Monadic regions (Kiselyov and Shan), and Haskell's `ResourceT`, whose `allocate` registers a finalizer in a region and returns a key that `release` uses to run it early.
- Effekt's regions, which bound the lifetime of local mutable state to a region.
- Generational arenas (Rust's `slotmap`, the entities of Bevy).

## Open questions

- 1つの領域に複数の供給者（敵と効果音を同じ領域に入れ，寿命の上限と release の順を揃える）．`Region.scope` が capability を複数受け取る形にするか，`multi-effect-handler`（#625）で複数の effect を実装する供給者を1つ受け取る形にするか．2つの effect が同じ名前の handler member を持つときの扱い
- `handler-member` が `with x = H` を普通の handler に許せば，`with enemies = Region(use Engine) { ... }` と書け，本体を関数として渡す形をやめられる
- `handler-member` が特定の供給者の handler member を使う形（`fs.Open(p)`）を許すと，with の項目では `fs.Open(p)`，領域では `r.Open(p)` と，受け手だけが違う形が並ぶ．両方を同じ規則として書くか
- `Region.scope` の `.` の読み方．今は `.` の左にモジュールしか書けないので，`Region.scope` はモジュール `Std.Region` の関数として読む．`handler-member` が effect を `.` の左に書けるようにすると（`Engine.Spawn`），モジュールと effect が同じ名前の `Region` を，どちらとして読むかが決まらない
- 二度閉じること．この提案は defect にした．何もしないことにすると，片付けのコードが「閉じていなければ閉じる」を気にせずに済むが，同じ項目を2か所で閉じる誤りが見えなくなる
- ほかの fiber で操作の途中にある項目を `close` した場合．途中の操作が再開したときに defect にするか，操作が終わるまで release を待つか
- `Region.scope` に Async を要るか．`r.M(args)` と `close` が中断しうるので入れた．init と release が中断しない member だけを開く領域では余分になる
- experimental feature にするか．`handler-member`，`capability-type`，`capability-trait` より先に採択するなら feature `regions` として入れる

## Decision
