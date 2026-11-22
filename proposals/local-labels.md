---
title: "Local labels: capability types carry their label"
authors: [Alliana]
status: draft
pr: 732
issue: 719
decided:
touches: [grammar.type, effect.use, effect.with, effect.handler, effect.label, effect.escape, fiber.scope, fiber.escape, type.rules, open.next, diag.list]
requires: []
related: [capability-type, capability-trait, handler-member, handler-init-arguments, capture-checking, scope-handle-values]
---

# Local labels: capability types carry their label

## Summary

`effect-alias`（#731）の二段目．capability の型に，どの handler かを表す label を入れる．

- with の名前付きの項目 `x = Item` は，二つのものを作る．本体の中だけで有効な名前のない label（局所の label）と，その label の capability x．
- capability の操作を呼ぶと，R にはその capability の label が入る．
- 引数の `x: use E` は，E の label を一つ受け取る型引数を持つ関数として扱う．呼び出しで label が実引数のものに決まる．
- label を元の effect の名前に広げない．
- 局所の label を含む型と R は，その with の本体の外に書けない．capability を持つ値が with の外へ出るのは，どの道を通っても型エラーになる．capability についての capture checking は，この型の範囲の検査で置き換える．
- capability は普通の値で，`use l` は普通の値の型になる．渡す，格納する，返す，データ構造に入れる，closure で捕えることは，ほかの値と同じにできる．
- 名前のない `use E` の値（`x = use E`）と，label を効果名に広げることを deprecated にし，段階を踏んでなくす．

## Motivation

### capability の型が handler を表していない

- 今の capability の型は `use ReadFile` で，どの with の handler かが型に出ない（`capability-type`）．値の操作は R に効果名を足す（`[effect.use]`）．
- どの handler かは，型とは別の capture set で追っている（`[effect.escape]`）．そのため，with の外へ出る道を一つずつ数える必要がある．
  - 本体の値しか見ていない（#719）．操作の引数，エラー，関数の境目が漏れている．
  - host への callback と Deferred / Queue はまだ決まっていない（#666）．
- R に効果名が入るので，handler member の値の操作は，member の with で外せない（#701）．
  - 開いた資源をリストに入れて `map` で回すと E0402 になる．
- 型が R の中で効果名に広げられるので，型だけでは何も止まらない（#719 の再現 2〜5 が `uses { Greeter }` で通る理由）．

### label を型に入れれば，型の範囲の検査で足りる

- 局所の label は，with の本体の中でだけ有効．with の中で宣言した型引数が外へ出せないのと同じく，局所の label を含む型は本体の外に書けない．
- 外へ出る道を数えなくても，外で決まった型（外の `State` の型引数，外の handler の操作の引数の型，エラーの型，関数の型引数）に label を入れられないので，すべての道が止まる．
- R にも label が入るので，member の with がそれを外せる（#701）．

### `capability-type` の動機は残る

- `capability-type` は，capability を関数に渡す，格納する，返すために，capability を普通の値にした．この proposal でも capability は普通の値で，できることは変わらない．
- 変わるのは型に label が入ることだけ．それによって capture set が要らなくなる．

## Proposal

### 用語

| 英語 | 日本語 | 何か | ソースでの形 |
|---|---|---|---|
| label | label | `effect L of E` で宣言した，E と同じ操作を持つ別の effect（今の `[effect.label]`） | `AuditLog` |
| local label | 局所の label | with の名前付きの項目が作る，名前のない label．その with の本体の中でだけ有効で，その項目の handler を表す | 書かない |
| label parameter | label の型引数 | 型が `use E` の引数が関数に足す label | 書かない |
| capability | 能力（capability） | 値．一つの label（または effect）の操作を呼べる | `with x = Item` の x，引数の x |
| capability type | 能力の型 | capability の型．ソースでは `use E` と書き，型検査の中では label を持つ | `use ReadFile` |
| capability parameter | 能力の引数 | 型が `use E` の引数 | `fn copy(from: use Db)` |
| named item | 名前付きの項目 | `with x = Item`．局所の label と capability x を作る | `with f = Files.Open(p)` |
| effect alias | 効果の別名 | `x <- E` の x（#731）．値でも label でもなく，R に E そのものを入れる | `log <- Logger` |
| widen | 広げる | label を元のエフェクトとして扱うこと．この proposal で deprecated | |
| label scope check | label の範囲の検査 | 局所の label を含む型と R が with の外へ出ないかの検査 | |

- 局所の label と label の型引数は，ソースに名前を書かない．型推論で決まり，型を書く場面では引数の `use E` を使う．
- capability と label は別のもの．capability は値で，label は型と R の中の名前．`x.read()` で R に入るのは x ではなく x の label．
- capture checking の root と hold（抱える）は，capability には使わなくなる．Fiber などのハンドルにだけ残る．

### 局所の label

