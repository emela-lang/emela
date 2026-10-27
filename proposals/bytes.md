---
title: Bytes
authors: [Alliana]
status: accepted
pr: 457
issue:
decided: 2026-10-27
touches: [lib.layers, api.std.bytes (new), ext.boundary, target.js.value, diag.list]
requires: []
related: [http-service]
---

# Bytes

## Summary

Add `Bytes`, an immutable sequence of bytes, to the value layer of std as `Std.Bytes`. It has functions to build, read, slice and join byte sequences, and to convert to and from UTF-8 text and hexadecimal. `Bytes` becomes a boundary type of external functions on the JS target, so that the platform layer can later pass binary data to and from the host.

## Motivation

Every value that crosses the host boundary today is text. The specification works around the missing type in several places:

- `Std.FileSystem` handles only UTF-8 text: "There is no Bytes, so binary data is left to a later version" (`[lib.layers]`). An image or an archive cannot be read, and a file that is not UTF-8 fails with `InvalidText`.
- The bodies of `Std.Http` requests and responses are `String`. A response that is not UTF-8 fails with `ResponseNotText`, and an HTTP service cannot answer with an image.
- `Std.Random` has no `bytes` "because there is no Bytes" (`spec/history.md`, 15.2).
- The proposal `http-service` rejected listening with `wasi:sockets` because it "cannot be written without Bytes".
- `[ext.decl]` already says that host features that most programs use are added to std "as value types (Socket, Bytes, and so on)".

A byte sequence can be written today as `List[Int]`, but nothing keeps each element in 0 to 255, it takes O(n) to reach an element, and it cannot cross the host boundary (`[ext.boundary]`), so a handler that reads a binary file has nothing to receive the result in.

```emel
# what a binary-safe FileSystem operation needs to return
suspend fn read_bytes(path: String) -> Bytes fails FileNotFound | ...
```

This proposal adds only the type and its pure functions. The operations of the platform layer that use it (reading and writing binary files, binary HTTP bodies, random bytes) are separate proposals on top of it.

## Proposal

```emel
import Std.Bytes.{Bytes}

const PNG_SIGNATURE = Bytes.hex("89504e47")

@test
fn bytes_round_trip() {
  data = Bytes.append(PNG_SIGNATURE, Bytes.encode_utf8("hi"))
  assert Bytes.size(data) == 6
  assert Bytes.get(data, 0) == Some(0x89)
  assert Bytes.decode_utf8(Bytes.slice(data, 4, 6)) == Some("hi")
  assert Bytes.to_hex(data) == "89504e476869"
  assert Bytes.from_hex("not hex") == None
}
```

### Meaning

- A `Bytes` is a finite sequence of bytes. Each byte is an integer from 0 to 255, read as `Int`.
- `Bytes` is an opaque type. Its values are made only with the functions of `Std.Bytes`, and they are immutable: no function changes a `Bytes` in place.
- A constant is written with `Bytes.hex`, which is a defect for a string that is not hexadecimal. A string that comes from outside the program is read with `Bytes.from_hex`, which returns `None` instead. There is no literal for Bytes.
- Positions are 0-based `Int`. A function that takes a position does not fail and is not a defect for a position out of range: `get` returns `None`, and `slice` clamps.
- `Bytes` is defined in `std/bytes.emel` as an opaque type with `@intrinsic`, and its functions are intrinsics. Like the effect layer that has `@intrinsic`, it works only with the same version of the runtime (`[lib.layers]`).

### Typing rules

No new typing rules. `Bytes` is an ordinary type with no type parameters.

## Specification changes

### `[lib.layers]`

In the table of the four layers, the row of the value layer becomes:

> | value | Pure and independent of the host | Instant, Duration, Path, Bytes. Collections (Map, Set, Array) in 0.24 |

Add after the bullet on Instant and Duration:

> - A byte sequence is represented by Bytes (`Std.Bytes`), an immutable and opaque type. Each byte is an `Int` from 0 to 255. Bytes is the type through which the platform layer passes binary data, and it is a boundary type of external functions (12.4). Its declarations are in `[api.std.bytes]`.

### `[api.std.bytes]` (new)

Placed after `[api.std.instant]`.

