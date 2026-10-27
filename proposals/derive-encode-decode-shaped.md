---
title: Dynamic, Shape and JSON in std, with derived Encode, Decode and Shaped
authors: [Alliana]
status: draft
pr: 455
issue:
decided:
touches: [kw.annotation, grammar.decl, lib.layers, trait.derive, api.std.dynamic (new), api.std.dynamic.number (new), api.std.shape (new), api.std.json (new), diag.list, open.list]
requires: []
related: []
---

# Dynamic, Shape and JSON in std, with derived Encode, Decode and Shaped

## Summary

x/dynamic（形式によらない値 `Dynamic` と trait `Encode`/`Decode`）と x/shape（型の形 `Shape` と trait `Shaped`）を std の value の層に移し，`Std.Dynamic` と `Std.Shape` にする．JSON の parse と書き出し（x/json）も `Std.Json` として移す．compiler が `derive Encode, Decode, Shaped` を導出できるようにする．

```emel
import Std.Dynamic.{Encode, Decode}
import Std.Shape.{Shaped}

type TodoBody(title: String, due: Option[String]) derive Decode, Shaped
type TodoView(id: String, title: String, done: Bool) derive Encode, Shaped
```

JSON Schema の生成（x/shape の `Shape.JsonSchema`）は x に残し，新しいパッケージ x/json-schema に移す．

## Motivation

- 外とやり取りする型ごとに，`Encode`，`Decode`，`Shaped` の impl を手で書いている．Todo の API の試作では，型が 6 つで web のファイルの半分を手書きの impl が占めた（1 つの型に 6〜10 行）
- 手書きの impl は型の宣言と別にフィールド名を書くので，フィールドを足したときに直し忘れる．`Shaped` の手書きは，JSON Schema と実際の JSON の形がずれる原因になる
- compiler は Prelude の `Eq`，`Ord`，`Show` しか導出できない（`[trait.derive]`）．導出の規則は compiler が持ち，trait の場所で決め打ちしている（`emela-core` の `lower.rs` の `derived`）．x の trait は compiler から見えない
- `[lib.layers]` は「JSON conversion (Encode, Decode) is provided by the standard library as traits with deriving rules」とすでに書いている．x/dynamic と x/shape はそのために x で形を固めてきた
  - x/dynamic v0.3.0：`Dynamic`，`Number`，`DecodeError`，decoder の組み合わせ，組み込みの型の impl．x/json がその上で JSON を読み書きしている
  - x/shape v0.2.0：`Shape`，`Shaped`（互いに参照する型も止まる `shape_in` と `visit`），`Rule`，JSON Schema の生成．29 本のテストと Ajv での検証がある
- trait の作者が導出の規則を書ける仕組み（`[open.list]` #3）が入れば x のままでも derive できるが，構文も決まっていない．この 3 つの trait は形が決まっているので，それを待たずに compiler が持つ

## Proposal

### 置き場所

| std のモジュール | 中身 | 元 |
|---|---|---|
| `Std.Dynamic` | `Dynamic`，`Segment`，`DecodeError`，trait `Encode`，`Decode`，`ObjectKey`，decoder（`string`，`int`，`field`，`list`，`optional`，`dict`，`index`，`items`，`tag`，`field_all`，`index_all`，`attempt`，`errors_of`），`DecodeErrors`，`Attempt`，`object`，`variant`，`kind` | x/dynamic |
| `Std.Dynamic.Number` | `Number`，`OutOfRange`，`parse`，`from_int`，`to_int` など | x/dynamic の `Number` |
| `Std.Json` | `SyntaxError`，`parse`，`to_string`，`decode`，`decode_all`，`encode` | x/json（`decode_all` は新しい） |
| `Std.Shape` | `Shape`，`Field`，`Variant`，`Contents`，trait `Shaped`，`visit`，`name_of`，`generic_name`，`annotate`，`strip`，`note`，`field_names`，`of_value`，`of_parameter`，`description`，`pattern`，`format` | x/shape |

どれも value の層（純粋でホストによらない）に入る．`[lib.layers]` が std に入れるものの例に「formats such as JSON」を挙げているとおり，JSON は HTTP のサービスのほぼすべてが読み書きする．`derive Decode` が std にあっても `Json.decode` が std の外にあれば，結局 x を足すことになる．`Rule`（条件と注記を 1 つにまとめた値）は形が固まっていないので std に入れず，x/rule に分ける（下の「Rule」）．中身は x の今の版と同じで，名前の頭が変わるだけ（`import Dynamic.{...}` は `import Std.Dynamic.{...}` になる）．

