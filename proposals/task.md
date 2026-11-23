---
title: Task replaces Fiber, and the range of concurrent work is a with item
authors: [Alliana]
status: draft
issue:
touches: [task, task.basic, task.group, task.background (new), task.combinators, task.interrupt, task.cause, task.parallel, task.deferred, task.semaphore, task.queue, api.std.task, api.std.exit, api.std.cause, api.std.deferred, api.std.semaphore, api.std.queue, api.std.http, api.std.process, module.resolution, kw.annotation, effect.decl, effect.handler, effect.escape, effect.async, effect.label, type.fn, type.rules, ext.call, dyn.suspend, dyn.memory, lib.layers, lib.http-service, lib.api-rules, target.js.outcome, target.js.abort, target.js.event-loop, target.wasm.output, lsp.hover, lsp.completion, lsp.code-action, glossary.context, glossary.terms, open.next, diag.list]
requires: [capability-identity, generic-handler]
related: [scope-handle-values, semaphore-queue, http-service, closure-r-by-caller]
---

# Task replaces Fiber, and the range of concurrent work is a with item

## Summary

- 並行処理の単位を `Fiber.scope` という特別な関数から外し，with の項目（組み込みの handler）にする
- 名前を Fiber から Task に変える．`Std.Fiber` は `Std.Task`，`Fiber[A]` は `Task[A]`，`fork` は `start`
- 結果を待つ並行は `Task.Group`，結果を見ずに切り出す仕事は `Task.Background`
- Deferred，Queue，Semaphore は Scope の操作から外し，独立した with の項目（組み込みの effect の capability）にする
- Task から受け取る方法は `join` だけにする．`await` と `interrupt` を外し，`CauseKind` から Interrupt を外す

## Motivation

- Scope に例外規則が集まっている．集合型引数，操作ごとの型引数，handler を書けない（E0413），型引数の省略（E0423 の例外），子の R の包含規則，E0402 の専用 note，E0425，寿命の実行時検査
- 子が使う handler を決める規則（`Fiber.scope` を呼んだ時点の handler）が，with の入れ子とも `closure-r-by-caller` とも違う三つ目の規則になっている．scope の本体に書いた with は子に届かない
- Deferred／Semaphore／Queue が Scope の操作でしか作れず，資源の寿命を with と handler member で決める仕組みの外にある
- 結果の受け取り方が join／await／attempt／raise の四つあり，Fail／Die／Interrupt と掛け合わされる（11.5 の表）
- 応答の後に続ける仕事（監査ログ，計測の送信）のように，結果を見ずに切り出す処理を書く場所がない．`@entry(Http.serve)` は要求ごとの打ち上げなので，打ち上げを越える仕事も書けない
- resume の後も走り続ける操作の実装（14.5）は，scope が待たない構造化されていない切り出しになっている
- `capability-identity` は `Fiber.scope(|s| ...)` と capability-bound type で漏れを型で止めたが，Scope の例外規則と「s を引数で受け取る」「R の Scope」の二重化は残る

## Proposal

```emel
import Std.Task.{Task}

with tasks = Task.Group {
  user = tasks.start(|| fetch_user(id))
  orders = tasks.start(|| fetch_orders(id))
  Profile(user: user.join(), orders: orders.join())
}

with bg = Task.Background(on_failure: |cause| log.error(Cause.to_string(cause))) {
  bg.start(|| audit(req))
  respond(req)
}

with jobs = Queue(capacity: 100), tasks = Task.Group {
  tasks.start(|| jobs.offer(job))
  next = jobs.take()
}

(user, orders) = Task.both(|| fetch_user(1), || fetch_orders(1))
```

### 決まったこと

#### Task.Group

