# 16. Targets `[target]`

The same program MUST give the same observable results on every target. To make this possible, the meaning of numbers and strings is defined independently of the targets.

There are three targets: JS, WASM and C, and an embedded C target outside the tiers. JS and WASM are Tier 1, and C is Tier 2. A Tier 2 target has the same meaning as Tier 1, meets every rule in this chapter, and passes the same test suite. However, new features MAY arrive later than on Tier 1.

The C target compiles the emitted C with cc into a native executable. It targets macOS (arm64) and Linux (x86_64), and assumes a C11 compiler. Windows is left for a later version (18.1 #39). The constructs that the C target does not support yet are in `[target.c.unsupported]`, and using them is a compile error. The observable behavior of each target is in `[target.js]`, `[target.wasm]` and `[target.c]`.

The embedded C target (`c-embedded`) makes C and firmware for microcontrollers (`[target.c-embedded]`). It is outside the tiers: it only builds, so it does not run the test suite, and the constructs it supports are a subset that is implementation-defined for now.

The JS target assumes a host that runs on a server (Node 18 or later, and Deno and Bun with `node:` modules and `fetch`). Browsers are not a target. The platform handlers (15.2) do not specify how they behave in a browser.

## 16.1 Numbers `[target.number]`

| Item | Rule |
|---|---|
| Range of Int | -2,147,483,648 to 2,147,483,647 |
| Range of Int64 | -2^63 to 2^63 - 1 |
| Integer overflow | Wraps around in two's complement. Not a defect |
| Integer division | Truncates toward zero. The sign of the remainder follows the dividend |
| Integer division by zero | defect |
| Float | IEEE 754 binary64. Division by zero gives infinity or NaN as IEEE specifies. `%` is fmod (sign of the dividend) |
| Type conversion | Never implicit. Written explicitly with functions such as `Int64.from_int` |
| Conversion from Float to Int | Written explicitly with `Float.floor`, `ceil`, `round` and `truncate`. If the result is out of the range of Int, or is `NaN` or ±`Infinity`, it is a defect (no wrapping, no saturation). `Float.round` rounds 0.5 away from 0 (`2.5` gives 3, `-2.5` gives -3) |
| Conversion from Int64 to Int | `Int64.to_int` wraps around to the low 32 bits |

To detect overflow, use functions such as `Int.checked_add`. Dividing the minimum value by -1 (`-2147483648 / -1`) also overflows and wraps around.

`Float.compare`, and `min` and `max` of Int, Int64 and Float, compare by a total order. For Float, this order differs from the operator `<`. `Float.compare` treats NaN as the largest value, two NaNs as Equal, and `0.0` and `-0.0` as Equal. `min` and `max` choose by this order, and return the first argument when the values are equal. Because NaN is the largest value, `Float.min` returns the other argument when exactly one is NaN (`Float.min(NaN, 1.0)` is `1.0`), and `Float.max` returns NaN (`Float.max(NaN, 1.0)` is NaN). `abs` wraps around at the minimum value and returns the minimum value.

## 16.2 Strings `[target.string]`

- A String is a sequence of Unicode scalar values (code points other than surrogates), and its internal encoding cannot be observed. A String never contains a surrogate or an invalid sequence: values received from a host are normalized at the boundary (12.4, 12.5). Lengths and `String.chars` count grapheme clusters, as below.
- There is no API that takes a position as a number. Strings are traversed with functions such as `String.chars` and `String.split`.
- `String.length` returns the number of characters as seen (the number of grapheme clusters). `String.length("👍")` is 1.
- There is no character type. A single character is a String of length 1. To work with code points, use `String.code_points(s) -> List[Int]` and `String.from_code_point(n) -> Option[String]`.
- Grapheme cluster boundaries change between Unicode versions. To make JS and WASM give the same results, the Unicode version used is fixed. Which version is still open (18.1 #18).
- When the number of bytes is needed, use `String.byte_size`. It returns the number of bytes when encoded in UTF-8.
- Strings are ordered lexicographically by code point (10.6).
- `String.split(s, "")` splits at each grapheme cluster (the same as `String.chars`; `String.split("", "")` is `[]`, and `String.split("", ",")` is `[""]`).
- `String.contains`, `starts_with`, `ends_with` and `split` match on sequences of code points, and do not look at grapheme boundaries.
- `String.from_code_point(n)` returns `None` when `n` is not a Unicode scalar value (0 to 10FFFF, excluding surrogates).
- The whitespace that `String.trim` removes is the 25 characters with the Unicode White_Space property, and the table is fixed (U+FEFF and U+200B are not included).

## 16.3 Tail calls `[target.tail-call]`

Every target guarantees that a tail call to the function itself does not consume stack. Tail position is defined in 6.8. The limit on the depth of other recursion is implementation-defined.

## 16.4 Unspecified points `[target.unspecified]`

The following points are implementation-defined, and programs MUST NOT depend on them.

- The memory representation of values, and when they are freed
- How traits are implemented (dictionary passing or a copy per type)
- How continuations are implemented
- The order of execution when several fibers can run

## 16.5 Informative: intended implementation `[target.impl-hint]`

Informative.

| Item | JS | WASM | C |
|---|---|---|---|
| Pure functions | Ordinary functions | Ordinary functions | Ordinary functions |
| fn operations | Pass the handler as an argument and call it directly | Same | Same |
| Functions that reach a suspend fn | Converted to generators | Converted to state machines. Replaced once Stack Switching becomes available | Converted to state machines (emitted from the same low-level IR as WASM) |
| Functions with a polymorphic R (`R(f)` of `List.map`) | The original function is kept as non-suspending output, and a separate "suspending copy" (`List.map$suspend8`) is emitted for calls that pass a suspending function (specialization) | Monomorphized together with generics | Same |
| Async | Has no handler value, so it is not put in the evidence table. main and test functions whose R is only Async are called with no arguments | Same | Same |
| Errors | Returned as return values | Same | Same |
| defect | An exception of a dedicated class | Propagated explicitly through return values (no exceptions) | Same |
| Memory management | The host's GC | Reference counting in linear memory (Perceus style) | Reference counting (Perceus style) |
| Runtime | The compiler writes it to the output directory | Linked into the emitted module | Compiled as C source together with the output |
