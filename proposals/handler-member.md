---
title: Handler members
authors: [Alliana]
status: draft
pr: 624
issue: 616
decided:
touches: [effect.decl, effect.handler, effect.with, type.rules, dyn.with, grammar.decl, grammar.expr, diag.list]
requires: [capability-type, handler-self-constructor]
related: []
---

# Handler members

## Summary

effect が「供給者が用意する handler」を宣言できるようにする（handler member）．供給する handler は，それを同じ名前の内側の handler で実装する．利用者は `with x = Files.Open("a.txt") { ... }` で init を呼び，入った capability を x に束縛する．handler の init の中でも handler member を開け，その寿命は開いた handler の寿命になる．

## Motivation

#616 が挙げる4つを同時に満たしたい．

- (a) 開いた資源ごとの状態（ホストの記述子など）を，その資源の handler が持つ
- (b) 資源の handler の実装を，親の handler（ファイルシステム）が選ぶ．親を本物からテスト用に差し替えると，資源の handler も替わり，呼び出し側は書き換えない
- (c) 資源を指す値も，閉じる操作も，effect の操作として出てこない
- (d) 資源を使う中身が，ふつうの with の本体である

今の書き方はどれも少なくとも1つを欠く（#616 の表）．

- 資源の handler を利用者が `with HostFile(path: ...)` と名前で書く: (b)
- 親の effect にハンドルを返す操作と閉じる操作を足して転送する: (a) と (c)
- 親の操作が capability を捕まえたクロージャの組を返す: (c)
- 親の操作が中身を関数として受け取る: (d)

`[api.std.file-system]` の操作はどれもパスを受け取り一回で完結するので，ロックのように操作と操作の間ずっと持っていることに意味があるものも表せない（#616 のコメント）．

```emel
# 書きたいもの
with src = Files.Open("in.txt"), dst = Files.Create("out.txt") {
  copy(src, dst)
}
```

議論の経緯（2026-11-16，17）:

- 操作が所有値を返す形（`suspend fn open(path) -> with ReadableFile`，型ごとの `release(f: T)`）は，値の中身が handler ごとに違うので effect の型メンバーが要り，型メンバーがあると別の handler の値を渡す穴が開いて，値ごとの所有者の規則が要った．資源の側に操作を置けば（資源ごとの handler），中身は handler の `self` なので，どちらも要らない
- 操作が handler を返す形（`-> handler ReadFile`）は，handler を値として返す見た目になり，「handler を変数に束縛しない」原則と食い違う
- handler を第一級の値にすると静的な解決（atlas「effect はコンパイル時 DI 化」）が崩れる

## Proposal

### Syntax

```emel
pub effect ReadFile {
  suspend fn read(max: Int) -> Option[Bytes] fails FileSystemFailure
}

pub effect WriteFile {
  suspend fn write(data: Bytes) -> () fails FileSystemFailure
  suspend fn sync() -> () fails FileSystemFailure
}

pub effect Files {
  handler Open(path: String): ReadFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
  handler Create(path: String): WriteFile
    fails FileNotFound | FileAccessDenied | IsADirectory | FileSystemFailure
}

pub handler SystemFiles: Files {
  handler Open(handle: Handle, path: String): ReadFile {
    init(path: String) {
      o = host_open(path, "r")
      check_open(o, path)
      Self(handle: outcome_handle(o), path:)
    }
    release { host_close(self.handle) }
    suspend fn read(max, resume) { ... }
  }

  handler Create(handle: Handle, path: String): WriteFile { ... }
}

fn copy(src: use ReadFile, dst: use WriteFile) -> () fails FileSystemFailure {
  src.read(65536) match {
    None        -> ()
    Some(chunk) -> {
      dst.write(chunk)
      copy(src, dst)
    }
  }
}

fn main() {
  with SystemFiles {
    with src = Files.Open("in.txt"), dst = Files.Create("out.txt") {
      copy(src, dst)
    }
  }
}
```

返された handler を包む供給者は，init の中で外側の handler member を開き，フィールドに持つ．

```emel
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) {
      Self(inner: Files.Open(path)) # the outer Files; lives as long as this handler
    }

    suspend fn read(max, resume) {
      (use Console).print_line("read #{max}")
      resume(self.inner.read(max))
    }
  }

  handler Create(inner: use WriteFile): WriteFile { ... }
}
```

- effect の中の `handler Name(params): Effect fails ...` が handler member の宣言．操作ではないので戻り値の型はない
- 供給する handler は，同じ名前の内側の handler で実装する．内側の handler の頭の括弧はフィールド，`init` の引数は宣言の params と一致させる
- with の項目は `x = Item` の形で書ける．Item が handler member（`Files.Open(...)`）なら，この形でしか書けない
- handler の init の中では，handler member を式として書ける（`Files.Open(path)`）

### Meaning

