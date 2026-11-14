# API of the bundled core and std `[api]`

This document defines the API of the `.emel` sources bundled with the compiler. Chapter 15 (`[lib]`) defines the scope of the standard library, how it is divided into layers, and the API conventions (`[lib.api-rules]`); this document defines the declarations, signatures, and behavior of the individual modules. If Chapter 15 and this document disagree, Chapter 15 takes precedence.

- core (`lib/core/`) is the Prelude and the modules that have `@intrinsic`. They have short names (`List`, `String`) and are usable without an import (`[lib.layers]`).
- std (`lib/std/`) is the modules under `Std`. They are used through an import, as in `import Std.Console.{Console, SystemConsole}`.
- The "Implementation" column of the tables is informative. `intrinsic` is a declaration without a body that uses the implementation of a built-in function. `runtime` is a type whose contents the runtime holds. `Emela` is a declaration whose body is written in the source.
- The subject is the first argument, so a function can be written with a pipe, as in `xs |> List.map(f)`.
- Argument names become the labels of named arguments (`[fn.args]`). A call can be written as `List.fold(xs, from: 0, f: add)`. Names follow the conventions of `[lib.api-rules]`.

## core `[api.core]`

### Prelude (`core/prelude.emel`) `[api.prelude]`

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Bool` | `enum Bool { False, True }` | Emela | A truth value. `False < True` by the derived Ord |
| `Option` | `enum Option[A] { Some(A), None }` | Emela | A value is present or absent |
| `Ordering` | `enum Ordering { Less, Equal, Greater }` | Emela | The result of a comparison |
| `panic` | `panic(message: String) -> Never` | intrinsic | Raises a defect |
| `todo` | `todo() -> Never` | intrinsic | A defect with `not yet implemented` |
| `dbg` | `dbg[A](value: A) -> A` | intrinsic | Writes the debug representation to standard error and returns the value. Does not require Show. For a type that satisfies Show, the same as `inspect`; for a type that does not, the constructed form (values inside that satisfy Show use Show, functions are `<fn>`) |
| `inspect` | `inspect[A: Show](value: A) -> String` | intrinsic | The debug representation. The result of the Show implementation; a String is enclosed in `"` even at the top level |
| `ParseError` | `error ParseError(input: String)` | Emela | A malformed input to `Int.parse`, `Float.parse`, or `Bool.parse`. `input` is the whole input |

impl (`[trait.derive]`): ParseError has `derive Show` (its contents are shown when it reaches main or a test; `[dyn.error]`).

### List (`core/list.emel`) `[api.list]`

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `map` | `map[A, B](xs: List[A], f: fn(A) -> B) -> List[B]` | Emela | Applies `f` to each element |
| `each` | `each[A](xs: List[A], f: fn(A) -> ()) -> ()` | Emela | Calls `f` on each element, from the first to the last. Stops at the first failure |
| `filter` | `filter[A](xs: List[A], f: fn(A) -> Bool) -> List[A]` | Emela | Keeps only the elements for which `f` is True |
| `fold` | `fold[A, B](xs: List[A], from: B, f: fn(B, A) -> B) -> B` | Emela | Folds from the front |
| `length` | `length[A](xs: List[A]) -> Int` | Emela | The number of elements |
| `is_empty` | `is_empty[A](xs: List[A]) -> Bool` | Emela | Whether the list is empty |
| `get` | `get[A](xs: List[A], index: Int) -> Option[A]` | Emela | The element at `index` (0-based). `None` if out of range or negative |
| `head` | `head[A](xs: List[A]) -> Option[A]` | Emela | The first element |
| `tail` | `tail[A](xs: List[A]) -> List[A]` | Emela | The rest after the first element. Empty if the list is empty |
| `reverse` | `reverse[A](xs: List[A]) -> List[A]` | Emela | Reverse order |
| `append` | `append[A](xs: List[A], ys: List[A]) -> List[A]` | Emela | Joins two lists |
| `flatten` | `flatten[A](xss: List[List[A]]) -> List[A]` | Emela | Flattens a list of lists |
| `join` | `join(xs: List[String], sep: String) -> String` | intrinsic | Joins with `sep` in between. Formerly `String.join` |
| `partition` | `partition[A](xs: List[A], f: fn(A) -> Bool) -> (List[A], List[A])` | Emela | Splits into the True elements and the False elements |
| `any` | `any[A](xs: List[A], f: fn(A) -> Bool) -> Bool` | Emela | Whether some element gives True (stops when one is found) |
| `all` | `all[A](xs: List[A], f: fn(A) -> Bool) -> Bool` | Emela | Whether all give True (stops at False) |
| `find` | `find[A](xs: List[A], f: fn(A) -> Bool) -> Option[A]` | Emela | The first element that gives True |
| `range` | `range(from: Int, to: Int) -> List[Int]` | Emela | From `from` inclusive to `to` exclusive |
| `zip` | `zip[A, B](xs: List[A], ys: List[B]) -> List[(A, B)]` | Emela | Pairs of elements at the same position. Truncated to the shorter list |
| `unzip` | `unzip[A, B](xys: List[(A, B)]) -> (List[A], List[B])` | Emela | The sequences of the first and second components |
| `enumerate` | `enumerate[A](xs: List[A]) -> List[(Int, A)]` | Emela | Pairs with an index starting from 0 |
| `take` | `take[A](xs: List[A], n: Int) -> List[A]` | Emela | The first `n` elements. All of them if there are fewer; empty if `n` is not positive |
| `drop` | `drop[A](xs: List[A], n: Int) -> List[A]` | Emela | The rest after the first `n` elements. All of them if `n` is not positive |
| `last` | `last[A](xs: List[A]) -> Option[A]` | Emela | The last element |
| `find_index` | `find_index[A](xs: List[A], f: fn(A) -> Bool) -> Option[Int]` | Emela | The index of the first element that gives True |
| `flat_map` | `flat_map[A, B](xs: List[A], f: fn(A) -> List[B]) -> List[B]` | Emela | Joins the results of `f` for each element in order |
| `filter_map` | `filter_map[A, B](xs: List[A], f: fn(A) -> Option[B]) -> List[B]` | Emela | The values for which `f` returned Some |
| `sort_by` | `sort_by[A](xs: List[A], f: fn(A, A) -> Ordering) -> List[A]` | Emela | Sorts in the order of `f`. A stable merge sort |
| `sort` | `sort[A: Ord](xs: List[A]) -> List[A]` | Emela | Sorts in the Ord order (`sort_by(xs, Ord.compare)`). Stable |

impl (`[trait.builtin]`): `impl List[A: Eq]: Eq`, `impl List[A: Ord]: Ord`, `impl List[A: Show]: Show`. Ord is lexicographic (if the common prefix is equal, the shorter list is smaller), and Show is `[1, 2, 3]`. The impls for basic types and tuples are held by the compiler.

### Option (`core/option.emel`) `[api.option]`

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `map` | `map[A, B](opt: Option[A], f: fn(A) -> B) -> Option[B]` | Emela | Applies `f` to the contents |
| `and_then` | `and_then[A, B](opt: Option[A], f: fn(A) -> Option[B]) -> Option[B]` | Emela | Applies `f`, which returns an Option, and flattens |
| `or` | `or[A](opt: Option[A], default: A) -> A` | Emela | The contents, or `default` if `None` |
| `is_some` | `is_some[A](opt: Option[A]) -> Bool` | Emela | Whether it is `Some` |
| `is_none` | `is_none[A](opt: Option[A]) -> Bool` | Emela | Whether it is `None` |
| `or_fail` | `or_fail[A, E](opt: Option[A], err: E) -> A fails E` | Emela | The contents, or fails with `err` if `None` (`[error.fail]`) |

impl (`[trait.builtin]`): `impl Option[A: Eq]: Eq`, `impl Option[A: Ord]: Ord`, `impl Option[A: Show]: Show`. Ord is `None < Some(_)`, and Show is `Some(1)`, `None`.

### String (`core/string.emel`) `[api.string]`

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `length` | `length(s: String) -> Int` | intrinsic | The number of grapheme clusters |
| `byte_size` | `byte_size(s: String) -> Int` | intrinsic | The number of UTF-8 bytes |
| `append` | `append(a: String, b: String) -> String` | intrinsic | Joins two strings |
| `concat` | `concat(parts: List[String]) -> String` | Emela | Joins all of them in order (`List.join(parts, "")`) |
| `contains` | `contains(s: String, sub: String) -> Bool` | intrinsic | Whether it contains `sub` |
| `starts_with` | `starts_with(s: String, prefix: String) -> Bool` | intrinsic | Whether it starts with `prefix` |
| `ends_with` | `ends_with(s: String, suffix: String) -> Bool` | intrinsic | Whether it ends with `suffix` |
| `split` | `split(s: String, sep: String) -> List[String]` | intrinsic | Splits at `sep`. By grapheme cluster if `sep` is empty |
| `chars` | `chars(s: String) -> List[String]` | intrinsic | A list of the grapheme clusters |
| `join` | `join(parts: List[String], sep: String) -> String` | Emela | `@deprecated`, remains for one minor version. `List.join(parts, sep)` |
| `trim` | `trim(s: String) -> String` | intrinsic | Removes leading and trailing White_Space |
| `code_points` | `code_points(s: String) -> List[Int]` | intrinsic | A list of the code point values |
| `from_code_point` | `from_code_point(n: Int) -> Option[String]` | intrinsic | `None` if not a scalar value |
| `to_ascii_lowercase` | `to_ascii_lowercase(s: String) -> String` | Emela | Converts only A to Z into a to z |
| `to_ascii_uppercase` | `to_ascii_uppercase(s: String) -> String` | Emela | Converts only a to z into A to Z |
| `show_nested` | `show_nested[A: Show](value: A) -> String` | intrinsic | `@internal`. The representation of a value placed inside another value (`[trait.builtin]`). A String is enclosed in `"`, and anything else uses `Show.show`. Used by the Show impls in core |