- with の名前付きの項目 `x = Item` は，Item が供給する effect E の局所の label を作る．Item の handler がその label を供給する．x はその label の capability．構文は今の名前付きの項目（`[effect.with]`）と同じ．
- handler member も同じ（`with f = Files.Open(path)`）．
- x は `L by x` の右にも書ける（x の label を L に差し込む）．
- `<-` は with の項目に使わない．文の `x <- E`（#731）は固定しない名前で，局所の label は項目の handler に固定される．

```emel
with primary = PrimaryDb {
  with replica = ReplicaDb {
    copy(replica, primary)
    primary.execute("...")
  }
}

with f = Files.Open("a.txt") {
  lines = read_all(f)
  [f].map(|x| x.read(10)) # closure の R は f の局所の label で，この with が外す
}
```

### capability の型

- capability の型は，型検査の中で label l を持つ（以下 `use l` と書くが，l はソースに書かない）．l は局所の label，label の型引数（下），宣言した label（`[effect.label]`），または effect の名前．
- effect の名前を l にした `use E` は「呼んだ時点でいちばん内側の E」を表す（#731 の `x <- E` と同じ）．
- `use l` の値の操作を呼ぶと，R に l が入る．
- `use l` を含む型と，l を含む effect set は，l の範囲の外に書けない．局所の label の範囲は with の本体．
- label を元の effect の名前に広げない．f の局所の label を R に持つ closure は，`uses { ReadFile }` と書いた関数型には入らない．

### 引数の `use E`

- 引数の型に `use E` と書くと，その関数は E の label を一つ受け取る型引数を持つ．`fn read_all(x: use ReadFile)` は，ReadFile の label l について `x: use l` と書き，R に l を持つ関数と同じ．
- 呼び出しで l が実引数の label に決まる（`read_all(f)` なら f）．呼ぶ側の R に f が入り，f の with が外す．
- 同じ effect の引数を二つ取ると，label の型引数も二つになる（`fn copy(from: use Db, to: use Db)`）．

### 型の範囲の検査

- with の型を決めるとき，本体の値の型，本体から伝わるエラーの型，with の R に，その with の局所の label が残っていれば型エラー（E04xx）．
- 外で型が決まったもの（外の `State` の型引数，外で供給された handler の操作の引数の型）は，そもそも局所の label を含めない．代入や呼び出しの型が合わないので，型エラーになる（E0403 か型の不一致）．
- 関数の境目でも同じ．型引数に局所の label が入るのは，その範囲の中で呼ぶときだけ．

### 名前のない `use E` の値

- `x = use E` の x は，どの with の handler かを表す label を持たない．
- 段階を踏んでなくす（下の deprecation）．移る先:
  - x を受け手にしか使わない → `x <- E`（#731）
  - 外の handler を指し続ける，渡す，格納する → 名前付きの with の項目（`with x = H`）

### deprecation の段階（たたき台）

1. 一段目（`effect-alias`，#731）: `(use E).op()` を `<-` に展開する．警告なし．
2. この proposal が入った版: 局所の label と，型の中の label を入れる．次の二つに W04xx（deprecated）を出す．
   - 名前のない `use E` の値
   - label を効果名に広げること
3. 次の minor: 上の二つをコンパイルエラーにする．capability についての capture checking（W0402，E0444 の capability の部分）と，`[effect.escape]` の実行時の検査（release 後の使用は defect）を外す．

## Specification changes

（たたき台．Open questions が決まってから書き直す．）

### `[effect.with]`

- A named item `x = Item` makes a local label of the effect E that Item provides, and binds x to a capability of that label. The handler of the item provides the label. The local label has no name in source, and is valid only inside the body. A handler member item is written `x = E.M(arguments)`, or `x = c.M(arguments)` where c is a capability of E.

### `[effect.use]`

- The type `use l` is the type of a capability of the label l. l is a local label, a label parameter, a label declared with `effect L of E`, or an effect. `use E` with an effect E stands for the innermost provider of E at the time of each call.
- Calling an operation of a value of type `use l` adds l to R.
- A label is not widened to its underlying effect. An effect set that contains l is not included in an effect set that contains only the underlying effect of l.
- A parameter of type `use E` gives the function a label parameter l of E, and the parameter has type `use l`. The parameter types `use E + F` give one label parameter for each effect. At a call, each label parameter is decided by the argument.
- `use E` written as an expression is deprecated (W04xx).

### `[effect.escape]`

- A type that contains a local label, and an effect set that contains one, cannot be written outside the body of the with that makes the label. If the type of the value of a with body, the error set that leaves the body, or the R that leaves the body contains a local label of that with, it is a compile error (E04xx).
- After the deprecation period, capture sets are not used for capabilities. The section covers only the handles of scopes (`[fiber.escape]`).

### `[glossary.terms]`

- Add: local label（局所の label），label parameter（label の型引数），capability type（能力の型），capability parameter（能力の引数），named item（名前付きの項目），widen（広げる），label scope check（label の範囲の検査）．