- with の項目 `x = Files.Open(args)` は，その位置から見て一番内側の Files の供給者が持つ `Open` の init を args で呼び，入った capability を x に束縛する．`H(args)` の項目（handler-init-arguments）と同じく，args は with の外で，書いた順に評価する
- 内側の handler の init，release，操作の本体が使う effect は，外側の handler の requirement になる（requires に含める）．外側の handler の with で解決されるので，`Files.Open` を書いた with の R には Files だけが出る．Files の供給者を差し替えても，呼び出し側の R は変わらない
- 内側の handler の値は，その init の中でしか作れない（E0418 のまま）．内側の handler は外から名前で参照できない
- `with x = Item` の x は，同じ with に同じ effect の項目が2つあっても，その項目の handler を指す
- handler member の項目が供給する effect は，名前で引く `use E` の対象にならない．束縛した x からだけ使う．資源は束縛して渡し，名前で引くのはサービスに限る
- 束縛した項目は，ほかの項目と同じく初期化と逆の順に release する
- init の中の handler member の式は，その handler member の init をその場で呼び，capability を返す．開いたものの寿命は，init を書いた handler の寿命になる
  - その handler の release の後に，開いた順と逆の順に release する
  - init が途中で失敗したら，それまでに開いたものを逆の順に release する
  - 自分の effect の handler member を書いたとき（TracedFiles の中の `Files.Open`）は，外側の供給者を指す．自分の effect への requirement が外側で満たされる規則（`[effect.with]`）と同じ

### Typing rules

- with の項目 `x = E.M(a)` は {E} ∪ (a の R) を要求し，M の effect を供給する．x の型は `use F`（F は M の effect）
- init の中の `E.M(a)` の型は `use F`．E は init の R に入る（外側の handler の requirement になる）．E は M の init の E

## Specification changes

### `[effect.decl]`

Add after the item on the `uses` clause of an operation:

> - An effect MAY declare handler members (`handler Open(path: String): ReadFile fails ...`). A handler member is not an operation: it names a handler that every handler of the effect provides. Its parameters are the parameters of that handler's init, and its `fails` is the E of that init. The effect after the colon is the effect that the handler implements.

### `[effect.handler]`

Add:

> - A handler of an effect with handler members implements each of them with an inner handler of the same name (`handler Open(handle: Handle, path: String): ReadFile { ... }`). The parentheses after the name declare its fields, and its init MUST take the parameters of the handler member, with the same names and types. A missing handler member is E0404, as for an operation.
> - The effects used in the init, the release and the operation bodies of an inner handler are requirements of the outer handler (requires(H), 9.4). They are provided where the outer handler is provided.
> - An inner handler cannot be referred to by name outside its outer handler.
> - Inside an init, a handler member can be written as an expression (`Files.Open(path)`). It runs the init of that handler member of the innermost handler that provides the effect, and its value is the capability of the handler opened. The handler opened lives as long as the handler whose init opened it: it is released after that handler's release, in the reverse order of opening. If the init fails, the handlers it opened so far are released in reverse order. A handler member of the handler's own effect refers to the outer handler.

### `[effect.with]`

Add:

> - An item can be written as `x = Item`. Within the body, x is the capability of the effect that Item provides (`use F`, 8.2), and refers to the handler of that item even when another item of the same with provides the same effect.
> - `E.M(arguments)`, where E is an effect and M one of its handler members, can be written as an item only in the form `x = E.M(arguments)`. It calls the init of M of the innermost handler that provides E at that position. The arguments are treated as for `H(arguments)`. The item requires E, not the requirements of the inner handler.
> - The effect provided by a handler member item is not provided to `use` by name. It is reached only through the bound capability.

### `[type.rules]`

Add rows:

> | `with x = E.M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | (Re ∖ {}) ∪ { E } ∪ Ra |
> | `E.M(a)` inside an init | `use F` (F is the effect of M) | Ea ∪ (E of the init of M) | { E } ∪ Ra |

### `[dyn.with]`

Add to step 1:

> A handler member item `x = E.M(arguments)` runs the init of M of the handler that provides E, and is ordered with the other items by its requirements (only E).

Add:

> - Handlers opened inside an init (8.4) are released after the release of the handler that opened them, in reverse order.

### `[grammar.decl]`

```ebnf
effect_decl  = "effect" type_name [ type_params ] "{" { effect_item NL } "}"
             | ...
effect_item  = op_sig | member_sig
member_sig   = "handler" type_name "(" [ params ] ")" ":" type_ref [ "[" type { "," type } "]" ]
               [ "fails" error_set ]

handler_item = "init" [ "(" [ params ] ")" ] block
             | "release" block
             | [ "suspend" ] "fn" lower_name "(" [ lower_name { "," lower_name } ] ")" block
             | handler_decl                                      (* inner handler; 8.4 *)