- `split(s, "")` splits by grapheme cluster (the same result as `chars(s)`). `split("", "")` is `[]`. With a non-empty separator, `split("", ",")` is `[""]`. In both cases, `List.join(String.split(s, sep), sep) == s` holds.
- `contains`, `starts_with`, `ends_with`, and `split` match on the sequence of code points and do not look at grapheme cluster boundaries (`String.contains("e\u{301}", "e")` is True).
- In `code_points`, one grapheme cluster can produce several values. `from_code_point(n)` returns None if n is not a Unicode scalar value (0 to 0x10FFFF inclusive, excluding the surrogates 0xD800 to 0xDFFF).
- The whitespace that `trim` removes is the 25 characters with the Unicode White_Space property (U+0009 to U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000 to U+200A, U+2028, U+2029, U+202F, U+205F, U+3000). U+FEFF and U+200B are not whitespace. This table is fixed and does not depend on the host's Unicode version.

### Bool (`core/bool.emel`) `[api.bool]`

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `to_string` | `to_string(b: Bool) -> String` | Emela | `"True"` / `"False"`, the same as Show |
| `parse` | `parse(s: String) -> Bool fails ParseError` | Emela | Accepts only `"True"` / `"False"`. Different case, whitespace, and `0` / `1` are `ParseError` |

### Int (`core/int.emel`) `[api.int]`

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `to_string` | `to_string(n: Int) -> String` | intrinsic | A decimal string |
| `parse` | `parse(s: String) -> Int fails ParseError` | Emela (intrinsic inside) | Decimal digits and a leading `-` or `+`. Anything else and out-of-range values are `ParseError` |
| `checked_add` | `checked_add(a: Int, b: Int) -> Option[Int]` | intrinsic | `None` on overflow |
| `bit_and` | `bit_and(a: Int, b: Int) -> Int` | intrinsic | Bitwise AND |
| `bit_or` | `bit_or(a: Int, b: Int) -> Int` | intrinsic | Bitwise OR |
| `bit_xor` | `bit_xor(a: Int, b: Int) -> Int` | intrinsic | Bitwise exclusive OR |
| `bit_not` | `bit_not(n: Int) -> Int` | intrinsic | Bitwise inversion |
| `shift_left` | `shift_left(n: Int, by: Int) -> Int` | intrinsic | Left shift. 0 if `by` is 32 or more; a defect if negative |
| `shift_right` | `shift_right(n: Int, by: Int) -> Int` | intrinsic | Arithmetic right shift. 0 or -1 if 32 or more |
| `shift_right_unsigned` | `shift_right_unsigned(n: Int, by: Int) -> Int` | intrinsic | Logical right shift. 0 if 32 or more |
| `min` / `max` | `(a: Int, b: Int) -> Int` | Emela | The smaller / the larger |
| `abs` | `abs(n: Int) -> Int` | Emela | Absolute value. The minimum value wraps around to the same value |

- `parse` treats whitespace, `_`, hexadecimal notation, and values outside the range of Int as ParseError.
- The shift amount is an Int even in the Int64 functions. A negative shift amount is a defect (`negative shift amount: -1`). If the shift amount is at least the width of the type (32 for Int, 64 for Int64), the result is the value with all bits shifted out: 0 for `shift_left` and `shift_right_unsigned`, and 0 or -1 depending on the sign for `shift_right`. The shift amount is not taken modulo the width (`Int.shift_left(1, 32)` is 0).
- The result of `shift_right_unsigned` is read as a signed value (`Int.shift_right_unsigned(-1, 0)` is -1).

### Int64 (`core/int64.emel`) `[api.int64]`

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `from_int` | `from_int(n: Int) -> Int64` | intrinsic | Creates from an Int |
| `to_string` | `to_string(n: Int64) -> String` | intrinsic | A decimal string. No suffix |
| `to_int` | `to_int(n: Int64) -> Int` | intrinsic | Wraps around to the low 32 bits |
| `bit_and` / `bit_or` / `bit_xor` | `(a: Int64, b: Int64) -> Int64` | intrinsic | Bitwise operations |
| `bit_not` | `bit_not(n: Int64) -> Int64` | intrinsic | Bitwise inversion |
| `shift_left` | `shift_left(n: Int64, by: Int) -> Int64` | intrinsic | Left shift. 0 if `by` is 64 or more; a defect if negative |
| `shift_right` | `shift_right(n: Int64, by: Int) -> Int64` | intrinsic | Arithmetic right shift. 0 or -1 if 64 or more |
| `shift_right_unsigned` | `shift_right_unsigned(n: Int64, by: Int) -> Int64` | intrinsic | Logical right shift. 0 if 64 or more |
| `min` / `max` | `(a: Int64, b: Int64) -> Int64` | Emela | The first argument if Equal |
| `abs` | `abs(n: Int64) -> Int64` | Emela | The minimum value wraps around to itself |

The shift rules are the same as for Int (`[api.int]`).

### Float (`core/float.emel`) `[api.float]`

A conversion to Int is a defect if the result is outside the range of Int or the value is NaN or ±Infinity.

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `from_int` | `from_int(n: Int) -> Float` | intrinsic | Converts to Float (every Int is represented exactly) |
| `parse` | `parse(s: String) -> Float fails ParseError` | Emela (intrinsic inside) | The forms that `to_string` writes (`1.5`, `1e+21`, `Infinity`, `NaN`, a leading sign). Anything else is `ParseError` |
| `to_string` | `to_string(x: Float) -> String` | intrinsic | The shortest number of digits that round-trips. `.0` is added to an integer value |
| `floor` | `floor(x: Float) -> Int` | intrinsic | Rounds toward negative infinity |
| `ceil` | `ceil(x: Float) -> Int` | intrinsic | Rounds toward positive infinity |
| `round` | `round(x: Float) -> Int` | intrinsic | The nearest integer. 0.5 rounds away from 0 |
| `truncate` | `truncate(x: Float) -> Int` | intrinsic | Rounds toward 0 |
| `is_nan` | `is_nan(x: Float) -> Bool` | Emela | Whether it is NaN |
| `compare` | `compare(a: Float, b: Float) -> Ordering` | Emela | NaN is greater than any other value; two NaNs are Equal, and ±0.0 are Equal to each other |
| `min` / `max` | `(a: Float, b: Float) -> Float` | Emela | Chooses according to compare. The first argument if Equal. If one is NaN, min returns the other and max returns NaN |
| `abs` | `abs(x: Float) -> Float` | Emela | Absolute value. -0.0 becomes +0.0, and NaN stays as is |

- `floor`, `ceil`, `round`, and `truncate` are a defect if the rounded result is outside the range of Int, or if the value is NaN or ±Infinity. They neither wrap around nor saturate.
- `round` rounds 0.5 away from 0 (`Float.round(2.5)` is 3, `Float.round(-2.5)` is -3).
- `parse` reads digits, `.` and a fractional part, `e` and an exponent, a leading sign, `Infinity`, `-Infinity`, and `NaN`. `1.`, `.5`, and `inf` are ParseError.

## std `[api.std]`

### Std.Duration (`std/duration.emel`) `[api.std.duration]`

A length of time. An opaque type whose contents are an Int64 of milliseconds (`[lib.layers]`). Overflow wraps around by the Int64 rules.

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `milliseconds` | `milliseconds(n: Int64) -> Duration` | Emela | `n` milliseconds |
| `seconds` | `seconds(n: Int64) -> Duration` | Emela | `n` seconds |
| `minutes` | `minutes(n: Int64) -> Duration` | Emela | `n` minutes |
| `hours` | `hours(n: Int64) -> Duration` | Emela | `n` hours |
| `to_milliseconds` | `to_milliseconds(d: Duration) -> Int64` | Emela | The number of milliseconds |
| `to_seconds` | `to_seconds(d: Duration) -> Int64` | Emela | The number of seconds. The fraction is truncated toward 0 |
| `add` | `add(d: Duration, other: Duration) -> Duration` | Emela | The sum |
| `subtract` | `subtract(d: Duration, other: Duration) -> Duration` | Emela | The difference. Can be negative |
| `to_string` | `to_string(d: Duration) -> String` | Emela | `1500ms` |

impl (`[trait.derive]`, `[trait.builtin]`): `derive Eq, Ord`. The Show impl gives `1500ms`, the same as `to_string`.

### Std.Instant (`std/instant.emel`) `[api.std.instant]`

A point in time. An opaque type whose contents are an Int64 of milliseconds since the Unix epoch (`[lib.layers]`). The current point in time is returned by the Clock effect (this module cannot obtain it).

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `from_unix_milliseconds` | `from_unix_milliseconds(ms: Int64) -> Instant` | Emela | The point `ms` milliseconds after the Unix epoch |
| `to_unix_milliseconds` | `to_unix_milliseconds(t: Instant) -> Int64` | Emela | Milliseconds since the Unix epoch |
| `add` | `add(t: Instant, d: Duration) -> Instant` | Emela | `d` later |
| `subtract` | `subtract(t: Instant, d: Duration) -> Instant` | Emela | `d` earlier |
| `since` | `since(t: Instant, earlier: Instant) -> Duration` | Emela | The length from `earlier` to `t` |
| `to_string` | `to_string(t: Instant) -> String` | Emela | ISO 8601 in UTC (`2025-10-12T03:04:05.678Z`) |

