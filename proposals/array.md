---
title: Array
authors: [Alliana]
status: draft
pr: 508
issue:
decided:
touches: [lib.layers, api.std.array (new), ext.boundary, target.js.value, target.impl-hint, diag.list]
requires: []
related: [bytes]
---

# Array

## Summary

Add `Array[A]`, an immutable sequence with O(1) access by position, to the value layer of std as `Std.Array`. Its representation is a flat block of elements. `set` and `push` return a new array; on targets that count references, they update the array in place when the argument is the only reference to it, so a loop that threads one array through `set` runs in O(1) per step there. `Array[A]` becomes a boundary type of external functions on the JS target, where it is a JS `Array`.

## Motivation

The only sequence in the language is `List[A]`, a singly linked list. `List.get` is O(n), so any algorithm that reads by position pays for the distance from the front on every read:

```emel
## Binary search over a sorted list. Each `List.get` walks from the front.
fn search_list(xs: List[Int], target: Int, low: Int, high: Int) -> Option[Int] {
  if low >= high {
    None
  } else {
    mid = (low + high) / 2
    List.get(xs, mid) match {
      Some(x) if x == target -> Some(mid)
      Some(x) if x < target  -> search_list(xs, target, mid + 1, high)
      _                      -> search_list(xs, target, low, mid)
    }
  }
}
```

This search is O(n log n), worse than a linear scan. The same holds for tables indexed by small integers: the transition table of an automaton (a regular expression engine), the rows of a dynamic programming table (edit distance), a histogram of byte values, the cells of a grid.

The workaround today is `Std.Map[Int, A]` keyed by position. It gives O(log n) reads and updates, but every read walks a tree of boxed nodes, the keys can have holes, nothing keeps them in 0 to `size - 1`, and the order of a map is not the order of a sequence when it is built from anything but consecutive keys.

`[open.list]` #36 already says that Array is an intrinsic "because it needs an array indexed by position" and that it comes in a later version, and `[lib.layers]` still lists it among the collections of 0.24, which it did not ship with.

Arrays also cross the host boundary. A JS host returns lists of values as JS arrays (rows of a query, the entries of a directory, the results of a batch). Today an external function cannot return a sequence at all (`[ext.boundary]`: "List and user-defined types cannot be passed directly"), so a handler author writes one external function per element, or encodes the sequence in a String.

## Proposal

```emel
import Std.Array.{Array}

@test
fn array_basics() {
  a = Array.from_list([10, 20, 30])
  assert Array.size(a) == 3
  assert Array.get(a, 1) == Some(20)
  assert Array.get(a, 3) == None
  assert Array.set(a, 0, 11) == Some(Array.from_list([11, 20, 30]))
  assert Array.set(a, 3, 0) == None
  assert Array.to_list(Array.push(a, 40)) == [10, 20, 30, 40]
  assert Array.to_list(Array.slice(a, 1, 9)) == [20, 30]
  assert inspect(a) == "Array.from_list([10, 20, 30])"
}
```

A loop that threads one array through `set`, where each `set` is the last use of the array it is given:

```emel
import Std.Array.{Array}

## Counts each byte value. `set` is the last use of `counts`, so it can update in place.
fn histogram(bytes: List[Int], counts: Array[Int]) -> Array[Int] {
  bytes match {
    [] -> counts
    [b, ..rest] -> {
      n = Array.get(counts, b) |> Option.or(0)
      Array.set(counts, b, n + 1) match {
        Some(next) -> histogram(rest, next)
        None       -> panic("not a byte: #{b}")
      }
    }
  }
}
```

An external function on the JS target:

```emel
@external(js, "./host.mjs", "sum")
fn sum(xs: Array[Int]) -> Int
```

### Meaning

