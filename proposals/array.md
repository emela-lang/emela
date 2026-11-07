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

Add `Array[A]`, an immutable sequence of fixed length with O(1) access by position, to the value layer of std as `Std.Array`. Its representation is a flat block of elements. `set` returns a new array; on WASM and C, it updates the array in place when the argument is the last reference to it, so a loop that threads one array through `set` runs in O(1) per step there. A sequence is built as a List and turned into an Array; Array has no operation that grows an array. `Array[A]` becomes a boundary type of external functions on the JS target, where it is a JS `Array`.

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

This search is O(n log n), worse than a linear scan. The same holds for tables indexed by small integers: the transition table of an automaton (a regular expression engine), the rows of a dynamic programming table (edit distance), a histogram of byte values, the cells of a grid. All of them have a size that is known once they are built.

The workaround today is `Std.Map[Int, A]` keyed by position. It gives O(log n) reads and updates, but every read walks a tree of boxed nodes, the keys can have holes, and nothing keeps them in 0 to `size - 1`.

`[open.list]` #36 already says that Array is an intrinsic "because it needs an array indexed by position" and that it comes in a later version, and `[lib.layers]` still lists it among the collections of 0.24, which it did not ship with.

Arrays also cross the host boundary. A JS host returns sequences of values as JS arrays (rows of a query, the entries of a directory, the results of a batch). Today an external function cannot return a sequence at all (`[ext.boundary]`: "List and user-defined types cannot be passed directly"), so a handler author writes one external function per element, or encodes the sequence in a String.

## Proposal

```emel
import Std.Array.{Array}

@test
fn array_basics() {
  a = Array.from_list([30, 10, 20])
  assert Array.size(a) == 3
  assert Array.get(a, 1) == Some(10)
  assert Array.get(a, 3) == None
  assert Array.set(a, 0, 31) == Array.from_list([31, 10, 20])
  sorted = Array.sort(a)
  assert Array.to_list(sorted) == [10, 20, 30]
  assert Array.binary_search(sorted, 20) == Some(1)
  assert Array.binary_search(sorted, 25) == None
  assert Array.to_list(Array.slice(a, 1, 9)) == [10, 20]
  assert inspect(a) == "Array.from_list([30, 10, 20])"
}
```

A loop that threads one array through `set`, where each `set` is the last use of the array it is given, and a call where it is not:

```emel
## Counts each byte value. `set` is the last use of `counts`, so it updates in place.
fn histogram(bytes: List[Int], counts: Array[Int]) -> Array[Int] {
  bytes match {
    [] -> counts
    [b, ..rest] -> {
      n = Array.get(counts, b) |> Option.or(0)
      histogram(rest, Array.set(counts, b, n + 1))
    }
  }
}

## `counts` is used again after `set`, so `set` copies.
fn bump_twice(counts: Array[Int], b: Int) -> (Array[Int], Array[Int]) {
  (Array.set(counts, b, 1), counts)
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
- An array has a fixed length. No function makes an existing array longer or shorter; `slice`, `append`, `concat` and `reverse` make new arrays. A sequence that grows is built as a List and turned into an Array with `from_list`.
- Reading a position out of range is not found: `get` returns `None`, and `slice` clamps. Writing a position out of range is a broken precondition of the caller: `set` is a defect (`[lib.api-rules]`).
- `Array` is defined in `std/array.emel` as an opaque type with `@intrinsic` and one type parameter. Like `Bytes`, it works only with the same version of the runtime (`[lib.layers]`).
- `set` returns a new array. When the array given to it is referred to from nowhere else, WASM and C update it in place and return it. Because the old array can no longer be observed, the result is the same as a copy; only the time taken differs.

List and Array divide the work by how a sequence is read. A List is read from the front and built a little at a time: recursion, `match` on `[x, ..rest]`, pipelines of `List.map` and `List.filter`. An Array is read by position: tables, binary search, grids, and values passed to the host. The functions of `Std.Array` beyond the basic ones are those where the position matters (`find_index`, `binary_search`, `sort`, `reverse`); functions that read from the front (`find`, `any`, `filter`) are reached through `to_list`.

### Typing rules

No new typing rules. `Array[A]` is an ordinary type with one type parameter. Its traits follow those of `A`, in the same way as `List[A]` (`[trait.builtin]`).

## Specification changes

### `[lib.layers]`

In the table of the four layers, the row of the value layer becomes:

> | value | Pure and independent of the host | Instant, Duration, Path, Bytes, Array. Collections (Map, Set) in 0.24 |

The bullet that begins "Collections (Map, Set, Array) are placed in 0.24" becomes:

> - Collections (Map, Set) are placed in 0.24 as `Std.Map` and `Std.Set`, on top of the traits (Eq, Ord, Show). This is because with the trait-less `==` and display of 0.20, comparison and display could vary with how a value was built even when the contents are the same. The form of the implementation and the ordering are in 18.1 #36.

Add after the bullet on Bytes:

> - A sequence read by position is represented by Array (`Std.Array`), an immutable and opaque type of fixed length with O(1) access by position. Its contents are a flat block of elements that the runtime represents, so it is an intrinsic. A List is read from the front and built a little at a time; an Array is built from a List (`Array.from_list`) and then read by position. Array is a boundary type of external functions (12.4). Its declarations are in `[api.std.array]`.

### `[api.std.array]` (new)

Placed after `[api.std.bytes]`.

> ### Std.Array (`std/array.emel`) `[api.std.array]`
>
> The value layer (`[lib.layers]`). An immutable sequence of fixed length, read by position. Positions are 0-based.
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
> | `set` | `set[A](a: Array[A], index: Int, value: A) -> Array[A]` | intrinsic | The elements of `a` with the one at `index` replaced by `value`. A defect if `index` is out of range or negative |
> | `slice` | `slice[A](a: Array[A], from: Int, to: Int) -> Array[A]` | intrinsic | The elements from `from` inclusive to `to` exclusive. Both are clamped to 0 to `size(a)`; empty if `from` is not less than `to` |
> | `append` | `append[A](a: Array[A], b: Array[A]) -> Array[A]` | Emela | The elements of `a`, then those of `b` |
> | `concat` | `concat[A](parts: List[Array[A]]) -> Array[A]` | intrinsic | The elements of all of them in order |
> | `reverse` | `reverse[A](a: Array[A]) -> Array[A]` | Emela | The elements in reverse order |
> | `map` | `map[A, B](a: Array[A], f: fn(A) -> B) -> Array[B]` | Emela | Applies `f` to each element, from the first |
> | `fold` | `fold[A, B](a: Array[A], from: B, f: fn(B, A) -> B) -> B` | Emela | Folds from the first element |
> | `find_index` | `find_index[A](a: Array[A], f: fn(A) -> Bool) -> Option[Int]` | Emela | The position of the first element that gives True |
> | `sort_by` | `sort_by[A](a: Array[A], f: fn(A, A) -> Ordering) -> Array[A]` | Emela | Sorts in the order of `f`. Stable, with the same result as `List.sort_by` |
> | `sort` | `sort[A: Ord](a: Array[A]) -> Array[A]` | Emela | Sorts in the Ord order (`sort_by(a, Ord.compare)`). Stable |
> | `binary_search` | `binary_search[A: Ord](a: Array[A], value: A) -> Option[Int]` | Emela | In an array sorted in the Ord order, the smallest position whose element is equal to `value`. `None` if there is none |
>
> - The defect message of `set` is ``Array.set was given position <index> in an array of size <size>``.
> - `binary_search` bisects as follows, so its result is the same on every target even when `a` is not sorted. Start with `low = 0` and `high = size(a)`. While `low < high`, let `mid = (low + high) / 2`; if the element at `mid` is less than `value`, set `low = mid + 1`, otherwise set `high = mid`. The result is `Some(low)` if `low < size(a)` and the element at `low` is equal to `value`, and `None` otherwise. It compares O(log n) times.
> - `size`, `get` and `is_empty` are O(1). `set` is O(n), where n is `size(a)`, except as below. `slice` is O(`to - from`) or less. `append`, `concat`, `repeat`, `from_list`, `to_list`, `reverse`, `map`, `fold` and `find_index` are O(n) in the number of elements of the result or of the argument, and `sort` and `sort_by` are O(n log n).
> - On the WASM and C targets, `set` MUST be O(1) when the argument `a` is a variable (a parameter or a local binding) that is not used after the call on any path, and no other variable or value that is still in use refers to the same array (a closure that captured it, a tuple, a record or a list that holds it, the state of State, or another variable bound to it). In other cases, a target MAY still update in place when it can determine that the array is not shared. The JS target always copies. The result is the same in every case.
>
> impl: `impl Array[A: Eq]: Eq`, `impl Array[A: Ord]: Ord`, `impl Array[A: Show]: Show`. Two arrays are equal when they have equal elements in the same order. Ord is lexicographic (if the common prefix is equal, the shorter one is smaller). Show is the source form `Array.from_list([1, 2, 3])`, so the output can be pasted back as an expression. Values inside are displayed as in `[trait.builtin]` (a String in `"`). An array inside another value is displayed the same way (`[Array.from_list([1])]`).