- If the year is outside 0 to 9999, `to_string` writes the year as a sign and six digits (`+010000-01-01T00:00:00.000Z`, `-000001-12-31T00:00:00.000Z`). Dates are counted in the proleptic Gregorian calendar. Every value of Instant is written by this rule, without a range limit.

impl (`[trait.derive]`, `[trait.builtin]`): `derive Eq, Ord`. The Show impl is the same as `to_string`.

### Std.Bytes (`std/bytes.emel`) `[api.std.bytes]`

The value layer (`[lib.layers]`). An immutable sequence of bytes. Each byte is an `Int` from 0 to 255.

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Bytes` | `opaque type Bytes` | intrinsic | A sequence of bytes |
| `empty` | `empty() -> Bytes` | Emela | The sequence of no bytes |
| `from_list` | `from_list(xs: List[Int]) -> Option[Bytes]` | intrinsic | The bytes in the order of `xs`. `None` if an element is outside 0 to 255 |
| `to_list` | `to_list(b: Bytes) -> List[Int]` | intrinsic | The bytes in order |
| `size` | `size(b: Bytes) -> Int` | intrinsic | The number of bytes |
| `is_empty` | `is_empty(b: Bytes) -> Bool` | Emela | Whether there are no bytes |
| `get` | `get(b: Bytes, index: Int) -> Option[Int]` | intrinsic | The byte at `index` (0-based). `None` if out of range or negative |
| `slice` | `slice(b: Bytes, from: Int, to: Int) -> Bytes` | intrinsic | The bytes from `from` inclusive to `to` exclusive. Both are clamped to 0 to `size(b)`; empty if `from` is not less than `to` |
| `append` | `append(a: Bytes, b: Bytes) -> Bytes` | Emela | The bytes of `a`, then those of `b` |
| `concat` | `concat(parts: List[Bytes]) -> Bytes` | intrinsic | The bytes of all of them in order |
| `encode_utf8` | `encode_utf8(s: String) -> Bytes` | intrinsic | The UTF-8 encoding of `s`. No BOM is added |
| `decode_utf8` | `decode_utf8(b: Bytes) -> Option[String]` | Emela | The string that `b` encodes in UTF-8. `None` if `b` is not valid UTF-8. A leading BOM is kept as U+FEFF |
| `to_hex` | `to_hex(b: Bytes) -> String` | Emela | Two lowercase hexadecimal digits per byte, with no separator (`"89504e47"`) |
| `from_hex` | `from_hex(s: String) -> Option[Bytes]` | Emela | The inverse of `to_hex`. Accepts uppercase and lowercase digits. `None` for an odd number of digits or any other character, including whitespace |
| `hex` | `hex(s: String) -> Bytes` | Emela | The bytes that `from_hex` reads from `s`. A defect if `from_hex` returns `None`. For constants written in the program (`const PNG_SIGNATURE = Bytes.hex("89504e47")`) |
| `to_base64` | `to_base64(b: Bytes) -> String` | Emela | Base64 in the standard alphabet (`A`–`Z`, `a`–`z`, `0`–`9`, `+`, `/`) of RFC 4648 section 4, padded with `=` to a multiple of 4 characters |
| `from_base64` | `from_base64(s: String) -> Option[Bytes]` | Emela | The inverse of `to_base64`. The padding is required. `None` for a length that is not a multiple of 4, misplaced padding, or any other character, including whitespace |

- The defect message of `hex` is ``Bytes.hex was given a string that is not hexadecimal: "<s>"``, where `<s>` is shown as by inspect. In a const, the name of the const is prepended (`[data.const]`).
- `size(encode_utf8(s)) == String.byte_size(s)`, and `decode_utf8(encode_utf8(s)) == Some(s)`, for every String `s`.
- Invalid UTF-8 is not replaced with the replacement character: `decode_utf8` returns `None`, so text that is read and written back is not changed silently. A sequence that encodes a surrogate (U+D800 to U+DFFF) or a value above U+10FFFF, and an overlong encoding, are not valid UTF-8.
- `size`, `get` and `is_empty` are O(1). `slice` is O(`to - from`) or less. `append` and `concat` are O(n) in the number of bytes of the result. `to_list`, `from_list`, `encode_utf8`, `decode_utf8`, `to_hex`, `from_hex`, `to_base64` and `from_base64` are O(n).

impl: `impl Bytes: Eq`, `impl Bytes: Ord`, `impl Bytes: Show`. Two Bytes are equal when they have the same bytes in the same order. Ord is lexicographic on the bytes as unsigned values (if the common prefix is equal, the shorter one is smaller). Show is the source form `Bytes.hex("89504e47")`, with the digits of `to_hex`, so the output can be pasted back as an expression. A Bytes inside another value is displayed the same way (`[Bytes.hex("00")]`).

### Std.Array (`std/array.emel`) `[api.std.array]`

The value layer (`[lib.layers]`). An immutable sequence of fixed length, read by position. Positions are 0-based.

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Array` | `opaque type Array[A]` | intrinsic | A sequence of values of type `A` |
| `empty` | `empty[A]() -> Array[A]` | Emela | The array of no elements |
| `from_list` | `from_list[A](xs: List[A]) -> Array[A]` | intrinsic | The elements in the order of `xs` |
| `to_list` | `to_list[A](a: Array[A]) -> List[A]` | intrinsic | The elements in order |
| `repeat` | `repeat[A](value: A, count: Int) -> Array[A]` | Emela | `count` copies of `value`. Empty if `count` is not positive |
| `size` | `size[A](a: Array[A]) -> Int` | intrinsic | The number of elements |
| `is_empty` | `is_empty[A](a: Array[A]) -> Bool` | Emela | Whether there are no elements |
| `get` | `get[A](a: Array[A], index: Int) -> Option[A]` | intrinsic | The element at `index`. `None` if out of range or negative |
| `set` | `set[A](a: Array[A], index: Int, value: A) -> Array[A]` | intrinsic | The elements of `a` with the one at `index` replaced by `value`. A defect if `index` is out of range or negative |
| `slice` | `slice[A](a: Array[A], from: Int, to: Int) -> Array[A]` | intrinsic | The elements from `from` inclusive to `to` exclusive. Both are clamped to 0 to `size(a)`; empty if `from` is not less than `to` |
| `append` | `append[A](a: Array[A], b: Array[A]) -> Array[A]` | Emela | The elements of `a`, then those of `b` |
| `concat` | `concat[A](parts: List[Array[A]]) -> Array[A]` | intrinsic | The elements of all of them in order |
| `reverse` | `reverse[A](a: Array[A]) -> Array[A]` | Emela | The elements in reverse order |
| `map` | `map[A, B](a: Array[A], f: fn(A) -> B) -> Array[B]` | Emela | Applies `f` to each element, from the first |
| `each` | `each[A](a: Array[A], f: fn(A) -> ()) -> ()` | Emela | Calls `f` on each element, from the first. Stops at the first failure |
| `fold` | `fold[A, B](a: Array[A], from: B, f: fn(B, A) -> B) -> B` | Emela | Folds from the first element |
| `find_index` | `find_index[A](a: Array[A], f: fn(A) -> Bool) -> Option[Int]` | Emela | The position of the first element that gives True |
| `sort_by` | `sort_by[A](a: Array[A], f: fn(A, A) -> Ordering) -> Array[A]` | Emela | Sorts in the order of `f`. Stable, with the same result as `List.sort_by` |
| `sort` | `sort[A: Ord](a: Array[A]) -> Array[A]` | Emela | Sorts in the Ord order (`sort_by(a, Ord.compare)`). Stable |
| `lower_bound` | `lower_bound[A: Ord](a: Array[A], value: A) -> Int` | Emela | In an array sorted in the Ord order, the smallest position whose element is not less than `value`: the position where `value` would be inserted to keep the order. `size(a)` if every element is less |
| `binary_search` | `binary_search[A: Ord](a: Array[A], value: A) -> Option[Int]` | Emela | In an array sorted in the Ord order, the smallest position whose element is equal to `value`. `None` if there is none |

- The defect message of `set` is ``Array.set was given position <index> in an array of size <size>``.
- `lower_bound` bisects as follows, so its result is the same on every target even when `a` is not sorted. Start with `low = 0` and `high = size(a)`. While `low < high`, let `mid = (low + high) / 2`; if the element at `mid` is less than `value`, set `low = mid + 1`, otherwise set `high = mid`. The result is `low`. It compares O(log n) times.
- `binary_search(a, value)` is `Some(i)` where `i = lower_bound(a, value)`, if `i < size(a)` and the element at `i` is equal to `value`, and `None` otherwise.
- `size`, `get` and `is_empty` are O(1). `set` is O(n), where n is `size(a)`, except as below. `slice` is O(`to - from`) or less. `append`, `concat`, `repeat`, `from_list`, `to_list`, `reverse`, `map`, `fold` and `find_index` are O(n) in the number of elements of the result or of the argument, and `sort` and `sort_by` are O(n log n).
- On the WASM and native targets, `set` MUST be O(1) when the argument `a` is a variable (a parameter or a local binding) that is not used after the call on any path, and no other variable or value that is still in use refers to the same array (a closure that captured it, a tuple, a record or a list that holds it, the state of State, or another variable bound to it). In other cases, a target MAY still update in place when it can determine that the array is not shared. The JS target always copies. The result is the same in every case.

