---
title: Region members, handler members that open and close other handler members at any time
authors: [somnicattus]
status: draft
pr: 660
issue: 616
decided:
touches: [effect.region (new), effect.decl, effect.use, effect.handler, effect.escape, type.rules, grammar.decl, grammar.expr, lib.layers, api.std.region (new), lsp.hover, diag.list, open.next]
requires: [handler-member, capability-type]
related: [capture-checking]
---

# Region members, handler members that open and close other handler members at any time

## Summary

effect が，Std.Region の `Region` を effect とする handler member（領域 member）を宣言できるようにする．領域 member を開くと領域ができ，領域を通して，その effect のほかの handler member を任意の時点で開き（init を呼び），任意の時点で閉じられる（release を呼ぶ）．領域が release されるとき，残っているものをすべて release する．

```emel
import Std.Region.{Region}

pub effect Engine {
  handler Spawn(kind: Kind): Enemy
  handler Wave: Region[use Engine]
}

with SystemEngine, enemies = Engine.Wave {
  a = enemies.Spawn(kind: Small) # opens Spawn of SystemEngine
  enemies.close(a)
}
```

領域そのものも handler member なので，with の項目，init の中，ほかの領域の中のどこでも，ほかの member と同じ規則で開ける．

| 領域を開く位置 | 領域の寿命の上限 | 領域の中のものを閉じる時点 |
|---|---|---|
| with の項目 | その with | `close` を呼んだとき，または with の終わり |
| init の中 | 開いた handler | `close` を呼んだとき，または handler の release の後 |
| ほかの領域の中 | 外側の領域 | `close` を呼んだとき，外側の領域でこの領域を閉じたとき，または外側の領域の終わり |

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

`handler-member` は，資源について次の (a) から (d) を with の項目と init で満たす．

- (a) 資源の状態は，その資源の handler が持つ
- (b) 資源の handler の実装は供給者が選ぶ．供給者を差し替えると，資源の handler も替わる
- (c) 資源を指す値も，閉じる操作も，effect の操作として出てこない
- (d) 資源を使うコードはふつうの式で，effect，エラー，値に制限がない

この提案は，次の2つを足す．

- (e) 資源を任意の時点で開き，任意の時点で閉じられる．数と順は実行時に決まる
- (f) すべての資源の寿命の上限が1つの範囲に結び付いていて，範囲が終わるとき残りが release される

(c) は供給者の effect（`Engine`）について言う．この提案の `close` は Std.Region の `Region` の操作で，`Engine` の操作ではない．

`handler-member` と `capability-type` が入った後の言語での書き方は，どれも少なくとも1つを欠く．

| 書き方 | 欠けるもの |
|---|---|
| with の項目で1つずつ開く | (e)．後から開いたものが先に閉じる |
| プールの handler の init で開き，フィールドに持つ | (e)．数は init の時点で決まり，プールより先に1つだけ release することもできない |
| `Engine` に `spawn(kind: Kind) -> EnemyId` と `despawn(id: EnemyId)` を置く | (a) と (c)．`EnemyId` は普通の値なので，capture set で持ち出しを止められない |
| 項目ごとに fiber を作り，その fiber の中の with で開き，ほかの fiber から命令を Queue で送る | (d)．操作ごとに命令の変種が要り，値を返す操作には返事の Deferred が要る．操作のエラーも命令と返事の型に入れて運ぶ |
| 全体を値で記述し，供給者が差分を当てる | (d)．操作は記述の変更になり，`damage` が返す HP を受け取れない |

範囲は1段とは限らない．ステージの間ずっといるボスと，ウェーブごとにまとめて消える雑魚は，寿命の上限が違う．操作の中で資源を開くサービス（呼ぶと敵を1体出す handler）もある．どちらも，範囲そのものを資源として開き，持ち，閉じられれば書ける．領域を handler member にすると，範囲は (a) から (d) をそのまま満たす資源になる．

## Proposal

### Syntax

```emel
import Std.Region.{Region}

pub effect Engine {
  handler Spawn(kind: Kind): Enemy
  handler Wave: Region[use Engine]
}

fn play() {
  with stage = Engine.Wave {
    boss = stage.Spawn(kind: Large)
    wave = stage.Wave
    a = wave.Spawn(kind: Small)
    b = wave.Spawn(kind: Small)
    if a.damage(10) <= 0 {
      wave.close(a) # A is released now
    }
    stage.close(wave) # the enemies left in the wave, then the wave, are released now
    boss.move_to(10, 0)
  } # the boss, then the stage, are released here
}

fn main() {
  with SystemEngine {
    play()
  }
}
```