- 組み込みの handler．with の項目として書き，init で範囲を作る
- `tasks.start(f)` は `Task[A]` を返す．子は親が次に suspend したときに走り始める
- 本体が値を返したら残りの子を中断し，release で巻き戻しの終わりを待つ
- 子が失敗したら兄弟を中断し，その失敗を本体の次の suspension point で本体の失敗として届ける．with の E に子の E が入る．release の E は空のまま（E0420 を破らない）
- 複数の子が失敗したら最初の失敗を結果にし，残りは note にする（今と同じ）
- 子の R は Group の with の外側が与える．同じ with の中では Group より前の項目まで（8.3 の向きと同じ）．本体の中の with で作った capability を捕まえた closure は，`capability-identity` で型が合わずに start できない
- `Task[A]` は Group の capability に縛られた型（capability-bound type）

#### Task.Background（決定 1，2）

- `bg.start(f)` は `()` を返す．手綱がない
- release は残りを中断せず，終わるまで待つ．本体が値で終わっても失敗で終わっても同じ．with の結果は本体の結果のまま
- 外から中断されたときだけ子も中断する．release が子を待っている間に中断が届いたら，その中断を子へ回し，子の巻き戻しの終わりを待ってから release を終える（「release 中は中断を保留する」は保ったまま，待っている子へは伝える）
- 子の失敗は兄弟を止めず，`on_failure` に渡す
- `on_failure: fn(Cause[E]) -> () uses { R }` は必須の引数．E は子の E の集合，R は子と同じく with の外側が与える
- `on_failure` の E は空．受け取るのは Fail と Die だけ
- `on_failure` は失敗した子の巻き戻しが終わった後，その子の中で呼ぶ．release はそれも待つ
- `on_failure` の中の defect は release の中の defect と同じ扱い（14.4）

#### `@entry(Http.serve)` とのつなぎ方（決定 3）

- app の R に許すものを「Async だけ」から「Async と Spawn だけ」に広げる（`pub fn app(request: Request) -> Response uses { Spawn }`）
- adapter は要求ごとに `Task.Background` を名前で与えて app を呼ぶ．app が返した時点で応答をホストへ渡し，打ち上げは release の終わりで終える
- adapter の `on_failure` は Cause を標準エラーへ書く
- Deno，Bun，`emela run` は Promise を先に解決するだけ．Cloudflare Workers は ctx の `waitUntil` に打ち上げの終わりを渡す．C は serve がないので対象外

#### Deferred，Queue，Semaphore（決定 4）

- `effect Deferred[A]`，`effect Queue[A]`，`effect Semaphore` を組み込みの effect にし，`offer`／`take`／`await` などは suspend fn の操作にする
- 組み込みの handler を effect と同じ名前で置き，with の項目として書く（`with ready = Deferred, jobs = Queue(capacity: 100), db = Semaphore(permits: 10)`）．型引数は本体の使い方から推論し，書くなら `Queue[Job](capacity: 100)`（決定 8）

#### effect と同じ名前の handler（決定 8）

- 今は effect と handler が同じ型の名前空間にあり（4.6），同じモジュールで同じ名前にすると E0213 になる
- 組み込みの handler（`@intrinsic`）に限って，実装する effect と同じ名前を許す．その handler はその effect だけを実装する
- 名前が書かれる場所で引き分ける．with の項目，layer の項目，`by` の右では handler を指す．`uses`，`use E`，`x <- E`，`L of E`，`E.M(...)` では effect を指す
- 重なるのは `L by P` の右だけで，同名のときは handler を指す（外側の effect へ戻す意味にはならない）
- ユーザーには許さない．effect と handler を分けて with で差し替えることが Emela の中心にあり，同名を許すと差し替えるべき抽象と本番の実装が同じ名前に見える．Deferred，Queue，Semaphore は実装が一つで差し替えるものがないので当たらない．必要になったら広げる
- 選ばなかった形：ユーザーにも許す，`Queue.New(100)` のような共通の handler 名（with の読み方として不自然），effect に既定の handler を宣言させる（`with Console` のような書き方が一般にでき，テストと本番が紛れる），`Deferred.Empty`／`Queue.Bounded`／`Semaphore.Permits`（長い）
- 任意の A について実装する handler は，別の proposal `generic-handler`（ユーザーにも総称 handler を許す）に任せる．この proposal では例外を作らない
- 三つとも capability なので，capability-bound type も「scope への所属」も要らない

