---
title: each for List, Array, Map and Set
authors: [Alliana]
status: draft
pr: 514
issue:
decided:
touches: [lib.api-rules, api.list, api.std.array, api.std.map, api.std.set]
requires: [array, method-call]
related: []
---

# each for List, Array, Map and Set

## Summary

`each(xs, f)` calls `f` on each element, in the order of the collection, for its effects, and returns `()`. It is added to List, Array, Map and Set.

`[lib.api-rules]` also gains a rule for every function of the standard library that calls `f` once per element (`map`, `filter`, `fold`, `find`, ...): it calls `f` in the order of the collection, at most once per element, and not after the result is decided or `f` has failed.

```emel
List.each(List.range(0, n), |i| {
  clock.sleep(100)
  console.print_line("#{n - i}")
})
```

## Motivation

- Running an effect for each element is written with `map` today, and the collection of `()` it builds is thrown away. Four places in this repository do this: `examples/clock`, `examples/hello`, `examples/http` and `crates/emela-codegen-c/tests/programs/effect_labels.emel` (`_ = List.map(["x", "y"], log.log)`). A reader cannot tell from the call whether the result is wanted.
- `[api.list]` says only that `map` "applies `f` to each element". It does not say in which order, so a program that prints or sleeps inside `map` (as `examples/clock` does) depends on an order the specification does not give. The descriptions disagree among themselves: `Array.map` says "from the first", `List.map` says nothing, and `List.any` says where it stops. Adding `each` alone does not give `map`, `filter` or `find` an order, and code that fails or logs inside them depends on it as much as code inside `each` does.
- The alternatives are a recursive helper (a `match` on `[]` and `[x, ..rest]`) or `fold` with `from: ()`. Both are longer than the intent. For Map and Set, `fold` is the only way.

## Proposal

```emel
## Calls `f` on each element, from the first to the last.
pub fn each[A](xs: List[A], f: fn(A) -> ()) -> ()
```

- `each(xs, f)` calls `f` once for each element, in the order below, and its value is `()`.

  | Function | Order |
  |---|---|
  | `List.each(xs, f)` | From the first element to the last |
  | `Array.each(a, f)` | From index 0 to the last |
  | `Map.each(m, f)` | In key order, `f(key, value)` |
  | `Set.each(s, f)` | In the order of the elements (the order of `Set.fold`) |

- The fails and uses of `f` are omitted inside a parameter type, so they are hidden set variables (`[type.fn]`): the errors and effects of `f` are those of the call. When `f` fails, the remaining elements are not visited and the error propagates.
- `f` returns `()`. Passing a function that returns a value is a type mismatch, so a call that should have been `map` is found.

### The order in which `f` is called

A rule in `[lib.api-rules]` covers every function of the standard library that calls `f` with the elements of a collection, now and in later versions.

- `f` is called in the order of the collection: List from the first element, Array from index 0, Map and Set in key order. It is called at most once per element.
- A function whose result is decided before the last element (`any`, `all`, `find`, `find_index`) does not call `f` on the elements after the one that decides it.
- When `f` fails, `f` is not called on the remaining elements, and the error propagates.
- Excluded:
  - `sort_by`. How many times its `f` is called, and with which pairs, is not specified, so that the sorting algorithm can change.
  - The functions of `Std.Fiber` (`map`, `all`). They run concurrently, as `[api.std.fiber]` states.
- `Option.map`, `Option.and_then` and `Map.update` call `f` at most once, so the rule holds for them without anything further.

`Map.map_values` today calls `f` on the root of the tree before its subtrees, not in key order. It is changed to follow the rule (see Implementation).

### Typing rules

No new rules.

## Specification changes

### `[lib.api-rules]`

Add after the item on the argument named `f`:

- A function that calls `f` with the elements of a collection calls it in the order of the collection (List from the first element, Array from index 0, Map and Set in key order), at most once per element. When the result is decided before the last element (`any`, `all`, `find`, `find_index`), or when `f` fails, `f` is not called on the remaining elements. This does not apply to `sort_by`, whose calls of `f` are not specified, or to the functions of `Std.Fiber`, which run concurrently (`[api.std.fiber]`).

### `[api.list]`

Add the row, after `map`:

| `each` | `each[A](xs: List[A], f: fn(A) -> ()) -> ()` | Emela | Calls `f` on each element, from the first to the last. Stops at the first failure |

### `[api.std.array]`

Add the row, after `map`:

| `each` | `each[A](a: Array[A], f: fn(A) -> ()) -> ()` | Emela | Calls `f` on each element, from the first. Stops at the first failure |

### `[api.std.map]`

Add the row, after `fold`:

| `each` | `each[K, V](m: Map[K, V], f: fn(K, V) -> ()) -> ()` | Emela | Calls `f` on each pair in key order. Stops at the first failure |

### `[api.std.set]`

Add the row, after `fold`:

| `each` | `each[A](s: Set[A], f: fn(A) -> ()) -> ()` | Emela | Calls `f` on each element in order. Stops at the first failure |

## Diagnostics

- [ ] None.

## Compatibility

Additive. The order rule changes no result, because the functions are pure apart from `f`; it changes only the order in which the effects and failures of `f` happen, and only in `Map.map_values`. A user module that defines its own `each` is not affected, because these are reached through the module of the type (`List.each`, `xs.each(f)`).

## Implementation

- `emela-core/lib/core/list.emel`; `emela-core/lib/std/array.emel`, `map.emel` and `set.emel`. All are written in Emela: by recursion for List (a self tail call, which uses no stack even when `f` can fail; `[fn.loop]`), by index for Array, and by the same traversal as `fold` for Map and Set.
- `Map.map_values` in `emela-core/lib/std/map.emel`: build the left subtree, then call `f` on the value, then build the right subtree, so that `f` is called in key order.
- Rewrite the four places listed in Motivation to `List.each`.
- Tests: programs under `emela-codegen-c/tests/programs/` that print in `each` for each type and fail in the middle, also in `wasm_parity.rs`. Programs that record the order of calls in `map`, `filter`, `filter_map`, `flat_map`, `partition`, `fold`, `find_index` and `map_values`, and where `any`, `all` and `find` stop.

## Alternatives

- **The name `for_each`.** Longer; the names of these modules are single words (`map`, `fold`, `find`).
- **`f: fn(A) -> B`, discarding the values.** Accepts any function, including `map`-like ones by mistake. Rejected for the type mismatch above.
- **Stating the order of `map` instead of adding `each`.** This proposal states the order too, but the order alone does not remove the collection of `()` or say that the result is unused.
- **Stating the order in each row of the tables.** Repeats the same words in every module, and a collection added later has to repeat them again. One rule in `[lib.api-rules]` covers them.
- **Including `sort_by` in the rule.** Fixes the number and the pairs of comparisons, which ties the implementation to one algorithm.
- **A concurrent `each` in `Std.Fiber`.** Calls `f` on the elements concurrently and discards the values, as `Fiber.map` does without its result. Not here: this proposal fixes the order of calls, which a concurrent function does not have, and no program in this repository discards the result of `Fiber.map` today. It belongs with Semaphore and Queue, or a later proposal, if running several workers needs it.
- **Only List.** Leaves `fold` with `from: ()` for Map and Set. The same name in every collection lets code move between them.
- **`Option.each` as well.** No code in this repository would use it: the one `None -> ()` arm is in a function that fails on `Some`. A `match` is enough, and Option has no other function that treats it as a collection (`fold`, `filter`). It can be added together with such functions.

## Prior art

- Rust: `Iterator::for_each`. Ruby: `Array#each`, `Hash#each`. Kotlin: `Iterable.forEach`, `Map.forEach`. Effect (TypeScript): `Effect.forEach` with `discard: true`.

## Open questions

- None.

## Decision
