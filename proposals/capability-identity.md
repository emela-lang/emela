---
title: "Capabilities are told apart in types and in R"
authors: [Alliana]
status: draft
pr: 732
issue: 719
decided:
touches: [lsp.hover, ext.boundary, glossary.terms, effect.use, effect.with, effect.handler, effect.escape, fiber.scope, fiber.escape, fiber.deferred, fiber.semaphore, fiber.queue, api.std.fiber, type.rules, open.next, diag.list]
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
- capability を effect に広げることは，0.29 からコンパイルエラー．`use E` の式（#791 で W0403）は 0.30 でコンパイルエラーにする．

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
- #666 は 0.29 で capture checking を既定にし，W0402 と実行時の検査を外す予定だった．この proposal は，その代わりに 0.29 に入る．

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
| widen | 広げる | R の中で capability を元の effect の名前として扱うこと．この proposal でコンパイルエラーにする | |
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
- capability の引数に渡せるのは，名前付きの項目の capability と，それを含む値だけ．effect alias は引数に渡せない（E0453 のまま）．
- いちばん内側の E を使いたい関数は，capability を受け取らず，R に E を書いて（`uses { Db }`）中で `db <- Db` と書く．呼んだ時点で決まるものは R，決まった handler は名前付きの項目，と役割を分ける．
- `use E` の値（W0403）は，移行の期間だけ，型の中で effect の名前 E に決まった capability として扱う．

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

- 型の宣言に，その型の値が E の capability を持つことを書ける（`pub type Fiber[A] uses Scope`）．`uses` は関数のシグネチャと同じ文脈キーワードで，この型の値はその capability が生きている間しか使えないことを表す．`use E`（capability そのものの型）とは別．
- E の capability c の操作がこの型の値を返すと，その値の型は c を持つ．名前で E を呼んだ操作（`x <- E` や `Fiber.fork`）が返した値の型は，effect の名前 E を持つ．
- capability を持つ値なので，持ち出せない原則がそのまま働く．
- 利用者の型にも使える．pool が返す番号を `pub type EnemyId uses Engine` とすれば，番号は pool の with の外へ出せない．

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
pub type Fiber[A] uses Scope

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

### handler のフィールドと操作

- handler のフィールドの `use E`（と capability を持つ型）の集合は，with の項目が init するときに決まる．`with users = PostgresUsers(db: replica)` の users は，フィールドの集合が {replica} の handler．
- users の型に replica を書き足さない．init の引数は with の外で評価されるので，replica の with は users の with を囲み，users が生きている間は replica も生きている．users を持ち出せない原則だけで replica も守られる．
- 操作の引数の `use E` は，関数の引数と同じく呼ぶたびに決まり，handler の実装の中では「どの capability でも」の扱いになる．実装は受け取った capability をフィールドや State に入れられない（型エラー）．#719 の A（長く生きる handler の操作の引数を通って出ていく）は，capability と capability を持つ型について型で閉じる．
- 操作の戻り値に capability の型と capability を持つ型を書くことは禁止する（E04xx）．capability を返せることは持ち出しに直結し，名前で呼んだ操作ではどの handler の capability かも決まらない．capability を作って渡すのは handler member（`with c = Pool.Conn()`）で書く．
- 関数の戻り値の `use E` は書ける．`fn pick(primary: use Db, replica: use Db, write: Bool) -> use Db` の戻り値の集合は二つの引数の和．

```emel
handler PostgresUsers: Users {
  init(db: use Db) { Self(db:) }
  fn find(id: Int) -> User { self.db.query("...") }
}

with replica = ReplicaDb {
  with users = PostgresUsers(db: replica) {
    users.find(1)
  }
}
```

### `Capability` trait

- `[C: Capability]`（`capability-trait`）はそのまま使える．C はトップレベル関数の型引数なので，呼ぶたびに渡した capability の型に，集合ごと決まる（`close[C: Capability](item: C)` に f を渡せば，C は f の capability）．
- 関数型の中の `use E` を量化する規則は，`use E` と書いた場所にだけ効き，`[C: Capability]` には関係しない．
- capability を持つ型（`uses E` で宣言した型）は `Capability` を満たさない．capability そのものではなく，操作も呼べない．
- ふつうの型引数（`[A]`）も capability の型を受け取れる（`List[use ReadFile]`）．`Capability` はそれを capability に限る．

### `self`

- 操作の本体と release の中の `self` は，その handler の capability として数える．`self` は，capability にフィールドを読む力を足したもの．
- `self` のフィールドを読むことは，その capability を使うこととして R に入る．操作が返す closure がフィールドを読むと，closure の R にその capability が入り，with の外へ出すと E04xx になる．
- `self` を `use E` の引数に渡せる．渡した先では，ほかの capability と同じく操作を呼べるが，フィールドには触れない．