impl: `impl Array[A: Eq]: Eq`, `impl Array[A: Ord]: Ord`, `impl Array[A: Show]: Show`. Two arrays are equal when they have equal elements in the same order. Ord is lexicographic (if the common prefix is equal, the shorter one is smaller). Show is the source form `Array.from_list([1, 2, 3])`, so the output can be pasted back as an expression. Values inside are displayed as in `[trait.builtin]` (a String in `"`). An array inside another value is displayed the same way (`[Array.from_list([1])]`).

### Std.Map (`std/map.emel`) `[api.std.map]`

A mapping from keys to values. Immutable and ordered by key (the Ord order) (`[lib.layers]`, `[open.list]` #36). An opaque type whose contents are a weight-balanced tree.

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `empty` | `empty[K, V]() -> Map[K, V]` | Emela | An empty Map |
| `from_list` | `from_list[K: Ord, V](pairs: List[(K, V)]) -> Map[K, V]` | Emela | For a repeated key, the later value |
| `insert` | `insert[K: Ord, V](m: Map[K, V], key: K, value: V) -> Map[K, V]` | Emela | Replaces an existing value |
| `remove` | `remove[K: Ord, V](m: Map[K, V], key: K) -> Map[K, V]` | Emela | Unchanged if the key is absent |
| `update` | `update[K: Ord, V](m: Map[K, V], key: K, f: fn(Option[V]) -> Option[V]) -> Map[K, V]` | Emela | Passes the current value to `f`. Removes the key on None |
| `get` | `get[K: Ord, V](m: Map[K, V], key: K) -> Option[V]` | Emela | The value of the key |
| `contains` | `contains[K: Ord, V](m: Map[K, V], key: K) -> Bool` | Emela | Whether the key is present |
| `size` | `size[K, V](m: Map[K, V]) -> Int` | Emela | The number of keys |
| `is_empty` | `is_empty[K, V](m: Map[K, V]) -> Bool` | Emela | Whether the map is empty |
| `fold` | `fold[K, V, B](m: Map[K, V], from: B, f: fn(B, K, V) -> B) -> B` | Emela | Folds in key order |
| `each` | `each[K, V](m: Map[K, V], f: fn(K, V) -> ()) -> ()` | Emela | Calls `f` on each pair in key order. Stops at the first failure |
| `to_list` | `to_list[K, V](m: Map[K, V]) -> List[(K, V)]` | Emela | The pairs in key order |
| `keys` / `values` | `(m: Map[K, V]) -> List[K]` / `List[V]` | Emela | In key order |
| `map_values` | `map_values[K, V, W](m: Map[K, V], f: fn(V) -> W) -> Map[K, W]` | Emela | Applies `f` to the values |
| `filter` | `filter[K, V](m: Map[K, V], f: fn(K, V) -> Bool) -> Map[K, V]` | Emela | The pairs for which `f` is True |
| `union` | `union[K: Ord, V](a: Map[K, V], b: Map[K, V]) -> Map[K, V]` | Emela | For a key in both, the value from `a` |

impl (`[trait.builtin]`): `impl Map[K: Eq, V: Eq]: Eq` (compares by contents), `impl Map[K: Show, V: Show]: Show` (`Map.from_list([(1, "a")])`).

### Std.Set (`std/set.emel`) `[api.std.set]`

A collection of values. Immutable and in Ord order. The contents are a `Map[A, ()]`.

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `empty` | `empty[A]() -> Set[A]` | Emela | An empty Set |
| `from_list` | `from_list[A: Ord](xs: List[A]) -> Set[A]` | Emela | Duplicates become one |
| `insert` / `remove` | `(s: Set[A], x: A) -> Set[A]` (`A: Ord`) | Emela | Adds / removes |
| `contains` | `contains[A: Ord](s: Set[A], x: A) -> Bool` | Emela | Whether it contains the value |
| `size` / `is_empty` | `(s: Set[A]) -> Int` / `Bool` | Emela | The number of elements / whether it is empty |
| `fold` | `fold[A, B](s: Set[A], from: B, f: fn(B, A) -> B) -> B` | Emela | Folds in order |
| `each` | `each[A](s: Set[A], f: fn(A) -> ()) -> ()` | Emela | Calls `f` on each element in order. Stops at the first failure |
| `to_list` | `to_list[A](s: Set[A]) -> List[A]` | Emela | Lists in order |
| `filter` | `filter[A](s: Set[A], f: fn(A) -> Bool) -> Set[A]` | Emela | The elements for which `f` is True |
| `union` | `union[A: Ord](a: Set[A], b: Set[A]) -> Set[A]` | Emela | The union |

impl (`[trait.builtin]`): `impl Set[A: Eq]: Eq`, `impl Set[A: Show]: Show` (`Set.from_list([1, 2])`).

### Std.Dynamic (`std/dynamic.emel`) `[api.std.dynamic]`

| Declaration | Contents | Description |
|---|---|---|
| `Dynamic` | `enum Dynamic { Null, Bool(Bool), Number(Number), Text(String), Items(List[Dynamic]), Fields(Map[String, Dynamic]), Binary(Bytes) }` | A value in no particular format |
| `Segment` | `enum Segment { Key(String), Index(Int) }` | A step of a path into a Dynamic |
| `DecodeError` | `error DecodeError(expected: String, actual: Dynamic, path: List[Segment])` | Show writes the kind of `actual`, not its contents |
| `Encode` | `trait Encode { fn to_dynamic(self) -> Dynamic }` | Derivable |
| `Decode` | `trait Decode { fn from_dynamic(d: Dynamic) -> Self fails DecodeError; fn from_dynamic_all(d: Dynamic) -> Self fails DecodeErrors }` | Derivable, except for opaque types. `from_dynamic` stops at the first failure. `from_dynamic_all` reports every failure; its default fails with the one failure of `from_dynamic` |
| `DecodeErrors` | `error DecodeErrors(errors: List[DecodeError])` | Every failure of `from_dynamic_all`, in the order the parts were read. Never empty |
| `Attempt` | `enum Attempt[A] { Decoded(A), Failed(List[DecodeError]) }` | The result of reading one part while collecting failures |
| `attempt`, `errors_of` | `attempt[A](f: fn() -> A fails DecodeErrors) -> Attempt[A]`, `errors_of[A](a: Attempt[A]) -> List[DecodeError]` | Keep the failures of `f` instead of failing, and read them back (empty for Decoded) |
| `field_all`, `index_all` | `field_all[A](value: Dynamic, name: String, decoder) -> A fails DecodeErrors`, `index_all[A](n: Int, decoder) -> decoder` | `field` and `index` for a decoder that fails with DecodeErrors; the key or the position is added in front of the path of every failure |
| `ObjectKey` | `trait ObjectKey: Ord { fn to_key(self) -> String; fn from_key(key: String) -> Self fails DecodeError }` | The keys of a Map written as Fields |
| `string`, `bytes`, `bool`, `int`, `int64`, `float` | `fn(value: Dynamic) -> A fails DecodeError` | Read a value of a basic type. `bytes` reads only Binary. `int` and `int64` accept a number whose value is an integer (`1.0`, `1e3`) |
| `list`, `optional`, `dict` | `fn(decoder) -> decoder` | Read Items, Null or a value, and Fields with each value read by `decoder` |
| `index` | `index[A](n: Int, decoder) -> decoder` | Read the element `n` of Items. Fewer elements is a DecodeError |
| `items` | `items(value: Dynamic, n: Int) -> List[Dynamic] fails DecodeError` | The elements of Items that has exactly `n` elements. Any other number is `DecodeError(expected: "n items", ...)` |
| `field` | `field[A](value: Dynamic, name: String, decoder) -> A fails DecodeError` | Read the key `name` of Fields. A missing key is read as Null |
| `tag` | `tag(value: Dynamic) -> String fails DecodeError` | The `"tag"` of an enum written by Encode |
| `object`, `variant` | `object(fields: List[(String, Dynamic)]) -> Dynamic`, `variant(tag: String, value: Option[Dynamic]) -> Dynamic` | Build Fields, and the form of an enum |
| `kind` | `kind(value: Dynamic) -> String` | `"null"`, `"bool"`, `"number"`, `"text"`, `"items"`, `"fields"` or `"binary"` |

impl: Bool, Int, Int64, Float, String, Bytes, `List[A]`, `Array[A]`, `Set[A]`, `Option[A]` and `Map[K: ObjectKey, V]` have Encode and Decode; String has ObjectKey. Bytes is encoded as Binary, and its Decode reads only Binary. `Array[A]` and `Set[A]` are encoded as Items in their order, and decoded from Items; the Decode of `Set[A]` drops repeated elements. The Decode of `List[A]`, `Array[A]`, `Set[A]` and `Map[K, V]` defines `from_dynamic_all` to read every element or value and collect their failures, and that of `Option[A]` to read its value with `from_dynamic_all`. A Float that is NaN or ±Infinity is encoded as Null. None is encoded as Null, and Null decodes as None.

### Std.Dynamic.Number (`std/dynamic/number.emel`) `[api.std.dynamic.number]`

| Declaration | Contents | Description |
|---|---|---|
| `Number` | `opaque type Number` | A decimal number of any size and precision: a sign, digits and an exponent. Two numbers that denote the same value are equal (`1.0` and `1e0`) |
| `OutOfRange` | `error OutOfRange(number: Number)` | A number that the target type cannot hold |
| `parse` | `parse(s: String) -> Number fails ParseError` | Read a number in the JSON grammar. Fails also when the exponent is outside the range of Int |
| `from_int`, `from_int64` | `from_int(n: Int) -> Number`, `from_int64(n: Int64) -> Number` | Exact |
| `from_float` | `from_float(f: Float) -> Option[Number]` | None for NaN and ±Infinity |
| `to_int`, `to_int64` | `to_int(n: Number) -> Int fails OutOfRange`, `to_int64(n: Number) -> Int64 fails OutOfRange` | Fails for a number that is not an integer or is outside the range |
| `to_float` | `to_float(n: Number) -> Float` | The nearest Float |
| `to_string` | `to_string(n: Number) -> String` | In the JSON grammar. An integer of up to 21 digits is written in full, a short fraction with a point, the rest with an exponent |

### Std.Shape (`std/shape.emel`) `[api.std.shape]`

| Declaration | Contents | Description |
|---|---|---|
| `Shape` | `enum Shape { Bool, Int, Int64, Float, String, Bytes, Any, List(Shape), Option(Shape), Map(key: Shape, value: Shape), Record(name: String, fields: List[Field]), Variants(name: String, variants: List[Variant]), Ref(String), Named(name: String, shape: Shape), Annotated(shape: Shape, notes: List[(String, Dynamic)]) }` | The shape of a type, without a value. The variants are named after the types they describe and are written with the module (`Shape.Int`, `Shape.List(Shape.String)`). `Any` is the shape of Dynamic. `Ref` is a record or enum that appears again inside itself. `Named` is a type known by its name with the shape of its contents (an opaque type). `Annotated` carries notes; a reader passes on the keys it does not know |
| `Field` | `type Field(name: String, shape: Shape)` | A field of a record or of named contents |
| `Variant` | `type Variant(name: String, contents: Contents)` | A variant of an enum |
| `Contents` | `enum Contents { NoFields, PositionalFields(List[Shape]), NamedFields(List[Field]) }` | The contents of a variant, in the form they are declared in |
| `Shaped` | `trait Shaped { fn shape_in(proxy: Option[Self], visiting: List[String]) -> Shape; fn shape(proxy: Option[Self]) -> Shape }` | Derivable. `proxy` only carries the type. `visiting` holds the names of the records and enums being read around this one, innermost first. `shape` calls `shape_in` with an empty `visiting` |
| `visit` | `visit(name: String, visiting: List[String], build: fn(List[String]) -> Shape) -> Shape` | `Ref(name)` if `name` is in `visiting`, otherwise `build` with `name` added in front |
| `of_value`, `of_parameter` | `of_value[A: Shaped](value: A) -> Shape`, `of_parameter[A: Shaped, B, E, R](f: fn(A) -> B fails E uses { R }) -> Shape` | The shape of the type of a value, and of the parameter of a function without calling it |
| `annotate` | `annotate(shape: Shape, name: String, value: Dynamic) -> Shape` | Add the note `name` after the notes of `shape`, replacing a note of the same name |
| `description`, `pattern`, `format` | `fn(shape: Shape, text: String) -> Shape` | The notes `"description"`, `"pattern"` (a regular expression) and `"format"` (`"date-time"`, `"email"`) |
| `minimum`, `maximum` | `fn(shape: Shape, n: Int) -> Shape` | The notes `"minimum"` and `"maximum"`: the least and the greatest number allowed |
| `min_length`, `max_length` | `fn(shape: Shape, n: Int) -> Shape` | The notes `"min_length"` and `"max_length"`: the length of a string, counted in code points |
| `min_items`, `max_items` | `fn(shape: Shape, n: Int) -> Shape` | The notes `"min_items"` and `"max_items"`: the number of items of a list |
| `strip` | `strip(shape: Shape) -> Shape` | `shape` without its notes and names |
| `note` | `note(shape: Shape, name: String) -> Option[Dynamic]` | The value of the note `name`, looking through names |
| `field_names` | `field_names(shape: Shape) -> List[String]` | The names of the fields of a record in declaration order, looking through notes and names; empty for any other shape |
| `name_of` | `name_of(shape: Shape) -> String` | The name of the type as it is written (`Int`, `List[String]`, `Page[Member]`) |
| `generic_name` | `generic_name(name: String, arguments: List[Shape]) -> String` | `name` followed by the names of `arguments` in brackets, or `name` alone without arguments |

impl: Bool, Int, Int64, Float, String, Bytes, Dynamic, `List[A]`, `Array[A]`, `Set[A]`, `Option[A]` and `Map[K, V]` have Shaped. The shape of Bytes is `Bytes`, and the shapes of `Array[A]` and `Set[A]` are `List` of the shape of `A`. The impls of `List`, `Array`, `Set`, `Option` and `Map` pass `visiting` on to their type arguments.

The notes defined by `Std.Shape` are `"description"`, `"pattern"`, `"format"`, `"minimum"`, `"maximum"`, `"min_length"`, `"max_length"`, `"min_items"` and `"max_items"`, each with the value its function above writes. Notes are only described: the notes do not change which values a shape describes, and nothing in `Std.Shape` checks them. Other notes may be added with `annotate`.

### Std.Json (`std/json.emel`) `[api.std.json]`

| Declaration | Contents | Description |
|---|---|---|
| `SyntaxError` | `error SyntaxError(message: String, offset: Int)` | Text that is not JSON. `offset` counts code points from the start of the input |
| `parse` | `parse(s: String) -> Dynamic fails SyntaxError` | Read one JSON value (RFC 8259), with whitespace allowed around it. Numbers are read with `Std.Dynamic.Number.parse`. For a key repeated in an object, the later value is kept. Never gives Binary |
| `to_string` | `to_string(value: Dynamic) -> String` | Write `value` as JSON without whitespace. Keys come in the order of the Map. A string escapes `"`, `\` and the control characters, and keeps every other character as it is. Binary is written as a string of `Bytes.to_base64` |
| `decode` | `decode[A: Decode](text: String) -> A fails SyntaxError \| DecodeError` | `parse`, then `Decode.from_dynamic` |
| `decode_all` | `decode_all[A: Decode](text: String) -> A fails SyntaxError \| DecodeErrors` | `parse`, then `Decode.from_dynamic_all` |
| `encode` | `encode[A: Encode](value: A) -> String` | `Encode.to_dynamic`, then `to_string` |

### Std.State (`std/state.emel`) `[api.std.state]`

The mutable state of `[effect.state]`. `run` is a handler provided by the runtime.

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `State` | `effect State[S] { fn get() -> S; fn set(value: S) -> (); fn update(f: fn(S) -> S) -> () }` | Emela | Reads and writes a state of type `S` |
| `run` | `run[S, A, E, R](initial: S, body: fn() -> A fails E uses { State[S], R }) -> (A, S) fails E uses { R }` | runtime | Runs `body` with a state whose initial value is `initial`, and returns the value and the final state. On failure, the state is discarded |

```emel
import Std.State.{State}

fn count() -> Int uses { State[Int] } {
  st = use State[Int]
  st.update(|n| n + 1)
  st.get()
}

fn main() {
  (n, last) = State.run(0, || count())
}
```

### Std.Console (`std/console.emel`, `std/console.c`) `[api.std.console]`

The platform layer and the test layer (`[lib.layers]`).

| Declaration | Contents | Description |
|---|---|---|
| `Console` | `effect Console { fn print_line(text: String) -> (); fn print_error_line(text: String) -> () }` | Writes one line at a time to standard output and standard error. Does not fail |
| `SystemConsole` | `handler SystemConsole: Console` | The host's standard output and standard error (`console.log` and `console.error` on JS, `stdout` and `stderr` on native). Has no state |
| `TestConsole` | `handler TestConsole: Console` | Appends the lines of `print_line` to `State[List[String]]` in the order written. Discards `print_error_line`. Used inside `State.run` |

```emel
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    (use Console).print_line("hi")
  }
}
```

### Std.Process (`std/process.emel`, `std/process.mjs`, `std/process.c`) `[api.std.process]`

The platform layer and the test layer (`[lib.layers]`). What the process has exactly one of.

| Declaration | Contents | Description |
|---|---|---|
| `Args` | `effect Args { fn get(index: Int) -> Option[String]; fn list() -> List[String] }` | The arguments of the process, in order, without the name of the program. `list` returns all of them, and `get` the one at `index` (from 0), or `None` when `index` is out of range |
| `SystemArgs` | `handler SystemArgs: Args` | The arguments given by the host. Has no state |
| error | `InvalidInput`, `StdinFailure(message: String)` | InvalidInput when standard input is not valid UTF-8. StdinFailure when the host cannot read standard input; `message` is the host's text |
| `Stdin` | `effect Stdin { suspend fn read_line() -> Option[String] fails InvalidInput \| StdinFailure; suspend fn read_text() -> String fails InvalidInput \| StdinFailure }` | Reads standard input as text (UTF-8). `read_line` returns the next line without its line terminator, and `None` at the end of the input. `read_text` returns the rest of the input, line terminators included, and `""` at the end of the input |
| `SystemStdin` | `handler SystemStdin: Stdin` | The standard input of the process. `process.stdin` on JS and file descriptor 0 on native. Requires Async. Has no state |
| `StdinLines` | `type StdinLines(lines: List[String])` | The state of TestStdin: the lines not read yet |
| `TestStdin` | `handler TestStdin: Stdin` | Kept in `State[StdinLines]` and resumes immediately. Does not raise InvalidInput or StdinFailure |
| `ExitCode` | `opaque type ExitCode` | The exit code of a program. Its contents are an Int from 0 to 255 |
| `exit_code` | `exit_code(code: Int) -> ExitCode` | The exit code `code`. A `code` outside 0 to 255 is a defect. Implemented in Emela |
| `exit_code_to_int` | `exit_code_to_int(code: ExitCode) -> Int` | The number of the exit code. Implemented in Emela |
| `EXIT_SUCCESS` | `const EXIT_SUCCESS: ExitCode` | `exit_code(0)` |
| `EXIT_FAILURE` | `const EXIT_FAILURE: ExitCode` | `exit_code(1)` |

Args:

- `list` of SystemArgs returns the same list on every call, and `get(index)` returns its element at `index`, or `None` when `index` is negative or not less than its length. The list is:
  - for `emela run`, the arguments written after `--` (`[cmd.build]`), on every target;
  - on native, `argv[1]` and after;
  - on JS, the arguments that the runtime's launcher passes to the runtime;
  - on WASM, the result of `get-arguments` of `wasi:cli/environment@0.3.0` without its first element;
  - `[]` for an HTTP service (`[lib.http-service]`) and under `emela test` (`[cmd.test.output]`).
- An argument that is not valid UTF-8 is decoded with each maximal invalid subsequence replaced by U+FFFD. Neither operation fails.

Stdin:

- A line ends at `\n`. The `\n`, and a `\r` immediately before it, are not part of the line. The text after the last `\n`, if not empty, is the last line. Once `read_line` has returned `None`, every later call returns `None` and every later `read_text` returns `""`.
- `read_line` and `read_text` read from the same position. `read_text` after `read_line` returns the input that follows the line.
- When a line (for `read_text`, the rest of the input) is not valid UTF-8, it is consumed and the operation fails with InvalidInput. Invalid UTF-8 is not replaced with the replacement character, as in `[api.std.bytes]`. The next `read_line` starts at the following line. U+0000 is read as is on every target.
- When the fiber waiting in an operation of SystemStdin is interrupted (11.4), the operation is canceled. Input that the host has already delivered is kept and returned by the next operation; no input is lost.
- Waiting for input does not stop other fibers on any target.
- SystemStdin does not hold the host open: a program that has no operation of Stdin waiting ends when its main ends, even if standard input is still open (a terminal, for example).
- Under `emela test`, standard input is not passed to the program (`[cmd.test.output]`). SystemStdin reads it as an empty input.
- `read_line` of TestStdin removes the first element of `lines` and returns it, or returns `None` when `lines` is empty. `read_text` returns the elements of `lines` joined with `\n` and sets `lines` to `[]`.

ExitCode:

- A `main` whose return type is `ExitCode` chooses the exit code of the program with its value (`[dyn.error]`). Every value from 0 to 255 can be chosen, including 1, 101 and 130. `ExitCode` is a value: making one uses no effect, and no handler is needed.
- The constants are named with the prefix `EXIT_` because a const is used through an import, not as a qualified name (`[grammar.expr]`), so its name appears in user code on its own.
- The upper bound 255 comes from the exit status of POSIX processes and the argument of `exit-with-code` of WASI, which are 8 bits wide.

impl (`[trait.derive]`): StdinLines has `derive Eq, Show`. Both errors have `derive Show`.

impl of `ExitCode` (`[trait.derive]`, `[trait.builtin]`): `derive Eq, Show`. The Show impl gives `ExitCode(2)`.

```emel
import Std.Console.{Console}
import Std.Process.{Args, Stdin, InvalidInput, StdinFailure}