```

### `[grammar.expr]`

```ebnf
with_item  = [ lower_name "=" ] supply [ "by" by_right ]
supply     = type_ref [ "." type_name ] [ "(" [ args ] ")" ]
```

## Diagnostics

- [ ] A handler member item is written without `x =`: code to be assigned
- [ ] A handler member is written outside a with item and an init (`f = Files.Open(...)` in a fn): code to be assigned
- [ ] The init of an inner handler does not take the parameters of its handler member: code to be assigned
- [ ] A missing handler member in a handler: E0404 (extended)
- [ ] A lint: the same capability variable passed to two parameters of one call (`copy(tmp, tmp)`): code to be assigned (`[lint]`, draft `linter`)

## Compatibility

追加だけ．既存の effect に handler member を足すと，利用者が書いたその effect の handler は E0404 で壊れる．操作を足すときと同じ．

## Implementation

- emela-syntax: member_sig，inner handler，`x = Item`
- emela-resolve: `E.M` の解決，inner handler の名前の可視性
- emela-types: handler member の実装の検査，inner handler の requirement を外側へ，init の中の member の式
- emela-core: handler member の項目の lowering（供給者の inner handler の init を呼ぶ），init の中で開いたものを release の後に閉じる
- std: Std.Files（仮）．Std.Tcp（PR #563）をこの形にするかは別に決める

## Alternatives

- 操作が所有値を返す（`suspend fn open(path) -> with ReadableFile`，型ごとの `release(f: T)`）: 値の中身が handler ごとに違うので effect の型メンバーが要り，別の handler の値を渡す穴を所有者の規則で塞ぐ必要があった
- 操作が初期化前の handler を返す（#616，`fn open(path: String) -> handler File`，式 `handler H(args)`）: (a) から (d) を満たし，返す handler の requirement を作った位置で満たす点はこの提案と同じ．違いは次のとおり
  - 初期化前の handler という値と型（`handler X`）が増え，置ける位置の規則が要る．この提案では値も型も出てこない
  - init の E を作る式の E に数えるので，その式に escape を付けられない特別な規則が要る．この提案では init の E は with の項目の E で，`with x = Files.Open(p) { ... } escape { ... }` がそのまま書ける
  - 操作の本体で，引数に応じて返す handler を選べる．この提案では，handler member の実装は供給者ごとに1つの宣言に決まり，静的に解決される．実行時に選ぶには，inner handler の中で分岐する
  - 返された handler を包む供給者は未決定の点として残っている．この提案では，init の中で外側の handler member を開いてフィールドに持つ（capability-type）
- 操作が継続に handler を被せる（`-> () uses { ReadFile }` を「呼んだ後に使える」と読む）: 閉じる場所が Files の with の終わりになる．R が文ごとに変わる．呼び出し元の `use` が実行時に被せた handler を見ることになり，静的な解決が崩れる
- 第一級の handler: 静的な解決が崩れる．寿命をレキシカルに保つには，持ち出しを型で止める必要がある
- 名前で引く形とラベルで資源を区別する（`with Source by Files.Open(...)`）: 役割ごとにラベルを最上位で宣言する必要がある．束縛の形と両方を許すと，ライブラリごとに `copy() uses { ReadFile }` と `copy(src: use ReadFile)` が混ざる
- inner handler に「一緒に初期化する項目」を書く構文（`init(path: String) with inner = Files.Open(path) { ... }`）: init の頭で束縛した名前が操作の本体から見えるスコープの例外が要る．フィールドに `use E` を持つ形なら，状態はフィールドという今の規則のまま
- 値の解放で release を走らせる，線形型で使い切らせる: 後始末を release だけに限り，解放を観測できないとする `[dyn.memory]` と衝突する

## Prior art

- Scoped resource management: acquisition and release tied to a scope, with the scope as a requirement in the effect row. Effect-TS (`Scope`, `acquireRelease`, `@effect/platform` FileSystem's `open`) has it; resources are first-class values and escaping them is not checked.
- Named handlers and scoped effects: a handler instance as a value, with a rank-2 type variable that keeps it inside its scope. Koka has them (Xie, Cong, Ikemoto, Leijen, "First-class names for effect handlers", OOPSLA 2022).
- Abstract members implemented per instance: an interface that declares a nested component, which each implementation supplies.

## Open questions

- inner handler から外側の handler の `self`（フィールド）を参照できるか
- `Files.Open` の `Files` の解決．Std.Files のようにモジュールと effect が同じ名前のとき，`Files.Open` はどちらとして読むか
- `with x = H` を通常の handler やラベルの項目にも許すか（`with log = ConsoleLogger { ... }`，`with s = Source by P`）
- 一番内側ではなく特定の供給者の handler member を使う形（`fn open_in(fs: use Files) { with f = fs.Open(p) { ... } }`）を許すか．#616 の `with fs.open(...)` はこれに当たる
- handler member を layer の項目に書けないことの確認（束縛できないため）
- テスト用の供給者で，開いたファイルごとに変わる状態の置き場（#616 のコメント）．inner handler はフィールドを操作の本体で変えられないので，外側と共有する State に表を置くことになる
- Std.Tcp（PR #563）の `Connection` をこの形に揃えるか

## Decision