#### 結果の受け取り方（決定 5）

- Task に残すのは `join` だけ．`await` と `interrupt` を外す
- Task が中断されるのは「Group の本体が値を返した」「兄弟が失敗した」の二つだけになり，どちらも Group 全体が巻き戻される場面なので，中断された子を中断されていない誰かが join する場面がなくなる
- `join` の「中断された子なら defect」という規則を外す．release の中で中断が保留されている間に失敗した子や中断された子を join したら，呼んだ側を中断として巻き戻す（今と同じ）
- `CauseKind` は Fail と Die の二つになる
- 一つの子だけ止めたいときは with を入れ子にし，その with を終わらせる．動的に増える子を個別に止めるときは，止める合図を Deferred で渡す
- `Exit.attempt` と `Cause.raise` は Task と関係なく残す

#### 名前

| 今 | 新しい名前 |
|---|---|
| `Std.Fiber` | `Std.Task` |
| `Fiber[A]` | `Task[A]` |
| `Fiber.scope` | `with tasks = Task.Group` |
| `fork` | `start` |
| `Fiber.all／race／both／map／timeout` | `Task.all／race／both／map／timeout` |

- fiber という語は 14 章とターゲットの章で実行時の用語として残す
- C の `em_task *` とは「完了を待っている仕事」という意味でそろう

#### effect と handler の名前（決定 7）

- effect は「何ができる capability か」を表し，`uses { ... }` と `use ...` に出る．handler は「どう与えるか（寿命の方針）」を表し，`モジュール.Handler` と書く
- `Task.Group` が実装する effect は `TaskGroup`．`start` は `Task[A]` を返す
- `Task.Background` が実装する effect は `Spawn`．`start` は `()` を返す．寿命は handler が決めるので effect 名に入れない
- 二つの名前の非対称（名詞と動詞）は，Group が「まとまり」，Spawn が「行為」であることに合わせたもの

```emel
fn crawl(tasks: use TaskGroup, url: String) -> () uses { Http, Async } {
  tasks.start(|| fetch(url))
}

@entry(Http.serve)
pub fn app(request: Request) -> Response uses { Spawn } {
  spawn <- Spawn
  spawn.start(|| audit(request))
  Routes.route(request)
}
```

#### resume の後の suspend を禁じる（決定 9，14.5）

- suspend fn の実装の本体で，直接の `resume(...)` 呼び出しの後に実行されうる，suspend しうる呼び出しは compile error（E0417 の仲間で新しいコード）
- 判定は Async の判定（8.7）と同じく構文で，安全側に倒す．分岐の片方で resume していれば，その後ろは「後」とみなす
- 取り残される継続が生まれるのはここだけ．host のコールバックの R は Immediate なので（9.5）resume の後に suspend できず，別の fiber からの resume はその fiber 自身が続きを走らせる
- resume の後に残るのは suspend しない部分だけになり，その場で最後まで走る
- 長く続く仕事が要るなら，handler が `Spawn` を要求して明示的に切り出す．切り出しが R に出るので型から見える

```emel
handler RemoteLog: Logger {
  suspend fn info(msg, resume) {
    spawn <- Spawn
    spawn.start(|| send(msg))
    resume(())
  }
}
```

- 14.5 から消す規則：操作の実装は同じ fiber の別の継続として走る，中断はそこへ届かない，scope はそれを待たない，打ち上げの終わりに閉じる，main の後で残った継続の with は release しない
- 残す規則：resume の後で fail したら defect（`failed after resume`）
- 選ばなかった形：handler の release が resume の後も走る実装の終わりを待つ（どの handler の with にも待ち合わせが付き，ランタイムが handler ごとに追跡する．`Spawn` を要求すれば同じことが書ける）
- std と examples は簡易な検索で該当なし（resume は本体の最後にある）．テストと tests/laws は未確認