```emel
# A handler that opens enemies in its operations
pub effect Director {
  suspend fn next_wave() -> ()
}

handler Waves(stage: use Region[use Engine]): Director {
  init {
    Self(stage: Engine.Wave)
  }

  suspend fn next_wave(resume) {
    wave = self.stage.Wave # stays until it is closed or the director is released
    wave.Spawn(kind: Small)
    wave.Spawn(kind: Small)
    resume(())
  }
}
```

```emel
# Std.Region
@intrinsic
pub effect Region[P] {
  suspend fn close[C](item: C) -> ()
}
```

- effect の中の `handler Name: Region[use F]` が領域 member の宣言．F は宣言している effect 自身．引数と `fails` はない
- 領域の capability に対して `r.M(args)` と書ける（引数のない member は `r.M`）．M は F の handler member
- `x.M` の形（`.` の後に型の名前）を式に足す（`[grammar.expr]`）

### Meaning

- 領域 member は言語が自動で付けるものではない．effect の作者が宣言したときだけあり，名前も作者が決める
- 領域 member は，F の handler が実装しない．Std.Region が実装する．Region は組み込みの effect なので，その inner handler を書くのは E0413．領域 member の実装がないことは E0404 にならない
- 領域 member は，ほかの handler member と同じ規則で開く．with の項目 `r = F.M` と `r = c.M`（c は F の capability，`handler-member`），init の中の `F.M` と `c.M`，ほかの領域の中の `r.M`．開いた元の F の handler が，その領域の供給者になる
- `handler-member` の `c.M(args)`（c は member を持つ effect の capability）は，with の項目と init の中にしか書けない．領域の capability の型 `use Region[use F]` の Region は member を持たないので，`r.M(args)` はそれとは別の，この提案の形である．どこにでも書ける
- with の項目 `x = r.M(args)` は書けない（仮．Open questions）．領域で開いたものの持ち主は領域で，with ではない
- 供給者は領域を開いた時点で決まり，その後は変わらない．`r.M(args)` を書いた位置の内側の with で F を供給しても，r の供給者は変わらない
- `r.M(args)` は組み込みの形で，effect の操作ではない．`r.M` を関数の値にはできない．args を評価した後，r の供給者の M の init を呼び，init が値を返したら，その handler を領域に登録して capability を返す（型は M が供給する effect の `use G`）．init がエラーで終わるか中断されたら，何も登録せず，同じように終わる．中断しうる（どの供給者の init が走るかは静的に決まらないので，E0417 の判定では中断しうる呼び出しに数える）．`close` も suspend fn なので，どちらも fn 操作の本体には書けない（E0417）．handler の操作の中で開くなら suspend fn にする
- M が領域 member なら，`r.M` は r に登録された子の領域を開く．子の領域の供給者は r と同じ
- `close(c)` は，`c` の handler をその場で release し，領域から外す．release は `close` を呼んだ fiber で走る．`close` はふつうの操作なので，`enemies.close` を関数の値にできる．c の型は F の handler member が供給する effect の capability でなければならない
- 領域が release されるとき（with の終わり，init で開いた handler の release の後，または外側の領域での `close`），閉じていないものを登録の逆の順に release する．値を返したとき，エラー，defect，中断のどれでも同じ．release の中の defect は `[dyn.defect]` に従い，残りの release も走る
- 寿命: 領域は供給者より長生きしない（handler member の規則）．領域で開いたものは領域より長生きしない．開いたものの init と release が使う effect は供給者の requirement（`handler-member`）なので，領域が開くときも閉じるときも供給されている．Region は自分の requirement を持たない（Async も要らない）
- Region は組み込みの effect．名前で供給されることはない（handler member の項目と同じく，束縛した capability からだけ使う）．ラベルの元の effect にもできない（E0429）
- 寿命のレキシカルでない部分: 領域の中のものの寿命の上限は領域でレキシカルに決まるが，終わる時点は `close` で決まる．この分を実行時に検査する
- 次は defect
  - release されたもの（`close` で閉じたもの，または領域と一緒に release されたもの）の操作を呼ぶ: `handler Spawn was used after release`
  - 閉じたものを `close` する: `handler Spawn was closed twice`
  - その領域で開いていない capability を `close` する: `handler Spawn was not opened in this region`
  - release された領域，または release の途中にある領域に `r.M(args)` か `close` を使う: `handler Wave was used after release`

