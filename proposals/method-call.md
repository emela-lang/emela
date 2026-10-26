---
title: Method calls
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [lex.name, data.type, type.rules, trait.call, module.resolution, api.list, api.string, api.std.http.request, api.std.http.response, lsp.completion, lsp.navigation, diag.list]
requires: []
related: [http-service]
---

# Method calls

## Summary

`x.f(a)` を `M.f(x, a)` として呼べるようにする．M は x の型を定義したモジュールで，f はそのモジュールの関数のうち第 1 引数の型の頭が x の型のもの．x の型が impl している trait の関数も同じ形で呼べる．関数に印は付けない．

あわせて，`String.join` を `List.join` に移し，`Request.headers` と `Response.headers`（関数）を `header_values` に改名する．

## Motivation

- 同じ型の操作を重ねると，内側から外側へ読むことになる．std，examples，x（今の構文で約 5600 行）には `Mod.f(Mod.g(...))` の形が 30 箇所ある

  ```emel
  # examples/http
  Option.or(List.head(String.split(res.body, "\n")), "")
  ```

- `|>` で並べ替えられるが，一段ごとにモジュール名を書くことになる．`|>` の連鎖（2 段以上）15 箇所のうち 8 箇所は同じモジュールの中で閉じている
- 第 1 引数がそのモジュールの型である関数の呼び出しは 312 箇所ある（List 129，String 50，Option 33，x/dynamic の Number 32，Int 23，Map 18）．どれもメソッドとして書ける
- そうでない呼び出しは 166 箇所で，ほとんどは値を作る関数（`Map.from_list` 24，`Number.from_int` 18，`Map.empty` 14，`Response.new` 10，`List.range` 10）．第 1 引数の型で決める規則なら，これらは自然にメソッドから外れる
- 今の言語での回避策は `|>` だけで，モジュール名は省けない

## Proposal

### Syntax

文法は変えない．`postfix = primary { "(" [ args ] ")" | "." lower_name }` がすでに `x.f(a)` を読める．今は，x が能力の値なら操作の呼び出し，そうでなければ関数型のフィールドの呼び出しとして読まれる．

```emel
fn first_line(res: Response) -> String {
  res.body.split("\n").head().or("")
}

fn is_field_value(value: String) -> Bool {
  value.code_points().all(fn(c) { c == 0x09 || (c >= 0x20 && c <= 0x7E) })
}

fn labels(users: List[User]) -> String {
  users.map(fn(u) { label(u) }).join(", ")
}

fn sum_lengths(words: List[String]) -> Int {
  words.fold(from: 0, f: fn(total, w) { total + w.length() })
}
```

今のコンパイラは，これらを E0313（``type `String` has no fields``）で止める．

### Meaning

- x の型の頭が名前付きの型 T のとき，`x.f(a1, ..., an)` は次の順で f を探す
  1. T のフィールド f（今の規則のまま．関数型なら呼ぶ）
  2. T を定義したモジュールの fn f のうち，第 1 引数の型の頭が T のもの．モジュールの外からは pub の fn だけ
  3. T が impl している trait の関数 f のうち，`self` をとるもの
- 見つかった fn は `M.f(x, a1, ..., an)` と同じに呼ぶ．名前付き引数，評価の順序（x が先），E と R はその呼び出しと同じ
- 探すときにスコープを見ない．`r.extend(2)` は，Reservation のモジュールを import していなくても呼べる．フィールドアクセスが import なしで使えるのに揃える
- x の型が型パラメータ A のときは，A の境界の trait から探す（3 だけ）
- 型の頭が判れば足りる．型引数まで決まっている必要はない．`List.join` は `List.join(xs: List[String], ...)` として見つかり，`[1, 2].join(", ")` は普通の引数の型の不一致になる
- 能力の値（`use X` の値）は名前付きの型ではないので，`log.info(...)` は今のまま操作の呼び出し
- `|>` は変えない．`xs |> List.map(f)` は今のまま書ける

### Declarations

- T のフィールドと同じ名前で，第 1 引数の型の頭が T である fn を，T と同じモジュールに宣言すると compile error（E0334）．呼ぶ側で 1 と 2 が両方見つかることはなくなる
- 1 と 3，2 と 3，3 の中の 2 つ以上が同時に見つかったときは，呼んだ位置で compile error（E0335）．`T.f(x)` や `Trait.f(x)` と書けば避けられる

