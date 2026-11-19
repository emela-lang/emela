---
title: Handler members without parameters are written without parentheses as with items, as handlers are
authors: [somnicattus]
status: draft
pr: 703
issue: 700
decided:
touches: [effect.decl, effect.use, effect.with, effect.handler, grammar.decl, diag.list]
requires: []
related: [handler-member, handler-member-gaps, handler-member-gaps-2, multi-effect-handler, handler-init-arguments]
---

# Handler members without parameters are written without parentheses as with items, as handlers are

## Summary

handler member を開く形の括弧を，書く位置で決める．

- with の項目では，handler と同じく引数で決める．handler は init が引数を取らなければ `H` と書き，`H()` は誤り（E0435），取るなら `H(arguments)` と書き，省くのは誤り（E0434）．member も，引数がなければ `x = E.M`，`x = c.M`（`x = E.M()` は E0435），引数があれば `x = E.M(arguments)`（省くと E0434）
- init の中の式では，今と同じく関数の呼び出しの形で，常に括弧を書く（`E.M()`，`E.M(arguments)`）
- 宣言の括弧は，引数がなければ省ける（`handler Scratch: E` は `handler Scratch(): E` と同じ）

## Motivation

今の仕様は，handler member を開く形を `E.M(arguments)` と `c.M(arguments)` で書いている．init の中の式は文法（`[grammar.expr]` の `postfix`）が括弧を要るので，引数のない member は `E.M()` と決まっている．with の項目は，文法（`supply`）は括弧を省けるが，本文は `x = E.M(arguments)` の形だけを書いていて，引数のない member の項目を `x = E.M` と書くか `x = E.M()` と書くかは決まっていない．宣言の文法 `member_sig` は括弧を要る．

一方，handler を with の項目に書く形は，init の引数で決まっている（`[effect.handler]`: 「A handler whose init takes no parameters cannot be given arguments, and `H()` cannot be written for it.」）．with の項目は，handler も括弧なしで init を走らせる位置である．member も「名前を付けた handler を開く」ものなので，その位置では同じ規則で書けるのが自然である．

main の実装は，括弧について次のようにばらばらである（main，b4312ee を build して確かめた．詳しくは issue #700）．

| 書き方 | 仕様（main） | 実装（main） | この提案 |
|---|---|---|---|
| 宣言 `handler Scratch: ReadFile` | 書けない（`member_sig` は括弧が要る） | 構文エラー | 書ける（`handler Scratch(): ReadFile` と同じ） |
| with の項目 `x = Files.Temporary`（`Temporary()`） | 決まっていない | 通る | 正しい形 |
| with の項目 `x = Files.Temporary()` | 決まっていない | 通る | E0435 |
| with の項目 `x = fs.Temporary`（`fs: use Files`） | 決まっていない（`supply` の2つ目の形は括弧が要る） | 構文エラー（E0110 `expected '('`） | 正しい形 |
| with の項目 `x = Files.Open`（`Open(path: String)`） | 引数がない | `emela check` は通り，`emela run` は IR の検証でコンパイラが panic する（issue #700） | E0434 |
| init の中の `Files.Temporary()` | 正しい形 | 通る | 正しい形（変わらない） |
| init の中の `Files.Temporary`（呼ばない） | 書けない（`postfix` は括弧が要る） | E0448（「with か init でしか開けない」．init の中なので合わない）と，続けて E0301（`fn() -> use ReadFile` と型が合わない） | 書けない．E0448 の文を「括弧を付けて呼ぶ」に直す |

with の項目 `x = Files.Open` が通るのは仕様との食い違いなので，この提案と別に直す（#700）．

init の中の式で括弧を残すのは，式の中で init が走る（中断も失敗もしうる）所には，関数の呼び出しと同じく括弧があるようにするためである．括弧のない `E.M` は，見た目ではフィールドの参照や値と区別がつかない．引数にデフォルト値を書ける機能が入っても，式の `E.M()` は「全部デフォルトで開く」と読めるので，あいまいにならない．

引数のない member は，開くたびに新しい資源を作るもの（一時ファイル，メモリ上の表）に使える．今は使っているコードが少ないが，書ける形である以上，with の項目での開き方を決めておく．

## Proposal

### Syntax

```emel
pub effect Files {
  handler Scratch: ReadFile + WriteFile
  suspend handler Open(path: String): ReadFile
    fails FileNotFound | FileAccessDenied
}

fn open_two(fs: use Files) -> () fails FileNotFound | FileAccessDenied uses { Files } {
  with s = fs.Scratch, src = Files.Open("in.txt") {
    ()
  }
}

handler Mirror(scratch: use WriteFile): WriteFile {
  init {
    fs = use Files
    Self(scratch: fs.Scratch())
  }

  fn write(text) { self.scratch.write(text) }
}
```

