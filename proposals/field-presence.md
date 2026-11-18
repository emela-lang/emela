---
title: Absent keys in derived Encode and Decode
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [trait.derive, api.std.dynamic, api.std.dynamic.presence (new)]
requires: []
related: [derive-encode-decode-shaped]
---

# Absent keys in derived Encode and Decode

## Summary

derive した Encode と Decode で，フィールドのキーがないことと null を，フィールドの型が決めるようにする．

- `Option[A]` の None は，キーを書かない（今は null を書く）．読むときは，キーがなくても null でも None（今と同じ）．
- 3つの状態（キーがない，null，値）を区別する型 `Presence[A]` を `Std.Dynamic.Presence` に足す．部分更新の body に使う．
- trait `Encode` に `to_field`，`Decode` に `from_absent` を足す．どちらも既定の実装を持つので，今の impl はそのまま動く．

属性も Pome の設定も足さない．

```emel
import Std.Dynamic.{Decode, Encode}
import Std.Dynamic.Presence.{Absent, Null, Present, Presence}

type TodoPatch(title: Option[String], due: Presence[String]) derive Encode, Decode
```

## Motivation

`derive-encode-decode-shaped` は，None を null で書き，キーがないことも null も None と読む，と決めた．`Option[A]` の状態は2つ，JSON のフィールドの状態は3つ（キーがない，null，値）で，この決め方では次のことができない．

1. キーがないことと null を区別できない．部分更新でよく使う意味づけ（キーがなければ変えない，null なら消す．JSON Merge Patch，RFC 7396）を受け取れず，送れもしない．
2. 「省けるが null は不可」のフィールドを持つ相手に送れない．None を null で送ると，相手の検証で弾かれる．
3. Shape が入力と出力の両方を表せない．Encode が出すのは「キーは必ずあり，null もありうる」，Decode が受け付けるのは「キーはなくてもよい」で，同じ `Shape.Option` が2つの形を表すことになる．

具体例を挙げる．

```emel
type TodoPatch(title: Option[String], due: Option[String]) derive Decode
```

`{}`（何も変えない）と `{"due": null}`（期限を消す）は，どちらも `TodoPatch(title: None, due: None)` になる．

Emela には null がない（`[intro.premises]`）．None は「値がない」で，オブジェクトの中で「値がない」に対応するのはキーがないこと．null は外の形式の都合で，それに意味を付けるのはアプリケーション．意味を付けるなら，それはデータの定義として書く．

## Proposal

### Syntax

構文は変えない．

### Meaning

フィールドのキーをどう書き，どう読むかは，フィールドの型の Encode と Decode が決める．

| フィールドの型 | キーがない | null | 値 | 書くとき |
|---|---|---|---|---|
| `A`（下の2つ以外） | `Decode.from_dynamic(Null)`（String ならエラー） | 同じ | `from_dynamic` | 値を書く |
| `Option[A]` | None | None | Some | None はキーを書かない |
| `Presence[A]` | Absent | Null | Present | Absent はキーを書かない，Null は null を書く |

新規作成の body:

```emel
type TodoBody(title: String, due: Option[String]) derive Encode, Decode
```

| JSON | 値 |
|---|---|
| `{"title": "買い物", "due": "2026-11-20"}` | `TodoBody(title: "買い物", due: Some("2026-11-20"))` |
| `{"title": "買い物", "due": null}` | `TodoBody(title: "買い物", due: None)` |
| `{"title": "買い物"}` | `TodoBody(title: "買い物", due: None)` |
| `{"due": "2026-11-20"}` | `DecodeError(expected: "text", actual: Null, path: [Key("title")])` |

`TodoBody(title: "買い物", due: None)` は `{"title": "買い物"}` と書く．

部分更新の body:

```emel
type TodoPatch(title: Option[String], due: Presence[String]) derive Encode, Decode

fn apply(todo: Todo, p: TodoPatch) -> Todo {
  title = p.title match {
    None    -> todo.title
    Some(t) -> t
  }
  due = p.due match {
    Absent     -> todo.due
    Null       -> None
    Present(d) -> Some(d)
  }
  Todo(title:, due:, ..todo)
}
```

| JSON | 値 |
|---|---|
| `{}` | `TodoPatch(title: None, due: Absent)` |
| `{"title": "牛乳"}` | `TodoPatch(title: Some("牛乳"), due: Absent)` |
| `{"due": null}` | `TodoPatch(title: None, due: Null)` |
| `{"due": "2026-12-01"}` | `TodoPatch(title: None, due: Present("2026-12-01"))` |