fn quote(prefix: String) -> () fails InvalidInput | StdinFailure uses { Console, Stdin } {
  (use Stdin).read_line() match {
    None -> ()
    Some(line) -> {
      (use Console).print_line("#{prefix}#{line}")
      quote(prefix)
    }
  }
}
```

### Std.Clock (`std/clock.emel`, `std/clock.mjs`, `std/clock.c`) `[api.std.clock]`

| Declaration | Contents | Description |
|---|---|---|
| `Clock` | `effect Clock { suspend fn sleep(ms: Int) -> (); fn now() -> Instant }` | Waits (a negative value is the same as 0). The current point in time |
| `SystemClock` | `handler SystemClock: Clock` | `sleep` waits on `setTimeout` through an `@external suspend fn` (an interruption clears the timer, `[ext.call]`), and `now` is `Date.now()` (`timespec_get` on native). On native, `sleep` waits on a timer of the runtime's event loop (`em_after`); when interrupted, it completes at once and clears the timer (`[ext.c-host]`). Has no state. Requires Async (`[effect.async]`) |
| `TestClock` | `handler TestClock: Clock` | The time is a virtual time in `State[Instant]` (`[effect.state]`). `sleep(ms)` really waits; when no fiber can make progress and the host has no pending work, the clock advances to the earliest scheduled time and wakes that sleeper (at the same time, in call order; a negative value is 0). `now` is that time. Requires Async. The waiting external function `when_stalled` is held by the runtime on both JS and native (`em_when_stalled` on native) |

- The time that sleep waits follows the precision of the timer, and its accuracy is not guaranteed.
- When a TestClock sleep wakes up, if the time in the state is already later than the wake-up time, the time is not advanced (nor moved back).

### Std.Random (`std/random.emel`, `std/random.mjs`, `std/random.c`) `[api.std.random]`

| Declaration | Contents | Description |
|---|---|---|
| `Random` | `effect Random { fn int() -> Int; fn float() -> Float }` | A value uniform over the whole range of Int. A value uniform over 0 inclusive to 1 exclusive |
| `SystemRandom` | `handler SystemRandom: Random` | Draws with `crypto.getRandomValues` on JS, and `arc4random_buf` (macOS) or `getrandom` (Linux) on native. Has no state |

- `float` of SystemRandom returns a uniform 53-bit integer divided by 2^53. Every value from 0 inclusive to 1 exclusive in steps of 2^-53 appears with the same probability.

### Std.Env (`std/env.emel`, `std/env.mjs`, `std/env.c`) `[api.std.env]`

| Declaration | Contents | Description |
|---|---|---|
| `Env` | `effect Env { fn get(name: String) -> Option[String] }` | The value of an environment variable. `None` if it is not set. An empty value is `Some("")` |
| `SystemEnv` | `handler SystemEnv: Env` | Reads `process.env` on JS and `getenv` on native. Has no state |

- `get` of SystemEnv sees only names set as environment variables of the process. Even on the JS host, it does not pick up names from the object prototype (such as `toString`) as values.

### Std.Fiber (`std/fiber.emel`) `[api.std.fiber]`

Concurrency (`[fiber]`). Used with `import Std.Fiber.{Fiber, Scope}`.

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Fiber` | `@external type Fiber[A]` | runtime | A handle to a fiber. Cannot be leaked out of the scope (`[fiber.escape]`) |
| `Scope` | `@intrinsic effect Scope[E, R] { fn fork[A](f: fn() -> A fails E uses { R }) -> Fiber[A]; fn deferred[A]() -> Deferred[A]; fn semaphore(permits: Int) -> Semaphore; fn queue[A](capacity: Int) -> Queue[A] }` | built-in | The capability to launch fibers in a scope. Only `scope` can provide it |
| `scope` | `scope[A, E, R](body: fn() -> A fails E uses { Scope[E, R], Async, R }) -> A fails E uses { Async, R }` | intrinsic | Runs the body and returns the value after all children have finished. Children use the handlers in effect where `scope` was called |
| `fork` | `fork[A, E, R](f: fn() -> A fails E uses { Scope[E, R], R }) -> Fiber[A] uses { Scope[E, R] }` | Emela | `(use Scope).fork(f)` |
| `deferred` | `deferred[A, E, R]() -> Deferred[A] uses { Scope[E, R] }` | Emela | `(use Scope).deferred()` |
| `semaphore` | `semaphore[E, R](permits: Int) -> Semaphore uses { Scope[E, R] }` | Emela | `(use Scope).semaphore(permits)` |
| `queue` | `queue[A, E, R](capacity: Int) -> Queue[A] uses { Scope[E, R] }` | Emela | `(use Scope).queue(capacity)` |
| `join` | `join[A](fiber: Fiber[A]) -> A uses { Async }` | intrinsic | Waits for the child to finish and returns its value. A defect if the child was interrupted |
| `await` | `await[A](fiber: Fiber[A]) -> Exit[A, {}] uses { Async }` | intrinsic | Waits for the child to finish and returns how it ended (`Exit` of `Std.Exit`) |
| `interrupt` | `interrupt[A](fiber: Fiber[A]) -> () uses { Async }` | intrinsic | Interrupts the child and waits until its unwinding finishes |
| `all` | `all[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> List[A] fails E uses { Async, R }` | Emela | Runs all of them concurrently and collects the results in the original order |
| `race` | `race[A, E, R](fs: List[fn() -> A fails E uses { R }]) -> A fails E uses { Async, R }` | Emela | Takes the first one to finish and interrupts the rest |
| `both` | `both[A, B, E, R](f: fn() -> A fails E uses { R }, g: fn() -> B fails E uses { R }) -> (A, B) fails E uses { Async, R }` | Emela | Runs the two concurrently and returns a pair |
| `map` | `map[A, B, E, R](xs: List[A], f: fn(A) -> B fails E uses { R }) -> List[B] fails E uses { Async, R }` | Emela | Applies `f` to each element concurrently |
| `timeout` | `timeout[A, E, R](ms: Int, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { Clock, Async, R }` | Emela | Waits up to `ms` milliseconds. If `f` does not finish in time, interrupts it and returns `None` |

