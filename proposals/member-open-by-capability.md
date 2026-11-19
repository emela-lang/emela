---
title: Handler members are opened only by calling them on a capability
authors: [somnicattus]
status: draft
pr: 702
issue:
decided:
touches: [effect.decl, effect.use, effect.with, effect.handler, effect.label, type.rules, dyn.with, module.resolution, grammar.decl, grammar.expr, diag.list]
requires: []
related: [handler-member, handler-member-gaps, handler-member-gaps-2]
---

# Handler members are opened only by calling them on a capability

## Summary

handler member を開く形を，capability に対する呼び出し `c.M(arguments)` だけにする．effect や label の名前から開く形（with の項目の `x = Files.Open(path)`，init の中の `Files.Open(path)`，`L.Open(path)`）はやめ，`(use Files).Open(path)` か，`fs = use Files` の後の `fs.Open(path)` と書く．capability の値に付けた `.M` は，operation と同じく呼び出しの形で書くので，引数のない member でも括弧を書く（`fs.Scratch()`）．with の member の項目は，同じ with の handler の項目をすべて init してから書いた順に開き，本体の `use E` と同じ供給者から開く．

これで `.` の左に effect の名前を書く形はなくなる．供給者に届くのは `use` だけになり，handler member を開けるのは capability だけになる．

この提案は，はじめは init の中だけを扱っていた．with の項目の括弧を位置で決める提案（#703）と合わせて検討し，with の項目にも広げ，capability を通して開くときは常に括弧を書くこととした．#703 はこの提案に統合して閉じた．

## Motivation

### `use` は引き，`.M(...)` は開く

- `use E` は「引く」: その位置で E を供給する handler の capability を取る．init は走らず，どこにでも書ける
- `c.M(arguments)` は「開く」: c を供給する handler の member M の init を走らせ，新しい handler を作る．失敗も中断もしうるし，開いた handler は後で release される

`(use E).M(arguments)` は，引いてから開く形で，2 つの操作にそれぞれ印がある．今の `E.M(arguments)` は 2 つを 1 つにまとめていて，引く印（`use`）を持たない．`[effect.use]` は operation を effect の名前で呼ぶ形を持たない（"There is no form that calls them directly by effect name (`Logger.info(...)`)"）．handler member を effect の名前から開く形だけが，この原則の外にある．

### with の項目の member は，供給者を名前で並べていない

with の項目で handler `H` を書くと，H は本体に effect を名前で供給する（本体の `use F` が届く）．member の項目はそうではない（`[effect.with]`: "The effect provided by a handler member item is not provided to `use` by name. It is reached only through the bound capability."）．member の項目は，capability を 1 つ作って束縛するだけで，供給者を並べていない．

なので，「with の項目は供給者を名前で並べる位置だから，effect の名前を書いてよい」という理由は，member の項目には当たらない．with の項目と init の中で，開く形を分ける理由はない．

### `.` の左の effect の名前の扱いが消える

今の `[module.resolution]` は，`.` の左で effect を引く．引くのは handler member を指すときだけで，モジュールと effect が同じ名前のときは member を先にする（`handler-member-gaps-2`，#670）．`[effect.use]` は operation を effect の名前で呼ぶ形を持たないのに，handler member は effect の名前で開ける．operation だけを止める理由は，operation の名前は小文字でモジュールの関数（`State.get`）とぶつかるが，member は大文字なのでぶつからない，という名前の都合だけである．

`.` の左に effect を書く形がなくなれば，この規則はいらない．`Logger.info` と `Files.Open` は，「effect は値ではない」という同じ理由で書けなくなる．

### capability の値に付けた `.M` は，operation と同じく呼び出しの形で書く

括弧の有無は，開くかどうかでは決まらない．with の項目の handler `H` も init を走らせて開くが，引数がなければ括弧を書かない（`H()` は E0435）．括弧の有無は，名前で書くか，値を通すかで決まる．