### Capture sets

- 根の種類に1つを足す: 領域で開いた handler．with の項目や init で開いた領域は，今ある種類（with の項目の handler，handler の `self`）のまま
- `r.M(args)` の値は，開いた handler の根，`r` が持つ根，args が持つ根を持つ．子の領域も同じ
- 開いた handler の操作を呼ぶと，その根が R に入り，effect の名前（`Enemy`）に広げられる．`r.M(args)` と `close` は，`r` が持つ根を R に入れる
- 領域で開いた handler の根は，領域の根が R から外れるところで外れる
- 開いた値は領域の根を持つので，領域を供給する with の外へ出る値は，今の規則（W0402，`capture-checking` を有効にすると E0444）で止まる．新しい規則は要らない
- 閉じたものの使用は静的には止めない（どれをいつ閉じるかは実行時に決まる）．上の defect の検査は，0.29 で `[effect.escape]` の実行時の検査を外した後も残す

### Typing rules

- 領域 member `handler M: Region[P]` の P は，宣言している effect F の capability の型 `use F` でなければならない
- `r.M(a)`: `r` の型は `use Region[use F]`．M は F の handler member．型は `use G`（G は M が供給する effect．M が領域 member なら `use Region[use F]`）．E は a の E と M の init の E の和．R は a の R と `r` の要素の和
- `close(c)`: c の型は `use G` で，G は F のいずれかの handler member が供給する effect．この制約は型引数 C が決まる位置で検査する．`enemies.close` を関数の値にしたときも，C が決まる位置で検査する
- Region は suspend fn を持つので Immediate ではない（`[type.effect-bound]`）

## Specification changes

### `[effect.region]` (new)

Add as 8.10, after `[effect.label]`:

> ## 8.10 Region members `[effect.region]`
>
> A region member is a handler member whose effect is Region (`[api.std.region]`). A region opens handlers from the handler members of its effect at any time, and closes them at any time. When the region is released, the handlers it still holds are released.
>
> ```emel
> import Std.Region.{Region}
>
> pub effect Engine {
>   handler Spawn(kind: Kind): Enemy
>   handler Wave: Region[use Engine]
> }
>
> with SystemEngine, enemies = Engine.Wave {
>   a = enemies.Spawn(kind: Small)
>   b = enemies.Spawn(kind: Large)
>   enemies.close(a) # A is released now
>   b.move_to(10, 0)
> } # B is released here
> ```
>
> - A handler member whose effect is Region is a region member. An effect F MAY declare region members. The type argument of their Region MUST be `use F`, the capability type of F itself (code to be assigned). A region member has no parameters and no `fails` (code to be assigned).
> - The handlers of F do not implement a region member: Std.Region provides it. An inner handler for it is a handler for Region, a compile error (E0413). A handler of F that has no inner handler for it is not E0404.
> - A region member is opened as any other handler member (8.3, 8.4): as a with item `r = F.M` or `r = c.M`, where c is a capability of F, inside an init, or through a region (below). The handler of F that it is opened from is the provider of the region. The provider does not change afterwards: an inner with that provides F where `r.M(arguments)` is written does not affect r.
> - `r.M(arguments)`, where r is a capability of type `use Region[use F]`, is a built-in form; without arguments it is written `r.M`. It is not an operation, and `r.M` cannot be made into a function value (code to be assigned). Region has no handler members, so this is not the form `c.M(arguments)` of 8.3 and 8.4, and unlike that form it can be written in any expression. It cannot be written as a with item `x = r.M(arguments)`: what it opens is owned by the region, not by the with (code to be assigned). M MUST be a handler member of F (code to be assigned). `r.M(arguments)` evaluates the arguments, then runs the init of M of the provider of r. When the init returns, the handler is registered in the region, and the value is its capability, of type `use G`, where G is the effect that M provides. When the init fails or is interrupted, nothing is registered, and `r.M(arguments)` ends in the same way.
> - `r.M(arguments)` may suspend: which provider's init runs is not known statically, so it counts as a call that may suspend for E0417 (8.1). `close` is a suspend fn. Neither can be written in the body of a fn operation whose R is not Immediate (E0417); a handler that opens or closes through a region in its operations does so in suspend fn operations.
> - When M is a region member, `r.M` opens a region registered in r, with the same provider.
> - `close(c)` releases the handler of c at that point, in the fiber that calls `close`, and removes it from the region. The type of c MUST be `use G`, where G is the effect that a handler member of F provides (code to be assigned). This is checked where the type argument of `close` is determined, also when `close` is taken as a function value (`enemies.close`).
> - When a region is released (at the end of its with, after the release of the handler whose init opened it, or by `close` on the region it was opened through), the handlers it still holds are released in the reverse order of registration. This holds however the region comes to be released: a value, an error, a defect or an interruption. A defect inside one of these releases follows 14.4, and the releases of the remaining handlers run.
> - A region does not outlive its provider, and a handler opened in a region does not outlive the region. The effects that the init and the release of an opened handler use are requirements of the provider (8.4), so they are provided whenever the region opens or releases it. Region has no requirements of its own.
> - Region is a built-in effect. It is provided only by region members, never by name: the region is reached only through the bound capability. Region cannot be the underlying effect of a label (E0429).
> - The upper bound of the lifetime of a handler opened in a region is the region, which is lexical. The point where it ends is the call of `close`, which is not. The following are defects, checked at run time:
>   - calling an operation of a handler after it has been released, by `close` or with its region (`handler Spawn was used after release`)
>   - closing a handler that has already been closed (`handler Spawn was closed twice`)
>   - closing a capability that was not opened through that region (`handler Spawn was not opened in this region`)
>   - `r.M(arguments)` or `close` on a region that has been released, or whose releases are running (`handler Wave was used after release`)