### Library

- `List.join(xs: List[String], sep: String) -> String` を足す．`String.join` は `@deprecated` にして 1 minor version 残す
- `Request.headers(req, name)` と `Response.headers(res, name)` を `header_values` に改名する．フィールド `headers` と E0334 でぶつかるので，旧名を残せない
- `Std.Http.headers`（`@deprecated`）は `Std.Http` のモジュールにあるので E0334 に当たらない．指す先の名前だけ `Response.header_values` に直す
- std の内部では `Cause` の非公開フィールド `raise` が関数 `Cause.raise` とぶつかる．フィールドの名前を変える（API には出ていない）

### Typing rules

- `x.f(a)` の型は，探し当てた fn を `M.f(x, a)` と呼んだときの型
- x の型の頭が，検査がそこに達した時点で決まっていなければ E0314．フィールドアクセスと同じ条件

## Specification changes

### `[lex.name]`

Replace the second bullet after "From these rules, the meaning of `.` is clear from its left-hand side." with:

- Lowercase name or uppercase name on the left: access to a value (a field, an operation of a capability obtained with `use`, or a method call, `[type.rules]`). Examples: `user.name`, `log.info`, `DEFAULT_CONFIG.port`, `xs.map(f)`

### `[data.type]`

After "Construction is written `User(id: 1, name: "a")`, and field access is written `user.name`.", add:

- A function of the module that defines a type, whose first parameter has that type, can also be called as `user.f(...)` (`[type.rules]`). A function MUST NOT have the name of a field of that type (E0334).

### `[type.rules]`

Add a row after `f(a)`:

| Expression | T | E | R |
|---|---|---|---|
| `x.f(a)` (method call) | The return type of the function found, called as `M.f(x, a)` | Ex ∪ Ef ∪ Ea | Rx ∪ Rf ∪ Ra |

After the bullet on field access, add:

- A method call `x.f(a1, ..., an)` applies when the type of x has a named type T at its head and x is not a capability value. f is looked up, without looking at the scope, among: (1) the fields of T, (2) the fns of the module that defines T whose first parameter has T at the head of its type (only pub fns outside that module), and (3) the functions that take `self` of the traits that T implements. When x has a type parameter A, only the functions of the bounds of A are looked up. A field is called as a value of function type. A function is called as `M.f(x, a1, ..., an)`, with the same named arguments, order of evaluation, E and R. Only the head of the type of x is needed: the type arguments are checked as for the first argument of that call.
- When more than one candidate is found, it is a compile error (E0335). A field and a function of the module cannot both be found, because declaring them is E0334. When none is found, it is E0312 (E0313 for a type without fields).
- The head of the type of x MUST already be determined when checking reaches the method call (E0314), as for a field access.

### `[trait.call]`

Replace "A trait function is called in the form `Trait.function(value)`. There is no method call syntax such as `user.show()`." with:

- A trait function is called in the form `Trait.function(value)`. A function that takes `self` can also be called as `value.function()` (`[type.rules]`).

### `[module.resolution]`

After "On the left of `.`, only modules and Traits are looked up.", add:

- A method call (`x.f(...)`, `[type.rules]`) does not look at the scope. The functions are looked up from the type of x, so the module that defines the type does not need to be imported.

### `[api.list]`

Add:

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `join` | `join(xs: List[String], sep: String) -> String` | intrinsic | Joins with `sep` in between. Formerly `String.join` |

### `[api.string]`

Replace the row of `join` with:

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `join` | `join(parts: List[String], sep: String) -> String` | Emela | `@deprecated`, remains for one minor version. `List.join(parts, sep)` |

### `[api.std.http.request]`

Replace the row of `headers` with:

| Declaration | Contents | Description |
|---|---|---|
| `header_values` | `header_values(req: Request, name: String) -> List[String]` | All values whose name matches. Formerly `headers`, renamed because the field `headers` has the same name (E0334) |

### `[api.std.http.response]`

Replace the row of `headers` with:

| Declaration | Contents | Description |
|---|---|---|
| `header_values` | `header_values(res: Response, name: String) -> List[String]` | All values whose name matches. Formerly `headers`, renamed because the field `headers` has the same name (E0334) |

In `[api.std.http]`, replace `Response.headers` in the list of new places of the deprecated functions with `Response.header_values`.

### `[lsp.completion]`

Replace "After `.`, otherwise: ..." with:

- After `.`, otherwise: if the type of the left expression is a capability value, the operations of the effect. If its head is a named type, its fields (the description is the text of the written type), then the functions that can be called as methods (`[type.rules]`; kind Method, the first parameter left out of the signature). For other types, empty.

### `[lsp.navigation]`

Add:

- A method call is a reference of the function it calls. Go to definition, references and rename treat `x.f(...)` as `M.f(x, ...)`.

## Diagnostics

- [ ] E0312: widen to methods. ``no field or function `nmae` for type `User` ``
- [ ] E0313: widen to methods. ``type `Int` has no fields or function `nmae` `` （文面は要検討）
- [ ] E0314: widen to methods. ``the type of this value must be known to call `f` ``
- [ ] E0334 (new): a fn with the name of a field of the type of its first parameter, in the module of that type. ``function `headers` has the same name as a field of `Request` ``. Points at the name of the fn
- [ ] E0335 (new): more than one candidate for a method call. ``ambiguous method `show` for `User`: `User.show` and `Show.show` `` with ``= help: write `User.show(x)` or `Show.show(x)` ``

## Compatibility

- 今の書き方（`M.f(x)`，`x |> M.f`）はすべてそのまま通る
- 今 E0312 や E0313 になっていたプログラムの一部が通るようになる
- E0334 で通らなくなるプログラムがある．型と同じモジュールに，フィールドと同じ名前で第 1 引数がその型の fn を持つもの．std，examples，x では `Request.headers`，`Response.headers`，`Cause` の非公開フィールド `raise` の 3 つ
- `Request.headers` と `Response.headers` の改名は，`@deprecated` の期間を置けない破壊的な変更になる

## Implementation

- emela-types: field access の検査に method lookup を足す．引く先は T の定義モジュールの関数表と，T の impl の表
- emela-resolve: E0334 を宣言の検査で出す
- emela-core: `List.join` を足し，`String.join` を `@deprecated` にする．`header_values` に改名する．`Cause` のフィールドの名前を変える
- emela-lsp: `.` の後の補完，定義への移動，参照，rename
- lowering は `M.f(x, a)` と同じ形に落とすので，codegen は変えない

## Alternatives

- `self` を書いた関数だけをメソッドにする：`self` という引数の名前が特別な意味を持つ．第 1 引数の型で決めれば，値を作る関数は自然に外れるので，印は要らない
- 呼ぶ側の import を要求する：フィールドアクセスと揃わない．どの関数が呼べるかが型だけで決まらなくなる
- フィールドとメソッドを呼び方で分ける（`x.f` はフィールド，`x.f(...)` は関数）：関数型のフィールドは今でも `x.f(...)` と呼べるので，この区別は成り立たない
- `|>` だけで済ませる：モジュール名を毎段書くことになり，入れ子の呼び出しの読みにくさは残る

## Prior art

- Uniform function call syntax（D，Nim）：第 1 引数を `.` の左に置く呼び出し．ただしスコープにある関数を引く
- 型の定義モジュールから関数を引く形（Rust の inherent impl，Go のメソッド集合）
- Kotlin の拡張関数は呼ぶ側の import を要る

## Open questions

- 呼ばずに書いた `x.f`（f がフィールドでなく関数）をどうするか．案：E0312 にして，help で ``call it as `x.f(...)`, or write `M.f` `` と出す．部分適用した関数値にはしない
- 3（trait の関数）を探すとき，trait がスコープにあることを要求するか．モジュールの関数（2）は要求しないので，揃えるなら要求しない
- `String.concat` も `List` に移すか．`List.concat` は「リストをつなぐ」と読まれやすい．`join("")` で足りるので，残すか `@deprecated` にするか
- 改名先の名前（`header_values` でいいか）
- `|>` とメソッドの使い分けを linter で寄せるか

## Decision