- 名前で書くもの（with の項目と layer の handler，label，`by` の右）は，宣言を名前で指す．括弧は init の引数を渡すときだけ書く
- capability の値に付けた `.name` は，値を通して届く．`log.info(...)` は呼び出しで，括弧のない `log.info` は呼ばずに関数の値を取る（`[effect.use]`）

handler member も capability の値を通して届くので，operation と同じく `c.M(...)` が呼び出しになり，引数がなくても括弧を書く．括弧のない `c.M` を「開く」とすると，capability の値に付けた括弧のない `.name` が，小文字（`log.info`）なら関数の値，大文字（`c.Scratch`）なら開く，と字の大小で意味が分かれる．この提案では，括弧のない `c.M` は誤り（E0448）とし，関数のような値（後で呼べば開ける値）にもしない．

member は with の項目と init の中でだけ開け（E0448），開いた handler の寿命はそこで決まる．with の項目で開いたものはその with と同じだけ生き，init の中で開いたものはその init を走らせた handler と同じだけ生きる（`[dyn.with]`，`[effect.handler]`）．なので，開くことはその位置から切り離せない．括弧のない `c.M` を関数のような値にすると，その値は fn に渡したり closure に入れたりして，別の位置へ運べる．c を持つ値として扱えば，c を供給する with の外には出られない（`[effect.escape]`）が，fn の中や後で呼ぶ closure の中で開いた handler を，誰がいつ release するかは決まらない．`log.info` が関数の値にできるのは，log を持つ値として log を供給する with の外に出られず（`[effect.use]`，`[effect.escape]`），呼んでも新しいものを作らないので，それで足りるからである．

今の文法でも，capability に対して開く形は括弧が要る．init の中の `postfix` は `"." type_name "(" [ args ] ")"` で，with の項目の `supply` の 2 つ目の形は `lower_name "." type_name "(" [ args ] ")"` である．括弧を省ける形は `supply` の 1 つ目，つまり effect の名前から開く `x = E.M` だけで，それもこの提案でなくなる．この提案の文法は，括弧のない `c.M` も受け付けて，本文の規則で誤り（E0448）にする．括弧を書くよう促す help を出すためである．一つの with に `with H, x = (use E).M()` と括弧の有無が違う項目が並ぶのは，名前と値の違いである．

今の実装は括弧について揃っていない（main，b4312ee）:

| 書き方 | 今の仕様 | 今の実装 | この提案 |
|---|---|---|---|
| with の項目 `x = Files.Temporary`（`Temporary()`） | 決まっていない | 通る | E0448（effect の名前．help は `(use Files).Temporary()`） |
| with の項目 `x = Files.Open`（`Open(path: String)`） | 引数がない | `emela check` は通り，`emela run` で panic（#700．#706 で E0434 にする） | E0448（effect の名前．help は `(use Files).Open(path: ...)`） |
| with の項目 `x = fs.Temporary` | 書けない（`supply` は括弧が要る） | 構文エラー（E0110） | E0448（括弧がない） |
| init の中の `Files.Temporary`（呼ばない） | 書けない（`postfix` は括弧が要る） | E0448（「with か init でしか開けない」で，init の中なので合わない）と E0301 | E0448（effect の名前．help は `(use Files).Temporary()`） |
| init の中の `fs.Temporary`（呼ばない） | 書けない（`postfix` は括弧が要る） | 確かめていない | E0448（括弧がない．help は `fs.Temporary()`） |

### 失うものはない

`(use Files).Open(path)` と，`fs = use Files` の後の `fs.Open(path)` は，今も with の項目でも init の中でも書け（with の項目の `(use Files)` は文法に足す），同じ意味になる．effect の名前から開く形は v0.28.0（2026-11-18）で出たばかりで，main で使っているのはテストだけである（Compatibility）．

## Proposal

### Syntax