### `[effect.decl]`

Add to the items on handler members (from `handler-member`):

> - A handler member whose effect is Region is a region member (8.10).

Add to the list of "Whether a call may suspend is determined as follows":

> - A handler member opened through a region (`r.M(arguments)`, 8.10) may suspend.

### `[effect.use]`

In the item on `c.M(arguments)` (from `handler-member`), add:

> On the capability of a region, `r.M(arguments)` opens a handler member through the region instead, and can be written in any expression (8.10).

### `[effect.handler]`

Add to the items on handler members (from `handler-member`):

> - A handler of an effect with a region member does not implement it (8.10).
> - A region member opened inside an init is a region that lives as long as the handler whose init opened it. The suspend fn operations of the handler can open handlers through it (`self.stage.Spawn(kind: Small)`, 8.10).

### `[effect.escape]`

In the item that lists the roots (from `capture-checking`), replace "The elements of a capture set are roots: the handler of a with item, the Scope of a `Fiber.scope`, and `self` inside the operations of a handler." with:

> The elements of a capture set are roots: the handler of a with item, the Scope of a `Fiber.scope`, a handler opened through a region (8.10), and `self` inside the operations of a handler.

Add:

> - The value of `r.M(arguments)` holds the root of the handler it opened, the roots that r holds, and the roots that the arguments hold. Calling an operation of that handler adds its root to R. `r.M(arguments)` and `close` add the roots that r holds to R. The root of a handler opened through a region is removed from R where the roots of the region are removed.
> - A value that holds the root of a handler opened through a region also holds the roots of the region, so leaving the with that provides the region is checked by the rules above.

### `[type.rules]`

Add a row:

> | `r.M(a)` (r: `use Region[use F]`, M a handler member of F) | `use G` (G is the effect of M) | Ea ∪ (E of the init of M) | Ra ∪ {r} |

### `[grammar.decl]`

Replace `member_sig` (from `handler-member`) with:

```ebnf
member_sig   = "handler" type_name [ "(" [ params ] ")" ] ":" type_ref [ "[" type { "," type } "]" ]
               [ "fails" error_set ]
```

### `[grammar.expr]`

Replace `postfix` and `supply` (with the alternatives that `handler-member` adds for `c.M(arguments)`) with:

```ebnf
postfix    = primary { "(" [ args ] ")" | "." lower_name | "." type_name }
supply     = type_ref [ "." type_name ] [ "(" [ args ] ")" ]
           | lower_name "." type_name [ "(" [ args ] ")" ]      (* a handler member of a capability; 8.3 *)
```

Add:

> - `"." type_name` after an expression opens a handler member: `c.M(arguments)` on a capability of an effect with handler members, inside an init (8.4), and `r.M(arguments)` on the capability of a region, in any expression (8.10): `stage.Wave`, `self.stage.Spawn(kind: Small)`. A `type_ref` is read as a whole first, so `Engine.Wave` is a `type_ref`, not this form. On any other value, or for `c.M(arguments)` outside an init, it is a compile error (code to be assigned).
> - The parentheses are omitted for a handler member without parameters (`engine.Wave`, `stage.Wave`), as for a handler whose init takes none (8.4).

### `[lib.layers]`

In the row of the effect layer, replace "State, Fiber, Exit, Cause, Deferred (0.20), Semaphore, Queue" with "State, Fiber, Exit, Cause, Deferred (0.20), Semaphore, Queue, Region".

### `[api.std.region]` (new)

Add after `[api.std.queue]`:

> ### Std.Region (`std/region.emel`) `[api.std.region]`
>
> Regions (`[effect.region]`). Used with `import Std.Region.{Region}`.
>
> | Declaration | Signature | Implementation | Description |
> |---|---|---|---|
> | `Region` | `@intrinsic pub effect Region[P] { suspend fn close[C](item: C) -> () }` | built-in | The capability of a region whose provider has the capability type P. An effect declares a region member with it (`handler Wave: Region[use Engine]`), and Std.Region implements that member. `close` releases a handler opened in the region; the type of its argument is restricted by 8.10. Handler members are opened with the built-in form `r.M(arguments)` (8.10) |

### `[lsp.hover]`

Add (after the item on `holds:` from `capture-checking`):

> - The root of a handler opened through a region is shown by the variable that the value of `r.M(arguments)` is bound to.

### `[diag.list]`

Add rows:

> | E04xx | The type argument of the Region of a region member is not the capability type of the effect that declares it (8.10) | ``region member `Wave` of `Engine` must be `Region[use Engine]` `` |
> | E04xx | A region member has parameters or `fails` (8.10) | ``region member `Wave` cannot take parameters`` |
> | E04xx | In `x.M(arguments)`, x is not the capability of a region, or M is not a handler member of the effect of the region (8.10) | `` `log` is not a region; only a region can open `Spawn` `` / ``the region of `Engine` has no handler member `Spwan` `` |
> | E04xx | `x = r.M(arguments)` on a region is written as a with item (8.10) | `` `Spawn` of a region is opened in an expression; it is owned by the region, not by this with `` |
> | E04xx | `r.M` on a region is used as a function value (8.10) | `` `Spawn` of a region cannot be used as a value; call it `` |
> | E04xx | The argument of `close` is not the capability of a handler member of the effect of the region (8.10) | `` `close` of the region of `Engine` takes a handler opened through it, not `use Db` `` |

Change the row of E0413 to cover Region: "A handler written for Async, Scope or Region (8.7, 11.2, 8.10)", with the message ``handler `Wave` cannot implement `Region`: Std.Region provides it``.

### `[open.next]`

In the item "0.29: make E0444 the default, retire W0402 and E0425, and remove the run-time check of 8.6" (from `capture-checking`), add:

> The run-time checks of region members (8.10) are not removed.

## Diagnostics

- [ ] The type argument of Region in a region member is not the declaring effect: code to be assigned
- [ ] A region member with parameters or `fails`: code to be assigned
- [ ] In `x.M(arguments)`, x is not a region, or M is not a handler member of the effect of the region: code to be assigned
- [ ] `r.M(arguments)` or `close` in the body of a fn operation whose R is not Immediate: E0417 (extended to `r.M`)
- [ ] `r.M` used as a function value: code to be assigned
- [ ] `x = r.M(arguments)` written as a with item: code to be assigned
- [ ] The argument of `close` is not the capability of a handler member of the effect of the region: code to be assigned
- [ ] An inner handler for a region member, or a handler for Region: E0413 (extended)
- [ ] Region as the underlying effect of a label: E0429
- [ ] A value that holds the root of a handler opened through a region leaves the with that provides the region: W0402, or E0444 with `capture-checking` (existing rule)
- [ ] Defects: `handler X was used after release` (the existing message, also for a released region), `handler X was closed twice`, `handler X was not opened in this region`