### `[ext.boundary]`

The first bullet becomes:

> - Int, Int64, Float, Bool, String, Bytes (`Std.Bytes`), `()`
> - `Array[A]` (`Std.Array`), where `A` is a boundary type other than a function type (so `Array[Array[Int]]` and `Array[Bytes]` are boundary types)

Add after the bullet on Bytes:

> - An Array received from the host is copied when it crosses the boundary, and each element is converted as a value of type `A` received from the host would be (a String element is normalized as in 12.5, and a nested Array is copied by this rule). A later change to the host's array does not change the Array. An Array passed to the host MUST NOT be modified by the host; if it is, the behavior is unspecified. Whether the runtime passes its own array or a copy is implementation-defined.

The bullet on Bytes in the C part becomes:

> - The C target does not support Bytes and Array at the boundary yet. When the build target is `c` and an external function reachable from the entry, or an exported fn, takes or returns a type that contains Bytes or an Array, it is a compile error (E0443). The C form (a pointer and a length) is decided together with callbacks (18.1 #38).

### `[target.js.value]`

Add a row to the table, after Bytes:

> | `Array[A]` | A JS `Array` whose elements are represented by this table |

Add after the bullets on Bytes:

> - Where an Array is expected from JS (the same places as for Bytes), the value MUST satisfy `Array.isArray`. Any other value, including a typed array and an object with a `length`, is a defect with the message ``external function `f` returned a value that is not an array`` (for an argument of a callback or of an exported fn, ``argument `x` of `f` is not an array``). Each element is then converted by this table; an element that is not a value of `A` is a defect as it would be for a single value of `A`.
> - A sparse JS array is read as if each hole held `undefined`.

### `[target.impl-hint]`

Add a row to the table, after Memory management:

> | `Array.set` (`[api.std.array]`) | Copies | Updates in place when the reference count of the argument is 1 | Same as WASM |

### `[diag.list]`

The row of E0443 becomes:

> | E0443 | The build target is `c`, and an external function reachable from the entry, or an exported fn, takes or returns a type that target `c` does not support at the boundary yet: one that contains Bytes or an Array (12.4) | ``external function `read_file` takes `Bytes`, which target `c` does not support yet`` / ``external function `rows` returns `Array[String]`, which target `c` does not support yet`` |

## Diagnostics

- [ ] Changed: E0443 covers every type that target `c` does not support at the boundary yet, now Bytes and `Array[A]`.
- [ ] `Array[fn(...) -> ...]` at the boundary is not a boundary type, so it is the existing errors for a non-boundary type (the check of `@external`, and E0436 for `@export`).

## Compatibility

No program that compiles now changes meaning. `Array` is reached only through `import Std.Array`, so a user's own type named `Array` and a file `array.emel` keep working: modules under `Std` do not add reserved names (`[module.name]`, `[lib.layers]`).

## Implementation

- emela-core (std sources): add `std/array.emel` with the opaque type, the `@intrinsic` declarations, the Emela bodies of the other functions (self tail calls over positions, so long arrays do not use the stack; `sort_by` through `List.sort_by`), and the impls of Eq, Ord and Show.
- emela-types / the table of built-in types: the first built-in type with a type parameter. `[kw.annotation]` already allows `@intrinsic` on "a type without fields"; the table of built-in types has to carry the parameter.
- emela-lir: builtins borrow their arguments today (`heap/rc.rs`), and there is no reuse yet. `set` needs to consume its array argument instead, so the runtime sees a count of 1 when the caller's reference was the last. The other array builtins keep borrowing. The rule of `[api.std.array]` (a variable not used after the call and not referred to from elsewhere) is the case where the existing liveness analysis already moves the last reference into the call.
- emela-codegen-wasm and emela-codegen-c: a runtime representation (the length and a block of element references in linear memory), the intrinsics, and the in-place path of `set` when the count is 1.
- emela-codegen-js: represent Array as a JS `Array` that is never mutated after it is made; `set` copies. Convert and copy at the boundary.
- No prototype yet.

## Alternatives

- A growable sequence with `push` in amortized O(1) (a block with spare capacity, written in place when the array is the last one pushed to). It would make Array a second way to build a sequence a little at a time, which is the work of List, and the name Array would describe something that grows. A growable sequence or a builder can be a separate type later, when uses need it.
- A persistent vector (a wide balanced tree, such as a 32-way trie) with O(log n) `get` and `set` on every target, which could be written in pure Emela. The bounds would be the same on every target, but `get` would no longer be O(1), and reading a transition table, the main use, would walk the tree on every step.
- A flat array that always copies on `set`, on every target. The specification would be the simplest and the bounds the same everywhere, but every loop that updates an array would be O(n) per step, and the type would be useful only for tables built once.
- `set` that returns `Option[Array[A]]` for a position out of range, as `[lib.api-rules]` does for reads out of range. The natural fallback, `Array.set(a, i, x) |> Option.or(a)`, keeps `a` in use after the call, so `set` would always copy, silently, in the most common form of a loop. A write out of range is a bug in the caller, which `[lib.api-rules]` makes a defect.
- `set` that returns the array unchanged for a position out of range. A wrong position would go unnoticed.
- Promising the in-place update only as the intended implementation (informative). Programs could not rely on O(1) `set`, which is the reason for the type. Promising it for every array whose reference count is 1 would fix the analyses of the compiler (inlining, captures, suspension) in the specification; the rule names only the case that a reader can see in the source.
- The functions that read from the front (`find`, `any`, `all`, `filter`, `enumerate`) in `Std.Array`. They are the work of List, and `Array.to_list(a) |> List.find(f)` reaches them. Putting them in both modules would blur which type to use.
- `Map[Int, A]` keyed by position (the workaround). O(log n), holes in the keys, and no boundary type.
- O(1) `List.get` by changing the representation of List. The representation of List is fixed by its patterns (`[x, ..rest]`) and by `List.tail` being O(1).
- A mutable array held in State or a handler. Every other value is immutable (`[dyn.memory]`); mutation would have to be tracked in effects, and an array could not be put in a Map or compared.
- Array in core and the Prelude. Core holds the types the compiler knows as syntax (`[lib.layers]`), and Array has no syntax. Placing it in core would make `array.emel` a reserved file name (`[module.name]`). If an array literal is added later, this can be revisited.
- No boundary for Array, with sequences passed to the host in a later proposal. Handler authors would keep writing one external function per element.
- Only arrays of the scalar boundary types at the boundary. The rule would need an exception, while the recursive rule copies a nested array the same way.
- A new code for Array on the `c` target instead of extending E0443. Both are decided together with the C form of 18.1 #38, so one code for "not supported at the boundary yet" is enough.

## Prior art

- Immutable arrays updated in place when the reference is unique, found by reference counting at run time ("functional but in-place"): Lean 4 `Array`, Koka (Perceus), Roc `List`.
- Uniqueness types that make in-place update part of the type: Clean, linear arrays in Linear Haskell.
- Persistent vectors with O(log n) update: Clojure `PersistentVector`, Scala `Vector`, Elm `Array` (relaxed radix balanced trees).
- Immutable flat arrays with copying update: Haskell `Data.Array`, `Data.Vector`.
- A binary search that returns the first position not less than the value (lower bound): C++ `std::lower_bound`, Rust `partition_point`.

## Open questions

- `binary_search` returns only a position whose element is equal. Whether to also return the position where `value` would be inserted (for example a separate `lower_bound(a, value) -> Int`), which the bisection already finds.

## Decision