- An `Array[A]` is a finite sequence of values of type `A`. Positions are 0-based `Int`.
- `Array` is an opaque type. Its values are made only with the functions of `Std.Array`, and they are immutable: no function changes an array that the program can still observe (`[dyn.memory]`).
- A function that takes a position does not fail and is not a defect for a position out of range: `get` and `set` return `None`, and `slice` clamps.
- `Array` is defined in `std/array.emel` as an opaque type with `@intrinsic` and one type parameter. Like `Bytes`, it works only with the same version of the runtime (`[lib.layers]`).
- `set` and `push` return a new array. When the array given to them is referred to from nowhere else, a target that counts references updates it in place and returns it. Because the old array can no longer be observed, the result is the same as a copy; only the time taken differs.

### Typing rules

No new typing rules. `Array[A]` is an ordinary type with one type parameter. Its traits follow those of `A`, in the same way as `List[A]` (`[trait.builtin]`).

## Specification changes

### `[lib.layers]`

In the table of the four layers, the row of the value layer becomes:

> | value | Pure and independent of the host | Instant, Duration, Path, Bytes, Array. Collections (Map, Set) in 0.24 |

The bullet that begins "Collections (Map, Set, Array) are placed in 0.24" becomes:

> - Collections (Map, Set) are placed in 0.24 as `Std.Map` and `Std.Set`, on top of the traits (Eq, Ord, Show). This is because with the trait-less `==` and display of 0.20, comparison and display could vary with how a value was built even when the contents are the same. The form of the implementation and the ordering are in 18.1 #36.

Add after the bullet on Bytes:

> - A sequence read by position is represented by Array (`Std.Array`), an immutable and opaque type with O(1) access by position. Its contents are a flat block of elements that the runtime represents, so it is an intrinsic. Array is a boundary type of external functions (12.4). Its declarations are in `[api.std.array]`.

### `[api.std.array]` (new)

Placed after `[api.std.bytes]`.

> ### Std.Array (`std/array.emel`) `[api.std.array]`
>
> The value layer (`[lib.layers]`). An immutable sequence read by position. Positions are 0-based.
>
> | Declaration | Signature | Implementation | Description |
> |---|---|---|---|
> | `Array` | `opaque type Array[A]` | intrinsic | A sequence of values of type `A` |
> | `empty` | `empty[A]() -> Array[A]` | Emela | The array of no elements |
> | `from_list` | `from_list[A](xs: List[A]) -> Array[A]` | intrinsic | The elements in the order of `xs` |
> | `to_list` | `to_list[A](a: Array[A]) -> List[A]` | intrinsic | The elements in order |
> | `repeat` | `repeat[A](value: A, count: Int) -> Array[A]` | Emela | `count` copies of `value`. Empty if `count` is not positive |
> | `size` | `size[A](a: Array[A]) -> Int` | intrinsic | The number of elements |
> | `is_empty` | `is_empty[A](a: Array[A]) -> Bool` | Emela | Whether there are no elements |
> | `get` | `get[A](a: Array[A], index: Int) -> Option[A]` | intrinsic | The element at `index`. `None` if out of range or negative |
> | `set` | `set[A](a: Array[A], index: Int, value: A) -> Option[Array[A]]` | intrinsic | The elements of `a` with the one at `index` replaced by `value`. `None` if out of range or negative |
> | `push` | `push[A](a: Array[A], value: A) -> Array[A]` | intrinsic | The elements of `a`, then `value` |
> | `slice` | `slice[A](a: Array[A], from: Int, to: Int) -> Array[A]` | intrinsic | The elements from `from` inclusive to `to` exclusive. Both are clamped to 0 to `size(a)`; empty if `from` is not less than `to` |
> | `append` | `append[A](a: Array[A], b: Array[A]) -> Array[A]` | Emela | The elements of `a`, then those of `b` |
> | `concat` | `concat[A](parts: List[Array[A]]) -> Array[A]` | intrinsic | The elements of all of them in order |
> | `map` | `map[A, B](a: Array[A], f: fn(A) -> B) -> Array[B]` | Emela | Applies `f` to each element, from the first |
> | `fold` | `fold[A, B](a: Array[A], from: B, f: fn(B, A) -> B) -> B` | Emela | Folds from the first element |
>
> - `size`, `get` and `is_empty` are O(1). `set` is O(n) and `push` is O(n), where n is `size(a)`, except as below. `slice` is O(`to - from`) or less. `append`, `concat`, `repeat`, `from_list`, `to_list`, `map` and `fold` are O(n) in the number of elements of the result or of the argument.
> - When the array given to `set` or `push` is not referred to from anywhere else, a target that counts references (`[target.impl-hint]`) updates it in place: `set` is then O(1), and `push` is O(1) amortized. Whether an array is referred to from elsewhere follows from the program: it is when the variable that holds it is used after the call, or when the array is held in another value that is still in use. The result is the same either way.
>
> impl: `impl Array[A: Eq]: Eq`, `impl Array[A: Ord]: Ord`, `impl Array[A: Show]: Show`. Two arrays are equal when they have equal elements in the same order. Ord is lexicographic (if the common prefix is equal, the shorter one is smaller). Show is the source form `Array.from_list([1, 2, 3])`, so the output can be pasted back as an expression. Values inside are displayed as in `[trait.builtin]` (a String in `"`). An array inside another value is displayed the same way (`[Array.from_list([1])]`).