- `member_sig` の括弧を省ける．`handler Scratch: E` と `handler Scratch(): E` は同じ意味で，どちらも引数を持たない
- with の項目 `x = c.M` の括弧を省ける（`x = E.M` は今も文法上省ける）
- init の中の式の形は変わらない（常に括弧）

### Meaning

- with の項目では，handler の項目と同じく，member の引数で括弧が決まる．引数のない member は `x = E.M`，`x = c.M` と書き，`x = E.M()` は誤り（E0435）．引数のある member は `x = E.M(arguments)` と書き，括弧を省くのは誤り（E0434）
- init の中の式では，今と同じく常に括弧を書く．引数のない member は `E.M()`，`c.M()`．括弧のない `E.M` は誤り（E0448 の文を直す．Diagnostics）．init の中で effect の名前から開く形（`E.M()`）をやめる提案（#702）が採られれば，init の中は `c.M()` だけになるが，括弧の規則は変わらない
- 宣言の括弧の有無は，開く形に関係しない．init の `init` と `init()` と同じく，引数があるかだけで決まる
- inner handler の書き方は変わらない．inner handler の名前の後の括弧はそのフィールドで，member の宣言の括弧とは関係ない．init の引数は今と同じく member の引数と一致させる（引数のない member なら，init は引数を取らない）

### Typing rules

- 型，E，R は変わらない．with の項目 `x = E.M` は，`[type.rules]` の `with x = E.M(a) { e }` の行で a が空のものと同じ

## Specification changes

### `[effect.decl]`

Replace the item "An effect MAY declare handler members (`handler Open(path: String): ReadFile fails ...`). ..." with:

> - An effect MAY declare handler members (`handler Open(path: String): ReadFile fails ...`). A handler member is not an operation: it names a handler that every handler of the effect provides. Its parameters are the parameters of that handler's init, and its `fails` is the E of that init. The effect after the colon is the effect that the handler implements. For a member without parameters, the parentheses after its name MAY be left out (`handler Scratch: ReadFile` is the same as `handler Scratch(): ReadFile`).

### `[effect.use]`

Replace the item "On a capability c of an effect E that has handler members, `c.M(arguments)` opens ..." with:

> - On a capability c of an effect E that has handler members, `c.M(arguments)` opens the handler member M of the handler that provides c. It can be written only as a with item `x = c.M(arguments)`, or `x = c.M` for a member without parameters (8.3), and inside an init (8.4).

### `[effect.with]`

Replace the item "`E.M(arguments)`, where E is an effect and M one of its handler members, can be written as an item only in the form `x = E.M(arguments)`. ..." with:

> - `E.M(arguments)`, where E is an effect and M one of its handler members, can be written as an item only in the form `x = E.M(arguments)`. It calls the init of M of the innermost handler that provides E at that position. `x = c.M(arguments)`, where c is a capability of E, calls the init of M of the handler that provides c. The arguments are treated as for `H(arguments)` (8.4): for a member without parameters, the item is written `x = E.M` or `x = c.M`, and writing `x = E.M()` is a compile error (E0435); for a member with parameters, leaving the arguments out is a compile error (E0434). The item requires E (or c), not the requirements of the inner handler.

### `[effect.handler]`

In the item "Inside an init, a handler member can be written as an expression. ...", after "runs that of the handler that provides c.", add:

> As in any call, the parentheses are written also for a member without parameters (`E.M()`).

### `[grammar.decl]`

Replace `member_sig` and `supply` with:

```ebnf
member_sig   = [ "suspend" ] "handler" type_name [ "(" [ params ] ")" ] ":" effect_ref { "+" effect_ref }
               [ "fails" error_set ]                              (* a handler member; 8.1 *)
supply       = type_ref [ "." type_name ] [ "(" [ args ] ")" ]
             | lower_name "." type_name [ "(" [ args ] ")" ]      (* a handler member of a capability; 8.3 *)
```

### `[diag.list]`

Replace the rows of E0434 and E0435 with:

> | E0434 | A handler whose init takes parameters is written without arguments, as an item of a with or a layer or on the right of `by` (8.4), or a handler member with parameters is written without arguments as an item of a with (8.3). The help is ``= help: write `WorkerKv(binding: ..., retries: ...)` `` (``= help: write `Files.Open(path: ...)` `` for a member) | ``handler `WorkerKv` needs arguments for its `init` `` / ``handler member `Open` needs arguments`` |
> | E0435 | Arguments are written for something that takes none: a handler whose init takes no parameters (including `H()`), a layer, a label, or a handler member without parameters as an item of a with (including `x = E.M()`) (8.3, 8.4). The help is ``= help: write `FakeKv` without parentheses`` | ``handler `FakeKv` takes no arguments`` / ``handler member `Scratch` takes no arguments`` |

In the row of E0448, add: for `E.M` or `c.M` written inside an init without being called, the message is ``handler member `Temporary` is opened by calling it`` with ``= help: write `Files.Temporary()` ``.

## Diagnostics

- [ ] A handler member with parameters written without arguments as a with item (`x = Files.Open`): E0434 (extended; it passes silently now)
- [ ] A handler member without parameters written with parentheses as a with item (`x = Files.Scratch()`): E0435 (extended)
- [ ] A handler member written inside an init without being called (`Files.Scratch`): E0448 with a message that asks for the call (the message changes)

## Compatibility

- init の中の式の形は変わらない．引数のある member の with の項目も変わらない
- 引数のない member を with の項目で `x = E.M()` と書く形が誤り（E0435）になる．main のソース，テスト，docs，spec には見つからなかった．`proposals/multi-effect-handler.md` の例 `tmp = Files.Temporary()`（with の項目）はこの形だが，採択済みの proposal の記録なので書き換えない（spec には入っていない）
- main にある引数のない member の宣言（`crates/emela-syntax/test_data/parser/ok/handler_member.emel` の `handler Memory(): State[Int]`，`multi_effect_handler.emel` の `handler Temporary(): ...`）は，そのまま書ける．開く箇所はない
- 引数のない member を with の項目で `x = E.M` と書く形は，今も実装が通しているので，それが仕様になる

## Implementation

- emela-syntax: `member_sig` の括弧を省ける（`parser/grammar/items.rs`）．`cap_member_supply` の括弧を省ける（`parser/grammar/expressions.rs`）
- emela-types: with の項目で，member の引数の有無と項目の括弧を照らす（`infer_with_args` の `None => Vec::new()` を，引数のある member なら E0434 にする．引数のない member に `()` があれば E0435 にする）．init の中の呼び出しでない `E.M` の E0448（`check_member_open`）を，呼び出しを促す文にする
- emela-driver: E0448 の文と help
- emela-fmt: 宣言と項目の括弧をそのまま保つ（宣言の `()` を消すかは Open questions）
- tree-sitter（`editors/tree-sitter-emela/grammar.js` の `handler_member`）: 宣言の括弧を省ける

## Alternatives

- **今の仕様のまま，member は常に括弧を付ける**: 意味の変更は要らない．ただ，with の項目で handler は引数がなければ括弧を書かないので，同じ位置で同じ「handler を開く」形の書き方が逆になる
- **init の中の式でも括弧を省く（`Files.Scratch`）**: with の項目と init の中で同じ形になる．ただ，式の中で init が走る（中断も失敗もしうる）所に括弧がなくなり，フィールドの参照や値と見分けがつかない．今の仕様（`postfix` は括弧が要る）を逆にする変更にもなる
- **宣言の括弧の有無で決める（`handler M: E` は `E.M`，`handler M(): E` は `E.M()`）**: 引数にデフォルト値が入っても，開く形があいまいにならない．ただ，handler の with の項目は宣言の形ではなく init の引数で決まり，handler の名前の後の括弧はフィールドの宣言なので，同じ考え方を handler に当てはめられない．member と handler で規則が分かれる
- **引数のない member は，with の項目で括弧があってもなくてもよい**: 書き手に優しいが，同じ member に2つの形ができる．handler の `H()` が誤りであることとも合わない

## Prior art

- Uniform access: a parameterless member is written the same way whether it is computed or stored, so that the caller does not depend on how it is provided.

## Open questions

- 引数にデフォルト値を書ける機能が入ると，with の項目で全部の引数を省いた `x = E.M` を，引数のない member と同じに読むかを決める規則が要る．handler の `H` も同じなので，その機能を入れるときに handler と member を一緒に決める．init の中の式は `E.M()` なので影響を受けない
- 宣言の `handler M(): E` と `handler M: E` のどちらかに揃えるか（`emela fmt` がどちらに直すか）．init の `init()` と `init` の扱いに合わせる
- with の項目と init の中で，引数のない member の書き方が違う（`x = E.M` と `E.M()`）ことを，チュートリアルや docs でどう説明するか

## Decision
