---
title: Dynamic, Shape and JSON in std, with derived Encode, Decode and Shaped
authors: [Alliana]
status: accepted
pr: 503
issue:
decided: 2026-11-15
touches: [kw.annotation, grammar.decl, lib.layers, trait.derive, api.std.bytes, api.std.dynamic (new), api.std.dynamic.number (new), api.std.shape (new), api.std.json (new), diag.list, open.list]
requires: []
related: []
---

# Dynamic, Shape and JSON in std, with derived Encode, Decode and Shaped

## Summary

Move x/dynamic (`Dynamic`, a value in no particular format, and the traits `Encode` and `Decode`) and x/shape (`Shape`, the shape of a type, and the trait `Shaped`) into the value layer of std, as `Std.Dynamic` and `Std.Shape`. Move JSON parsing and writing (x/json) into std as `Std.Json`. The compiler derives `Encode`, `Decode` and `Shaped`. `Std.Bytes` gains `to_base64` and `from_base64`, which `Std.Json` uses to write bytes. This is planned for 0.28.

```emel
import Std.Dynamic.{Decode, Encode}
import Std.Shape.{Shaped}

type TodoBody(title: String, due: Option[String]) derive Decode, Shaped
type TodoView(id: String, title: String, done: Bool) derive Encode, Shaped
```

JSON Schema generation (`json_schema.emel` of x/shape) stays in x, in a new package x/json-schema.

## Motivation

- The impls of `Encode`, `Decode` and `Shaped` are written by hand for every type that goes in or out of a program. In a prototype of a Todo API, with six types, the hand-written impls took half of the web files (6 to 10 lines per type).
- A hand-written impl repeats the field names apart from the declaration of the type, so they are forgotten when a field is added. A hand-written `Shaped` makes the JSON Schema drift from the actual JSON.
- The compiler can derive only `Eq`, `Ord` and `Show` of the Prelude (`[trait.derive]`). The compiler owns the derivation rules and recognizes the traits by where they are (`derived` in `lower.rs` of `emela-core`). The traits in x are not visible to the compiler.
- `[lib.layers]` already says "JSON conversion (Encode, Decode) is provided by the standard library as traits with deriving rules". x/dynamic and x/shape have settled their form in x for that purpose.
  - x/dynamic v0.5.1: `Dynamic` (including `Binary`), `Number`, `DecodeError`, the decoder combinators, and the impls for the built-in types. x/json, x/toml, x/yaml, x/redacted, x/config and x/sql are built on it.
  - x/shape v0.4.0: `Shape`, `Shaped` (with `shape_in` and `visit`, which terminate on mutually recursive types), the note functions (`description`, `minimum` and others), `Rule`, and JSON Schema generation, with tests and validation by Ajv.