### `[ext.boundary]`

The first bullet becomes:

> - Int, Int64, Float, Bool, String, Bytes (`Std.Bytes`), `()`
> - `Array[A]` (`Std.Array`), where `A` is a boundary type other than a function type

Add after the bullet on Bytes:

> - An Array received from the host is copied when it crosses the boundary, and each element is converted as a value of type `A` received from the host would be (a String element is normalized as in 12.5). A later change to the host's array does not change the Array. An Array passed to the host MUST NOT be modified by the host; if it is, the behavior is unspecified. Whether the runtime passes its own array or a copy is implementation-defined.

Add after the bullet on Bytes in the C part:

> - The C target does not support Array yet. When the build target is `c` and an external function reachable from the entry, or an exported fn, takes or returns an Array, it is a compile error (E0443).

### `[target.js.value]`

Add a row to the table, after Bytes:

> | `Array[A]` | A JS `Array` whose elements are represented by this table |

Add after the bullets on Bytes:

> - Where an Array is expected from JS (the same places as for Bytes), the value MUST satisfy `Array.isArray`. Any other value, including a typed array and an object with a `length`, is a defect with the message ``external function `f` returned a value that is not an array`` (for an argument of a callback or of an exported fn, ``argument `x` of `f` is not an array``). Each element is then converted by this table; an element that is not a value of `A` is a defect as it would be for a single value of `A`.
> - A sparse JS array is read as if each hole held `undefined`.

### `[target.impl-hint]`

Add a row to the table, after Memory management:

> | In-place update of Array (`[api.std.array]`) | Never: `set` and `push` copy | When the reference count of the argument is 1 | Same as WASM |

### `[diag.list]`

The row of E0443 becomes:

> | E0443 | The build target is `c`, and an external function reachable from the entry, or an exported fn, takes or returns `Bytes` or an `Array` (12.4) | ``external function `read_file` takes `Bytes`, which target `c` does not support yet`` / ``external function `rows` returns `Array[String]`, which target `c` does not support yet`` |

## Diagnostics

- [ ] Changed: E0443 also covers `Array[A]` at the boundary of an external function or an exported fn on the `c` target.
- [ ] `Array[fn(...) -> ...]` at the boundary is not a boundary type, so it is the existing errors for a non-boundary type (the check of `@external`, and E0436 for `@export`).

## Compatibility

No program that compiles now changes meaning. `Array` is reached only through `import Std.Array`, so a user's own type named `Array` and a file `array.emel` keep working: modules under `Std` do not add reserved names (`[module.name]`, `[lib.layers]`).

## Implementation