> ### Std.Bytes (`std/bytes.emel`) `[api.std.bytes]`
>
> The value layer (`[lib.layers]`). An immutable sequence of bytes. Each byte is an `Int` from 0 to 255.
>
> | Declaration | Signature | Implementation | Description |
> |---|---|---|---|
> | `Bytes` | `opaque type Bytes` | intrinsic | A sequence of bytes |
> | `empty` | `empty() -> Bytes` | intrinsic | The sequence of no bytes |
> | `from_list` | `from_list(xs: List[Int]) -> Option[Bytes]` | intrinsic | The bytes in the order of `xs`. `None` if an element is outside 0 to 255 |
> | `to_list` | `to_list(b: Bytes) -> List[Int]` | intrinsic | The bytes in order |
> | `size` | `size(b: Bytes) -> Int` | intrinsic | The number of bytes |
> | `is_empty` | `is_empty(b: Bytes) -> Bool` | Emela | Whether there are no bytes |
> | `get` | `get(b: Bytes, index: Int) -> Option[Int]` | intrinsic | The byte at `index` (0-based). `None` if out of range or negative |
> | `slice` | `slice(b: Bytes, from: Int, to: Int) -> Bytes` | intrinsic | The bytes from `from` inclusive to `to` exclusive. Both are clamped to 0 to `size(b)`; empty if `from` is not less than `to` |
> | `append` | `append(a: Bytes, b: Bytes) -> Bytes` | intrinsic | The bytes of `a`, then those of `b` |
> | `concat` | `concat(parts: List[Bytes]) -> Bytes` | intrinsic | The bytes of all of them in order |
> | `encode_utf8` | `encode_utf8(s: String) -> Bytes` | intrinsic | The UTF-8 encoding of `s`. No BOM is added |
> | `decode_utf8` | `decode_utf8(b: Bytes) -> Option[String]` | intrinsic | The string that `b` encodes in UTF-8. `None` if `b` is not valid UTF-8. A leading BOM is kept as U+FEFF |
> | `to_hex` | `to_hex(b: Bytes) -> String` | intrinsic | Two lowercase hexadecimal digits per byte, with no separator (`"89504e47"`) |
> | `from_hex` | `from_hex(s: String) -> Option[Bytes]` | intrinsic | The inverse of `to_hex`. Accepts uppercase and lowercase digits. `None` for an odd number of digits or any other character, including whitespace |
> | `hex` | `hex(s: String) -> Bytes` | Emela | The bytes that `from_hex` reads from `s`. A defect if `from_hex` returns `None`. For constants written in the program (`const PNG_SIGNATURE = Bytes.hex("89504e47")`) |
>
> - The defect message of `hex` is ``Bytes.hex was given a string that is not hexadecimal: "<s>"``, where `<s>` is shown as by inspect. In a const, the name of the const is prepended (`[data.const]`).
> - `size(encode_utf8(s)) == String.byte_size(s)`, and `decode_utf8(encode_utf8(s)) == Some(s)`, for every String `s`.
> - Invalid UTF-8 is not replaced with the replacement character: `decode_utf8` returns `None`, so text that is read and written back is not changed silently. A sequence that encodes a surrogate (U+D800 to U+DFFF) or a value above U+10FFFF, and an overlong encoding, are not valid UTF-8.
> - `size`, `get` and `is_empty` are O(1). `slice` is O(`to - from`) or less. `append` and `concat` are O(n) in the number of bytes of the result. `to_list`, `from_list`, `encode_utf8`, `decode_utf8`, `to_hex` and `from_hex` are O(n).
>
> impl: `impl Bytes: Eq`, `impl Bytes: Ord`, `impl Bytes: Show`. Two Bytes are equal when they have the same bytes in the same order. Ord is lexicographic on the bytes as unsigned values (if the common prefix is equal, the shorter one is smaller). Show is the source form `Bytes.hex("89504e47")`, with the digits of `to_hex`, so the output can be pasted back as an expression. A Bytes inside another value is displayed the same way (`[Bytes.hex("00")]`).

### `[ext.boundary]`

The first bullet becomes:

> - Int, Int64, Float, Bool, String, Bytes (`Std.Bytes`), `()`

Add after the bullet on callbacks:

> - A Bytes received from the host is copied when it crosses the boundary, so a later change to the host's buffer does not change the Bytes, and the Bytes does not keep the rest of a larger host buffer alive. A Bytes passed to the host MUST NOT be modified by the host; if it is, the behavior is unspecified. Whether the runtime passes its own buffer or a copy is implementation-defined.

Add after the table of C types:

> - The C target does not support Bytes yet. When the build target is `c` and an external function reachable from the entry, or an exported fn, takes or returns Bytes, it is a compile error (E0443). The C form (a pointer and a length) is decided together with callbacks (18.1 #38).

### `[target.js.value]`

Add a row to the table, after String:

> | Bytes | `Uint8Array` |

Add after the table:

> - Where Bytes is expected from JS (the return value of an external function, a value resolved by an external suspend fn, an argument of a callback, and an argument of an exported fn), the value MUST satisfy `instanceof Uint8Array`. Subclasses are accepted (a Node.js `Buffer`). Any other value, including an `ArrayBuffer`, another typed array, a `DataView` and a `Uint8Array` from another realm, is a defect with the message ``external function `f` returned a value that is not a `Uint8Array` `` (for an argument of a callback or of an exported fn, ``argument `x` of `f` is not a `Uint8Array` ``). Wrap other values with `new Uint8Array(...)` in the host code.
> - Only the bytes visible through the `Uint8Array` (from `byteOffset`, `byteLength` bytes) are copied, not the whole underlying `ArrayBuffer`.

### `[diag.list]`

> | E0443 | The build target is `c`, and an external function reachable from the entry, or an exported fn, takes or returns `Bytes` (12.4) | ``external function `read_file` takes `Bytes`, which target `c` does not support yet`` |

## Diagnostics

- [x] New: E0443, Bytes at the boundary of an external function or an exported fn on the `c` target.

## Compatibility

No program that compiles now changes meaning. `Bytes` is reached only through `import Std.Bytes`, so a user's own type named `Bytes` and a file `bytes.emel` keep working: modules under `Std` do not add reserved names (`[module.name]`, `[lib.layers]`).

## Implementation

- emela-core (std sources): add `std/bytes.emel` with the opaque type and the `@intrinsic` declarations, and the Emela body of `is_empty`.
- emela-types: treat `Std.Bytes.Bytes` as a boundary type in the checks of `@external` and `@export`; report the new error for the `c` target.
- emela-codegen-js: represent Bytes as `Uint8Array`; implement the intrinsics (`TextEncoder`, a strict `TextDecoder` with `fatal: true` and `ignoreBOM: true`); copy at the boundary; Eq, Ord and Show in the runtime.
- emela-codegen-wasm: a runtime representation (length and a byte buffer in linear memory) and the intrinsics. No boundary on WASM yet, because user external functions are not supported there (`[target.wasm.external]`).
- emela-codegen-c: the intrinsics in the runtime, without the boundary.

## Alternatives

- `List[Int]` with helper functions. It does not keep elements in 0 to 255, has O(n) access, and cannot cross the host boundary. Allowing `List[Int]` at the boundary would make every list a candidate for conversion, which `[ext.boundary]` avoids on purpose.
- Bytes in core and the Prelude. Core holds the types the compiler knows as syntax (`[lib.layers]`), and Bytes has no syntax. Placing it in core would also make `bytes.emel` a reserved file name (`[module.name]`). If a byte literal is added later, this can be revisited.
- A mutable byte buffer. Every other value in the language is immutable, and mutation would have to be tracked in effects. A builder for building large Bytes efficiently can be added later as a separate type.
- A literal for Bytes (`x"89504e47"` for hexadecimal, or `b"\x89PNG"` mixing ASCII and escapes). It would check the digits at compile time and let a const of Bytes take part in exhaustiveness (`[data.const]`), but the type of a literal is known to the compiler, so Bytes would move to core and the Prelude, `bytes.emel` would become a reserved name (E0203), and the lexer, the grammar, `emela fmt`, tree-sitter, the LSP and the three code generators would change. `Bytes.hex` covers constants for now. A literal can be added later without breaking programs other than those with a `bytes.emel`, by keeping `Std.Bytes.Bytes` as a name for the core type.
- Only `from_hex`, returning `Option`, with no `hex`. Every constant and every test fixture would need a match and a panic.
- base64 (`to_base64`, `from_base64`) in `Std.Bytes`. It can be written in pure Emela on top of `get` and `from_list`, and the variants that std needs (the standard or the URL-safe alphabet, with or without padding) depend on the JSON representation of Bytes and on JWT. It is released first in `github.com/emela-lang/x`, and taken into std, as intrinsics if speed matters, together with the decision on the JSON representation of Bytes.
- Reading and writing fixed-width integers (`read_u16_be` and so on). They can be written in pure Emela on top of `get` (O(1)), and the open points (whether a `u32` is returned as Int64 or as a signed Int, how a read out of range is reported, whether writing needs a builder) are decided better after binary formats are written with them. They are released first in `github.com/emela-lang/x`.
- Accepting any typed array, `ArrayBuffer` and `DataView` at the JS boundary, or values from another realm (checked with `ArrayBuffer.isView` and `Symbol.toStringTag`). The rule grows, and `Uint8ClampedArray` and `Int8Array` would need to be told apart. Host code wraps such values with `new Uint8Array(...)`.
- `from_list` that takes the low 8 bits of each element instead of returning `Option`. It never fails, but `256` silently becomes `0`. The rest of std returns `Option` for an argument outside the domain (`String.from_code_point`).
- `decode_utf8` that replaces invalid sequences with U+FFFD. Lossy decoding hides corrupted input; it can be written on top of the strict version later if needed.

## Prior art

- Immutable byte strings as a separate type from text, with explicit encoding and decoding: Haskell `ByteString`, Erlang binaries, Go's distinction between `string` and `[]byte`, Rust `Vec<u8>` / `bytes::Bytes`, JavaScript `Uint8Array`.
- Strict UTF-8 decoding that fails instead of replacing: Rust `String::from_utf8`, the WHATWG Encoding Standard's `fatal` flag.

## Open questions

## Decision

Binary data could not cross the host boundary, which blocked binary files, binary HTTP bodies and random bytes. Bytes goes in std rather than core, as [ext.decl] already intends, so no file name becomes reserved. Constants use Bytes.hex instead of a literal, which keeps the lexer and the three code generators unchanged and leaves a literal possible later. base64 and fixed-width integers start in x, because their variants depend on uses not written yet. The JS boundary accepts only Uint8Array and copies it, so a Bytes never changes after it is received.