- With a way for trait authors to write derivation rules (`[open.list]` #3), x could be derived as it is, but not even its syntax is decided. The form of these three traits is settled, so the compiler owns them without waiting.

## Proposal

### Where things go

| std module | Contents | From |
|---|---|---|
| `Std.Dynamic` | `Dynamic`, `Segment`, `DecodeError`, the traits `Encode`, `Decode` and `ObjectKey`, the decoders (`string`, `bytes`, `int`, `field`, `list`, `optional`, `dict`, `index`, `items`, `tag`, `field_all`, `index_all`, `attempt`, `errors_of`), `DecodeErrors`, `Attempt`, `object`, `variant`, `kind` | x/dynamic |
| `Std.Dynamic.Number` | `Number`, `OutOfRange`, `parse`, `from_int`, `to_int` and the others | `Number` of x/dynamic |
| `Std.Json` | `SyntaxError`, `parse`, `to_string`, `decode`, `decode_all`, `encode` | x/json (`decode_all` is new) |
| `Std.Shape` | `Shape`, `Field`, `Variant`, `Contents`, the trait `Shaped`, `visit`, `name_of`, `generic_name`, `annotate`, `strip`, `note`, the note functions, `field_names`, `of_value`, `of_parameter` | x/shape |
| `Std.Bytes` | `to_base64`, `from_base64` | x/base64 (`encode`, `decode`) |

All of them belong to the value layer (pure and independent of the host). `[lib.layers]` names "formats such as JSON" among what std provides, and almost every HTTP service reads and writes JSON. With `derive Decode` in std and `Json.decode` outside it, a program would still add x. `Rule` (a condition and its notes in one value) is not settled and stays out of std (see "Rule" below). The contents are those of the current versions in x; only the prefix of the names changes (`import Dynamic.{...}` becomes `import Std.Dynamic.{...}`).

The variants of `Shape` are named after the types they describe (`Bool`, `Int`, `Int64`, `Float`, `String`, `Bytes`, `List`, `Option`, `Map`; the shape of `Dynamic` is `Any`). They are written with the module, as `Shape.Int`. A variant `List` does not change what `List.map` refers to, the module `List`, and the type `Int` and the variant `Int` are in different name spaces. Only when they are imported unqualified next to the `Bool` of `Dynamic` does one of them have to be qualified.

### Bytes

- `Dynamic` keeps the variant `Binary(Bytes)` of x/dynamic. `Bytes` has Encode (`Binary`) and Decode (which reads only `Binary`; text is not read as base64 or hexadecimal, because the format of the text is the caller's to choose).
- `Shape` gains the variant `Bytes`, and `Bytes` has Shaped.
- JSON has no bytes, so `Std.Json.to_string` writes `Binary` as a string of standard base64 with padding. `Json.parse` never gives `Binary`: the string is read back as `Text`, and `Bytes.from_base64` turns it into the bytes.
- `Std.Bytes` gains `to_base64(b) -> String` (the standard alphabet of RFC 4648 section 4, padded with `=`) and `from_base64(s) -> Option[Bytes]`, the inverse, in the same form as `to_hex` and `from_hex`. The URL-safe alphabet stays in x/base64.

### Array and Set

- `Array[A]` and `Set[A]` have Encode, Decode and Shaped, as `List[A]` does. Both are written as Items, and their shape is `Shape.List` of the shape of the element.
- The Decode of `Set[A]` reads Items in order and drops repeated elements.
- So a type with a field of these types can be derived.

### Rule

`Rule` of x/shape (a condition and its notes in one value) does not go into std. Its API, which lets the caller choose the form of the failure (`check`, `check_all`, `check_or`, `check_all_or`), is new and has no users yet.

- What std promises is only the names of the notes between the writer and the reader of a `Shape`. `minimum`, `maximum`, `min_length`, `max_length`, `min_items` and `max_items` are in `Std.Shape` as functions that only add a note, as `description` is. JSON Schema and OpenAPI read only these names.
- `Rule` moves to a new package x/rule (module `Rule`), which adds the notes with the functions of `Std.Shape`. Left in x/shape, the root module `Shape` of the package would clash with `Std.Shape` in imports.
- Derivation does not touch `Rule`. A derived `Shaped` gives only the shapes of the types inside it; the notes of conditions are added only by hand-written impls of opaque types (`Named` and the note functions).

### Derivation rules

Each passes every field through the impl of the type of that field (as `derive Show` does). The type parameters get bounds of the same trait (the current rule of `[trait.derive]`).

Encode (takes one value and combines the results)

- A type: an object of the names of the fields and `Encode.to_dynamic(field)`. A field of an `Option` type whose value is `None` is written as null; the key is not left out.
- An enum: `Std.Dynamic.variant(name of the variant, contents)`. Named contents are an object, positional contents are `Items` with as many elements as the variant has fields, and a variant without contents has no `"value"`.

```emel
type TodoView(id: String, done: Bool) derive Encode
# generates
impl TodoView: Encode {
  fn to_dynamic(self) {
    Dynamic.object([("id", Encode.to_dynamic(self.id)), ("done", Encode.to_dynamic(self.done))])
  }
}
```

Decode (builds a value)

- A type: reads each field with `Std.Dynamic.field(d, name, Decode.from_dynamic)` and constructs the value. A missing key is read as null (what `field` does now), so a field of an `Option` type may be left out.
- An enum: chooses the variant with `Std.Dynamic.tag(d)` and reads `"value"` in the form of its contents. For positional contents, `Std.Dynamic.items(value, n)` first checks that the number of elements is the declared one, and then `index(i, Decode.from_dynamic)` reads each. A different number is `DecodeError(expected: "2 items", ...)`. Types and named contents ignore unknown keys, as now.
- An unknown tag is `DecodeError(expected: "one of Leaf, Pair, Node", actual: Text(tag), path: [Key("tag")])`.
- The names of the fields and the positions are pushed onto the path.

Failures are handled by two functions.

- `from_dynamic` stops at the first failure and fails with one `DecodeError`.
- `from_dynamic_all` reads every field, collects every failure in declaration order, and fails with `DecodeErrors`. The failures of nested types, of the elements of `List` and of the values of `Map` are collected too, each with its path.
- In the trait `Decode`, `from_dynamic_all` has a default that wraps the one failure of `from_dynamic`. A hand-written impl works with `from_dynamic` alone and stops at its first failure within that type. Derived impls and the impls of `List`, `Array`, `Set`, `Option` and `Map` override `from_dynamic_all` and collect.
- An API that answers 400 can report every broken field at once.

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
    title = Dynamic.attempt(|| Dynamic.field_all(d, "title", Decode.from_dynamic_all))
    due = Dynamic.attempt(|| Dynamic.field_all(d, "due", Decode.from_dynamic_all))
    (title, due) match {
      (Decoded(title), Decoded(due)) -> TodoBody(title:, due:)
      _ -> fail DecodeErrors(
        errors: List.flatten([Dynamic.errors_of(title), Dynamic.errors_of(due)]),
      )
    }
  }
}
```

This form (the default in the trait, the override in the impl of `List`, and the generated code above) was checked to work with the current compiler, added to a copy of x/dynamic. From a value whose `title` is null and whose second and third `tags` are null, the three failures `[.title]`, `[.tags[1]]` and `[.tags[2]]` are collected.

Shaped (the shape of a type)

- A type: `Std.Shape.visit(name, visiting, |visiting| Record(name: name, fields: [...]))`. Each field is `Shaped.shape_in(None: Option[type of the field], visiting)`.
- An enum: `Variants`, wrapped in `visit` the same way. The contents are `NoFields`, `PositionalFields` or `NamedFields`.
- The name of a type with type parameters is `Std.Shape.generic_name(name of the type, [shapes of the type arguments])` (`Page[Member]`).
- A type that contains itself, and mutually recursive types, terminate, because `visit` turns the second visit into `Ref(name)`.

Encode, Decode and Shaped are derived from the same declaration by the same rules, so the shape that `Shaped` describes matches, by construction, the JSON that `Encode` writes and `Decode` reads.

### The names of keys and tags

A field can have `@key("...")`, and a variant `@tag("...")`. The derivations of Encode, Decode and Shaped use these names instead of the name of the field and of the variant.

```emel
type TodoView(id: String, @key("dueAt") due_at: Option[String]) derive Encode, Decode, Shaped

