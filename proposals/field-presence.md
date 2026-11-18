---
title: Absent keys in derived Encode and Decode
authors: [Alliana]
status: draft
pr: 652
issue:
decided:
touches: [trait.derive, api.std.dynamic, api.std.dynamic.presence (new)]
requires: []
related: [derive-encode-decode-shaped]
---

# Absent keys in derived Encode and Decode

## Summary

In derived Encode and Decode, the type of a field decides how a missing key and null are written and read.

- None of `Option[A]` leaves the key out (it is written as null now). A missing key and null are both read as None (as now).
- `Std.Dynamic.Presence` gains `Presence[A]`, a type that tells the three states apart: a missing key, null and a value. It is meant for the bodies of partial updates.
- The trait `Encode` gains `to_field`, and `Decode` gains `from_absent`. Both have default implementations, so the existing impls keep working.

No attribute and no setting of a Pome is added.

```emel
import Std.Dynamic.{Decode, Encode}
import Std.Dynamic.Presence.{Absent, Null, Present, Presence}

type TodoPatch(title: Option[String], due: Presence[String]) derive Encode, Decode
```

## Motivation

`derive-encode-decode-shaped` decided that None is written as null, and that a missing key and null are both read as None. `Option[A]` has two states and a field of JSON has three (a missing key, null, a value), so this cannot:

1. Tell a missing key from null. A partial update in its usual meaning (a missing key leaves the member as it is, null removes it; JSON Merge Patch, RFC 7396) can be neither read nor written.
2. Write to a counterpart whose field may be left out but may not be null. None written as null fails the counterpart's validation.
3. Describe input and output with one Shape. Encode writes "the key is always there and may be null", Decode accepts "the key may be missing", and the same `Shape.Option` stands for both.

```emel
type TodoPatch(title: Option[String], due: Option[String]) derive Decode
```

`{}` (change nothing) and `{"due": null}` (remove the due date) both become `TodoPatch(title: None, due: None)`.

Emela has no null (`[intro.premises]`). None means "no value", and inside an object "no value" corresponds to a missing key. Null belongs to the outside format, and only an application gives it a meaning. When it does, the meaning is written as the definition of data.

## Proposal

### Syntax

No change.

### Meaning

How the key of a field is written and read is decided by the Encode and Decode of the type of the field.

| Type of the field | Missing key | null | Value | Writing |
|---|---|---|---|---|
| `A` (other than the two below) | `Decode.from_dynamic(Null)` (an error for String) | the same | `from_dynamic` | The value |
| `Option[A]` | None | None | Some | None leaves the key out |
| `Presence[A]` | Absent | Null | Present | Absent leaves the key out, Null writes null |

The body of a creation:

```emel
type TodoBody(title: String, due: Option[String]) derive Encode, Decode
```

| JSON | Value |
|---|---|
| `{"title": "Shopping", "due": "2026-11-20"}` | `TodoBody(title: "Shopping", due: Some("2026-11-20"))` |
| `{"title": "Shopping", "due": null}` | `TodoBody(title: "Shopping", due: None)` |
| `{"title": "Shopping"}` | `TodoBody(title: "Shopping", due: None)` |
| `{"due": "2026-11-20"}` | `DecodeError(expected: "text", actual: Null, path: [Key("title")])` |

`TodoBody(title: "Shopping", due: None)` is written as `{"title": "Shopping"}`.

The body of a partial update:

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

| JSON | Value |
|---|---|
| `{}` | `TodoPatch(title: None, due: Absent)` |
| `{"title": "Milk"}` | `TodoPatch(title: Some("Milk"), due: Absent)` |
| `{"due": null}` | `TodoPatch(title: None, due: Null)` |
| `{"due": "2026-12-01"}` | `TodoPatch(title: None, due: Present("2026-12-01"))` |

`TodoPatch(title: None, due: Null)` is written as `{"due": null}`, and `TodoPatch(title: Some("Milk"), due: Absent)` as `{"title": "Milk"}`.

A key of an Option field that is misspelled by the sender (`"due_at"` for `"dueAt"`) is read as None, without an error, since unknown keys are ignored. A field whose type is not Option still fails, and the JSON Schema made from Shaped lets the sender check its keys.

#### The traits