#### 名前付きの項目の型引数（決定 11）

- 名前付きの項目（`with x = H`）の型引数を x の使われ方から推論する規則は，`generic-handler`（#817，実装 #825）が `[effect.with]` に入れた．`with ready = Deferred` の A は `ready.await()` の戻り値の型との単一化で決まり，決まらなければ E0459
- この proposal で足す規則はない

#### Semaphore の許可を handler member にする（決定 12）

- 許可は Semaphore の handler member で開く資源にする．取るのは init，返すのは release
- `suspend handler Permits(n: Int): Permit` は待って取る．`handler TryPermits(n: Int): Permit fails NoPermits` は待たずに取り，取れなければ init が失敗する
- `with_permits` と `try_with_permits` は std の普通の Emela の関数で，member を開いて f を呼ぶだけ．操作が関数を受け取って E と R を呼び出し側に足す例外は要らない
- 「どう終わっても返す」「f の中の with が先に閉じる」「待っている間に中断されたら取らずに抜ける」は，release と with の入れ子と init の規則（8.4，14.2）から出る
- `Permit` effect（`fn count() -> Int`）と `NoPermits` エラーが増える．E0446 を避けるため Permit に操作を一つ置く
- 選ばなかった形：`with_permits` を組み込みの操作にする（関数を受け取る例外），`acquire`／`release` を操作にする（中断を受け取る手段が要る），`@intrinsic` の関数にする（本体のない関数の R に capability を書けない）

#### Spawn.start の関数は () を返す（決定 13）

- `fn start(f: fn() -> () fails E uses { R }) -> ()`．`List.each` の `f: fn(A) -> ()` と同じ形
- 結果が欲しいのに Background に入れた誤り（`bg.start(|| fetch_user(1))`）が型エラーになる
- 選ばなかった形：任意の A を受けて捨てる（取り違えが通る），A を受けて捨てたら警告（新しい警告が要る）

#### 節の名前（決定 6）

- `[fiber.*]` を `[task.*]` に改名し，旧名は `spec/history.md` に「(renamed)」の項で残す（`[target.c.export]` の前例）
- 採択済みの proposals は旧名のまま．spec と code comment は実装の PR で置き換える

| 今 | 新しい名前 |
|---|---|
| `[fiber]` | `[task]` |
| `[fiber.basic]` | `[task.basic]` |
| `[fiber.scope]` | `[task.group]` |
| （新規） | `[task.background]` |
| `[fiber.combinators]` | `[task.combinators]` |
| `[fiber.interrupt]` | `[task.interrupt]` |
| `[fiber.cause]` | `[task.cause]` |
| `[fiber.parallel]` | `[task.parallel]` |
| `[fiber.deferred]`，`[fiber.semaphore]`，`[fiber.queue]` | `[task.deferred]`，`[task.semaphore]`，`[task.queue]` |
| `[fiber.escape]` | 外す（`capability-identity` の規則で足りる）．history に「(retired)」で残す |
| `[api.std.fiber]` | `[api.std.task]` |

### 消えるもの，残るもの

- 消える：`Fiber.scope`，effect `Scope` とその型の例外，Scope 経由の資源の作成，E0425，`Fiber.await`，`Fiber.interrupt`，`CauseKind.Interrupt`
- 残る：Group が組み込みであること．任意の E と R を持つ関数を受け取る例外は Group と Background の `start` に寄る
- 残る：capability-bound type（`Task[A]`）．一般の仕組みとして使う
- 変わらない：async／await の構文を持たないこと，suspend を呼ぶ側で印を付けないこと，Async と Immediate

### Syntax

### Meaning

### Typing rules

## Specification changes

The changes are applied to `spec/` in this pull request.