- `all` and `map` launch all children first, then join them in the original order.
- `timeout` launches a child that puts `Some(f())` into a Deferred and a child that waits ms with Clock and then puts None, and returns whichever was put first. Time is measured with the Clock handler at the call site, so inside TestClock it is measured in virtual time.

### Std.Exit (`std/exit.emel`) `[api.std.exit]`

How a computation ended, and `attempt`, which turns errors and defects into that value (`[fiber.cause]`). Used with `import Std.Exit.{Success, Failure}`.

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Exit` | `enum Exit[A, E] { Success(A), Failure(Cause[E]) }` | Emela | How a computation ended |
| `attempt` | `attempt[A, E, R](f: fn() -> A fails E uses { R }) -> Exit[A, E] uses { R }` | intrinsic | Runs `f` in place and turns Fail and Die into values. Starts no fiber. Does not catch an interruption of the calling fiber itself |

### Std.Cause (`std/cause.emel`) `[api.std.cause]`

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Cause` | `opaque type Cause[E]` | runtime | The reason a fiber did not end normally. The error inside cannot be taken out |
| `CauseKind` | `enum CauseKind { Fail, Die, Interrupt }` | Emela | The kind of cause |
| `kind` | `kind[E](c: Cause[E]) -> CauseKind` | intrinsic | The kind of cause |
| `to_string` | `to_string[E](c: Cause[E]) -> String` | intrinsic | `error: …`, `defect: …`, `interrupted` |
| `raise` | `raise[A, E](c: Cause[E]) -> A fails E` | intrinsic | Raises it again. Fail fails, Die is a defect, and Interrupt unwinds as an interruption |