```emel
pub trait Encode {
  fn to_dynamic(self) -> Dynamic
  fn to_field(self) -> Option[Dynamic] {
    Some(self.to_dynamic())
  }
}

pub trait Decode {
  fn from_dynamic(d: Dynamic) -> Self fails DecodeError
  fn from_dynamic_all(d: Dynamic) -> Self fails DecodeErrors { ... } # unchanged
  fn from_absent() -> Self fails DecodeError {
    Decode.from_dynamic(Null)
  }
}
```

- `to_field` is the form of a value written as a field of an object. None leaves the key out.
- `from_absent` is the value of a field whose key is missing. Its default reads the key as null, which gives the same result as "a missing key is read as Null" now.
- Option overrides only `to_field` (None for None). It keeps the default `from_absent`, which gives None.
- `Presence` overrides both.
- `to_field` and `from_absent` are used only for the fields of a type and of the named contents of a variant. The elements of a List, the values of a Map and positional contents use `to_dynamic` and `from_dynamic`, as now; there, Absent and None are both written as null.
- `Std.Dynamic.lookup` gives the value of a key, or None if the key is missing, for hand-written impls that need to tell the two apart.

#### Presence

```emel
pub enum Presence[A] { Absent, Null, Present(A) }
```

- It is in `Std.Dynamic.Presence`. `Std.Dynamic` has `Dynamic.Null`, and one module cannot have two variants of the same name (E0213).
- Encode: `to_dynamic` writes Absent and Null as null, and `Present(a)` as the `to_dynamic` of `a`. `to_field` gives None for Absent, `Some(Null)` for Null, and `Some(a.to_dynamic())` for `Present(a)`.
- Decode: `from_dynamic` reads null as Null and any other value as `Present(A.from_dynamic(d))`. `from_absent` is Absent. `from_dynamic_all` reads the value with `from_dynamic_all`.
- Shaped: `Shape.Option` of the shape of A.
- Eq, Ord and Show are derived. The order is Absent, Null, Present.

#### Shape

Shape does not change. `Shape.Option(s)` describes "a missing key, null, or s". Option and Presence both accept these three; Option never writes null, but what it writes is within what it accepts. A JSON Schema made from a Shape leaves the fields of `Shape.Option` out of `required`, and allows null for them.

### Typing rules

No change.

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

None.

## Compatibility

- The output changes: a derived Encode leaves out the key of a field whose value is None. Derivation of Encode was implemented in #589 for 0.28, which is not released yet. This proposal is implemented in the same version, so no released behavior changes.
- The methods added to the traits have default implementations, so hand-written impls keep compiling.
- A field whose type has a hand-written Encode that writes None as null is still written as null, since it does not go through the impl of Option.

## Implementation

- std (`std/dynamic.emel`): `to_field`, `from_absent`, `lookup`, and the `to_field` of Option
- std (`std/dynamic/presence.emel`): `Presence` and its impls
- emela-core (`lower/derive_std.rs`): read fields with `lookup` and `from_absent`, and write them with `to_field`
- x/json-schema: leave the fields of `Shape.Option` out of `required`

## Alternatives

- Deciding it with an attribute (`@optional` on a field, which alone may be left out): the same Option is read in three ways depending on the attribute, and the type does not show the form. Three states would need `@optional(exact)` together with `Option[Option[A]]` as well.
- Three states as `Option[Option[A]]`: one has to remember what `Some(None)` stands for. `Std.Dynamic.field` reads a missing key as null, so they could not even be told apart as it is.
- Requiring the keys of all declared fields, with Option reading only null as None: a misspelled key becomes an error, but a counterpart that leaves keys out can no longer be read, and 1 is not solved.
- Keep writing None as null: 2 remains. Emela has no null, so there is no reason to tie None to null.
- A variant for "missing" in `Dynamic`: Dynamic would have two kinds of null, and no function that reads a format (`Json.parse`) would make it.
- `Presence` in `Std.Dynamic` with other names for its variants (`Cleared`): the names would no longer describe the form of JSON, and std would decide the meaning that the application gives (removing).

## Prior art

- Separating whether a key may be missing from whether a value may be null, as two properties of a field: Effect Schema (`optional` and `NullOr` on property signatures), TypeScript (`a?: T` and `T | null`).
- A three-state value for missing, null and present: `JsonNullable` (jackson-databind-nullable), `Option<Option<T>>` with `double_option` in serde_with.
- Partial updates where a missing key leaves a member as it is and null removes it: JSON Merge Patch (RFC 7396).

## Open questions

## Decision