`Shape` の variant は説明する型の名前にする（`Bool`，`Int`，`Int64`，`Float`，`String`，`List`，`Option`，`Map`．`Dynamic` の形は `Any`）．モジュールで修飾して `Shape.Int` と書く．variant `List` があっても `List.map` はモジュール `List` を指し，型 `Int` と variant `Int` は名前の空間が別なのでぶつからない．`Dynamic` の `Bool` と並べて修飾なしで import したときだけ，どちらかを修飾する．

### Rule

x/shape の `Shape.Rule`（条件とその注記を 1 つにまとめた値）は std に入れない．失敗の形を呼ぶ側が選ぶ API（`check`，`check_all`，`check_or`，`check_all_or`）は作ったばかりで，使った例がまだない．

- std が約束するのは，`Shape` を書く側と読む側の間の注記の名前だけにする．`minimum`，`maximum`，`min_length`，`max_length`，`min_items`，`max_items` を，`description` と同じく注記を付けるだけの関数として `Std.Shape` に足す．JSON Schema も OpenAPI も，読むのはこの名前だけ
- `Rule` は新しいパッケージ x/rule（モジュール `Rule`）に移し，注記を `Std.Shape` の関数で付ける．x/shape に残すと，パッケージの根のモジュール `Shape` と `Std.Shape` が import の名前でぶつかる
- 導出は `Rule` に触らない．derive した `Shaped` は中身の型の形を出すだけで，条件の注記を付けるのは手で書く opaque type の impl（`Named` と `Rule.annotate`）だけ

### 導出の規則

どれも，各フィールドをそのフィールドの型の impl に通す（`derive Show` と同じ）．型パラメータには同じ trait の境界を付ける（`[trait.derive]` の今の規則のまま）．

Encode（値を 1 つとって組み合わせる）

- record：フィールドの名前と `Encode.to_dynamic(フィールド)` の object．`Option` のフィールドが `None` でもキーを省かず null を書く
- enum：`Std.Dynamic.variant(variant 名, 中身)`．名前付きの中身は object，位置の中身は要素数どおりの `Items`，中身なしは `"value"` を置かない

```emel
type TodoView(id: String, done: Bool) derive Encode
# generates
impl TodoView: Encode {
  fn to_dynamic(self) {
    Dynamic.object([("id", Encode.to_dynamic(self.id)), ("done", Encode.to_dynamic(self.done))])
  }
}
```

Decode（値を作る）

- record：フィールドごとに `Std.Dynamic.field(d, 名前, Decode.from_dynamic)` で読み，構築する．キーがないときは null を読んだものとして扱う（`field` の今の動き）ので，`Option` のフィールドは省いてよい
- enum：`Std.Dynamic.tag(d)` で variant を選び，`"value"` を中身の形で読む．位置の中身は，まず `Std.Dynamic.items(value, n)` で要素の数が宣言どおりかを確かめ，それから `index(i, Decode.from_dynamic)` で読む．数が違えば `DecodeError(expected: "2 items", ...)`．名前付きの中身と record は，知らないキーを今までどおり無視する
- 知らない tag は `DecodeError(expected: "one of Leaf, Pair, Node", actual: Text(tag), path: [Key("tag")])`
- path にはフィールド名と位置が積まれる

失敗の扱いは 2 つの関数で分ける．

- `from_dynamic` は最初の失敗で止まり，`DecodeError` を 1 つ返す
- `from_dynamic_all` はすべてのフィールドを読み，失敗を宣言の順にすべて集めて `DecodeErrors` で返す．入れ子の型，`List` の要素，`Map` の値の失敗も，それぞれの path を付けて集まる
- `Decode` の trait では，`from_dynamic_all` は `from_dynamic` の失敗を 1 つだけ包む既定の実装を持つ．手で書いた impl は `from_dynamic` だけ書けば動き，その型の中では最初の失敗で止まる．導出した impl と `List`，`Option`，`Map` の impl は `from_dynamic_all` を上書きして集める
- API が 400 を返すときに，壊れたフィールドを一度に全部伝えられる

