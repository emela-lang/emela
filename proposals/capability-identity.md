---
title: "Capabilities are told apart in types and in R"
authors: [Alliana]
status: draft
pr: 732
issue: 719
decided:
touches: [glossary.terms, effect.use, effect.with, effect.handler, effect.escape, fiber.scope, fiber.escape, fiber.fork, type.rules, open.next, diag.list]
requires: [effect-alias]
related: [closure-r-by-caller, capability-type, capability-trait, handler-member, handler-init-arguments, capture-checking, scope-handle-values]
---

# Capabilities are told apart in types and in R

## Summary

`effect-alias`（#731）と `closure-r-by-caller`（#736）の次の段．二つは #791 で実装され，次の minor（0.29）に入る．

- capability を「init が終わり，まだ release されていない handler（handler の実体）」と定める．with の項目ごとに一つできる．
- `with x = Item` の x は capability そのもの．型検査は，型と R の中で，どの capability かを区別する．
- capability の操作を呼ぶと，R にはその capability が入る（`x.read()` なら x）．capability の型は，どの capability かを集合で持つので，別々の with の capability も同じリストに入れられる．R に入るものは，effect，宣言した label，capability の三種類になる．
- 原則は一つ: capability は，それを作った with の外へ持ち出せない．型か R にその capability を含む値は with の外に出せず，型検査がこれを見る．どの道を通っても型に capability が残るので，道を数える必要がない．
- capability の型はソースでは `use E` のまま書く．引数の `x: use E` は，呼ぶ側が渡す capability を受け取る．
- capability を元の effect に広げない．
- capability は普通の値のまま．渡す，格納する，返す，データ構造に入れる，closure で捕えることは，同じ with の中ならほかの値と同じにできる．
- capability についての capture checking（capture set，root）は，この型検査で置き換える．
- `use E` の式（#791 で W0403）と，capability を effect に広げること（この proposal で deprecated）を，段階を踏んでなくす．

## Motivation

### #791 のあとに残っているもの

#791 で，名前を付けずに操作を呼ぶ形は型（R）だけで守られるようになった．

- `x <- E` と `(use E).op(...)` は root を作らず，E は closure の R に残って呼ぶ側が満たす（#731）．
- closure と local fn の R は呼ぶ側だけが満たす．囲む with は handler を渡さない（#736）．#719 の再現 2〜5 と，scope の子に渡した closure（再現 3）は，外の handler に届くようになった．
- std，examples，tests/laws の `use E` の式は `<-` に書き換えた（`(use E).op` も `x = use E` も 0 件）．

固定された値は，まだ capture checking だけが守っている．

- 名前付きの項目の capability（`with x = H` の x）と，値として書いた `use E`（W0403）．closure がこれを捕えると，closure の型の R には effect の名前が入り（広げる），型は何も止めない．
- 持ち出す道は capture set で数えている．操作の引数とエラー（#719 の A，B）は今も見ていない．host の callback は #736 で effect を R に入れられなくなったが，capability を捕えた callback（`set_timeout(10, || g.greet("later"))`）が with より長く生きることは止めていない．
- #701（handler member の capability を closure の引数や戻り値を通して使うと E0402）は残っている．
- Fiber，Deferred，Queue のハンドルを持ち出す形（#719 の再現 1，6）も capture set で見ている．
- #666 は 0.29 で capture checking を既定にし，W0402 と実行時の検査を外す予定だった．この proposal は，capability についてはその代わりになる．

### 型が，どの capability かを表していない

- 今の capability の型は `use ReadFile` で，どの with の handler かが型に出ない（`capability-type`）．capability の操作は R に effect の名前を足す（`[effect.use]`）．
- どの handler かは，型とは別の capture set で追っている（`[effect.escape]`）．そのため，with の外へ出る道を一つずつ数える必要がある．
  - 今は本体の値しか見ていない（#719）．操作の引数，エラー，関数の境目が漏れている．
  - host への callback と Deferred / Queue はまだ決まっていない（#666）．
