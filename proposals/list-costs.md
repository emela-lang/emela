---
title: The costs of List operations
authors: [somnicattus]
status: draft
issue:
touches: [api.list, lib.layers]
requires: []
related: [array, bytes]
---

# The costs of List operations

## Summary

State in `[api.list]` how long each operation on a List takes, on every target. Building `[x, ..rest]` and matching `[x, ..rest]` take O(1) whatever the length of `rest`, and share `rest` without copying it. `append(xs, ys)` takes O(length of `xs`) and shares `ys`. The functions that walk a list take O(n), and none of them uses deeper stack as the list grows. This is the property that makes "add each element in front and reverse once at the end" the way to build a list, and today the specification does not state it. `[lib.layers]`, which states the costs of Map and Set, points to the costs of List.

## Motivation

The specification states the costs of Map and Set (`[lib.layers]`), Bytes (`[api.std.bytes]`) and Array (`[api.std.array]`), but not of List, the sequence that most programs use. Of List, `[lib.layers]` says only "A List is read from the front and built a little at a time". It also says that "even a large Map does not use deep stack", and nothing says the same of List, though `[target.tail-call]` leaves the depth of recursion other than tail calls to the function itself to the implementation.

Whether a loop is linear or quadratic depends on these costs. Adding at the end in each step copies the whole list each time:

```emel
fn by_append(n: Int, i: Int, acc: List[Int]) -> List[Int] {
  if i == n { acc } else { by_append(n, i + 1, List.append(acc, [i])) }
}

fn by_prepend(n: Int, i: Int, acc: List[Int]) -> List[Int] {
  if i == n { List.reverse(acc) } else { by_prepend(n, i + 1, [i, ..acc]) }
}
```

Both return the same list. With the compiler at `0.29.0+5ad03e7` (2026-11-28), `by_append` takes 186 ms, 674 ms and 2497 ms for 5000, 10000 and 20000 elements on JS, and 241 ms, 985 ms and 3795 ms on WASM: four times as long for twice the elements. `by_prepend` takes 1 ms or less for each.

A program can rely on this only through the implementation. All three targets represent a List as cells that each hold one element and the rest (`crates/emela-codegen-js/src/lib.rs`, `em_cons` on native, the list cells of WASM), and `core/list.emel` builds every result in front and reverses it, but neither is part of the specification. Teaching material that explains the idiom can cite no rule for it. `proposals/array.md` rejects an O(1) `List.get` because "the representation of List is fixed by its patterns (`[x, ..rest]`) and by `List.tail` being O(1)", and neither the cost of the patterns nor that of `List.tail` is stated.

## Proposal

### Meaning

The costs are stated as bounds in O notation, as for Bytes and Array. They hold on every target. A call of an argument `f` counts as one step, apart from the steps inside `f`. The bounds are upper bounds: an implementation may be faster.

- Building a list with `[x, ..rest]` (`[lex.literal]`, `[grammar.expr]`) takes O(1) whatever the length of `rest`, and the result shares `rest` instead of copying it. `[x1, ..., xk, ..rest]` and `[x1, ..., xk]` take O(k), apart from evaluating the elements.
- Matching `[]`, `[x, ..rest]` and `[x, ..]` (`[fn.match]`) takes O(1), and `rest` is bound to the rest of the list without copying it. A pattern with k elements takes O(k), apart from matching the patterns written for the elements.
- `==`, `Ord.compare` and the functions of `[api.list]` other than `join` take the times in the specification changes below.
- None of them uses deeper stack as the list grows. The stack used inside `f` is not counted. A program can pass a list of a million elements to `List.map` and know that the call does not overflow the stack because of the length.
- All of these costs are stated in `[api.list]`, including those of the list expression and the list pattern, so that every cost of List is in one place, as the costs of Bytes and Array are in their sections.
- `join` and Show (`impl List[A: Show]: Show`) are not covered. Their costs depend on the costs of String, which the specification does not state; they are left to a proposal that states those.

Sharing is not observable through values, since a List does not change. It is stated so that the memory a program uses can be predicted: a thousand lists made with `[x, ..base]` hold `base` once.

### Typing rules

No new rules.

## Specification changes

### `[api.list]`

Add after the table, before the impl line:

- The operations on List take the following times on every target, with n the length of the first argument. A call of `f` counts as one step, apart from the steps inside `f`.
  - Building `[x, ..rest]` is O(1) whatever the length of `rest`, and the result shares `rest` instead of copying it. Matching `[]`, `[x, ..rest]` or `[x, ..]` is O(1), and `rest` is bound to the rest of the list, not to a copy. A list expression or a list pattern that writes k elements (`[x1, ..., xk]`, `[x1, ..., xk, ..rest]`) is O(k), apart from evaluating the expressions and matching the patterns written for the elements.
  - `is_empty`, `head` and `tail` are O(1).
  - `get`, `take` and `drop` are O(the smaller of n and their second argument), and O(1) when that argument is not positive.
  - `append(xs, ys)` is O(the length of `xs`), and the result shares `ys`.
  - `flatten(xss)` and `flat_map(xs, f)` are O(n + the length of the result).
  - `range(from, to)` is O(`to - from`), and O(1) when `to <= from`.
  - `sort_by` calls `f`, and `sort` calls `Ord.compare`, O(n log n) times, and both take O(n log n).
  - `==` and `Ord.compare` on two lists compare at most as many pairs of elements as the shorter list has.
  - `length`, `last`, `reverse`, `map`, `each`, `filter`, `fold`, `partition`, `any`, `all`, `find`, `find_index`, `filter_map`, `zip`, `unzip` and `enumerate` are O(n).
  - None of these operations uses deeper stack as the list grows. The stack used inside `f` is not counted.

### `[lib.layers]`

In the item on Array, point to the costs of List, as the items on Bytes and Array point to their declarations:

- Before: "A List is read from the front and built a little at a time; an Array is built from a List (`Array.from_list`) and then read by position."
- After: "A List is read from the front and built a little at a time (its costs are in `[api.list]`); an Array is built from a List (`Array.from_list`) and then read by position."

## Diagnostics

None.

## Compatibility

No program changes meaning. The three targets already meet every bound and do not use deeper stack for longer lists (Implementation). A future implementation that changes the representation of List must keep them.

## Implementation

Nothing changes. The bounds were read from `crates/emela-core/lib/core/list.emel` and the list cells of the three back ends:

- JS: `{ h, t }` cells, the empty list `null` (`crates/emela-codegen-js/src/lib.rs`).
- WASM: list cells with a descriptor for a reference head and one for a word head (`crates/emela-codegen-wasm/src/emit/mod.rs`).
- native: `em_cons` (`crates/emela-codegen-c/src/emela_runtime.h`).

`append` is `reverse_onto(reverse(xs), ys)`, `flatten` appends each list onto the rest from the back, and `sort_by` is a bottom-up merge sort. Every loop in `core/list.emel` is a tail call to the function itself (`[fn.loop]`), which `[target.tail-call]` guarantees not to consume stack on every target; `==` and `Ord.compare` recurse through `eq_lists` and `compare_lists` for the same reason. With `0.29.0+5ad03e7`, every function of the table, `==` and `Ord.compare` ran on a list of 1000000 elements on JS and WASM without overflowing the stack (native was not run).

## Alternatives

- Leave the costs unstated (today). The idiom stays a fact of the implementation, and a program that builds a list at the end runs in quadratic time with nothing in the specification to say so.
- Add a row "List: cells of one element and the rest" to `[target.impl-hint]`. It is informative, so programs could not rely on it, and it states a representation rather than what a program observes.
- State the representation (a singly linked list) normatively. It says more than programs need, and the costs would still have to be derived from it.
- State the cost of `[x, ..rest]` and of list patterns in `[grammar.expr]` and `[fn.match]`, where the forms are defined. The costs of List would then be in three places, and no section of the language chapters states a cost today.
- Leave out the promise about the stack, and state only the times. A program that passes a long list would still depend on the implementation to know whether it overflows, though the functions are already written so that it does not.
- Add an operation that adds at the end in O(1). An immutable List cannot do this without changing its representation; a sequence that is read by position is Array, built from a List (`[lib.layers]`).

## Prior art

- Haskell's `Data.List` states the time of some functions: `last` is O(n), and `++` "takes linear time in the number of elements of the first list" (base 4.22). Gleam's `gleam/list` says that `length` and `append` run in linear time, that `append` "traverses and copies the first list", and that elements are added and removed at the front in constant time (gleam_stdlib 1.2).
- The specification of Emela states the costs of Map and Set (`[lib.layers]`), Bytes (`[api.std.bytes]`) and Array (`[api.std.array]`) in the same notation.

## Open questions


## Decision