impl (`[trait.derive]`): CauseKind has `derive Eq, Show`. Cause holds functions, so it does not satisfy Eq, Ord, or Show (display it with `to_string` or dbg). For the same reason, Exit of Std.Exit does not satisfy them either.

### Std.Deferred (`std/deferred.emel`) `[api.std.deferred]`

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Deferred` | `@external type Deferred[A]` | runtime | A container that can be filled with a value only once. Only `deferred` of Scope can create one |
| `complete` | `complete[A](d: Deferred[A], value: A) -> Bool` | intrinsic | Puts in the value and returns True only the first time. False from the second time on |
| `await` | `await[A](d: Deferred[A]) -> A uses { Async }` | intrinsic | Waits until a value is put in |
| `poll` | `poll[A](d: Deferred[A]) -> Option[A]` | intrinsic | `Some` if a value is in. Does not wait |

### Std.Semaphore (`std/semaphore.emel`) `[api.std.semaphore]`

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Semaphore` | `@external type Semaphore` | runtime | A fixed number of permits. Only `semaphore` of Scope can create one |
| `with_permit` | `with_permit[A, E, R](s: Semaphore, f: fn() -> A fails E uses { R }) -> A fails E uses { Async, R }` | Emela | `with_permits(s, 1, f)` |
| `with_permits` | `with_permits[A, E, R](s: Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> A fails E uses { Async, R }` | intrinsic | Waits for n permits, runs `f`, and gives them back however `f` ends |
| `try_with_permit` | `try_with_permit[A, E, R](s: Semaphore, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { R }` | Emela | `try_with_permits(s, 1, f)` |
| `try_with_permits` | `try_with_permits[A, E, R](s: Semaphore, n: Int, f: fn() -> A fails E uses { R }) -> Option[A] fails E uses { R }` | intrinsic | Runs `f` only if n permits can be taken without waiting |
| `available` | `available(s: Semaphore) -> Int` | intrinsic | The number of free permits |

### Std.Queue (`std/queue.emel`) `[api.std.queue]`

| Declaration | Signature | Implementation | Description |
|---|---|---|---|
| `Queue` | `@external type Queue[A]` | runtime | Values in order, up to a capacity. Only `queue` of Scope can create one |
| `QueueClosed` | `error QueueClosed` | Emela | The queue is closed (and, for `take`, empty) |
| `offer` | `offer[A](q: Queue[A], value: A) -> () fails QueueClosed uses { Async }` | intrinsic | Adds value at the end. Waits while full |
| `take` | `take[A](q: Queue[A]) -> A fails QueueClosed uses { Async }` | intrinsic | Removes the value at the front. Waits while empty |
| `try_offer` | `try_offer[A](q: Queue[A], value: A) -> Bool` | intrinsic | Adds value if neither full nor closed |
| `poll` | `poll[A](q: Queue[A]) -> Option[A]` | intrinsic | Removes the value at the front if there is one |
| `size` | `size[A](q: Queue[A]) -> Int` | intrinsic | The number of values |
| `close` | `close[A](q: Queue[A]) -> ()` | intrinsic | Closes the queue. The values in it can still be taken |

### Std.Http (`std/http.emel`, `std/http.mjs`) `[api.std.http]`

The client effect, its errors, and the entry adapter for HTTP services. Requests and responses are in `[api.std.http.request]` and `[api.std.http.response]`. Requests and responses are values, and the functions on top of send can be tested without a handler.

| Declaration | Contents | Description |
|---|---|---|
| `InvalidUrl` | `error InvalidUrl(url: String)` | Cannot be read as a URL. Also for schemes other than `http:` and `https:` |
| `ConnectionFailed` | `error ConnectionFailed(url: String, message: String)` | No response could be received (name resolution, connection, TLS, exceeding the redirect limit). The cause is in `message` |
| `ResponseNotText` | `error ResponseNotText(url: String)` | The response body cannot be read as UTF-8 |
| `Http` | `effect Http { suspend fn send(request: Request) -> Response fails InvalidUrl \| ConnectionFailed \| ResponseNotText }` | Sends a request. 4xx and 5xx are not failures |
| `get` | `get(url: String) -> Response fails ... uses { Http }` | Sends a GET |
| `post` | `post(url: String, body: String) -> Response fails ... uses { Http }` | Sends a POST with a body |
| `SystemHttp` | `handler SystemHttp: Http` | JS `fetch`. Follows redirects and does not store cookies. Requires Async |
| `TestServer` | `type TestServer(respond: fn(Request, Int) -> Option[Response], sent: List[Request])` | The counterpart of TestHttp. `respond` takes a request and its count (from 0) |
| `TestHttp` | `handler TestHttp: Http` | The state is `State[TestServer]`. Appends the request to `sent` and resumes immediately with the response from `respond`. `None` means ConnectionFailed |
| `Incoming` | `@external type Incoming` | The request as the host handed it over. Used only when writing an export by hand |
| `Outgoing` | `@external type Outgoing` | The response for the host to send |
| `serve` | `serve[E](app: fn(Request) -> Response fails E uses { Async }, incoming: Incoming) -> Outgoing uses { Async }` | An entry adapter (`[ext.entry]`). `@entry(Http.serve)` makes the fn the entry of an HTTP service (`[lib.http-service]`) |

- `request`, `add_header`, `header`, `headers`, `encode_query`, `require_ok`, and `response` remain for one minor version with `@deprecated`, pointing to their new places (`Request.new`, `Request.add_header`, `Response.header`, `Response.header_values`, `Request.encode_query`, `Response.require_ok`, `Response.new`).
- The message of ConnectionFailed is the text of the host's exception. If cause information is available, the cause's code and text, joined by a space, are appended after `: ` (`fetch failed: ECONNREFUSED connect ECONNREFUSED 127.0.0.1:1234`). The text depends on the host.
- A failure while reading the response body is also ConnectionFailed.
- The default content-type (`text/plain;charset=UTF-8`) is added only when a body is sent. It is not added to GET and HEAD, which are sent without a body.
- The `set-cookie` pairs of a response stay in the order received.
- Up to 20 redirects are followed; exceeding that is ConnectionFailed. The proxy environment variables are not read by default. When a proxy is enabled by host-side configuration (`NODE_USE_ENV_PROXY` in Node), that setting is followed.
- TestHttp appends the request to the end of `sent` and then calls `respond`. The count passed to `respond` is the length of `sent` before the append. When `respond` returns None, the message of ConnectionFailed is `TestServer returned no response`.

impl (`[trait.derive]`): InvalidUrl, ConnectionFailed, and ResponseNotText have `derive Show` (their contents are shown when they reach main or a test; `[dyn.error]`).

```emel
import Std.Http.{Http, TestServer, TestHttp}
import Std.Http.Response
import Std.State.{State}

@test
fn fetches() {
  server = TestServer(respond: |req, n| Some(Response.new(200, "ok")), sent: [])
  (res, after) = State.run(server, || with TestHttp { Http.get("https://example.com") })
  assert res.body == "ok"
  assert List.length(after.sent) == 1
}
```

### Std.Http.Request (`std/http/request.emel`) `[api.std.http.request]`

| Declaration | Contents | Description |
|---|---|---|
| `Method` | `enum Method { Get, Head, Post, Put, Patch, Delete, Options }` | The methods that can be sent. Moved from `Std.Http` |
| `Request` | `type Request(method: Method, url: String, headers: List[(String, String)], body: String)` | A request. Headers are sent in the order written. The body is sent as UTF-8. Moved from `Std.Http` |
| `new` | `new(method: Method, url: String) -> Request` | A request with no headers and no body. Formerly `Http.request` |
| `add_header` | `add_header(req: Request, name: String, value: String) -> Request` | Appends a header at the end |
| `header` | `header(req: Request, name: String) -> Option[String]` | The first value whose name matches. Case-insensitive (ASCII) |
| `header_values` | `header_values(req: Request, name: String) -> List[String]` | All values whose name matches. Formerly `headers`, renamed because the field `headers` has the same name (E0334) |
| `path` | `path(req: Request) -> String` | The path of `url`, without the query, as written (not decoded). `/` if empty |
| `segments` | `segments(req: Request) -> List[String]` | The path split at `/`, without empty segments, each percent-decoded. A segment with a malformed `%` sequence, or whose decoded bytes are not UTF-8, is kept as written |
| `query` | `query(req: Request) -> List[(String, String)]` | The part of `url` after `?`, read as `decode_query` does. Empty if there is no `?`. A pair with a malformed `%` sequence, or whose decoded bytes are not UTF-8, is kept as written |
| `encode_query` | `encode_query(pairs: List[(String, String)]) -> String` | A query that percent-encodes each name and value and joins the `name=value` pairs with `&` (`q=%E6%9D%B1&page=2`). The characters that are not escaped are in the Http item of `[lib.layers]`. Moved from `Std.Http` |
| `decode_query` | `decode_query(s: String) -> List[(String, String)] fails ParseError` | The inverse of `encode_query`. Splits at `&`, splits each pair at the first `=` (a pair without `=` has an empty value), replaces each `+` in the name and the value with a space, and then percent-decodes them. A malformed `%` sequence, or decoded bytes that are not UTF-8, fail with ParseError |