enum Status {
  @tag("open")
  Open
  @tag("done")
  Done
} derive Encode, Decode, Shaped
```

- Emela fields are snake_case, and the other side often wants names such as `dueAt`. One differing name no longer sends the type back to a hand-written impl.
- The `name` of `Field` and `Variant` of the Shape holds these names too, so the JSON Schema and the actual form do not drift. The `name` of `Record` and `Variants` (the name of the type) does not change.
- Two fields of one type with the same key, or two variants of one enum with the same tag, is a compile error (E0337).
- `@key` on anything but a field, `@tag` on anything but a variant, or an argument that is not one string literal, is a compile error (E0338).
- There is no attribute that leaves out the key of a None, and no conversion of the names of a whole type (every field to camelCase). A derived Decode reads a missing key and a null alike, so the form without the key is needed only to match a strict outside schema. A conversion of the whole type would have to fix the conversion rules too (whether `id_url` becomes `idUrl` or `idURL`).

### Opaque types

- `derive Encode` and `derive Shaped` can be written on an opaque type. They show its contents, as `derive Show` does.
- `derive Decode` cannot be written on an opaque type (E0336). A derived Decode constructs the value without the function that checks it, and could break the condition the opaque type keeps. Write `impl Email: Decode` by hand and call `Email.parse` in it.

### ObjectKey

`ObjectKey`, the trait that writes the keys of a `Map` as the keys of Fields, can be derived too, for two forms only.

```emel
enum Status {
  @tag("open")
  Open
  @tag("done")
  Done
} derive Eq, Ord, ObjectKey