- `[task]` (chapter 11, was `[fiber]`): replaced. `[fiber.*]` are renamed to `[task.*]`, `[task.background]` is new, and `[fiber.escape]` is retired. The old names are kept in `spec/history.md` as renamed and retired sections.
- `[module.resolution]`: a built-in handler can have the name of the one effect it implements.
- `[effect.decl]`, `[effect.escape]`, `[effect.async]`, `[effect.label]`, `[type.fn]`, `[type.rules]`, `[kw.annotation]`: TaskGroup and Spawn take the place of Scope.
- `[effect.handler]`, `[dyn.suspend]`, `[ext.call]`, `[target.js.abort]`: no call that may suspend after resume (E0461). The continuations left after resume are gone.
- `[api.std.task]` (was `[api.std.fiber]`), `[api.std.deferred]`, `[api.std.semaphore]`, `[api.std.queue]`, `[api.std.exit]`, `[api.std.cause]`: the new declarations. `CauseKind` loses Interrupt.
- `[api.std.http]`, `[lib.http-service]`, `[target.wasm.output]`: the app can use Spawn, and its tasks run after the response.
- `[lsp.code-action]`: the quickfix for a closure without parameters passed to `Fiber.scope` is removed. `[lsp.hover]`, `[lsp.completion]`: examples.
- `[diag.list]`: E0213, E0402, E0413, E0455 and the new E0461.
- Other sections: the names Fiber and Scope, and numbers of sections, in `[api.std.process]`, `[dyn.memory]`, `[lib.layers]`, `[lib.api-rules]`, `[target.js.outcome]`, `[target.js.event-loop]`, `[glossary.context]`, `[glossary.terms]` and `[open.next]`.

## Diagnostics

- [ ] E0461（新規）resume の後に suspend しうる呼び出し
- [ ] E0413 に TaskGroup，Spawn，Deferred，Semaphore，Queue を足す
- [ ] E0213 に組み込みの handler の例外を足す
- [ ] E0402 の note を `Fiber.scope` から `Task.Group` に移す
- [ ] 番号：E0455〜E0458 は capability-identity，E0459，E0460，E0462 は generic-handler が使う．E0461 は空いている

## Compatibility

- 前提の `capability-identity`（#732，実装 #813）と `generic-handler`（#817，実装 #825）は実装済み．capability-identity のうち `Fiber.scope(|s| ...)` の部分はこの proposal で置き換わる
- 旧名（`Fiber.scope`，`Std.Fiber`）は 1 minor の間警告にし，次で compile error にする

## Implementation

- WASM の `@entry(Http.serve)`（決定 10）：応答を先に返し，`Spawn` で切り出した仕事は同じ task の中で `task.return` の後に走らせる．今の entry は待つときに `waitable-set.wait` でブロックするので，その間 instance の lock を持ち続け，次の `handle` が待たされる．これは WASM ターゲットの実装上の制約として書き，言語の意味は変えない．entry を callback に WAIT を返す形へ作り直すのは別の issue にする
- WASM で `Spawn` を compile error にする形は取らない（entry の型が通ればどのターゲットでも動く，`[effect.host]`）
- 触る場所：`crates/emela-codegen-wasm/src/emit/runtime/http.rs` の `respond`（応答の確定とその後に分ける），`emit/mod.rs` の `Run::Service`，`std/http.emel` の `serve`

## Alternatives

- 案 A：`capability-identity` の `Fiber.scope(|s| ...)` で止める．Scope の例外規則が残り，切り出しの需要に答えない
- 案 C：スケジューラを利用者が書ける handler にする．継続を捨てる `resume` の使い方と，効果を持つ関数を受け取る操作が要る．後者は 8.1 の「handler を差し替えても呼ぶ側の R は変わらない」に触れる．長期の方向として残す
- 案 D：手綱をなくし組み合わせ関数だけにする．途中の join，接続ごとの処理，準備完了の合図を覆えない
- 名前：Fiber（仕組みの名前），Concurrent（長い，打ち間違えやすい），Bud ほか果樹の語（普遍的な語を選んだ），Job，Thread，Coroutine
- Background の本体が失敗したら子を中断する形：失敗した要求の記録が消える
- `on_failure` を effect にする形：`Cause[E]` を操作の引数にするには集合型引数が要る（E0424）
- app が capability を引数で受け取る形：すべての app の型が変わる
- Deferred などを Group の操作に戻す形，`Queue.run(100, |q| ...)` の形
- `await` と `interrupt` を残す形