`TodoPatch(title: None, due: Null)` は `{"due": null}`，`TodoPatch(title: Some("牛乳"), due: Absent)` は `{"title": "牛乳"}` と書く．

#### trait

```emel
pub trait Encode {
  fn to_dynamic(self) -> Dynamic
  fn to_field(self) -> Option[Dynamic] {
    Some(self.to_dynamic())
  }
}

pub trait Decode {
  fn from_dynamic(d: Dynamic) -> Self fails DecodeError
  fn from_dynamic_all(d: Dynamic) -> Self fails DecodeErrors { ... } # 今と同じ
  fn from_absent() -> Self fails DecodeError {
    Decode.from_dynamic(Null)
  }
}
```

- `to_field` は，値をオブジェクトのフィールドとして書くときの形．None ならキーを書かない．
- `from_absent` は，キーがないときの値．既定はキーを null として読むので，今の「キーがなければ null として読む」と同じ結果になる．
- Option が上書きするのは `to_field` だけ（None は None）．`from_absent` は既定のままで None になる．
- `Presence` は両方を上書きする．
- `to_field` と `from_absent` を使うのは，型のフィールドと，enum の名前付きの中身のフィールドだけ．List の要素，Map の値，位置の中身は，今と同じく `to_dynamic` と `from_dynamic` を使う．そこでは Absent も None も null になる．

#### Presence

```emel
pub enum Presence[A] { Absent, Null, Present(A) }
```

- 置き場所は `Std.Dynamic.Presence`．`Std.Dynamic` には `Dynamic.Null` があり，同じモジュールに同じ名前の variant は置けない（E0213）．
- Encode: `to_dynamic` は Absent と Null を null に，`Present(a)` を `a` の `to_dynamic` にする．`to_field` は Absent を None，Null を `Some(Null)`，`Present(a)` を `Some(a.to_dynamic())` にする．
- Decode: `from_dynamic` は null を Null に，それ以外を `Present(A.from_dynamic(d))` にする．`from_absent` は Absent．`from_dynamic_all` は値を `from_dynamic_all` で読む．
- Shaped: `Shape.Option` に A の形を入れたもの．
- Eq，Ord，Show を derive する．順序は Absent，Null，Present．

#### Shape

Shape は変えない．`Shape.Option(s)` は「キーがない，null，s のどれでもよい」を表す．Option も Presence もこの3つを受け付け，Option は null を書かないが，書く形は受け付ける形に含まれる．JSON Schema に写すときは，`Shape.Option` のフィールドを `required` から外し，null も許す．

### Typing rules

変えない．

## Specification changes

### `[trait.derive]`

Replace the items on derived Encode and Decode of a type with:

> - A derived Encode of a type writes an object of its fields, by key, each through `Encode.to_field`. A field for which `to_field` returns None is left out. A derived Encode of an enum writes `Std.Dynamic.variant(name, contents)`: an object for named contents, written as the fields of a type are, Items with one element for each positional field, through `Encode.to_dynamic`, and no value for a variant without contents.
> - A derived Decode of a type reads each field, in declaration order: if the object has the key, with `Decode.from_dynamic` of its value, and otherwise with `Decode.from_absent`. A failure has the key in front of its path. A derived Decode of an enum reads the variant from `Std.Dynamic.tag(value)` and its contents from `"value"` in the form of the variant: named contents as the fields of a type, and a positional field `i` with `Std.Dynamic.index(i, Decode.from_dynamic)`, after `Std.Dynamic.items(value, n)` has checked that the contents hold exactly as many items as the variant has fields. (The rest of the item, from "Keys that name no field", stays.)

### `[api.std.dynamic]`

Replace the rows of `Encode` and `Decode` with:

> | `Encode` | `trait Encode { fn to_dynamic(self) -> Dynamic; fn to_field(self) -> Option[Dynamic] }` | Derivable. `to_field` is the value written for a field of an object, and None leaves the key out; its default is `Some(to_dynamic(self))` |
> | `Decode` | `trait Decode { fn from_dynamic(d: Dynamic) -> Self fails DecodeError; fn from_dynamic_all(d: Dynamic) -> Self fails DecodeErrors; fn from_absent() -> Self fails DecodeError }` | Derivable, except for opaque types. `from_dynamic` stops at the first failure. `from_dynamic_all` reports every failure; its default fails with the one failure of `from_dynamic`. `from_absent` is the value of a field whose key is missing; its default is `from_dynamic(Null)` |