- R に effect の名前が入るので，handler member の capability の操作は，member の with で外せない（#701）．開いた資源をリストに入れて `map` で回すと E0402 になる．
- R の中で capability が effect の名前に広げられるので，型だけでは何も止まらない（#719 の再現 2〜5 が `uses { Greeter }` で通る理由）．

### 区別すれば，型で原則を守れる

- capability は with の本体の中でだけ生きている．with の中で宣言した型引数が外へ出せないのと同じく，capability を含む型と R は本体の外に書けない．
- 外で型が決まったもの（外の `State` の型引数，外の handler の操作の引数の型，エラーの型，関数の型引数）には capability を入れられない．持ち出す道はすべて型エラーになる．
- R にも capability が入るので，その with が外せる（#701 が直る）．

### 原則は今と同じ

- 「capability を持ち出せない」は今の `[effect.escape]` の原則そのもの．変わるのは守り方で，capture set の代わりに型が守る．
- `capability-type` の動機（capability を関数に渡す，格納する，返す）も残る．capability は普通の値のまま．

## Proposal

### 用語

| 英語 | 日本語 | 何か | ソースでの形 |
|---|---|---|---|
| handler | handler | 宣言（`handler Plain: Greeter`）．今と同じ | `Plain` |
| capability | capability（能力） | init が終わり，まだ release されていない handler．handler の実体．with の項目ごとに一つ，`Fiber.scope` ごとに Scope の一つができる．値として扱える | `with x = Item` の x，引数の x |
| capability type | capability の型 | capability の型．ソースでは `use E` と書き，型検査はどの capability かまで区別する | `use ReadFile` |
| capability parameter | capability の引数 | 型が `use E` の引数．どの capability かは呼び出しで決まる | `fn copy(from: use Db)` |
| named item | 名前付きの項目 | `with x = Item`．その項目の capability に x という名前を付ける | `with f = Files.Open(p)` |
| provide by name | 名前で供給する | with が R から effect の名前を外すこと．名前のない項目の capability はこれだけをする | `with Plain { ... }` |
| leak | 持ち出す | 型か R に capability を含む値が，その capability を作った with の外へ出ること．コンパイルエラー | |
| widen | 広げる | R の中で capability を元の effect の名前として扱うこと．この proposal で deprecated | |
| effect alias | 効果の別名 | `x <- E` の x（#731）．capability ではなく，R に E そのものを入れる | `log <- Logger` |

- R の要素は，effect，宣言した label，capability．
- capability ではないもの: effect alias（名前で供給される E を呼ぶ名前），handler の宣言，`L by P` の L（宣言した label）．
- 使わなくなるもの: capture checking の root と hold（抱える）は，capability には使わない．Fiber などのハンドルにも同じ原則を広げるなら，そちらでも使わなくなる（Open questions）．

### capability

- with の項目は，init を走らせて capability を作る．with を出るときに release し，capability は終わる．
- `with x = Item` は，その capability に x という名前を付ける．handler member も同じ（`with f = Files.Open(path)`）．
- 名前のない項目（`with Plain { ... }`）の capability は，名前で供給するだけ．本体の中の `x <- Greeter` や `(use Greeter).op()` は R に Greeter を入れ，この with がそれを外す．
- `L by x` の右に capability を書ける（x を L に差し込む）．

```emel
with primary = PrimaryDb {
  with replica = ReplicaDb {
    copy(replica, primary)
    primary.execute("...")
  }
}

with f = Files.Open("a.txt") {
  lines = read_all(f)
  [f].map(|x| x.read(10)) # closure の R は f．この with が外す
}
```

### R と型

- capability x の操作を呼ぶと，R に x が入る．x を作った with が，R から x を外す．
- 型が `use E` の値は，型検査の中では，どの capability かを持つ．`[f]` の要素の型は f の capability．
- capability を元の effect の名前に広げない．R に f を持つ closure は，`uses { ReadFile }` と書いた関数型には入らない．