with の項目に `x = (use E).M(arguments)` の形を足し，effect の名前から開く `supply` の形を外す．init の中の式は今も `(use E).M(arguments)` と書ける（`postfix`）．capability に付けた `.M` は，文法では括弧を省けるようにし，括弧がなければ本文の規則で誤り（E0448）にする．

```emel
effect ReadFile {
  fn read(max: Int) -> Int
}

effect Files {
  handler Open(path: String): ReadFile
  handler Scratch(): ReadFile
}

fn total(path: String) -> Int uses { Files } {
  with src = (use Files).Open(path), tmp = (use Files).Scratch() {
    src.read(10) + tmp.read(10)
  }
}

handler Pair(first: use ReadFile, second: use ReadFile): ReadFile {
  init(path: String) {
    fs = use Files
    Self(first: fs.Open("#{path}.1"), second: fs.Open("#{path}.2"))
  }

  fn read(max) { self.first.read(max) + self.second.read(max) }
}
```

### Meaning

- handler member は，capability に対する呼び出し `c.M(arguments)` でだけ開く．c は `(use E)` か，E の capability の値．書ける位置は今と同じで，with の項目 `x = c.M(arguments)` と init の中である
- `.` の左に effect または label の名前を書いて開くのは誤り（E0448）．with の項目でも（`x = Files.Open(path)`），init の中でも（`Files.Open(path)`）
- 引数のない member でも括弧を書く（`c.Scratch()`）．括弧のない `c.M` は誤り（E0448）．引数が足りないのは，ほかの呼び出しと同じ誤り（E0305．今の実装が init の中の `E.M()` でこれを出すかは確かめていない）
- with は，handler の項目（`H`，`H(arguments)`，`x = H`，label の項目）をすべて init してから，member の項目を書いた順に開く．member の項目 `x = (use E).M(arguments)` の `use E` は，本体の `use E` と同じ供給者を指す．同じ with の handler をすべて見て，上書きも済んだ後の供給者である（`with a = (use Files).Open("a"), MemFiles { ... }` は `MemFiles` の `Open` を開き，`with MemFiles, a = (use Files).Open("a"), DiskFiles { ... }` は `DiskFiles` の `Open` を開く）．`with H1, H2, x = (use E).M(a) { body }` は，`with H1, H2 { with x = (use E).M(a) { body } }` と同じ．違うのは引数 a だけで，a は with の外で init より前に評価されるので，a からは H1 と H2 が供給する effect が見えない
- member の項目の引数は，今と同じく with の外で，どの init よりも前に，項目を書いた順に評価する．同じ with の項目が供給する effect は使えない
- member の項目は同じ with のどの handler よりも後に開くので，どの handler よりも先に release される
- `x = c.M(arguments)`（c は変数）は今と同じ
- init の中の `(use E).M(arguments)` は，今の `E.M(arguments)` と同じ意味になる．自分の effect への `use E` は，今と同じく外側の供給者を指すので，`Files` の handler の init の中の `(use Files).Open(path)` は外側の handler の member を開く
- label: `(use L).M(arguments)` と `c.M(arguments)`（`c: use L`）
- 開いたものの寿命，requirement，E，R は変わらない

### Typing rules

- `[type.rules]` の `with x = E.M(a) { e }` の行は `with x = (use E).M(a) { e }` になる．型，E，R は変わらない
- init の中の行は `c.M(a)` だけになる．R は `{ c } ∪ Ra` で，c が `(use E)` のときは `use E` の R（`{ E }`）

## Specification changes

### `[effect.decl]`

In the item "Whether a call may suspend is determined as follows.", replace "Opening a handler member (`x = E.M(arguments)` in a with, or `E.M(arguments)` inside an init, 8.3, 8.4)" with:

> Opening a handler member (`x = c.M(arguments)` in a with, or `c.M(arguments)` inside an init, 8.3, 8.4)

### `[effect.use]`

Replace the item "On a capability c of an effect E that has handler members, `c.M(arguments)` opens ..." with:

> - On a capability c of an effect E that has handler members, `c.M(arguments)` opens the handler member M of the handler that provides c. c is `(use E)` or a value of type `use E`. As for an operation of a capability, `c.M(...)` is written as a call, and the parentheses are written also for a member without parameters (`fs.Scratch()`). It can be written only as a with item `x = c.M(arguments)` (8.3) and inside an init (8.4). `c.M` without the parentheses is a compile error (E0448). Unlike `log.info`, it does not give a function value, or any other value that opens the member when it is used later: a handler opened as a with item is released at the end of that with, and one opened inside an init after the release of the handler whose init opened it; at another position, an opened handler would have nothing that releases it. A function value of an operation (`log.info`) holds its capability, so it cannot leave the with that provides the capability (8.6); that is enough, because calling it opens nothing.
> - A handler member is not opened through the name of an effect or a label, as an operation is not called through it: `Files.Open(path)` is a compile error (E0448), and is written `(use Files).Open(path)`.

### `[effect.with]`

Replace the item "`E.M(arguments)`, where E is an effect and M one of its handler members, can be written as an item only in the form `x = E.M(arguments)`. ..." with:

> - A handler member M of an effect E can be written as an item only in the form `x = c.M(arguments)`, where c is a capability of E (8.2). The handler member items of a with are opened after all the other items are initialized, in the order they are written. `x = (use E).M(arguments)` calls the init of M of the handler that `use E` in the body refers to. As in the body, this `use E` sees the items of the same with, and a later item takes precedence for the same effect; when an item of the same with provides E, E is therefore not a requirement of the with. `x = c.M(arguments)`, where c is a variable, calls the init of M of the handler that provides c. The arguments are evaluated as for `H(arguments)`: outside the with, before any init runs. The item requires E (or c), not the requirements of the inner handler. `x = E.M(arguments)`, with the name of an effect or a label, is a compile error (E0448).

### `[effect.handler]`

In the item "Inside an init, a handler member can be written as an expression. ...", replace "Inside an init, a handler member can be written as an expression. `E.M(arguments)` runs the init of the handler member M of the innermost handler that provides E, and `c.M(arguments)`, where c is a capability of E, runs that of the handler that provides c." with:

> Inside an init, a handler member can be written as an expression `c.M(arguments)`, where c is a capability of E (`(use Files).Open(path)`, or `fs.Open(path)` after `fs = use Files`, 8.2). It runs the init of the handler member M of the handler that provides c.

In the same item, replace "A handler member of the handler's own effect refers to the outer handler." with:

> `use E` of the handler's own effect refers to the outer handler (8.3), so a handler member opened through it is that of the outer handler.

### `[effect.label]`

Replace the item "A label L of E has the handler members of E. ..." with:

> - A label L of E has the handler members of E. On a capability c of L (`(use L)`, or `c: use L`), `c.M(arguments)` opens M of the handler that provides c. `x = (use L).M(arguments)` in a with requires L.

### `[type.rules]`

Replace the row "| `with x = E.M(a) { e }` | ..." with:

> | `with x = (use E).M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | Re ∪ { E } ∪ Ra |

Replace the row "| `E.M(a)`, `c.M(a)` inside an init | ..." with:

> | `c.M(a)` inside an init | `use F` (F is the effect of M) | Ea ∪ (E of the init of M) | { c } ∪ Ra (for `(use E).M(a)`, the R of `use E`, { E }, ∪ Ra) |

### `[dyn.with]`

In step 1, replace "A handler member item `x = E.M(arguments)` or `x = c.M(arguments)` runs the init of M of the handler that provides E (or c), and is ordered with the other items by its requirements (only E or c)." with:

> After the inits of the other items, the handler member items `x = (use E).M(arguments)` and `x = c.M(arguments)` run the init of M of the handler that provides E (or c), in the order the items are written. They are therefore released before the handlers of the same with (step 3).

### `[module.resolution]`

Replace "On the left of `.`, modules and Traits are looked up, and effects, only to name their handler members (`Files.Open`, 8.3)." with:

> On the left of `.`, modules and Traits are looked up.

Replace "If both a module and an effect are found, `X.M` names the handler member M of the effect if it has one, and otherwise the definition M of the module." with:

> Effects are not looked up on the left of `.` to name a definition: if both a module and an effect are named X, `X.M` names the definition M of the module. When X names no module or Trait but an effect or a label that has a handler member M, or names a module that has no definition M while an effect or a label of the same name has a handler member M, `X.M` is a compile error (E0448) whose help opens M through a capability (8.2).

Replace "An operation cannot be named on an effect (`Logger.info` is a compile error, 8.2); only its handler members can." with:

> Neither an operation nor a handler member can be named on an effect or a label (`Logger.info` and `Files.Open` are compile errors, 8.2). Both are reached through a capability (`(use Files).Open(path)`).

### `[grammar.decl]`

Replace `supply` with:

```ebnf
supply       = type_ref [ "(" [ args ] ")" ]
             | member_open                                        (* a handler member of a capability; 8.3 *)
member_open  = ( lower_name | "(" "use" type_ref [ "[" type { "," type } "]" ] ")" )
               "." type_name [ "(" [ args ] ")" ]                  (* without the parentheses, E0448; 8.2 *)
```

### `[grammar.expr]`

Replace `postfix` with:

```ebnf
postfix    = primary { "(" [ args ] ")" | "." lower_name | "." type_name [ "(" [ args ] ")" ] }
             (* "." type_name: a handler member of a capability, inside an init only; without the parentheses, E0448; 8.2, 8.4 *)