## Compatibility

追加だけである．`Std.Region` は新しいモジュール．既存の effect に領域 member を足しても，その effect の handler は実装を足さなくてよいので壊れない．

## Implementation

- emela-syntax: `member_sig` の括弧を省けること，式の `x.M`
- emela-resolve: `r.M` の M を領域の effect の handler member として解決する
- emela-types: 領域 member の宣言の検査（型引数，引数と `fails` がないこと），handler が領域 member を実装しないこと，`r.M(args)` の検査と型（関数の値にさせない），`r.M(args)` を E0417 の中断しうる呼び出しに数えること，`close` の引数の検査（関数の値にしたときも），領域で開いた handler を根に加えること
- emela-lsp: 領域で開いた handler の根を，束縛した変数で `holds:` に出す
- emela-core: 領域が持つ供給者，項目の表（登録の順），`close` での release，領域の release での逆順の release，項目ごとの閉じた印，子の領域
- ランタイム（JS，C，WASM）: 閉じたものの操作の検査は，今の `[effect.escape]` の実行時の検査（release した handler の印）をそのまま使う
- std: `Std.Region`

## Alternatives

- **領域を関数で作る（`regions`，#651）**: `Region.scope(use Engine, |enemies| { enemies.Spawn(...) })`．開く形（`r.M`）と供給者への結び付きはこの提案と同じ．違いは領域の作り方で，次の点で劣る
  - 供給者を同じ with に並べられない．`with SystemEngine { Region.scope(use Engine, ...) }` と2段になる
  - 本体が無名関数なので，E と R を型変数で運ぶ関数の型（`scope[P: Capability, A, E, R](...)`）が要る．`escape` を with に付けられない
  - 領域がいつ release されるか（本体がどう終わっても release されること）と領域の根に，`Region.scope` のための規則が要る．この提案では with の項目と handler member の規則で足りる（中身を逆順に release する規則は，どちらの案にも要る）
  - init で開いてフィールドに持つこと（操作の中で開くサービス）と，入れ子の領域をまとめて閉じることができない
  - `Region.scope` の `.` をモジュールと effect のどちらとして読むかが決まらない
  - 供給者を値で受け取り，`close` が任意の capability を受け取るので，`capability-trait` が要る
- **領域を with の項目にする（`with r = Region(use Engine) { ... }`）**: release と根は with の規則で足りる．ただ，項目の引数は with の外で評価されるので，供給者を同じ with に並べられない．`Region[P]` は型引数を取る handler になり，generic な handler がないという規則の例外になる
- **すべての effect に領域 member を自動で付ける**: 言語が effect に member を足すことになる．領域は標準ライブラリの機能で，effect の作者が選んで宣言するものとする
- **位置で持ち主を決める（`with r = Region { a = Engine.Spawn(kind: Small) }`）**: 開く式が with の項目，init，領域の本体で同じ `F.M(args)` になり，1つの領域に複数の供給者を入れられる．代わりに，開ける位置を字面で限る規則，本体の中の with が供給者を差し替えることを禁じる規則，1つの with に Region の項目は1つまでという規則が要る．操作の中では開けず，入れ子の領域をまとめて閉じることもできない
- **持ち主を受け手にし，開くものを引数にする（`r.open(Engine.Spawn(...))`，`r.open(PostgresUsers(url: u))`）**: 普通の handler も開ける．`open` の引数にだけ書ける組み込みの形が要り，供給者は `open` の位置で一番内側のものになるので，本体の中の with で供給した供給者が領域より先に終わりうる．領域で開いた普通の handler を名前で引けないようにする規則も要る．普通の handler を開く必要が出たときに，この提案に足せる
- **`close` を `Capability` で制約する（`capability-trait`）**: `close[C: Capability](item: C)` は `use Db` も受け取り，実行時の defect でしか止まらない．この提案の `close` は，領域の effect の handler member が供給する effect の capability だけを受け取る
- **どこでも開け，持ち主は供給者**: 開ける位置の規則も領域も要らない．代わりに寿命の上限が供給者の with まで延び，閉じ忘れたものはそこまで残る．(f) を欠き，init で開いたものも handler と一緒に閉じなくなる
- **開く前の handler を値にする（`fn pick(kind: Kind) -> handler Enemy`，`r.open(h)`）**: 開くものを実行時に選べる．`handler-member` が退けた理由（静的な解決，値と型と置ける位置の規則，init の E の行き先）がそのまま当てはまる
- **項目を専用の型（`Item[F]`）で返す**: `close` に渡せるものを領域の項目に絞れる．代わりに，項目の操作を呼ぶたびに capability を取り出す形が要り，`use G` として引数に渡したりデータに入れたりする形から外れる
- **項目を番号（位置と世代の組）で返す**: 番号はただの値なので，capture set で持ち出しを止められない
- **届かなくなった項目を release する**: 領域は開いた項目をすべて持つので，領域が生きている間に項目が届かなくなることはない．届かなくなったことで解放するには，領域が項目を弱く持つ必要があり，領域の終わりにまとめて release できなくなる．解放の時期もプログラムから決まらなくなり，release の副作用がほかの処理の途中に割り込む