- emela-core (std sources): add `std/array.emel` with the opaque type, the `@intrinsic` declarations, the Emela bodies of `empty`, `repeat`, `is_empty`, `append`, `map` and `fold` (self tail calls over positions, so long arrays do not use the stack), and the impls of Eq, Ord and Show.
- emela-types / the table of built-in types: the first built-in type with a type parameter. `[kw.annotation]` already allows `@intrinsic` on "a type without fields"; the table of built-in types has to carry the parameter.
- emela-lir: builtins borrow their arguments today (`heap/rc.rs`), and there is no reuse yet. `set` and `push` need to consume the array argument instead, so the runtime sees a count of 1 when the caller's reference was the last. The other array builtins keep borrowing.
- emela-codegen-wasm and emela-codegen-c: a runtime representation (length, capacity and a block of element references in linear memory), the intrinsics, and the in-place path of `set` and `push` when the count is 1. `push` grows the capacity geometrically so that repeated in-place pushes are amortized O(1).
- emela-codegen-js: represent Array as a JS `Array`, frozen or never exposed for mutation; `set` and `push` copy (`slice` then write). Convert and copy at the boundary.
- No prototype yet.

## Alternatives

- A persistent vector (a wide balanced tree, such as a 32-way trie) with O(log n) `get`, `set` and `push` on every target, which could be written in pure Emela. The bounds would be the same on every target, but `get` would no longer be O(1), and reading a transition table, the main use, would walk the tree on every step.
- A flat array that always copies on `set` and `push`, on every target. The specification would be the simplest and the bounds the same everywhere, but every loop that updates an array would be O(n) per step, and the type would be useful only for tables built once with `from_list`.
- `Map[Int, A]` keyed by position (the workaround). O(log n), holes in the keys, and no boundary type.
- O(1) `List.get` by changing the representation of List. The representation of List is fixed by its patterns (`[x, ..rest]`) and by `List.tail` being O(1).
- A mutable array held in State or a handler. Every other value is immutable (`[dyn.memory]`); mutation would have to be tracked in effects, and an array could not be put in a Map or compared.
- `set` that returns the array unchanged for a position out of range, or a defect. `[lib.api-rules]` returns an Option for "out of range", and `List.get` and `Bytes.get` do. See also the open question on `set` and in-place update below.
- Array in core and the Prelude. Core holds the types the compiler knows as syntax (`[lib.layers]`), and Array has no syntax. Placing it in core would make `array.emel` a reserved file name (`[module.name]`). If an array literal is added later, this can be revisited.
- No boundary for Array, with sequences passed to the host in a later proposal. Handler authors would keep writing one external function per element.

## Prior art

- Immutable arrays updated in place when the reference is unique, found by reference counting at run time ("functional but in-place"): Lean 4 `Array`, Koka (Perceus), Roc `List`.
- Uniqueness types that make in-place update part of the type: Clean, linear arrays in Linear Haskell.
- Persistent vectors with O(log n) update: Clojure `PersistentVector`, Scala `Vector`, Elm `Array` (relaxed radix balanced trees).
- Immutable flat arrays with copying update: Haskell `Data.Array`, `Data.Vector`.

## Open questions

- In-place update and the Option of `set`. Writing the fallback with the original array, `Array.set(counts, b, n + 1) |> Option.or(counts)`, keeps `counts` alive after the call, so `set` always copies. Only a `None` arm that does not use the array (a defect, or another value) keeps the update in place. Is that acceptable, or should `set` return `Array[A]` and leave the array unchanged out of range (deviating from `[lib.api-rules]`), or should there be both (`set` returning Option, and an `update` that cannot fail)?
- How strongly to promise the in-place update on WASM and C: as a requirement of the targets (MUST, so that programs can rely on O(1)), or only as a statement of what the intended implementation does (`[target.impl-hint]`, informative, as written above).
- `push` copies on JS every time, so a loop of n pushes is O(n²) there and O(n) on WASM and C. Is a builder (or advice to build with `from_list`) needed to keep JS usable, or is the difference acceptable under 16.4?
- The name `push`. `[lib.api-rules]` names joining two values `append`; there is no name yet for adding one element at the end.
- Whether nested arrays (`Array[Array[Int]]`) are boundary types, as written above, or only arrays of the scalar boundary types.
- Whether to extend E0443 to Array, as written above, or to give Array its own code.
- Whether the first version needs more functions (`find`, `any`, `all`, `enumerate`, `sort`), or whether converting through `to_list` is enough until uses are written.

## Decision