type Code(value: String) derive Eq, Ord, ObjectKey
```

- An enum whose variants all have no contents: `to_key` returns the tag (the name given by `@tag`, if any), and `from_key` chooses the variant by the tag. A key that is no tag is `DecodeError(expected: "one of open, done", ...)`.
- A type with one field: `to_key` and `from_key` are delegated to the `ObjectKey` of the type of that field.
- Any other form is a compile error (E0339). Variants with contents and two or more fields have no fixed way to be written as one string. Choosing a separator breaks on values that contain it.
- It cannot be derived for an opaque type (E0336), because `from_key` would construct the value without the function that checks it.
- `ObjectKey` has `Ord` as its supertrait, so an impl of `Ord` is also needed, by the current rules of traits.
- A type that uses an enum as the key of a Map can then derive `Encode, Decode`.

### Derivable traits

`Eq`, `Ord`, `Show` (Prelude), `Std.Dynamic.Encode`, `Std.Dynamic.Decode`, `Std.Dynamic.ObjectKey` and `Std.Shape.Shaped`. A user trait of the same name (`trait Encode`) cannot be derived (E0326, as now).

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
| value | Pure and independent of the host | Instant, Duration, Path, Bytes, Array, Dynamic (values in no particular format, and Encode and Decode), Shape (the shapes of types, and Shaped), Json. Collections (Map, Set) in 0.24 |

Replace "JSON conversion (Encode, Decode) is provided by the standard library as traits with deriving rules." with:

- Values that go out of and come into a program are converted through `Dynamic` (`Std.Dynamic`), a value in no particular format: Null, Bool, Number, Text, Items, Fields and Binary. Encode turns a value into a Dynamic and Decode builds a value from one; both are derivable (`[trait.derive]`). A format only parses into and writes from Dynamic. JSON is provided by `Std.Json`; other formats (TOML, YAML) are provided outside std until they settle.
- The shape of a type (`Std.Shape`) describes all the values of the type without a value: its fields, its variants and the types inside them. Shaped gives it and is derivable. A shape carries notes for its readers; `Std.Shape` names the notes it defines (`[api.std.shape]`), and a reader passes on the notes it does not know.

### `[api.std.bytes]`

Add after the row of `hex`:

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `to_base64` | `to_base64(b: Bytes) -> String` | Emela | Base64 in the standard alphabet (`A`–`Z`, `a`–`z`, `0`–`9`, `+`, `/`) of RFC 4648 section 4, padded with `=` to a multiple of 4 characters |
| `from_base64` | `from_base64(s: String) -> Option[Bytes]` | Emela | The inverse of `to_base64`. The padding is required. `None` for a length that is not a multiple of 4, misplaced padding, or any other character, including whitespace |

Add to the bullet on complexity: `to_base64` and `from_base64` are O(n).

### `[api.std.dynamic]`

A new section after `[api.std.set]`.

> ### Std.Dynamic (`std/dynamic.emel`) `[api.std.dynamic]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `Dynamic` | `enum Dynamic { Null, Bool(Bool), Number(Number), Text(String), Items(List[Dynamic]), Fields(Map[String, Dynamic]), Binary(Bytes) }` | A value in no particular format |
> | `Segment` | `enum Segment { Key(String), Index(Int) }` | A step of a path into a Dynamic |
> | `DecodeError` | `error DecodeError(expected: String, actual: Dynamic, path: List[Segment])` | Show writes the kind of `actual`, not its contents |
> | `Encode` | `trait Encode { fn to_dynamic(self) -> Dynamic }` | Derivable |
> | `Decode` | `trait Decode { fn from_dynamic(d: Dynamic) -> Self fails DecodeError; fn from_dynamic_all(d: Dynamic) -> Self fails DecodeErrors }` | Derivable, except for opaque types. `from_dynamic` stops at the first failure. `from_dynamic_all` reports every failure; its default fails with the one failure of `from_dynamic` |
> | `DecodeErrors` | `error DecodeErrors(errors: List[DecodeError])` | Every failure of `from_dynamic_all`, in the order the parts were read. Never empty |
> | `Attempt` | `enum Attempt[A] { Decoded(A), Failed(List[DecodeError]) }` | The result of reading one part while collecting failures |
> | `attempt`, `errors_of` | `attempt[A](f: fn() -> A fails DecodeErrors) -> Attempt[A]`, `errors_of[A](a: Attempt[A]) -> List[DecodeError]` | Keep the failures of `f` instead of failing, and read them back (empty for Decoded) |
> | `field_all`, `index_all` | `field_all[A](value: Dynamic, name: String, decoder) -> A fails DecodeErrors`, `index_all[A](n: Int, decoder) -> decoder` | `field` and `index` for a decoder that fails with DecodeErrors; the key or the position is added in front of the path of every failure |
> | `ObjectKey` | `trait ObjectKey: Ord { fn to_key(self) -> String; fn from_key(key: String) -> Self fails DecodeError }` | The keys of a Map written as Fields |
> | `string`, `bytes`, `bool`, `int`, `int64`, `float` | `fn(value: Dynamic) -> A fails DecodeError` | Read a value of a basic type. `bytes` reads only Binary. `int` and `int64` accept a number whose value is an integer (`1.0`, `1e3`) |
> | `list`, `optional`, `dict` | `fn(decoder) -> decoder` | Read Items, Null or a value, and Fields with each value read by `decoder` |
> | `index` | `index[A](n: Int, decoder) -> decoder` | Read the element `n` of Items. Fewer elements is a DecodeError |
> | `items` | `items(value: Dynamic, n: Int) -> List[Dynamic] fails DecodeError` | The elements of Items that has exactly `n` elements. Any other number is `DecodeError(expected: "n items", ...)` |
> | `field` | `field[A](value: Dynamic, name: String, decoder) -> A fails DecodeError` | Read the key `name` of Fields. A missing key is read as Null |
> | `tag` | `tag(value: Dynamic) -> String fails DecodeError` | The `"tag"` of an enum written by Encode |
> | `object`, `variant` | `object(fields: List[(String, Dynamic)]) -> Dynamic`, `variant(tag: String, value: Option[Dynamic]) -> Dynamic` | Build Fields, and the form of an enum |
> | `kind` | `kind(value: Dynamic) -> String` | `"null"`, `"bool"`, `"number"`, `"text"`, `"items"`, `"fields"` or `"binary"` |
>
> impl: Bool, Int, Int64, Float, String, Bytes, `List[A]`, `Array[A]`, `Set[A]`, `Option[A]` and `Map[K: ObjectKey, V]` have Encode and Decode; String has ObjectKey. Bytes is encoded as Binary, and its Decode reads only Binary. `Array[A]` and `Set[A]` are encoded as Items in their order, and decoded from Items; the Decode of `Set[A]` drops repeated elements. The Decode of `List[A]`, `Array[A]`, `Set[A]` and `Map[K, V]` defines `from_dynamic_all` to read every element or value and collect their failures, and that of `Option[A]` to read its value with `from_dynamic_all`. A Float that is NaN or ±Infinity is encoded as Null. None is encoded as Null, and Null decodes as None.

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
> | `Shape` | `enum Shape { Bool, Int, Int64, Float, String, Bytes, Any, List(Shape), Option(Shape), Map(key: Shape, value: Shape), Record(name: String, fields: List[Field]), Variants(name: String, variants: List[Variant]), Ref(String), Named(name: String, shape: Shape), Annotated(shape: Shape, notes: List[(String, Dynamic)]) }` | The shape of a type, without a value. The variants are named after the types they describe and are written with the module (`Shape.Int`, `Shape.List(Shape.String)`). `Any` is the shape of Dynamic. `Ref` is a record or enum that appears again inside itself. `Named` is a type known by its name with the shape of its contents (an opaque type). `Annotated` carries notes; a reader passes on the keys it does not know |
> | `Field` | `type Field(name: String, shape: Shape)` | A field of a record or of named contents |
> | `Variant` | `type Variant(name: String, contents: Contents)` | A variant of an enum |
> | `Contents` | `enum Contents { NoFields, PositionalFields(List[Shape]), NamedFields(List[Field]) }` | The contents of a variant, in the form they are declared in |
> | `Shaped` | `trait Shaped { fn shape_in(proxy: Option[Self], visiting: List[String]) -> Shape; fn shape(proxy: Option[Self]) -> Shape }` | Derivable. `proxy` only carries the type. `visiting` holds the names of the records and enums being read around this one, innermost first. `shape` calls `shape_in` with an empty `visiting` |
> | `visit` | `visit(name: String, visiting: List[String], build: fn(List[String]) -> Shape) -> Shape` | `Ref(name)` if `name` is in `visiting`, otherwise `build` with `name` added in front |
> | `of_value`, `of_parameter` | `of_value[A: Shaped](value: A) -> Shape`, `of_parameter[A: Shaped, B, E, R](f: fn(A) -> B fails E uses { R }) -> Shape` | The shape of the type of a value, and of the parameter of a function without calling it |
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
> impl: Bool, Int, Int64, Float, String, Bytes, Dynamic, `List[A]`, `Array[A]`, `Set[A]`, `Option[A]` and `Map[K, V]` have Shaped. The shape of Bytes is `Bytes`, and the shapes of `Array[A]` and `Set[A]` are `List` of the shape of `A`. The impls of `List`, `Array`, `Set`, `Option` and `Map` pass `visiting` on to their type arguments.
>
> The notes defined by `Std.Shape` are `"description"`, `"pattern"`, `"format"`, `"minimum"`, `"maximum"`, `"min_length"`, `"max_length"`, `"min_items"` and `"max_items"`, each with the value its function above writes. Notes are only described: the notes do not change which values a shape describes, and nothing in `Std.Shape` checks them. Other notes may be added with `annotate`.