```emel
handler SystemFiles: Files {
  handler Open(handle: Handle): ReadFile {
    init(path: String) { Self(handle: host_open(path)) }
    release { host_close(self.handle) }
    fn reader(resume) { resume(|| host_read(self.handle)) } # closure の R は，この handler の capability
  }
}
```

### LSP の hover

- 型はソースと同じ形（`use Counter`，`Fiber[Int]`）で出し，その下に `capabilities: ...` の行で，どの capability かを出す．`holds:` の行（capture checking）はなくなる．
- 名前付きの項目の capability はその名前，名前のない項目は handler の名前（`Plain`），関数の引数の集合は引数の名前，量化した関数型の引数は `any` で出す．
- closure や関数の hover の `uses { ... }` には，capability の名前もそのまま並べる（`uses { f, Console }`）．
- 持ち出しのエラーのメッセージにも同じ名前を使う．

```
ns: List[use Counter]
capabilities: x, y
```

### host の callback

- 外部関数の引数と戻り値の callback は，既定では host が持ち続けるものとする．host は外部関数が返った後にも呼べるので，callback の R に capability を入れられない（E0454 を capability にも広げる）．
- 外部関数が返る前にしか呼ばない callback（並べ替えの比較関数など）は，宣言の引数に属性 `@borrowed` を付ける．host は呼び出しの間だけ callback を借り，返るときに手放す．`@borrowed` の引数の callback だけが，R に capability を入れられる．印が正しいかは，`@external` の型と同じく宣言を書く人が責任を持つ．
- `@borrowed` は外部関数の関数型の引数にだけ付けられる．ほかの位置（戻り値，外部関数でない関数の引数，関数型でない引数）に付けるとコンパイルエラー（E04xx）．

```emel
@external("js", "./array.js", "sortBy")
fn sort_by(items: Array[Int], @borrowed compare: fn(Int, Int) -> Int) -> Array[Int]

@external("js", "./timer.js", "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> () # host が持ち続けうる．f に capability は入れられない
```
- 型だけでは二つを区別できない．外部関数の型の callback の集合は呼ぶたびに決まり，戻り値に capability が残らないので，持ち出しの検査では見えない．
- `closure-r-by-caller` の例 `with g = Plain { set_timeout(10, || g.greet("later")) }` はエラーになる．host が後で呼ぶ処理から handler を使うには，host の側で完結させるか，Queue で Emela の fiber に渡す．

### 持ち出せない

- with の型を決めるとき，本体の値の型，本体から伝わるエラーの型，with の R に，その with の capability が残っていればコンパイルエラー（E04xx）．
- 外で型が決まったもの（外の `State` の型引数，外で供給された handler の操作の引数の型）には，そもそも capability を入れられない．代入や呼び出しの型が合わないので，型エラーになる（E0403 か型の不一致）．
- 関数の境目でも同じ．型引数に capability が入るのは，その with の中で呼ぶときだけ．

### `use E` の式

- `x = use E` の x は，どの capability かが決まらない（書いた位置でいちばん内側の E の capability に固定されるが，型には出ない）．
- `use E` の式は，略記も値も #791 で W0403 になっている．この proposal では型の中で，effect の名前 E に決まった capability として扱い，0.30 でなくす．移る先:
  - x を同じ関数の中で受け手にしか使わない → `x <- E`．W0403 の quick fix が書き換える（#791）
  - 外の handler を指し続ける，渡す，格納する → 名前付きの項目（`with x = H`）．機械的には書き換えられない

### 版

0.29 に #791 と一緒に入れる．0.29 はこの proposal の実装を待って出す．

- 0.29:
  - 型と R で capability を区別する．`uses E` の型，関数型の中の capability の引数，`self`，handler のフィールドと操作，host の callback，`Fiber.scope(|s| ...)` を入れる．
  - 新しく止める形（持ち出し，広げること，操作の戻り値の capability，capability を捕えた callback）は，最初からコンパイルエラーにする．警告の段階は挟まない．
  - capability とハンドルの capture checking（W0402，E0444，capture set），E0425，`[effect.escape]` と `[fiber.escape]` の実行時の検査を外す．
  - `use E` の式は W0403 のまま読む（#731 の「一つの minor の間は読む」を守る）．型の中では effect の名前 E の capability として扱う．名前で E を供給する with は，型の中の名前 E を自分の capability として数えるので，`x = use E` の値を持ち出すことも型で止まる．
  - 書き換えは W0403 の quick fix と，`Fiber.scope(|| ...)` を `|s|` に直す quick fix で助ける．