```emel
type TodoBody(title: String, due: Option[String]) derive Decode
# generates
impl TodoBody: Decode {
  fn from_dynamic(d) {
    TodoBody(
      title: Dynamic.field(d, "title", Decode.from_dynamic),
      due: Dynamic.field(d, "due", Decode.from_dynamic),
    )
  }

  fn from_dynamic_all(d) {
    title = Dynamic.attempt(fn() { Dynamic.field_all(d, "title", Decode.from_dynamic_all) })
    due = Dynamic.attempt(fn() { Dynamic.field_all(d, "due", Decode.from_dynamic_all) })
    (title, due) match {
      (Decoded(title), Decoded(due)) -> TodoBody(title:, due:)
      _ -> fail DecodeErrors(
        errors: List.flatten([Dynamic.errors_of(title), Dynamic.errors_of(due)]),
      )
    }
  }
}
```

この形（trait の既定の実装，`List` の impl の上書き，上の生成されるコード）は，x/dynamic の写しに足して今の compiler で動くことを確かめた．`title` が null で `tags` の 2 つめと 3 つめが null の値からは，`[.title]`，`[.tags[1]]`，`[.tags[2]]` の 3 つが集まる．

Shaped（型の形）

- record：`Std.Shape.visit(名前, visiting, fn(visiting) { Record(name: 名前, fields: [...]) })`．各フィールドは `Shaped.shape_in(None: Option[フィールドの型], visiting)`
- enum：同じく `visit` で包んだ `Variants`．中身は `NoFields`，`PositionalFields`，`NamedFields`
- 型パラメータを持つ型の名前は `Std.Shape.generic_name(型の名前, [型引数の形])`（`Page[Member]`）
- 自分を含む型も互いに参照する型も，`visit` が 2 回目を `Ref(名前)` にするので止まる

Encode，Decode，Shaped は同じ宣言から同じ規則で導出されるので，`Shaped` が説明する形は，`Encode` が書き出し `Decode` が読む JSON の形と作りからして一致する．

### キーと tag の名前

フィールドには `@key("...")`，variant には `@tag("...")` を付けられる．Encode，Decode，Shaped の導出は，フィールドの名前と variant の名前の代わりにこの名前を使う．

```emel
type TodoView(id: String, @key("dueAt") due_at: Option[String]) derive Encode, Decode, Shaped

enum Status {
  @tag("open")
  Open
  @tag("done")
  Done
} derive Encode, Decode, Shaped
```

- Emela のフィールドは snake_case で，外の相手は `dueAt` のような名前を求めることが多い．名前が 1 つ違うだけで手書きの impl に戻らずに済む
- Shape の `Field` と `Variant` の `name` にもこの名前が入るので，JSON Schema と実際の形はずれない．`Record` と `Variants` の `name`（型の名前）は変わらない
- 同じ型の中で 2 つのフィールドが同じキーに，同じ enum の中で 2 つの variant が同じ tag になれば compile error（E0337）
- `@key` をフィールド以外に，`@tag` を variant 以外に付けたり，引数が文字列リテラル 1 つでなかったりすれば compile error（E0338）
- None のキーを省く属性と，型全体の名前の変換（全フィールドを camelCase にする）は入れない．導出した Decode はキーがなくても null でも読むので，省いた形が要るのは厳しい外の schema に合わせるときだけ．型全体の変換は，変換の規則（`id_url` を `idUrl` にするか `idURL` にするか）まで決めることになる

### opaque type

- `derive Encode` と `derive Shaped` は opaque type にも書ける．`derive Show` と同じく中身を見せる
- `derive Decode` は opaque type には書けない（E0336）．導出した Decode は条件を確かめて作る関数を通らずに値を作るので，opaque type が守っている条件を破れてしまう．`impl Email: Decode` を手で書き，その中で `Email.parse` を呼ぶ

### ObjectKey

`Map` のキーを Fields のキーに書く trait `ObjectKey` も導出できる．対象は 2 つの形だけ．

```emel
enum Status {
  @tag("open")
  Open
  @tag("done")
  Done
} derive Eq, Ord, ObjectKey

type Code(value: String) derive Eq, Ord, ObjectKey
```

- すべての variant が中身を持たない enum：`to_key` は tag（`@tag` があればその名前）を返し，`from_key` は tag から variant を選ぶ．どの tag でもなければ `DecodeError(expected: "one of open, done", ...)`
- フィールドが 1 つの型：`to_key` と `from_key` をそのフィールドの型の `ObjectKey` に任せる
- それ以外の形は compile error（E0339）．中身のある variant や 2 つ以上のフィールドは，1 つの文字列にする決まった書き方がない．区切り文字を決めると，その文字を含む値で壊れる
- opaque type には導出できない（E0336）．`from_key` が条件を確かめる関数を通らずに値を作るため
- `ObjectKey` は `Ord` を親に持つので，今の trait の規則どおり `Ord` の impl も要る
- Map のキーに enum を使う型でも `derive Encode, Decode` が通るようになる