```

### `[diag.list]`

In the row of E0447, replace the help with ``= help: bind it to a name: `x = (use Files).Open(...)` ``.

Replace the row of E0448 with:

> | E0448 | A handler member is opened outside an item of with and an init (`f = (use Files).Open(p)` in a fn, 8.2, 8.4), through the name of an effect or a label (`x = Files.Open(p)`, or `Files.Open(p)` inside an init, 8.2), or without parentheses (`x = fs.Scratch`, 8.2). The help is ``= help: open it as an item of `with`: `with x = (use Files).Open(...) { ... }` `` outside an item of with and an init, ``= help: open it through a capability: `(use Files).Open(...)` `` for the name of an effect or a label, and ``= help: write `fs.Scratch()` `` without parentheses. When more than one of these holds, only the first in this order is reported, and the help shows the form that fixes all of them, with the names of the parameters and the label as written (``x = Audit.Open`` gives ``= help: open it through a capability: `(use Audit).Open(path: ...)` ``) | ``handler member `Open` can only be opened in `with` or called in an `init` `` / ``handler member `Open` is opened through a capability`` / ``handler member `Scratch` is opened by calling it`` |

If the row of E0434 says that a handler member with parameters written without arguments as an item of a with is E0434 (#706), remove that case: such an item is E0448, either for the name of an effect or for the missing parentheses.

## Diagnostics

- [ ] `x = E.M(arguments)` or `x = L.M(arguments)` as an item of a with: E0448 (extended), with the help `(use E).M(...)` (`(use L).M(...)` for a label)
- [ ] `E.M(arguments)` or `L.M(arguments)` inside an init: E0448 (extended; it opens the member now), with the help `(use E).M(...)`
- [ ] One E0448 for a form that breaks more than one rule, in the order: outside a with item and an init, through the name of an effect or a label, without parentheses. The help fixes all of them (`x = Files.Open` gives `(use Files).Open(path: ...)`)
- [ ] `c.M` without parentheses, as an item of a with (now E0110) or inside an init (now E0448 with another message, and E0301): E0448 with the help `c.M()`
- [ ] `c.M()` with missing arguments: E0305, as for any call (to be checked: what the implementation reports now for `E.M()` inside an init)
- [ ] `E.M(arguments)` outside a with item and an init: E0448 (unchanged)
- [ ] E0434 no longer applies to handler members (the case added by #706)

## Compatibility

- 互換性を壊す．effect または label の名前から開く形（with の項目の `x = E.M(arguments)`，init の中の `E.M(arguments)`）と，with の項目で括弧を省いた `x = E.M` は，`(use E).M(arguments)` と `(use E).M()` に書き換えが要る．意味は同じなので，書き換えは機械的にできる
- with の init の順が変わる．今は member の項目が依存の順に handler の項目の間に入る（`with a = Files.Open("a"), MemFiles, quiet = Quiet, b = Files.Open("b"), Loud` は `MemFiles`，`a`，`Quiet`，`b`，`Loud`）．この提案では handler の項目の後になる（`MemFiles`，`Quiet`，`Loud`，`a`，`b`）．init や release に出力があるプログラムでは順が変わる．開く供給者は変わらないので，項目を並べ替えずに `(use E).M(...)` に書き換えてよい
- main で effect の名前から開いているのはテストだけ:
  - `crates/emela-codegen-c/tests/programs/` の `handler_members.emel`，`handler_member_failure.emel`，`handler_member_suspend.emel`，`multi_effect_handler_member.emel`
  - `crates/emela-core/tests/lower.rs`，`crates/emela-driver/tests/wasm_parity.rs`，`crates/emela-lsp/tests/server.rs`
  - `crates/emela-types/tests/` の `handler_member.rs`，`effects.rs`，`capture.rs`，`multi_effect_handler.rs`
  - `crates/emela-resolve/tests/resolve.rs`
  - `crates/emela-syntax/test_data/parser/ok/handler_member.emel`，`crates/emela-fmt/test_data/handler_member.emel`
  - これらのスナップショット
- spec では `[type.rules]`，`[dyn.with]`，`[diag.list]` の行と `[effect.*]` の本文だけ（Specification changes）．std と examples にはない．`CHANGELOG.md`，`spec/history.md` と採択済みの proposal（`handler-member.md`，`multi-effect-handler.md` など）は記録なので書き換えない
- #706（#700 の修正）が先に merge されると，E0434 に handler member の場合（``handler member `Open` needs arguments``，help ``write `Files.Open(path: ...)` ``）が入る．この提案はその場合を E0448 に移す
- 審議中の `region-member`（#660）は，例の `stage = Engine.Wave`（with の項目）と `Self(stage: Engine.Wave)`（init）が `(use Engine).Wave()` になる．また「括弧は with の項目でだけ省ける」という扱いをやめ，開くときは常に括弧を書く形に直す

## Implementation

- emela-syntax: `supply` から effect の名前から開く形（`type_ref "." type_name`）を外し，`"(" "use" ... ")" "." type_name "(" [ args ] ")"` を足す．括弧のない `c.M` と `E.M` は，型検査で E0448 を出せるように受け付ける
- emela-resolve: `.` の左で effect を引かない．ただし E0448 の help を出すため，`.` の左が effect か label で，その右が handler member のときを見分ける
- emela-types: with の項目と init の中の，effect の名前から開く経路（`MemberProvider::Effect`）を E0448 にする．with の項目の `(use E)` は，今の `MemberProvider::Effect` と同じく同じ with の項目が供給する E を指す．上書きされた handler は指さない．括弧のない `c.M` を E0448 にする．E0434 の handler member の場合を外す
- emela-core: lowering は今の `x = E.M` の経路を `x = (use E).M()` に使う．with の init の順を，handler の項目（依存の順）の後に member の項目（書いた順）にする
- emela-driver: E0447 と E0448 の文と help．LSP の補完で，`Files.` の後に member を出さない．code action で `(use E).M(...)` に書き換えられるようにするか（Open questions）
- emela-fmt，tree-sitter（`editors/tree-sitter-emela/grammar.js`）: with の項目の `(use E).M(...)`
- テストを書き換える（Compatibility の一覧）

## Alternatives

- **init の中だけをやめ，with の項目の `x = E.M(arguments)` は残す**（この提案の最初の版）: with の項目は短いまま．ただ，member の項目は供給者を名前で並べていない（Motivation）ので，with の項目に effect の名前を書く理由が立たない．`.` の左で effect を引く扱いと #670 の規則も残る
- **`E.M(arguments)` を，`uses { E }` を持つ組み込みの関数として位置づけて残す**: 短く書けて，互換性も壊さない．`uses { E }` を持つ関数を呼ぶとき，呼ぶ側は `use` を書かないので，それと同じだと説明できる．ただ，そういう関数は宣言の側に `uses` と `use` が書いてあり，呼ぶ側はふつうの関数の呼び出しである．`E.M` にはその宣言にあたるものがなく，effect の名前そのものが供給者に届く
- **略記 `use E.M(arguments)` を足す**（`(use E).M` の括弧を省く）: `use (use E).M` のような重なりがなく短い．ただ，`use` は「引く」印で，`use E.M` は開く操作にも `use` を使うことになる．`use A.B` は今「モジュール A の effect B」と読む形でもあり，member の読みを足すと #670 と同じ規則が `use` の後にも要る
- **開く操作に `use` を付ける（`use c.M(arguments)`）**: capability が新しくできる所に必ず `use` が付く．ただ，`use` が引くと開くの 2 つの意味を持つ．region-member（#660）の `stage.Spawn(k)` のように fn の中で capability を作る形にも `use` が要るかという問いも生む．開く印は capability に付けた大文字の `.M(...)` で足りる
- **with の項目では，handler と同じく引数で括弧を決める（`x = c.Scratch`）**（#703）: with の項目の handler と見た目が揃い，どちらも開く操作である．ただ，`H` は宣言を名前で指す形で，`c.Scratch` は capability の値を通す形である．値を通す `.name` は括弧がなければ関数の値（`log.info`）なので，括弧のない `c.Scratch` を開く形にすると，字の大小で意味が分かれる．with の項目と init の中で同じ `c.M` の形が括弧だけ違うことにもなる
- **member の項目の `(use E)` は，先に書いた項目だけを見る**: 項目を左から読む順に `use E` の規則を当てはめられる．ただ，`with MemFiles, a = (use Files).Open("a"), DiskFiles` で，本体から見ると上書きされた `MemFiles` を `a` のために init する例外が要り，今の `with a = Files.Open("a"), MemFiles` の意味も変わる．member の項目には依存できる項目がないので，循環を防ぐ理由にもならない
- **member の項目の `(use E)` は，同じ with の項目を見ない（引数の中の `use E` と同じ）**: 規則はいちばん単純．ただ，供給者と member を同じ with に並べられず，入れ子の with が要る
- **`E.op()` も許し，operation でも `use` を省けるようにする**: `E.M()` と揃う．ただ，`[effect.use]` の原則を捨てることになり，#670 が解いたモジュールとの名前の衝突も戻る

## Prior art

- Capability-passing style: effects are reached only through capabilities that are values in scope, so where a provider is reached is visible in the code.

## Open questions

- with の項目が `x = (use Files).Open(path)` と長くなる．with の前で `fs = use Files` と束縛するのを勧めるだけでよいか
- 移行: 1 つのリリースだけ警告（W04xx）にしてから誤りにするか，すぐ誤りにするか．v0.28.0 で出たばかりで，使っているのはテストだけなので，すぐ誤りにしてよいと考える．LSP の code action で `(use E).M(...)` に書き換えられるようにするか
- 宣言の括弧（`handler Scratch: E` を `handler Scratch(): E` と同じにするか）はこの提案では扱わない（#703 にあったもの）．開く側の括弧とは独立で，init の `init` と `init()` の揃え方と一緒に決める

## Decision