### capability の集合

- capability の型は，どの capability かを一つではなく集合で持つ．ソースの形は `use E` のまま．
- 別々の with で作った capability も，同じリストに入れられる．`[x, y]` の要素の型は「E の capability のうち x か y」．単一化は R と同じく集合の和で行い，一般の部分型は使わない．
- 集合を持つ値の操作を呼ぶと，R に集合のすべてが入る（`[x, y]` の要素なら x と y）．どれが来ても足りるように，すべてを要求する．
- x の with と y の with がそれぞれ外す．`[x, y]` は x と y の両方が生きている範囲でしか使えず，持ち出せない原則がそのまま働く．
- 失うのは精度だけ．要素ごとにどの capability かは型から分からないが，安全には影響しない．

```emel
with x = Counters.Open(1), y = Counters.Open(10) {
  ns = [x, y].map(|c| c.next()) # closure の R は x と y．この with が両方を外す
}
```

### capability の引数

- 引数の型に `use E` と書くと，その関数は E の capability の集合を受け取る型引数を持つ．関数の中の `x.read()` は R にその型引数を入れる．
- 呼び出しで，型引数は渡した値の集合に決まる．`read_all(f)` は呼ぶ側の R に f を入れ，`[x, y]` の要素を渡せば x と y を入れる．
- 同じ effect の引数を二つ取ると，型引数も二つになる（`fn copy(from: use Db, to: use Db)`）．
- 引数に effect alias や `use E`（名前のない値）を渡すと，型引数は effect の名前 E に決まる．呼んだ時点でいちばん内側の E に届く．

### 関数型の中の capability の引数

- 関数型の引数に書いた `use E`（`fn(use ReadFile) -> String`）は，その関数型で量化する．E のどの capability の集合を渡されても動く関数の型になる．
- そのような関数型を期待する位置に書いた closure は，中身の分からない集合 C を受け取るものとして型検査する．C を戻り値に入れることはできない．closure の R には C が入ってよく，closure を呼んだ側でその集合の持ち主の with が外す．
- 期待する型がない closure（`k = |fs| ...`）は量化しない．引数の集合はふつうの集合の変数になり，使い方から決まる．量化した型は推論せず，書いてある型と照らし合わせるだけにする．
- capability の型引数に名前を付ける構文（`[C: use Db]`）は足さない．二つの引数が同じ集合を共有する形は，capability を引数で渡す代わりに，それを使う closure に捕えさせて書く．

```emel
fn retry(attempts: Int, f: fn() -> ()) { ... }

with primary = PrimaryDb {
  retry(3, || primary.execute("...")) # closure の R は primary．この with が外す
}
```

### 開く数が実行時に決まる資源

- with の項目は一つにつき capability を一つ作るので，`paths.map(|p| Files.Open(p))` とは書けない．with を再帰で入れ子にして書く．
- capability の引数は集合の型引数なので，再帰で呼ぶたびに集合が増えても型が付く（シグネチャに型があるので，多相再帰を推論する必要はない）．
- 寿命は入れ子になり，開いた順の逆に閉じる．同じ寿命で使う用途はこれで書ける．

```emel
fn concat_all(paths: List[String], acc: List[use ReadFile]) -> String {
  match paths {
    [] -> acc.map(|f| f.read_all()).join("")
    [p, ..rest] -> with f = Files.Open(p) { concat_all(rest, acc ++ [f]) }
  }
}
```

- 本体を関数として受け取る汎用の補助関数も，関数型の中の capability の引数で書ける．

```emel
fn with_all[T](paths: List[String], acc: List[use ReadFile], k: fn(List[use ReadFile]) -> T) -> T {
  match paths {
    [] -> k(acc)
    [p, ..rest] -> with f = Files.Open(p) { with_all(rest, acc ++ [f], k) }
  }
}
```
- この proposal は regions（#651）に頼らない．任意の時点で開閉する用途に regions が要るかは #651 で決める．この proposal のもとでは，pool が返す番号の型に pool の capability を入れて，持ち出しを型で止める形も取れる．