### 導出できる trait

`Eq`，`Ord`，`Show`（Prelude）と，`Std.Dynamic.Encode`，`Std.Dynamic.Decode`，`Std.Dynamic.ObjectKey`，`Std.Shape.Shaped`．同じ名前の利用者の trait（`trait Encode`）は導出できない（E0326 のまま）．

## Specification changes

### `[trait.derive]`

Replace "Only Eq, Ord and Show can be derived, and the compiler owns the derivation rules. Writing any other trait in derive is a compile error (E0326)." with:

- Eq, Ord and Show of the Prelude, Encode, Decode and ObjectKey of `Std.Dynamic`, and Shaped of `Std.Shape` can be derived, and the compiler owns the derivation rules. Writing any other trait in derive is a compile error (E0326), including a user trait with one of these names.

Add after the bullet on Ord:

- A derived Encode of a type writes an object of its fields, by name, each through `Encode.to_dynamic`. A field whose value is None is written as Null; the key is not left out. A derived Encode of an enum writes `Std.Dynamic.variant(name, contents)`: an object for named contents, Items with one element for each positional field, and no value for a variant without contents.
- A derived Decode of a type reads each field with `Std.Dynamic.field(value, name, Decode.from_dynamic)`, in declaration order, and constructs the value. A missing key is read as Null, so a field of an Option type may be left out. A derived Decode of an enum reads the variant from `Std.Dynamic.tag(value)` and its contents from `"value"` in the form of the variant, a positional field `i` with `Std.Dynamic.index(i, Decode.from_dynamic)`, after `Std.Dynamic.items(value, n)` has checked that the contents hold exactly as many items as the variant has fields. Keys that name no field of a type or of named contents are ignored. A tag that names no variant fails with `DecodeError(expected: "one of A, B", actual: the tag, path: [Key("tag")])`, listing the variants in declaration order. `from_dynamic` stops at the first failure. A derived Decode also defines `from_dynamic_all`, which reads every field in the same way with `Std.Dynamic.field_all` and `Std.Dynamic.index_all` and `Decode.from_dynamic_all`, and fails with `DecodeErrors` holding every failure in declaration order, each with its path. It constructs the value only when no field failed.
- A derived Shaped of a type or an enum is `Std.Shape.visit(name, visiting, build)`, where build makes a Record of its fields (or Variants of its variants), each field through `Shaped.shape_in` with the visiting it is given. The name of a type with type parameters is `Std.Shape.generic_name(name, shapes)`, with the shapes of its type arguments in order.
- In all three, the key of a field is the argument of its `@key` attribute if it has one, and its name otherwise; the tag of a variant is the argument of its `@tag` attribute if it has one, and its name otherwise. The names in `Field` and `Variant` of a derived Shaped are these keys and tags.
- A derived ObjectKey of an enum whose variants all have no contents writes the tag of the variant as the key and reads a key by the tags; a key that is no tag fails with `DecodeError(expected: "one of A, B", actual: the key, path: [])`. A derived ObjectKey of a type with exactly one field delegates `to_key` and `from_key` to the ObjectKey of that field. Deriving ObjectKey for any other type is a compile error (E0339).
- Decode and ObjectKey cannot be derived for an opaque type (E0336): a derived Decode or ObjectKey constructs the value without the functions of its module that check it. Encode and Shaped can be derived for an opaque type, and show its contents, as Show does.

Replace the table of the kinds of derivation and the text before it with:

The derivations that the compiler owns come in three kinds. Letting trait authors write derivation rules is for a later version (18.1 #3). Writing an item that starts with `derive` in the body of a trait is a compile error (E0129).

| Kind of derivation | Shape | Derivable traits |
|---|---|---|
| Fold | Takes one value and combines the results for each field | Show, Encode, Shaped (over the types of the fields), `to_key` of ObjectKey |
| Comparison | Takes two values and compares them field by field | Eq, Ord |
| Construction | Builds a value from the results generated for each field | Decode, `from_key` of ObjectKey |

### `[kw.annotation]`

Replace the row of field attributes with:

| Attribute | Target | Meaning |
|---|---|---|
| `@key("name")` | Fields of types and of named contents of variants | The key of the field in a derived Encode, Decode and Shaped (`[trait.derive]`), instead of the name of the field |
| `@tag("name")` | Variants of enums | The tag of the variant in a derived Encode, Decode and Shaped, instead of the name of the variant |

The argument of `@key` and `@tag` is one string literal. `@key` on anything but a field, `@tag` on anything but a variant, or any other argument is a compile error (E0338). Two fields of one type or of one variant with the same key, or two variants of one enum with the same tag, is a compile error (E0337). The attributes have no effect on code that is not derived: a field is still accessed and constructed by its name.

### `[grammar.decl]`

Allow attributes before a variant:

```ebnf
variant      = { annotation } type_name [ "(" ( fields | type { "," type } ) ")" ]
```

### `[lib.layers]`

In the table of the layers, replace the contents of the value layer with:

| Layer | Nature | Contents |
|---|---|---|
| value | Pure and independent of the host | Instant, Duration, Path, Dynamic (values in no particular format, and Encode and Decode), Shape (the shapes of types, and Shaped). Collections (Map, Set, Array) in 0.24 |

Replace "JSON conversion (Encode, Decode) is provided by the standard library as traits with deriving rules." with:

- Values that go out of and come into a program are converted through `Dynamic` (`Std.Dynamic`), a value in no particular format: Null, Bool, Number, Text, Items and Fields. Encode turns a value into a Dynamic and Decode builds a value from one; both are derivable (`[trait.derive]`). A format only parses into and writes from Dynamic. JSON is provided by `Std.Json`; other formats (TOML) are provided outside std until they settle.
- The shape of a type (`Std.Shape`) describes all the values of the type without a value: its fields, its variants and the types inside them. Shaped gives it and is derivable. A shape carries notes for its readers; `Std.Shape` names the notes it defines (`[api.std.shape]`), and a reader passes on the notes it does not know.

### `[api.std.dynamic]`

A new section after `[api.std.map]`.

> ### Std.Dynamic (`std/dynamic.emel`) `[api.std.dynamic]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Dynamic` | `enum Dynamic { Null, Bool(Bool), Number(Number), Text(String), Items(List[Dynamic]), Fields(Map[String, Dynamic]) }` | A value in no particular format |
> | `Segment` | `enum Segment { Key(String), Index(Int) }` | A step of a path into a Dynamic |
> | `DecodeError` | `error DecodeError(expected: String, actual: Dynamic, path: List[Segment])` | Show writes the kind of `actual`, not its contents |
> | `Encode` | `trait Encode { fn to_dynamic(self) -> Dynamic }` | Derivable |
> | `Decode` | `trait Decode { fn from_dynamic(d: Dynamic) -> Self fails DecodeError; fn from_dynamic_all(d: Dynamic) -> Self fails DecodeErrors }` | Derivable, except for opaque types. `from_dynamic` stops at the first failure. `from_dynamic_all` reports every failure; its default fails with the one failure of `from_dynamic` |
> | `DecodeErrors` | `error DecodeErrors(errors: List[DecodeError])` | Every failure of `from_dynamic_all`, in the order the parts were read. Never empty |
> | `Attempt` | `enum Attempt[A] { Decoded(A), Failed(List[DecodeError]) }` | The result of reading one part while collecting failures |
> | `attempt`, `errors_of` | `attempt[A](f: fn() -> A fails DecodeErrors) -> Attempt[A]`, `errors_of[A](a: Attempt[A]) -> List[DecodeError]` | Keep the failures of `f` instead of failing, and read them back (empty for Decoded) |
> | `field_all`, `index_all` | `field_all[A](value: Dynamic, name: String, decoder) -> A fails DecodeErrors`, `index_all[A](n: Int, decoder) -> decoder` | `field` and `index` for a decoder that fails with DecodeErrors; the key or the position is added in front of the path of every failure |
> | `ObjectKey` | `trait ObjectKey: Ord { fn to_key(self) -> String; fn from_key(key: String) -> Self fails DecodeError }` | The keys of a Map written as Fields |
> | `string`, `bool`, `int`, `int64`, `float` | `fn(value: Dynamic) -> A fails DecodeError` | Read a value of a basic type. `int` and `int64` accept a number whose value is an integer (`1.0`, `1e3`) |
> | `list`, `optional`, `dict` | `fn(decoder) -> decoder` | Read Items, Null or a value, and Fields with each value read by `decoder` |
> | `index` | `index[A](n: Int, decoder) -> decoder` | Read the element `n` of Items. Fewer elements is a DecodeError |
> | `items` | `items(value: Dynamic, n: Int) -> List[Dynamic] fails DecodeError` | The elements of Items that has exactly `n` elements. Any other number is `DecodeError(expected: "n items", ...)` |
> | `field` | `field[A](value: Dynamic, name: String, decoder) -> A fails DecodeError` | Read the key `name` of Fields. A missing key is read as Null |
> | `tag` | `tag(value: Dynamic) -> String fails DecodeError` | The `"tag"` of an enum written by Encode |
> | `object`, `variant` | `object(fields: List[(String, Dynamic)]) -> Dynamic`, `variant(tag: String, value: Option[Dynamic]) -> Dynamic` | Build Fields, and the form of an enum |
> | `kind` | `kind(value: Dynamic) -> String` | `"null"`, `"bool"`, `"number"`, `"text"`, `"items"` or `"fields"` |
>
> impl: Bool, Int, Int64, Float, String, `List[A]`, `Option[A]` and `Map[K: ObjectKey, V]` have Encode and Decode; String has ObjectKey. The Decode of `List[A]` and `Map[K, V]` defines `from_dynamic_all` to read every element or value and collect their failures, and that of `Option[A]` to read its value with `from_dynamic_all`. A Float that is NaN or ±Infinity is encoded as Null. None is encoded as Null, and Null decodes as None.

### `[api.std.dynamic.number]`

A new section after `[api.std.dynamic]`.

> ### Std.Dynamic.Number (`std/dynamic/number.emel`) `[api.std.dynamic.number]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Number` | `opaque type Number` | A decimal number of any size and precision: a sign, digits and an exponent. Two numbers that denote the same value are equal (`1.0` and `1e0`) |
> | `OutOfRange` | `error OutOfRange(number: Number)` | A number that the target type cannot hold |
> | `parse` | `parse(s: String) -> Number fails ParseError` | Read a number in the JSON grammar. Fails also when the exponent is outside the range of Int |
> | `from_int`, `from_int64` | `from_int(n: Int) -> Number`, `from_int64(n: Int64) -> Number` | Exact |
> | `from_float` | `from_float(f: Float) -> Option[Number]` | None for NaN and ±Infinity |
> | `to_int`, `to_int64` | `to_int(n: Number) -> Int fails OutOfRange`, `to_int64(n: Number) -> Int64 fails OutOfRange` | Fails for a number that is not an integer or is outside the range |
> | `to_float` | `to_float(n: Number) -> Float` | The nearest Float |
> | `to_string` | `to_string(n: Number) -> String` | In the JSON grammar. An integer of up to 21 digits is written in full, a short fraction with a point, the rest with an exponent |

### `[api.std.shape]`

A new section after `[api.std.dynamic.number]`.

> ### Std.Shape (`std/shape.emel`) `[api.std.shape]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Shape` | `enum Shape { Bool, Int, Int64, Float, String, Any, List(Shape), Option(Shape), Map(key: Shape, value: Shape), Record(name: String, fields: List[Field]), Variants(name: String, variants: List[Variant]), Ref(String), Named(name: String, shape: Shape), Annotated(shape: Shape, notes: List[(String, Dynamic)]) }` | The shape of a type, without a value. The variants are named after the types they describe and are written with the module (`Shape.Int`, `Shape.List(Shape.String)`). `Any` is the shape of Dynamic. `Ref` is a record or enum that appears again inside itself. `Named` is a type known by its name with the shape of its contents (an opaque type). `Annotated` carries notes; a reader passes on the keys it does not know |
> | `Field` | `type Field(name: String, shape: Shape)` | A field of a record or of named contents |
> | `Variant` | `type Variant(name: String, contents: Contents)` | A variant of an enum |
> | `Contents` | `enum Contents { NoFields, PositionalFields(List[Shape]), NamedFields(List[Field]) }` | The contents of a variant, in the form they are declared in |
> | `Shaped` | `trait Shaped { fn shape_in(proxy: Option[Self], visiting: List[String]) -> Shape; fn shape(proxy: Option[Self]) -> Shape }` | Derivable. `proxy` only carries the type. `visiting` holds the names of the records and enums being read around this one, innermost first. `shape` calls `shape_in` with an empty `visiting` |
> | `visit` | `visit(name: String, visiting: List[String], build: fn(List[String]) -> Shape) -> Shape` | `Ref(name)` if `name` is in `visiting`, otherwise `build` with `name` added in front |
> | `of_value`, `of_parameter` | `of_value[A: Shaped](value: A) -> Shape`, `of_parameter[A: Shaped, B, E, R](f: fn(A) -> B fails E use R) -> Shape` | The shape of the type of a value, and of the parameter of a function without calling it |
> | `annotate` | `annotate(shape: Shape, name: String, value: Dynamic) -> Shape` | Add the note `name` after the notes of `shape`, replacing a note of the same name |
> | `description`, `pattern`, `format` | `fn(shape: Shape, text: String) -> Shape` | The notes `"description"`, `"pattern"` (a regular expression) and `"format"` (`"date-time"`, `"email"`) |
> | `minimum`, `maximum` | `fn(shape: Shape, n: Int) -> Shape` | The notes `"minimum"` and `"maximum"`: the least and the greatest number allowed |
> | `min_length`, `max_length` | `fn(shape: Shape, n: Int) -> Shape` | The notes `"min_length"` and `"max_length"`: the length of a string, counted in code points |
> | `min_items`, `max_items` | `fn(shape: Shape, n: Int) -> Shape` | The notes `"min_items"` and `"max_items"`: the number of items of a list |
> | `strip` | `strip(shape: Shape) -> Shape` | `shape` without its notes and names |
> | `note` | `note(shape: Shape, name: String) -> Option[Dynamic]` | The value of the note `name`, looking through names |
> | `field_names` | `field_names(shape: Shape) -> List[String]` | The names of the fields of a record in declaration order, looking through notes and names; empty for any other shape |
> | `name_of` | `name_of(shape: Shape) -> String` | The name of the type as it is written (`Int`, `List[String]`, `Page[Member]`) |
> | `generic_name` | `generic_name(name: String, arguments: List[Shape]) -> String` | `name` followed by the names of `arguments` in brackets, or `name` alone without arguments |
>
> impl: Bool, Int, Int64, Float, String, Dynamic, `List[A]`, `Option[A]` and `Map[K, V]` have Shaped. The impls of `List`, `Option` and `Map` pass `visiting` on to their type arguments.
>
> The notes defined by `Std.Shape` are `"description"`, `"pattern"`, `"format"`, `"minimum"`, `"maximum"`, `"min_length"`, `"max_length"`, `"min_items"` and `"max_items"`, each with the value its function above writes. Notes are only described: the notes do not change which values a shape describes, and nothing in `Std.Shape` checks them. Other notes may be added with `annotate`.


### `[api.std.json]`

A new section after `[api.std.shape]`.

> ### Std.Json (`std/json.emel`) `[api.std.json]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `SyntaxError` | `error SyntaxError(message: String, offset: Int)` | Text that is not JSON. `offset` counts code points from the start of the input |
> | `parse` | `parse(s: String) -> Dynamic fails SyntaxError` | Read one JSON value (RFC 8259), with whitespace allowed around it. Numbers are read with `Std.Dynamic.Number.parse`. For a key repeated in an object, the later value is kept |
> | `to_string` | `to_string(value: Dynamic) -> String` | Write `value` as JSON without whitespace. Keys come in the order of the Map. A string escapes `"`, `\` and the control characters, and keeps every other character as it is |
> | `decode` | `decode[A: Decode](text: String) -> A fails SyntaxError \| DecodeError` | `parse`, then `Decode.from_dynamic` |
> | `decode_all` | `decode_all[A: Decode](text: String) -> A fails SyntaxError \| DecodeErrors` | `parse`, then `Decode.from_dynamic_all` |
> | `encode` | `encode[A: Encode](value: A) -> String` | `Encode.to_dynamic`, then `to_string` |

### `[diag.list]`

Change E0326 to list the derivable traits, and add E0336, E0337, E0338 and E0339.

| Code | Condition | Message |
|---|---|---|
| E0337 | Two fields of one type or variant with the same key, or two variants of one enum with the same tag, after `@key` and `@tag` (3.5) | ``key `dueAt` is used by both `due_at` and `due` `` / ``tag `done` is used by both `Done` and `Closed` `` |
| E0338 | `@key` not on a field, `@tag` not on a variant, or an argument that is not one string literal (3.5) | `` `@key` takes one string literal `` / `` `@tag` can only be placed on a variant `` |
| E0336 | `derive Decode` or `derive ObjectKey` on an opaque type (10.5). The help is ``= help: write `impl Email: Decode` that builds the value with a function that checks it`` | ``cannot derive `Decode` for opaque type `Email` `` |
| E0339 | `derive ObjectKey` on a type with no field or more than one, or on an enum with a variant that has contents (10.5) | ``cannot derive `ObjectKey` for `Shape`: variant `Circle` has contents`` / ``cannot derive `ObjectKey` for `Point`: it has 2 fields`` |

### `[open.list]`

In #3, record that Encode, Decode and Shaped are derived by the compiler with the field attributes `@key` and `@tag`, and that the question is now only the syntax for trait authors.

## Diagnostics

- [ ] E0326: 導出できる trait の一覧を広げる．利用者が同じ名前で宣言した trait は今までどおり E0326
- [ ] E0336（新規）：opaque type に `derive Decode` と `derive ObjectKey`
- [ ] E0339（新規）：`derive ObjectKey` を付けられない形
- [ ] E0337（新規）：`@key` と `@tag` を当てた後のキーや tag の重複
- [ ] E0338（新規）：`@key` と `@tag` の置き場所と引数
- [ ] W0303：導出した Encode，Decode，Shaped も「どこからも要求されない」の対象にする．境界を持つ関数への引き渡し（`Json.encode(view)`）が要求に当たる

## Compatibility

- 今のプログラムで意味が変わるものはない．`derive Encode` は今 E0326 なので，それが通るようになるだけ
- 利用者が `Encode`，`Decode`，`Shaped` という名前の trait を自分で宣言していても，その derive は今と同じく E0326
- x/dynamic と x/shape を使っているコードは，import を `Std.Dynamic` と `Std.Shape` に書き換える．Emela には再エクスポートがない（`[module.resolution]`）ので，x の側で std の名前を見せ続けることはできない．x/dynamic と x/shape は最後の版で README に移行先を書き，更新をやめる．`Shape.Rule` を使っているコードは x/rule に移る
- x/json を使っているコードは，import を `Std.Json` に書き換える．x/json は最後の版で README に移行先を書き，更新をやめる

## Implementation

- emela-core（std）：`std/dynamic.emel`，`std/dynamic/number.emel`，`std/shape.emel` を x から移す．`Std.Shape` に注記の関数（`minimum` など 6 つ）を足す．`Std.Dynamic` に `items` を足す（x/dynamic にはない）
- emela-core（lower.rs）：`derived` が Prelude の名前だけでなく，`Std.Dynamic.Encode`，`Std.Dynamic.Decode`，`Std.Dynamic.ObjectKey`，`Std.Shape.Shaped` を見分ける．生成するコードが呼ぶ std の関数（`Dynamic.object`，`Dynamic.field`，`Dynamic.tag`，`Dynamic.index`，`Dynamic.variant`，`Shape.visit`，`Shape.generic_name`）を，名前で引ける表に入れる
- emela-core（mono/derive.rs）：`DeriveOp` に Encode，Decode，ObjectKey，Shaped を足す．Decode は `from_dynamic` と `from_dynamic_all` の 2 つを生成する．型が単相化で決まったところで本体を組み立てる．Shaped のフィールドの `None: Option[T]` は IR で型付きの値として直接作る．`generic_name` に渡す名前は，単相化の時点で決まっているので定数でもよい
- emela-types：E0336，E0339 を derive の検査に，E0337，E0338 を属性の検査に足す
- emela-lsp：derive の補完の候補に 3 つの trait を足す（import されているとき）
- emela-core（std）：`std/json.emel` を x/json から移し，`decode_all` を足す
- x：JSON Schema の生成を x/shape から新しいパッケージ x/json-schema（モジュール `JsonSchema`）に移す．x/json に置くと，根のモジュール `Json` が `Std.Json` と import の名前でぶつかる．`Shape.Rule` を x/rule に移す

## Alternatives

- `Rule` も `Std.Shape.Rule` として std に入れる：opaque type を作る関数の書き方が std の中で完結する．ただ `Rule` の API は使った例がなく，std で固めるには早い．注記の名前が std にあれば，後から入れても読む側は変わらない
- trait の作者が導出の規則を書ける仕組み（#3）を先に入れ，x のまま derive する：構文も意味も決まっておらず，この 3 つのために待つ理由がない．#3 が入ったら，compiler が持つ規則をその構文で書き直せばいい
- 型の宣言から impl を生成する外部の道具（`emela gen` など）：生成したファイルと宣言がずれる．型が変わるたびに道具を走らせる手間が残る
- Encode と Decode だけを std に入れ，Shaped は x に残す：Shaped を手で書くことになり，JSON Schema と実際の形がずれる原因が残る．同じ宣言から同じ規則で導出することが，この提案の一番の利点

## Prior art

- Rust の serde（`#[derive(Serialize, Deserialize)]`）と schemars（`#[derive(JsonSchema)]`）
- Haskell の Generic と aeson
- Elm の Json.Decode（decoder の組み合わせ．x/dynamic の decoder の形）

## Open questions

なし．

## Decision