## Prior art

- Effect-TS の Fiber，Scope，Deferred（今の 11 章の元）
- 構造化並行（nursery，task group）

## std の組み合わせ関数の案

今ある `all`，`race`，`both`，`map`，`timeout` は `Task` に移して残す．次は，必要になったら足す候補．この proposal では足さない．

| 関数 | 型 | 意味 | 動機 |
|---|---|---|---|
| `until` | `until[A, E, R](stop: use Deferred[()], f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { Async, R }` | stop が埋まるまで f を走らせる．先に埋まれば f を中断して None | `interrupt` を外した後，動的に増える子を個別に止める形（決定 5） |
| `map_limit` | `map_limit[A, B, E, R](xs: List[A], limit: Int, f: fn(A) -> B fails E uses { R }) -> List[B] fails E uses { Async, R }` | 同時に走るのを limit 個までにして map する | 11.3 が Semaphore を組む形として挙げている「1000 件を 10 並行で」を一行にする |
| `each` | `each[A, E, R](xs: List[A], f: fn(A) -> () fails E uses { R }) -> () fails E uses { Async, R }` | 結果を集めずに並行に回す | 15.4 の `each` に合わせる．`map` の結果を捨てる書き方を避ける |
| `first_success` | `first_success[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> A fails E uses { Async, R }` | 最初に成功したものを取り，失敗は無視する．全部失敗したら最後の原因で失敗する | 11.3 が「attempt と Deferred で書ける」と書いている形．複製先の読み取りなど |
| `settle` | `settle[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> List[Exit[A, E]] uses { Async, R }` | 全部を走らせ，失敗で止めずに Exit の列を返す | 一括処理で「成功したものだけ使い，失敗は報告する」形．各子を attempt で包む手間を省く |
| `workers` | `workers[A, E, R](jobs: use Queue[A], n: Int, f: fn(A) -> () fails E uses { R }) -> () fails E uses { Async, R }` | n 個の子が jobs から取り出して f を回し，jobs が閉じて空になったら終わる | 受け付けループと処理の分離，生産者と消費者．`semaphore-queue` の例の `work` を一般にした形 |

- 名前は 15.4 の規則（`map`，`each`）に合わせる．`map_limit` は `map` に引数を足す形（`limit:` を省けない引数にはできないので別名）にしている
- `settle` の Exit は `CauseKind` が Fail と Die の二つになった後の形
- 監督（失敗したら走らせ直す）は，方針の種類が多く std で一つに決めにくいので候補から外した．案 C の方向で利用者が書けるようになってから考える

## Open questions

- 型引数を省いた `TaskGroup` と `Spawn` を書けるか．決定 7 の例（`tasks: use TaskGroup`，`uses { Spawn }`）は型引数を省いているが，今の規則では uses 節と型の位置で省くと E0423 になる（省けるのは `use` 式だけ）．spec には今の規則のまま型引数を書く形を入れた（11.2 の `fn crawl[E, R](tasks: use TaskGroup[E, R], ...)`，15.3 の `uses { Spawn[{}, { Async }] }`）．省けるようにするなら，省いたときの意味（その場で量化するか，推論するか）を決める
- `Http.serve` の app の型．spec には `serve[E, F](app: fn(Request) -> Response fails E uses { Spawn[F, { Async }], Async }, ...)` を入れた．F は切り出した仕事の E，`{ Async }` は adapter がその仕事に与える R．app の側で E を持つ仕事を切り出せるか（F を開くか `{}` に固定するか），R に Async 以外（ProcessHost）を入れるかが未決
- `try_with_permits` の f が NoPermits で失敗したとき，escape が受けて None になる．f の E に NoPermits が入る場合を許すか，`with_permits` と同じく組み込みの側で区別するか

## Decision