### capability を持つ型

- 型の宣言に，その型の値が E の capability を持つことを書ける（たたき台: `pub type Fiber[A] by Scope`）．
- E の capability c の操作がこの型の値を返すと，その値の型は c を持つ．名前で E を呼んだ操作（`x <- E` や `Fiber.fork`）が返した値の型は，effect の名前 E を持つ．
- capability を持つ値なので，持ち出せない原則がそのまま働く．
- 利用者の型にも使える．pool が返す番号を `pub type EnemyId by Engine` とすれば，番号は pool の with の外へ出せない．

### 名前で供給する with と，型の中の effect の名前

- 名前のない項目（`with Plain { ... }`）や `Fiber.scope` が E を名前で供給するとき，持ち出しの検査では，値の型の中の名前 E を自分の capability として数える．
- `Fiber.fork(f)` が返す Fiber は，型に名前 Scope を持つ．それを名前で供給した scope の外へは出せない．

### `Fiber.scope`

- `Fiber.scope` が作る Scope は capability．本体は，その capability を引数で受け取る．引数の型は量化した `use Scope[E, R]` なので，本体は s を含む値を返せない．

```emel
pub fn scope[A, E, R](body: fn(use Scope[E, R]) -> A fails E uses { Async, R }) -> A
  fails E
  uses { Async, R }

@external
pub type Fiber[A] by Scope

@intrinsic
pub effect Scope[E, R] {
  fn fork[A](f: fn(use Scope[E, R]) -> A fails E uses { R }) -> Fiber[A]
  fn deferred[A]() -> Deferred[A]
  fn semaphore(permits: Int) -> Semaphore
  fn queue[A](capacity: Int) -> Queue[A]
}
```

- `Fiber`，`Deferred`，`Semaphore`，`Queue` は Scope の capability を持つ型．`s.fork(f)` の Fiber は s を持つ．
- 子は，自分が入った scope の capability を引数で受け取る．兄弟を fork する規則（型検査が f の R に Scope を足す）は要らなくなる．
- これで `Fiber.scope` に型の上の特別扱いはなくなる．子の実行，本体が返ったときの中断，失敗の伝わり方はランタイムが実装する（`@intrinsic`）．
- なくなるもの: E0425（型の名前で `Fiber` などを探す規則），ハンドルを capture set の root として追うこと，終わった scope のハンドルを使ったときの実行時の検査．
- #719 の再現 1（外の State に Fiber を入れる）は型の不一致，再現 6（Fiber をエラーに入れる）は持ち出しの検査（E04xx）で止まる．
- 外の scope に fork するには，外の scope の本体の引数 s を使う（今の `outer = use Scope` の代わり）．

### 持ち出せない

- with の型を決めるとき，本体の値の型，本体から伝わるエラーの型，with の R に，その with の capability が残っていればコンパイルエラー（E04xx）．
- 外で型が決まったもの（外の `State` の型引数，外で供給された handler の操作の引数の型）には，そもそも capability を入れられない．代入や呼び出しの型が合わないので，型エラーになる（E0403 か型の不一致）．
- 関数の境目でも同じ．型引数に capability が入るのは，その with の中で呼ぶときだけ．

### `use E` の式

- `x = use E` の x は，どの capability かが決まらない（書いた位置でいちばん内側の E の capability に固定されるが，型には出ない）．
- `use E` の式は，略記も値も #791 で W0403 になっている．この proposal では型の中で，effect の名前 E に決まった capability として扱い，次の minor でなくす．移る先:
  - x を同じ関数の中で受け手にしか使わない → `x <- E`．W0403 の quick fix が書き換える（#791）
  - 外の handler を指し続ける，渡す，格納する → 名前付きの項目（`with x = H`）．機械的には書き換えられない

### deprecation の段階（たたき台）