impl (`[trait.derive]`): Method and Request have `derive Eq, Show`.

### Std.Http.Response (`std/http/response.emel`) `[api.std.http.response]`

| Declaration | Contents | Description |
|---|---|---|
| `Response` | `type Response(status: Int, headers: List[(String, String)], body: String)` | A response. The headers of a response received by `send` have lowercase names sorted by name, and values with the same name are joined with `, ` (only `set-cookie` is kept one per pair). Moved from `Std.Http` |
| `UnexpectedStatus` | `error UnexpectedStatus(status: Int, body: String)` | Not 2xx in `require_ok`. Moved from `Std.Http` |
| `new` | `new(status: Int, body: String) -> Response` | A response without headers. Formerly `Http.response` |
| `add_header` | `add_header(res: Response, name: String, value: String) -> Response` | Appends a header at the end |
| `header` | `header(res: Response, name: String) -> Option[String]` | The first value whose name matches. Case-insensitive (ASCII). Moved from `Std.Http` |
| `header_values` | `header_values(res: Response, name: String) -> List[String]` | All values whose name matches. Moved from `Std.Http`. Formerly `headers`, renamed because the field `headers` has the same name (E0334) |
| `require_ok` | `require_ok(res: Response) -> Response fails UnexpectedStatus` | Fails on anything other than 2xx. Moved from `Std.Http` |

impl (`[trait.derive]`): Response has `derive Eq, Show`, and UnexpectedStatus has `derive Show`.

### Std.FileSystem (`std/file_system.emel`, `std/file_system.mjs`, `std/file_system.c`) `[api.std.file-system]`

The platform layer and the test layer (`[lib.layers]`). Handles only text (UTF-8). All operations are `suspend fn`. Paths stay as String and are not normalized.

| Declaration | Contents | Description |
|---|---|---|
| error | `FileNotFound(path)`, `FileAccessDenied(path)`, `NotADirectory(path)`, `IsADirectory(path)`, `DirectoryNotEmpty(path)`, `InvalidText(path)`, `FileSystemFailure(path, message)` | One error per kind. Each operation lists in `fails` only the ones it can raise |
| `read_text` | `suspend fn read_text(path: String) -> String fails FileNotFound \| FileAccessDenied \| IsADirectory \| InvalidText \| FileSystemFailure` | InvalidText if the file cannot be read as UTF-8 (no replacement characters; a leading BOM is kept) |
| `write_text` | `suspend fn write_text(path: String, text: String) -> () fails FileNotFound \| FileAccessDenied \| IsADirectory \| FileSystemFailure` | Creates or overwrites. FileNotFound if the parent directory does not exist |
| `append_text` | `suspend fn append_text(path: String, text: String) -> ()` (`fails` is the same as write_text) | Creates the file if it does not exist |
| `exists` | `suspend fn exists(path: String) -> Bool` | Does not fail. False if the path is not visible |
| `is_directory` | `suspend fn is_directory(path: String) -> Bool` | Does not fail |
| `list_directory` | `suspend fn list_directory(path: String) -> List[String] fails FileNotFound \| FileAccessDenied \| NotADirectory \| FileSystemFailure` | Only the names, sorted by name (lexicographic order of code points) |
| `create_directory` | `suspend fn create_directory(path: String) -> () fails FileAccessDenied \| NotADirectory \| FileSystemFailure` | Also creates the parents. Does nothing if the directory already exists. NotADirectory if a file is at that path or along the way |
| `remove` | `suspend fn remove(path: String) -> () fails FileNotFound \| FileAccessDenied \| DirectoryNotEmpty \| FileSystemFailure` | Deletes a file or an empty directory |
| `SystemFileSystem` | `handler SystemFileSystem: FileSystem` | `node:fs/promises` on JS. On native, it calls the POSIX functions synchronously inside an external function and completes immediately (other fibers are also stopped during reads and writes). Has no state |
| `TestFiles` | `type TestFiles(files: List[(String, String)], directories: List[String])` | The state of TestFileSystem. files is in path order. The ancestors of files and directories are also treated as directories |
| `TestFileSystem` | `handler TestFileSystem: FileSystem` | Kept in `State[TestFiles]` and resumes immediately. Errors follow the same rules as SystemFileSystem. Does not raise FileAccessDenied, InvalidText, or FileSystemFailure |

- SystemFileSystem maps host errors as follows. ENOENT and ENOTDIR are FileNotFound, EACCES and EPERM are FileAccessDenied, and everything else is FileSystemFailure (message is the host's text). However, EISDIR in read_text, write_text, and append_text is IsADirectory; ENOTDIR in list_directory is NotADirectory; ENOTDIR and EEXIST in create_directory are NotADirectory; and ENOTEMPTY and EEXIST in remove are DirectoryNotEmpty.
- exists and is_directory look at the target that a symbolic link points to. remove does not follow symbolic links and deletes the link itself.
- An operation of FileSystem is not canceled partway when the waiting fiber is interrupted.
- On the native target, SystemFileSystem fails with FileSystemFailure when it reads a file that contains U+0000 (it cannot be passed as a C string). The message of FileSystemFailure has the form `EIO: Input/output error, open 'a.txt'`. The descriptive text is that of the C library of the OS, and MAY differ between operating systems.
- TestFileSystem compares paths as strings (`a/b` and `./a/b` are different paths). The paths treated as directories are the paths in directories and the ancestors (following Path.parent) of the paths in files or directories. `/` is always a directory. A path with no parent (`a.txt`) can be written regardless of a parent.
- create_directory of TestFileSystem adds to directories the ancestors that are not yet directories and the path itself. If the path is already a directory, the state does not change. remove adds the ancestors of the removed path to directories. A directory that existed only as an ancestor of files remains after its last file is removed. directories is also kept in path order.
- In TestFileSystem, a path that has a file among its ancestors is FileNotFound in read_text, write_text, append_text, and remove, and NotADirectory in list_directory and create_directory.

impl (`[trait.derive]`): TestFiles has `derive Eq, Show`. Every error has `derive Show` (their contents are shown when they reach main or a test; `[dyn.error]`).

```emel
import Std.FileSystem.{FileSystem, FileNotFound}

fn show(path: String) {
  (use FileSystem).read_text(path) escape {
    FileNotFound(..) -> ""
  }
}
```

### Std.Path (`std/path.emel`) `[api.std.path]`

The value layer (`[lib.layers]`). Pure functions that build path strings. Only `/` is a separator. No normalization. A trailing `/` is ignored.

| Function | Signature | Implementation | Description |
|---|---|---|---|
| `join` | `join(a: String, b: String) -> String` | Emela | Joins with one `/` in between. `b` if `b` starts with `/`. If one is empty, the other |
| `parent` | `parent(path: String) -> Option[String]` | Emela | The path without its last segment. `parent("/a")` is `Some("/")`. `None` for a single segment, `/`, or empty |
| `file_name` | `file_name(path: String) -> Option[String]` | Emela | The last segment. `None` for `/`, empty, and paths that end in `.` or `..` |
| `extension` | `extension(path: String) -> Option[String]` | Emela | The part of the file name after the last `.`. `None` if there is no `.`, if the only `.` is the leading one (`.bashrc`), or if the name ends in `.` |

- A segment is a non-empty part after splitting at `/`. Repeated `/` are also ignored (`Path.parent("a//b/")` is `Some("a")`).
- If `a` ends in `/`, `join(a, b)` joins without adding `/`.

### Std.Platform (`std/platform.emel`) `[api.std.platform]`

| Declaration | Contents | Description |
|---|---|---|
| `Platform` | `layer Platform { SystemConsole, SystemArgs, SystemStdin, SystemClock, SystemRandom, SystemEnv, SystemHttp, SystemFileSystem }` | A bundle of the System handlers. To replace some of them, list the replacements after it, as in `with Platform, TestClock { ... }` |

- The order of the list has no meaning (`[effect.layer]`). SystemStdin, SystemClock, SystemHttp, and SystemFileSystem require Async, so Async remains in the R of `with Platform { ... }`. In main and test functions, the runtime provides Async.

```emel
import Std.Platform.{Platform}
import Std.Console.{Console}

fn main() {
  with Platform {
    (use Console).print_line("hi")
  }
}
```

## Examples of test handlers `[api.test-handlers]`

```emel
import Std.Clock.{Clock, TestClock}
import Std.Console.{TestConsole}
import Std.Instant
import Std.State.{State}

@test
fn greets() {
  (_, lines) = State.run([], || with TestConsole { greet(1) })
  assert lines == ["Hello, 1!"]
}

@test
fn waits() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, || with TestClock { (use Clock).sleep(1000) })
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(1000)
}
```
