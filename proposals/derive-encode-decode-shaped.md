---
title: Encode, Decode and Shaped in std, with their derives
authors: [Alliana]
status: draft
pr: 455
issue:
decided:
touches: [lib.layers, trait.derive, api.std.dynamic (new), api.std.dynamic.number (new), api.std.shape (new), api.std.shape.rule (new), diag.list, open.list]
requires: []
related: []
---

# Encode, Decode and Shaped in std, with their derives

## Summary

x/dynamic（形式によらない値 `Dynamic` と trait `Encode`/`Decode`）と x/shape（型の形 `Shape` と trait `Shaped`，`Rule`）を std の value の層に移し，`Std.Dynamic` と `Std.Shape` にする．compiler が `derive Encode, Decode, Shaped` を導出できるようにする．

```emel
import Std.Dynamic.{Encode, Decode}
import Std.Shape.{Shaped}

type TodoBody(title: String, due: Option[String]) derive Decode, Shaped
type TodoView(id: String, title: String, done: Bool) derive Encode, Shaped
```

JSON の parse と書き出し（x/json）と JSON Schema の生成（x/shape の `Shape.JsonSchema`）は x に残す．

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
| `Std.Dynamic` | `Dynamic`，`Segment`，`DecodeError`，trait `Encode`，`Decode`，`ObjectKey`，decoder（`string`，`int`，`field`，`list`，`optional`，`dict`，`index`，`tag`），`object`，`variant`，`kind` | x/dynamic |
| `Std.Dynamic.Number` | `Number`，`OutOfRange`，`parse`，`from_int`，`to_int` など | x/dynamic の `Number` |
| `Std.Shape` | `Shape`，`Field`，`Variant`，`Contents`，trait `Shaped`，`visit`，`name_of`，`generic_name`，`annotate`，`strip`，`note`，`field_names`，`of_value`，`of_parameter`，`description`，`pattern`，`format` | x/shape |
| `Std.Shape.Rule` | `Rule`，`Broken`，`AllBroken`，条件と確かめる関数 | x/shape の `Shape.Rule` |