- 0.30: `use E` の式をコンパイルエラーにする．
- #666 は，この proposal に置き換えて閉じる．capture checking は既定にならないまま，capability とハンドルについては外れる．

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
- A parameter of type `use E` gives the function a type parameter that ranges over the capabilities of E. At a call, it is decided by the argument. An effect alias cannot be an argument (E0453). During the deprecation period, a `use E` written as an expression decides it as E itself.
- `use E` written as an expression (deprecated by `effect-alias`) is a compile error from 0.30.

### `[effect.escape]`

- A capability cannot leak: if the type of the value of a with body, the error set that leaves the body, or the R that leaves the body contains a capability made by that with, it is a compile error (E04xx).
- Capture sets are not used for capabilities and handles, and the run-time checks of leaks are removed.

### `[ext.boundary]`

- Replace "The R may contain capabilities; a callback reaches a handler only through a capability it captures (8.2), since the host calls it without passing handlers." by: "The R MUST NOT contain a capability either, since the host may keep the callback and call it after the with of the capability has ended (E0454). A parameter of an external function marked `@borrowed` (the host holds the callback only during the call) may take a callback whose R contains capabilities. `@borrowed` can be written only on a parameter of function type of an external function (E04xx)."

### `[lsp.hover]`

- Replace the line `holds: ...` by a line `capabilities: ...` that names which capabilities the type of the value records: the variable of a named item, the handler of an item without a name, the parameter that received it, or `any` for a parameter of a quantified function type. The type itself is shown as written in source.
- The `uses { ... }` of a closure or a function lists capabilities by the same names.

### `[open.next]`

- The 0.29 item ("make E0444 the default ... remove the run-time check of 8.6") is replaced by this proposal: in 0.29, capture sets and the run-time checks are removed for capabilities and handles, and their leaks are compile errors from the types.
- `use E` written as an expression becomes a compile error in 0.30.

## Diagnostics

- [ ] E04xx: capability を持ち出した（型か R に capability を含む値が with の外へ出る）
- [ ] E04xx: `@borrowed` を外部関数の関数型の引数以外に書いた
- [ ] E04xx: effect の操作の戻り値に capability の型か capability を持つ型を書いた
- [ ] `use E` の式の W0403 は #791 で入った．0.30 でエラーにする
- [ ] E04xx: capability を effect の名前に広げた．help: 引数を `use E` にする
- [ ] W0402，E0444，E0425 を外す（0.29）
- [ ] 0.30: W0403 をエラーにする

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
- capture checking: capability とハンドルの部分を外す．`use E` の値は，型の中の effect の名前として持ち出しの検査で見る．

## Alternatives

- 値を残し，capture checking で道を塞ぎ続ける（`capture-checking-exits`）．#719 の「決めること 1，2」と，host と Deferred / Queue の未決が残る．#701 は直らない．
- capability を値でなくす（この PR の最初の版）．capability をコレクションに入れる形と，`State[use Name]` が書けなくなる．型で区別すれば，値のままで同じ安全が得られる．
- 局所の label を立て，x をその label の capability とする（この PR の二つ目の版）．利用者が capability と label の二つを覚える必要がある．capability そのものを R に入れれば，label は要らない．
- capability を effect の名前に広げて，別々の with の capability を同じリストに入れる．広げた値は型が R で追えず，今の #719 の穴に戻る．集合で持てば広げずに入れられる．
- capability の型引数に名前を付ける（`fn retry[C: use Db](db: C, f: fn(C) -> ())`）．二つの引数が同じ集合を共有する形が書けるが，closure に捕えさせれば同じことが書け，名前が要る場面は少ない．後から足しても今の書き方の意味は変わらない．
- 名前付きの項目を `with f <- Files.Open(path)` や `with Files.Open(path) as f` と書く．`<-` は effect alias と同じ記号になり，`as` は今の `with x = ...` の書き換えが要る．
- 名前のない `use E` の値を，なくす代わりに「呼んだ時点でいちばん内側の E に届く値」として残す．型の中では effect の名前としてしか表せず，渡したり格納したりできる限り，どの handler を指すかが型から分からない（#719 の穴の元）．内側で同じ E を上書きした with で外の値を使う 18 箇所の意味も黙って変わる．
- 一段で `use E` の式をなくす（deprecation の期間を置かない）．#731 で一つの minor の間は読むと決めたので，0.30 まで読む．
- 0.29 は #791 だけで出し，この proposal を 0.30 に入れる．0.29 で W0403 を出したあと 0.30 でまた大きく壊すことになり，capture checking の扱いも 0.29 と 0.30 で二度変わる．

## Prior art

## Open questions

- capability を effect の名前に広げられないことで書けなくなるコードがどれだけあるか．
- 日本語の訳．用語集は capability を「能力」と訳している．定義を変えるのに合わせて訳を見直すか．

## Decision