## Prior art

- Region-based memory management: MLKit (Tofte and Talpin), Cyclone.
- Monadic regions (Kiselyov and Shan), and Haskell's `ResourceT`, whose `allocate` registers a finalizer in a region and returns a key that `release` uses to run it early.
- Effekt's regions, which bound the lifetime of local mutable state to a region.
- Generational arenas (Rust's `slotmap`, the entities of Bevy).

## Open questions

- 「領域は供給者より長生きしない」は `handler-member` の規則に頼っている．`handler-member` には，init の中の with で供給した供給者から member を開くと，開いたものが供給者より長生きする穴がある（`init(p: String) { with TestFiles { Files.Open(p) } ... }`．開いたものは値として外に出ないので capture set でも止まらない）．領域 member を init で開く場合も同じなので，`handler-member` がこの穴を塞ぐ（開く式の effect を，開く位置と持ち主のあいだの with が供給していてはならない）ことを前提にするか，この提案で塞ぐか
- 領域 member の型引数を，宣言している effect 自身（`Region[use F]`）に限るか．ほかの effect を許すと，1つの effect が別の effect の member を開く領域を持てるが，供給者がどれになるかの規則が要る
- 1つの領域に複数の供給者（敵と効果音を同じ領域に入れ，寿命の上限と release の順を揃える）．`multi-effect-handler`（#625）で複数の effect を実装する供給者に領域 member を置く形にするか．2つの effect が同じ名前の handler member を持つときの扱い
- 引数のない handler member の書き方．この提案は `handler Wave: Region[use Engine]` と括弧を省き，開くときも `Engine.Wave`，`engine.Wave`，`stage.Wave` と書く．`handler-member` の `member_sig` と `c.M(...)` の文法は括弧を要るので，ほかの引数のない member（`handler Lock: Held` など）も括弧を省けるようにするか
- with の項目 `x = r.M(args)`．この下書きでは書けないとした（領域で開いたものの持ち主は領域）．許すなら，「領域の供給者から開き，持ち主はその with」という意味になり，`x = c.M(args)` と同じものを供給者の capability なしに書ける形になる
- `c.M(args)`（`handler-member`．with の項目と init の中だけ，持ち主はその with か handler）と `r.M(args)`（どこでも，持ち主は領域）が同じ見た目で別の規則になる．受け手の型（member を持つ effect か Region か）で読み分けるのでよいか
- 二度閉じること．この提案は defect にした．何もしないことにすると，片付けのコードが「閉じていなければ閉じる」を気にせずに済むが，同じ項目を2か所で閉じる誤りが見えなくなる
- ほかの fiber で操作の途中にあるものを `close` した場合．途中の操作が再開したときに defect にするか，操作が終わるまで release を待つか
- 閉じた後の使用を静的に見つける lint．同じ関数の中で，`close(x)` の後のどの経路でも `x` に触れていれば警告する．領域の根が R に出ること（関数の境界）と，fiber が中断点でしか切り替わらないこと（`[fiber]`）を根拠にできる．別の提案にするか
- experimental feature にするか．`handler-member` と `capability-type` より先に採択するなら feature `region-member` として入れる

## Decision