どれも value の層（純粋でホストによらない）に入る．中身は x の今の版と同じで，名前の頭が変わるだけ（`import Dynamic.{...}` は `import Std.Dynamic.{...}` になる）．

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
- enum：`Std.Dynamic.tag(d)` で variant を選び，`"value"` を中身の形で読む．位置の中身は `index(i, Decode.from_dynamic)`
- 知らない tag は `DecodeError(expected: "one of Leaf, Pair, Node", actual: Text(tag), path: [Key("tag")])`
- 最初の失敗で止まる．path にはフィールド名と位置が積まれる

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
}
```

Shaped（型の形）

- record：`Std.Shape.visit(名前, visiting, fn(visiting) { Record(name: 名前, fields: [...]) })`．各フィールドは `Shaped.shape_in(None: Option[フィールドの型], visiting)`
- enum：同じく `visit` で包んだ `Variants`．中身は `NoFields`，`PositionalFields`，`NamedFields`
- 型パラメータを持つ型の名前は `Std.Shape.generic_name(型の名前, [型引数の形])`（`Page[Member]`）
- 自分を含む型も互いに参照する型も，`visit` が 2 回目を `Ref(名前)` にするので止まる

Encode，Decode，Shaped は同じ宣言から同じ規則で導出されるので，`Shaped` が説明する形は，`Encode` が書き出し `Decode` が読む JSON の形と作りからして一致する．

### opaque type

- `derive Encode` と `derive Shaped` は opaque type にも書ける．`derive Show` と同じく中身を見せる
- `derive Decode` は opaque type には書けない（E0336）．導出した Decode は条件を確かめて作る関数を通らずに値を作るので，opaque type が守っている条件を破れてしまう．`impl Email: Decode` を手で書き，その中で `Email.parse` を呼ぶ

### 導出できる trait

`Eq`，`Ord`，`Show`（Prelude）と，`Std.Dynamic.Encode`，`Std.Dynamic.Decode`，`Std.Shape.Shaped`．同じ名前の利用者の trait（`trait Encode`）は導出できない（E0326 のまま）．

## Specification changes

### `[trait.derive]`

Replace "Only Eq, Ord and Show can be derived, and the compiler owns the derivation rules. Writing any other trait in derive is a compile error (E0326)." with:

- Eq, Ord and Show of the Prelude, Encode and Decode of `Std.Dynamic`, and Shaped of `Std.Shape` can be derived, and the compiler owns the derivation rules. Writing any other trait in derive is a compile error (E0326), including a user trait with one of these names.

Add after the bullet on Ord:

- A derived Encode of a type writes an object of its fields, by name, each through `Encode.to_dynamic`. A field whose value is None is written as Null; the key is not left out. A derived Encode of an enum writes `Std.Dynamic.variant(name, contents)`: an object for named contents, Items with one element for each positional field, and no value for a variant without contents.
- A derived Decode of a type reads each field with `Std.Dynamic.field(value, name, Decode.from_dynamic)`, in declaration order, and constructs the value. A missing key is read as Null, so a field of an Option type may be left out. A derived Decode of an enum reads the variant from `Std.Dynamic.tag(value)` and its contents from `"value"` in the form of the variant, a positional field `i` with `Std.Dynamic.index(i, Decode.from_dynamic)`. A tag that names no variant fails with `DecodeError(expected: "one of A, B", actual: the tag, path: [Key("tag")])`, listing the variants in declaration order. Decoding stops at the first failure.
- A derived Shaped of a type or an enum is `Std.Shape.visit(name, visiting, build)`, where build makes a Record of its fields (or Variants of its variants), each field through `Shaped.shape_in` with the visiting it is given. The name of a type with type parameters is `Std.Shape.generic_name(name, shapes)`, with the shapes of its type arguments in order.
- Decode cannot be derived for an opaque type (E0336): a derived Decode constructs the value without the functions of its module that check it. Encode and Shaped can be derived for an opaque type, and show its contents, as Show does.

Replace the table of the kinds of derivation and the text before it with:

The derivations that the compiler owns come in three kinds. Letting trait authors write derivation rules is for a later version (18.1 #3). Writing an item that starts with `derive` in the body of a trait is a compile error (E0129).

| Kind of derivation | Shape | Derivable traits |
|---|---|---|
| Fold | Takes one value and combines the results for each field | Show, Encode, Shaped (over the types of the fields) |
| Comparison | Takes two values and compares them field by field | Eq, Ord |
| Construction | Builds a value from the results generated for each field | Decode |

### `[lib.layers]`

In the table of the layers, replace the contents of the value layer with:

| Layer | Nature | Contents |
|---|---|---|
| value | Pure and independent of the host | Instant, Duration, Path, Dynamic (values in no particular format, and Encode and Decode), Shape (the shapes of types, Shaped and Rule). Collections (Map, Set, Array) in 0.24 |

Replace "JSON conversion (Encode, Decode) is provided by the standard library as traits with deriving rules." with:

- Values that go out of and come into a program are converted through `Dynamic` (`Std.Dynamic`), a value in no particular format: Null, Bool, Number, Text, Items and Fields. Encode turns a value into a Dynamic and Decode builds a value from one; both are derivable (`[trait.derive]`). A format (JSON, TOML) only parses into and writes from Dynamic; formats are provided outside std until they settle.
- The shape of a type (`Std.Shape`) describes all the values of the type without a value: its fields, its variants and the types inside them. Shaped gives it and is derivable. Rules (`Std.Shape.Rule`) hold a condition on values together with the note that describes it, so a function that checks its input and the shape of its type read the same rules.

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
> | `Decode` | `trait Decode { fn from_dynamic(d: Dynamic) -> Self fails DecodeError }` | Derivable, except for opaque types |
> | `ObjectKey` | `trait ObjectKey: Ord { fn to_key(self) -> String; fn from_key(key: String) -> Self fails DecodeError }` | The keys of a Map written as Fields |
> | `string`, `bool`, `int`, `int64`, `float` | `fn(value: Dynamic) -> A fails DecodeError` | Read a value of a basic type. `int` and `int64` accept a number whose value is an integer (`1.0`, `1e3`) |
> | `list`, `optional`, `dict` | `fn(decoder) -> decoder` | Read Items, Null or a value, and Fields with each value read by `decoder` |
> | `index` | `index[A](n: Int, decoder) -> decoder` | Read the element `n` of Items. Fewer elements is a DecodeError |
> | `field` | `field[A](value: Dynamic, name: String, decoder) -> A fails DecodeError` | Read the key `name` of Fields. A missing key is read as Null |
> | `tag` | `tag(value: Dynamic) -> String fails DecodeError` | The `"tag"` of an enum written by Encode |
> | `object`, `variant` | `object(fields: List[(String, Dynamic)]) -> Dynamic`, `variant(tag: String, value: Option[Dynamic]) -> Dynamic` | Build Fields, and the form of an enum |
> | `kind` | `kind(value: Dynamic) -> String` | `"null"`, `"bool"`, `"number"`, `"text"`, `"items"` or `"fields"` |
>
> impl: Bool, Int, Int64, Float, String, `List[A]`, `Option[A]` and `Map[K: ObjectKey, V]` have Encode and Decode; String has ObjectKey. A Float that is NaN or ±Infinity is encoded as Null. None is encoded as Null, and Null decodes as None.

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
> | `Shape` | `enum Shape { BoolShape, IntShape, Int64Shape, FloatShape, StringShape, DynamicShape, ListOf(Shape), OptionOf(Shape), MapOf(key: Shape, value: Shape), Record(name: String, fields: List[Field]), Variants(name: String, variants: List[Variant]), Ref(String), Named(name: String, shape: Shape), Annotated(shape: Shape, notes: List[(String, Dynamic)]) }` | The shape of a type, without a value. `Ref` is a record or enum that appears again inside itself. `Named` is a type known by its name with the shape of its contents (an opaque type). `Annotated` carries notes; a reader passes on the keys it does not know |
> | `Field` | `type Field(name: String, shape: Shape)` | A field of a record or of named contents |
> | `Variant` | `type Variant(name: String, contents: Contents)` | A variant of an enum |
> | `Contents` | `enum Contents { NoFields, PositionalFields(List[Shape]), NamedFields(List[Field]) }` | The contents of a variant, in the form they are declared in |
> | `Shaped` | `trait Shaped { fn shape_in(proxy: Option[Self], visiting: List[String]) -> Shape; fn shape(proxy: Option[Self]) -> Shape }` | Derivable. `proxy` only carries the type. `visiting` holds the names of the records and enums being read around this one, innermost first. `shape` calls `shape_in` with an empty `visiting` |
> | `visit` | `visit(name: String, visiting: List[String], build: fn(List[String]) -> Shape) -> Shape` | `Ref(name)` if `name` is in `visiting`, otherwise `build` with `name` added in front |
> | `of_value`, `of_parameter` | `of_value[A: Shaped](value: A) -> Shape`, `of_parameter[A: Shaped, B, E, R](f: fn(A) -> B fails E use R) -> Shape` | The shape of the type of a value, and of the parameter of a function without calling it |
> | `annotate` | `annotate(shape: Shape, name: String, value: Dynamic) -> Shape` | Add the note `name` after the notes of `shape`, replacing a note of the same name |
> | `description`, `pattern`, `format` | `fn(shape: Shape, text: String) -> Shape` | The notes `"description"`, `"pattern"` (a regular expression) and `"format"` (`"date-time"`, `"email"`) |
> | `strip` | `strip(shape: Shape) -> Shape` | `shape` without its notes and names |
> | `note` | `note(shape: Shape, name: String) -> Option[Dynamic]` | The value of the note `name`, looking through names |
> | `field_names` | `field_names(shape: Shape) -> List[String]` | The names of the fields of a record in declaration order, looking through notes and names; empty for any other shape |
> | `name_of` | `name_of(shape: Shape) -> String` | The name of the type as it is written (`Int`, `List[String]`, `Page[Member]`) |
> | `generic_name` | `generic_name(name: String, arguments: List[Shape]) -> String` | `name` followed by the names of `arguments` in brackets, or `name` alone without arguments |
>
> impl: Bool, Int, Int64, Float, String, Dynamic, `List[A]`, `Option[A]` and `Map[K, V]` have Shaped. The impls of `List`, `Option` and `Map` pass `visiting` on to their type arguments.

### `[api.std.shape.rule]`

A new section after `[api.std.shape]`.

> ### Std.Shape.Rule (`std/shape/rule.emel`) `[api.std.shape.rule]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Rule` | `opaque type Rule[A]` | A condition on values of A, held with the note that describes it to a shape. Opaque, so that a condition and its note are always made together |
> | `Broken` | `error Broken(message: String)` | The first rule broken, by its message |
> | `AllBroken` | `error AllBroken(messages: List[String])` | Every rule broken, in the order of the rules |
> | `at_least`, `at_most` | `fn(n: Int) -> Rule[Int]` | Notes `"minimum"` and `"maximum"` |
> | `min_length`, `max_length` | `fn(n: Int) -> Rule[String]` | Counted in code points. Notes `"min_length"` and `"max_length"` |
> | `min_items`, `max_items` | `fn[A](n: Int) -> Rule[List[A]]` | Notes `"min_items"` and `"max_items"` |
> | `custom` | `custom[A](message: String, holds: fn(A) -> Bool fails {} use {}) -> Rule[A]` | A condition of one's own. It adds no note |
> | `message` | `message[A](rule: Rule[A]) -> String` | What the rule asks for (`"at least 1"`) |
> | `broken` | `broken[A](value: A, rules: List[Rule[A]]) -> List[Rule[A]]` | The rules that `value` breaks, in order |
> | `check`, `check_all` | `check[A](value: A, rules: List[Rule[A]]) -> A fails Broken`, `check_all[A](...) -> A fails AllBroken` | `value` if it keeps every rule. `check` stops at the first broken rule; `check_all` tries them all |
> | `check_or`, `check_all_or` | `check_or[A, E](value: A, rules: List[Rule[A]], to_error: fn(Rule[A]) -> E) -> A fails E`, `check_all_or[A, E](..., to_error: fn(List[Rule[A]]) -> E) -> A fails E` | The same, failing with the error `to_error` makes |
> | `annotate` | `annotate[A](shape: Shape, rules: List[Rule[A]]) -> Shape` | `shape` with the notes of `rules`, in order |

### `[diag.list]`

Change E0326 to list the derivable traits, and add E0336.

| Code | Condition | Message |
|---|---|---|
| E0336 | `derive Decode` on an opaque type (10.5). The help is ``= help: write `impl Email: Decode` that builds the value with a function that checks it`` | ``cannot derive `Decode` for opaque type `Email` `` |

### `[open.list]`

In #3, record that Encode, Decode and Shaped are derived by the compiler, and that the question is now only the syntax for trait authors and field attributes.

## Diagnostics

- [ ] E0326: 導出できる trait の一覧を広げる．利用者が同じ名前で宣言した trait は今までどおり E0326
- [ ] E0336（新規）：opaque type に `derive Decode`
- [ ] W0303：導出した Encode，Decode，Shaped も「どこからも要求されない」の対象にする．境界を持つ関数への引き渡し（`Json.encode(view)`）が要求に当たる

## Compatibility

- 今のプログラムで意味が変わるものはない．`derive Encode` は今 E0326 なので，それが通るようになるだけ
- 利用者が `Encode`，`Decode`，`Shaped` という名前の trait を自分で宣言していても，その derive は今と同じく E0326
- x/dynamic と x/shape を使っているコードは，import を `Std.Dynamic` と `Std.Shape` に書き換える．Emela には再エクスポートがない（`[module.resolution]`）ので，x の側で std の名前を見せ続けることはできない．x/dynamic と x/shape は最後の版で README に移行先を書き，更新をやめる
- x/json は `Std.Dynamic` に依存する版に上げる（v0.3.0）

## Implementation

- emela-core（std）：`std/dynamic.emel`，`std/dynamic/number.emel`，`std/shape.emel`，`std/shape/rule.emel` を x から移す
- emela-core（lower.rs）：`derived` が Prelude の名前だけでなく，`Std.Dynamic.Encode`，`Std.Dynamic.Decode`，`Std.Shape.Shaped` を見分ける．生成するコードが呼ぶ std の関数（`Dynamic.object`，`Dynamic.field`，`Dynamic.tag`，`Dynamic.index`，`Dynamic.variant`，`Shape.visit`，`Shape.generic_name`）を，名前で引ける表に入れる
- emela-core（mono/derive.rs）：`DeriveOp` に Encode，Decode，Shaped を足す．型が単相化で決まったところで本体を組み立てる．Shaped のフィールドの `None: Option[T]` は IR で型付きの値として直接作る．`generic_name` に渡す名前は，単相化の時点で決まっているので定数でもよい
- emela-types：E0336 を derive の検査に足す
- emela-lsp：derive の補完の候補に 3 つの trait を足す（import されているとき）
- x：x/json を `Std.Dynamic` に移し，JSON Schema の生成を x/shape から x/json（`Json.Schema`）に移す

## Alternatives

- trait の作者が導出の規則を書ける仕組み（#3）を先に入れ，x のまま derive する：構文も意味も決まっておらず，この 3 つのために待つ理由がない．#3 が入ったら，compiler が持つ規則をその構文で書き直せばいい
- 型の宣言から impl を生成する外部の道具（`emela gen` など）：生成したファイルと宣言がずれる．型が変わるたびに道具を走らせる手間が残る
- Encode と Decode だけを std に入れ，Shaped は x に残す：Shaped を手で書くことになり，JSON Schema と実際の形がずれる原因が残る．同じ宣言から同じ規則で導出することが，この提案の一番の利点

## Prior art

- Rust の serde（`#[derive(Serialize, Deserialize)]`）と schemars（`#[derive(JsonSchema)]`）
- Haskell の Generic と aeson
- Elm の Json.Decode（decoder の組み合わせ．x/dynamic の decoder の形）

## Open questions

- `Shape` の variant 名（`IntShape`，`StringShape`）を std で固めてよいか．`Dynamic` の variant（`Bool`，`Text`）とぶつからないようにした名前だが，enum を import したときの修飾（`Shape.Int`）が入るなら，そちらに合わせて付け直したい．std に入れた後は変えにくい
- `Rule` も std に入れるか．入れないと，`Shaped` の注記と作る関数の条件をまとめる手段が std の外になる
- 位置の中身の数：derived Decode は余った要素を無視する（`index` の今の動き）が，JSON Schema の生成は `maxItems` を書く．どちらに揃えるか
- derived Decode が最初の失敗で止まるか，全部集めるか（std-json の下書きの未決）
- フィールドの属性（名前の変換，None のキーを省く）．`[open.list]` #3 で決めるとして，それまで手書きの impl に任せてよいか
- JSON（x/json）を同じ proposal で std に入れるか，x に残すか
- `ObjectKey` も導出できるようにするか（enum を Map のキーに使う場合）

## Decision