Add the row:

> | `lookup` | `lookup(value: Dynamic, name: String) -> Option[Dynamic] fails DecodeError` | The value of the key `name` of Fields, or None if the key is missing. A value that is not Fields is a DecodeError |

Replace "None is encoded as Null, and Null decodes as None." with:

> None is encoded as Null, and Null decodes as None. As a field of an object, None leaves the key out (`to_field`), and a missing key decodes as None (`from_absent`, by its default).

### `[api.std.dynamic.presence]` (new)

> ### Std.Dynamic.Presence (`std/dynamic/presence.emel`) `[api.std.dynamic.presence]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Presence` | `enum Presence[A] { Absent, Null, Present(A) }` | A field that tells a missing key, null and a value apart |
>
> impl: Eq, Ord and Show are derived. Encode: `to_dynamic` writes Absent and Null as Null and `Present(a)` as `a`; `to_field` is None for Absent, `Some(Null)` for Null, and `Some(a)` written for `Present(a)`. Decode: `from_dynamic` reads Null as Null and any other value as `Present` of the Decode of `A`; `from_absent` is Absent; `from_dynamic_all` reads the value with `from_dynamic_all`. Shaped: `Shape.Option` of the shape of `A`.

## Diagnostics

なし．

## Compatibility

- 出力が変わる: derive した Encode は，None のフィールドのキーを書かなくなる．derive は 0.28 で入る予定（`derive-encode-decode-shaped`）で，この提案を同じ版に入れるなら，リリースされたふるまいは変わらない．
- trait に足すメソッドはどちらも既定の実装を持つので，手で書いた impl はそのまま通る．
- 手で書いた Encode の impl が None を null で書いていても，それを使う型のフィールドでは null が書かれ続ける（Option の impl を通らないため）．

## Implementation

- std（`std/dynamic.emel`）: `to_field`，`from_absent`，`lookup`，Option の `to_field`
- std（`std/dynamic/presence.emel`）: `Presence` と impl
- emela-core（`lower/derive_std.rs`）: フィールドを `lookup` と `from_absent` で読み，`to_field` で書く
- x/json-schema: `Shape.Option` のフィールドを `required` から外す

## Alternatives

- 属性で決める（`@optional` をフィールドに付け，付けたフィールドだけキーを省ける）: 同じ Option が属性によって3通りに読まれ，型を見ても形が分からない．3状態には，さらに `@optional(exact)` と `Option[Option[A]]` の組み合わせが要る．
- `Option[Option[A]]` で3状態を表す: `Some(None)` が何を指すかを覚えなければならない．`Std.Dynamic.field` がキーがないことを null と読むので，今のままでは区別もできない．
- 宣言したフィールドのキーはすべて必須にし，Option は null だけを None にする: キー名の間違い（`"dueAt"` を `"due_at"` と送る）をエラーにできるが，キーを省いて送る相手から読めなくなり，1も解けない．
- None を null で書き続ける: 2が残る．Emela に null はないので，None を null に結びつける理由がない．
- `Dynamic` に「ない」を表す variant を足す: Dynamic が null を2種類持つことになり，形式を読む関数（`Json.parse`）がそれを作ることはない．
- `Presence` を `Std.Dynamic` に置き，variant の名前を変える（`Cleared` など）: 名前が JSON の形を表さなくなり，アプリケーションの意味（消す）を std が決めることになる．

## Prior art

- Separating whether a key may be missing from whether a value may be null, as two properties of a field: Effect Schema (`optional` and `NullOr` on property signatures), TypeScript (`a?: T` and `T | null`).
- A three-state value for missing, null and present: `JsonNullable` (jackson-databind-nullable), `Option<Option<T>>` with `double_option` in serde_with.
- Partial updates where a missing key leaves a member as it is and null removes it: JSON Merge Patch (RFC 7396).

## Open questions

- 型とモジュールの名前（`Presence`，`Std.Dynamic.Presence`）
- Option のフィールドで，キー名の間違いが None になることを受け入れるか．Option でないフィールドならエラーになり，Option のフィールドは Shaped から作る JSON Schema で相手が検証できる
- #589 のマージ前に入れるか，マージ後に別の PR にするか（0.28 のリリースまでに入れば，ふるまいの変更にはならない）

## Decision