1. 済み（#791，0.29）: `(use E).op()` を `<-` の略記にする．`use E` の式（略記も値も）に W0403 を出す．
2. この proposal が入った版: 型と R で capability を区別する．capability を effect の名前に広げることに W04xx（deprecated）を出す．
3. その次の minor: `use E` の式と，広げることをコンパイルエラーにする．capability についての capture checking（W0402，E0444 の capability の部分）と，`[effect.escape]` の実行時の検査（release 後の使用は defect）を外す．

`[effect.use]` は「`use E` の式は one more minor version 読む」と書いている（#731）．2 と 3 の版の並べ方は Open questions．

## Specification changes

（たたき台．Open questions が決まってから書き直す．）

### `[glossary.terms]`

- capability: a handler after its init has run and before its release. Each item of a with makes one, and each `Fiber.scope` makes one capability of Scope.
- Add: capability type, capability parameter, named item, provide by name, widen. Redefine leak: a value whose type or effect set contains a capability leaves the with that made the capability.

### `[effect.with]`

- Each item of a with runs its init and makes a capability. Leaving the with releases it, and the capability ends.
- A named item `x = Item` binds x to the capability of the item. A handler member item is written `x = E.M(arguments)`, or `x = c.M(arguments)` where c is a capability of E.
- An item without a name provides its effect by name only.

### `[effect.use]`

- The type of a capability is written `use E`. Type checking tells capabilities apart: the type of a capability records which capability it is.
- Calling an operation of a capability adds that capability to R. The with that made the capability removes it from R.
- A capability is not widened to its effect. An effect set that contains a capability is not included in an effect set that contains only the effect of the capability.
- A parameter of type `use E` gives the function a type parameter that ranges over the capabilities of E. At a call, it is decided by the argument. An argument that is an effect alias, or a `use E` written as an expression, decides it as E itself.
- `use E` written as an expression (deprecated by `effect-alias`) is a compile error after the deprecation period.

### `[effect.escape]`

- A capability cannot leak: if the type of the value of a with body, the error set that leaves the body, or the R that leaves the body contains a capability made by that with, it is a compile error (E04xx).
- After the deprecation period, capture sets are not used for capabilities.

### `[open.next]`

- The 0.29 item ("make E0444 the default ... remove the run-time check of 8.6") is replaced for capabilities by this proposal.

## Diagnostics

- [ ] E04xx: capability を持ち出した（型か R に capability を含む値が with の外へ出る）
- [ ] `use E` の式の W0403 は #791 で入った．廃止の版でエラーにする
- [ ] W04xx: capability を effect の名前に広げた（deprecated）．help: 引数を `use E` にする
- [ ] 廃止の版: W04xx をエラーにする．W0402，E0444 の capability の部分を外す

## Compatibility

数え上げ（#791 のあとの main，.snap を除く）．std，examples，tests/laws には `use E` の式が残っていない．残りはコンパイラのテストのプログラム（Rust の文字列），`spec/`，`proposals/` の中．

| 形 | 数 | 移る先 |
|---|---|---|
| `(use E).op()` | 817 | W0403 の quick fix で `<-` に書き換えられる |
| `x = use E` | 278 | 同じ関数の中で受け手にしか使わないなら quick fix で `x <- E`．それ以外は `with x = H` |
| `x <- E` | 160 | 変わらない |
| 引数や型の `: use E` | 171 | 書き方は変わらない．capability の引数になる |
| 外の値を，同じ E を上書きした内側の with で使う | 18 | `with x = H` |
| `Fiber.scope(|| ...)` | 244 | `Fiber.scope(|s| ...)` か `|_|`．機械的に書き換えられる |
| `= use Scope` | 34 | `Fiber.scope` の本体の引数 |
| `with x = H`，`with x = E.M(...)`（`.emel`） | 14 | そのまま通る．x の型が，どの capability かを持つようになる |
| capability を effect の名前に広げているもの | 未計数 | 試作で数える |