### `[open.next]`

- The 0.29 item ("make E0444 the default ... remove the run-time check of 8.6") is replaced for capabilities by this proposal.

## Diagnostics

- [ ] E04xx: 局所の label を含む型か R が with の外へ出る
- [ ] W04xx: 名前のない `use E` の値（deprecated）．help: `x <- E` か `with x = H`
- [ ] W04xx: label を効果名に広げた（deprecated）．help: 関数型に label を書くか，引数を `use E` にする
- [ ] 廃止の版: W04xx をエラーにする．W0402，E0444 の capability の部分を外す

## Compatibility

数え上げ（.snap を除く）．

| 形 | 数 | 移る先 |
|---|---|---|
| `(use E).op()` | 803 | 一段目で展開される．書き換え不要 |
| `x = use E` | 278 | 受け手にしか使わないなら `x <- E`（機械的に判定できる）．それ以外は `with x = H` |
| 値として渡す，格納する，返す | 約 59 行 | 名前付きの with の値ならそのまま通る．`use E` から取った値は `with x = H` に書き換える |
| 外の値を，同じ E を上書きした内側の with で使う | 18 | `with x = H` |
| `with x = H`，`with x = E.M(...)` | 未計数 | そのまま通る．x の型が持つものが effect E から局所の label に変わる（ソースの形は `use E` のまま） |
| label を効果名に広げているもの | 未計数 | 試作で数える |

- 引数の `use E` は書き方が変わらない（`fn copy(from: use Db, to: use Db)`，`copy(replica, primary)`）．
- capability をコレクションに入れる形，`State[use Name]`（#708），handler のフィールドの `use E` は，同じ with の範囲の中なら今までどおり書ける．

## Implementation

- 型検査: 型と effect set に，範囲を持つ label を足す．with の出口で，本体の値の型，エラー，R に局所の label が残らないかを確かめる．型引数の範囲の検査と同じ仕組みを使う．
- 引数の `use E` を label の型引数として扱う．呼び出しで単一化する．
- lowering: 局所の label の evidence は，with の項目の handler．`use E`（effect の名前）の値は，呼んだ時点の evidence を使う．
- capture checking: deprecation の期間は今のまま動かし，廃止の版で capability の部分を外す．

## Alternatives

- 値を残し，capture checking で道を塞ぎ続ける（`capture-checking-exits`）．#719 の「決めること 1，2」と，host と Deferred / Queue の未決が残る．#701 は直らない．
- capability を値でなくす（この PR の前の版）．型は簡単になるが，capability をコレクションに入れる形と，`State[use Name]` が書けなくなる．型に label を入れれば，値のままで同じ安全が得られる．
- 局所の label を `with f <- Files.Open(path)` と書く．文の `x <- E`（固定しない名前）と同じ記号が，固定する名前を作ることになる．
- 局所の label を `with Files.Open(path) as f` と書く．今の `with x = ...` の書き換えが要る．with の項目の `=` は文の束縛とは別の文法（`with_item`）なので，`=` のまま意味を変える．
- 一段で名前のない `use E` の値をなくす（deprecation の期間を置かない）．影響が言語の中心なので期間を置く．

## Prior art

## Open questions

- 別々の with で作った capability を一つのリストに入れる形（`[f, g]`）．要素の型を label の集合（`use f | g` など）にし，操作は R に両方を足すか．#651（regions）の「同じ region で開いた資源は同じ label を持つ」とどう合わせるか．
- label の型引数を明示する構文が要るか（`fn read_all[l: ReadFile](x: use l)`）．関数型にも書けるか（`fn(use ReadFile) -> ()`）．
- handler の init の引数とフィールドに `use l` を持つ形（`with PostgresUsers(db: replica)`）．handler の型に label が入る．
- 名前のない `use E` の値を，なくす代わりに「型が `use E`（effect の名前）で，呼んだ時点で解決する値」にする手もある．`x <- E` とほぼ同じ意味の値になる．内側で同じ E を上書きした with で使う 18 箇所の意味が変わる．
- deprecation の版．警告の期間を一つの minor にするか．#666 の 0.29（E0444 を既定にする）とどう並べるか．
- 移行の道具．`x = use E` → `x <- E` の書き換えを自動にするか．
- `Fiber.scope` の Scope と，Fiber，Deferred，Queue のハンドルにも scope の局所の label を型に入れるか（#719 の再現 1，6）．入れれば `[fiber.escape]` の capture set も要らなくなる．
- `[C: Capability]`（`capability-trait`）は `use l` も満たす．#651 の `close[C: Capability](item: C)` はそのまま書けるか．
- 局所の label を効果名に広げられないことで書けなくなるコードがどれだけあるか．
- LSP の hover で，局所の label と label の型引数をどう表示するか（ソースに書かない名前なので，表示の形だけ決める）．

## Decision