### `[api.std.json]`

A new section after `[api.std.shape]`.

> ### Std.Json (`std/json.emel`) `[api.std.json]`
>
> | Declaration | Contents | Description |
> |---|---|---|
> | `SyntaxError` | `error SyntaxError(message: String, offset: Int)` | Text that is not JSON. `offset` counts code points from the start of the input |
> | `parse` | `parse(s: String) -> Dynamic fails SyntaxError` | Read one JSON value (RFC 8259), with whitespace allowed around it. Numbers are read with `Std.Dynamic.Number.parse`. For a key repeated in an object, the later value is kept. Never gives Binary |
> | `to_string` | `to_string(value: Dynamic) -> String` | Write `value` as JSON without whitespace. Keys come in the order of the Map. A string escapes `"`, `\` and the control characters, and keeps every other character as it is. Binary is written as a string of `Bytes.to_base64` |
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

- [ ] E0326: widen the list of derivable traits. A user trait declared with one of these names is E0326, as now.
- [ ] E0336 (new): `derive Decode` and `derive ObjectKey` on an opaque type.
- [ ] E0339 (new): a form that cannot derive `ObjectKey`.
- [ ] E0337 (new): a repeated key or tag after `@key` and `@tag`.
- [ ] E0338 (new): the place and the argument of `@key` and `@tag`.
- [ ] W0303: derived Encode, Decode and Shaped are also subject to "never required anywhere". Passing a value to a function with the bound (`Json.encode(view)`) counts as requiring it.

## Compatibility

- No current program changes meaning. `derive Encode` is E0326 now, and it only starts to compile.
- A user trait named `Encode`, `Decode` or `Shaped` is still E0326 in derive.
- `Std.Bytes` gains `to_base64` and `from_base64`. Std functions are reached through the module (`Bytes.to_base64`), so no user name is taken.
- x/dynamic, x/shape and x/json stay published, so code that uses them keeps building. Emela has no re-exports (`[module.resolution]`), so the `Dynamic` of x/dynamic and `Std.Dynamic.Dynamic` are different types, and a value of one cannot be passed where the other is expected.
- After 0.28 is released, the packages of x built on them (x/json-schema, x/rule, x/toml, x/yaml, x/redacted, x/config and x/sql) publish new versions that require emela 0.28 and use `Std.Dynamic`, `Std.Shape` and `Std.Json`. Code that uses x/dynamic, x/shape or x/json rewrites its imports to the std modules when it moves to those versions.

## Implementation

- emela-core (std): move `std/dynamic.emel`, `std/dynamic/number.emel`, `std/shape.emel` and `std/json.emel` from x. Add `decode_all` to `Std.Json`, the variant `Bytes` and its impl to `Std.Shape`, and the impls for `Array` and `Set` to both. Add `to_base64` and `from_base64` to `std/bytes.emel`, from x/base64.
- emela-core (`lower.rs`): `derived` recognizes `Std.Dynamic.Encode`, `Std.Dynamic.Decode`, `Std.Dynamic.ObjectKey` and `Std.Shape.Shaped` besides the names of the Prelude. The std functions that the generated code calls (`Dynamic.object`, `Dynamic.field`, `Dynamic.tag`, `Dynamic.index`, `Dynamic.variant`, `Shape.visit`, `Shape.generic_name`) go into a table looked up by name.
- emela-core (`mono/derive.rs`): add Encode, Decode, ObjectKey and Shaped to `DeriveOp`. Decode generates both `from_dynamic` and `from_dynamic_all`. The bodies are built once monomorphization has fixed the types. The `None: Option[T]` of the fields of Shaped is made directly in the IR as a typed value. The names passed to `generic_name` are fixed at monomorphization and can be constants.
- emela-types: add E0336 and E0339 to the checks of derive, and E0337 and E0338 to the checks of attributes.
- emela-lsp: add the four traits to the completion of derive (when they are imported).
- x: move JSON Schema generation from x/shape to a new package x/json-schema (module `JsonSchema`); in x/json, its root module `Json` would clash with `Std.Json` in imports. Move `Rule` to x/rule. Then, after 0.28, update the packages in Compatibility.

## Alternatives

- Put `Rule` into std as `Std.Shape.Rule`: the way to write the functions that build opaque types would be complete inside std. But the API of `Rule` has no users yet, and it is too early to fix it in std. With the names of the notes in std, adding it later does not change the readers.
- Add the way for trait authors to write derivation rules (#3) first, and derive x as it is: neither its syntax nor its meaning is decided, and there is no reason for these three to wait. Once #3 arrives, the rules that the compiler owns can be rewritten in its syntax.
- An external tool that generates impls from type declarations (`emela gen`): the generated files drift from the declarations, and the tool has to be run every time a type changes.
- Put only Encode and Decode into std and leave Shaped in x: Shaped would be written by hand, and the JSON Schema could still drift from the actual form. Deriving all three from the same declaration by the same rules is the main point of this proposal.
- Leave `Std.Json` in x: a program with `derive Decode` from std would still add x to read a request. JSON is the format that `[lib.layers]` names for std. TOML and YAML stay in x.
- Leave `Binary` out of `Dynamic`: x/dynamic already has it, and Bytes is in std. Without it, bytes would be written as text in a format each program chooses, and binary formats could not carry them.
- Keep base64 in x and let `Std.Json` write bytes some other way: `Std.Json` writes Binary as base64, so std needs base64. The URL-safe alphabet stays in x/base64.

## Prior art

- Deriving the conversion to and from a format-independent value from a type declaration, with attributes for the names of keys: Rust serde (`#[derive(Serialize, Deserialize)]`, `#[serde(rename = "...")]`), and schemars (`#[derive(JsonSchema)]`) for the shape.
- Haskell, Generic and aeson.
- Elm, Json.Decode (decoder combinators, the form of the decoders of x/dynamic).

## Open questions

## Decision

Every type that goes in or out of a program had its Encode, Decode and Shaped written by hand, apart from its declaration, so a field added later was forgotten and the JSON Schema drifted from the JSON actually written. The compiler now derives all three from the same declaration by the same rules, so the shape a type describes and the JSON it reads and writes match by construction. These traits are settled in x, and there is no reason for them to wait for a syntax that lets trait authors write derivation rules. Dynamic, Shape and JSON move into the value layer of std, since a derived Decode is of little use if reading a request still needs x, and `[lib.layers]` already names JSON for std. TOML and YAML stay in x. `@key` and `@tag` let a type match the names the other side expects without going back to a hand-written impl. Decode and ObjectKey are not derived for opaque types, because they would build the value without the function that checks it. `derive Decode` reports every broken field at once through `from_dynamic_all`, so an API can answer 400 with all of them. `Rule` stays in x until it has users. Bytes is carried as Binary and written to JSON as base64, so `Std.Bytes` gains `to_base64` and `from_base64`. Array and Set are read and written as Items like List. x/dynamic, x/shape and x/json stay published, and after 0.28 the packages built on them move to the std modules in new versions.