- 引数の `use E` は書き方が変わらない（`fn copy(from: use Db, to: use Db)`，`copy(replica, primary)`）．
- capability をコレクションに入れる形，`State[use Name]`（#708），handler のフィールドの `use E` は，同じ with の中なら今までどおり書ける．

## Implementation

- 型検査: capability の型と effect set に，どの capability かを持たせる．with の出口で，本体の値の型，エラー，R にその with の capability が残らないかを確かめる．型引数の範囲の検査と同じ仕組みを使う．
- capability の型の集合と，引数の `use E` の型引数は，R の集合と同じ仕組み（集合の和と set variable）で推論する．
- lowering: capability の evidence は，その項目の handler．effect の名前に決まった型引数は，呼んだ時点の evidence を使う．
- capture checking: deprecation の期間は今のまま動かし，廃止の版で capability の部分を外す．#791 で `use_root` は closure の境界で止まるようになっている．

## Alternatives

- 値を残し，capture checking で道を塞ぎ続ける（`capture-checking-exits`）．#719 の「決めること 1，2」と，host と Deferred / Queue の未決が残る．#701 は直らない．
- capability を値でなくす（この PR の最初の版）．capability をコレクションに入れる形と，`State[use Name]` が書けなくなる．型で区別すれば，値のままで同じ安全が得られる．
- 局所の label を立て，x をその label の capability とする（この PR の二つ目の版）．利用者が capability と label の二つを覚える必要がある．capability そのものを R に入れれば，label は要らない．
- capability を effect の名前に広げて，別々の with の capability を同じリストに入れる．広げた値は型が R で追えず，今の #719 の穴に戻る．集合で持てば広げずに入れられる．
- capability の型引数に名前を付ける（`fn retry[C: use Db](db: C, f: fn(C) -> ())`）．二つの引数が同じ集合を共有する形が書けるが，closure に捕えさせれば同じことが書け，名前が要る場面は少ない．後から足しても今の書き方の意味は変わらない．
- 名前付きの項目を `with f <- Files.Open(path)` や `with Files.Open(path) as f` と書く．`<-` は effect alias と同じ記号になり，`as` は今の `with x = ...` の書き換えが要る．
- 一段で `use E` の式をなくす（deprecation の期間を置かない）．影響が言語の中心なので期間を置く．

## Prior art

## Open questions

- LSP の hover で capability の集合をどう表示するか．
- handler の init の引数とフィールドに capability を持つ形（`with PostgresUsers(db: replica)`）．handler の型に capability が入る．
- handler の操作の中の `self` を，その handler の capability と見るか．`self` はフィールドに触れるので，capability より多くのことができる．
- capability を持つ型の書き方．`by E` は label の `L by P` と同じ語になる．`of E` や属性にするか．
- 名前のない `use E` の値を，なくす代わりに「型引数が effect の名前に決まった capability の値」（呼んだ時点で解決する値）にする手もある．内側で同じ E を上書きした with で使う 18 箇所の意味が変わる．
- deprecation の版．W0403 は 0.29 に入る．`use E` の式をエラーにするのを 0.30 にするか，この proposal の実装（広げることの警告）を待つか．#666 の 0.29（capture checking を既定にし，W0402 と実行時の検査を外す）を capability について取り下げるか．
- host の callback に capability を捕えさせる形（`with g = Plain { set_timeout(10, || g.greet("later")) }`）．外部関数の型は with の外で決まっているので，「持ち出せない」をそのまま当てると g を R に持つ callback は渡せない．#736 は「callback の R は capability を含んでよい」とした．host が with の後で呼ぶ callback をどう止めるか（host に渡すことを持ち出しとみなすか，同期の callback だけ許すか）．
- `[C: Capability]`（`capability-trait`）はそのまま使えるか．#651 の `close[C: Capability](item: C)` を確かめる．
- capability を effect の名前に広げられないことで書けなくなるコードがどれだけあるか．
- 日本語の訳．用語集は capability を「能力」と訳している．定義を変えるのに合わせて訳を見直すか．

## Decision
