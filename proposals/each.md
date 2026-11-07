---
title: each for List, Array, Map, Set and Option
authors: [Alliana]
status: draft
pr: 514
issue:
decided:
touches: [api.list, api.option, api.std.array, api.std.map, api.std.set]
requires: [array]
related: []
---

# each for List, Array, Map, Set and Option

## Summary

`each(xs, f)` calls `f` on each element, in the order of the collection, for its effects, and returns `()`. It is added to List, Array, Map, Set and Option.

```emel
List.each(List.range(0, n), |i| {
  clock.sleep(100)
  console.print_line("#{n - i}")
})
```

## Motivation

- Running an effect for each element is written with `map` today, and the collection of `()` it builds is thrown away. Four places in this repository do this: `examples/clock`, `examples/hello`, `examples/http` and `crates/emela-codegen-c/tests/programs/effect_labels.emel` (`_ = List.map(["x", "y"], log.log)`). A reader cannot tell from the call whether the result is wanted.
- `[api.list]` says only that `map` "applies `f` to each element". It does not say in which order, so a program that prints or sleeps inside `map` (as `examples/clock` does) depends on an order the specification does not give. A function whose purpose is the effects has to state the order.
- The alternatives are a recursive helper (a `match` on `[]` and `[x, ..rest]`) or `fold` with `from: ()`. Both are longer than the intent. For Map and Set, `fold` is the only way, and for Option it is a `match`.

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
  | `Option.each(opt, f)` | Once with the contents if `Some`; not at all if `None` |

- The fails and uses of `f` are omitted inside a parameter type, so they are hidden set variables (`[type.fn]`): the errors and effects of `f` are those of the call. When `f` fails, the remaining elements are not visited and the error propagates.
- `f` returns `()`. Passing a function that returns a value is a type mismatch, so a call that should have been `map` is found.

### Typing rules

No new rules.

## Specification changes

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

### `[api.option]`

Add the row, after `map`:

| `each` | `each[A](opt: Option[A], f: fn(A) -> ()) -> ()` | Emela | Calls `f` on the contents if `Some` |

## Diagnostics

- [ ] None.

## Compatibility

Additive. A user module that defines its own `each` is not affected, because these are reached through the module of the type (`List.each`, `xs.each(f)`).

## Implementation

- `emela-core/lib/core/list.emel` and `option.emel`; `emela-core/lib/std/array.emel`, `map.emel` and `set.emel`. All are written in Emela: by recursion for List, by index for Array, by the same traversal as `fold` for Map and Set, and by a match for Option.
- Rewrite the four places listed in Motivation to `List.each`.
- Tests: programs under `emela-codegen-c/tests/programs/` that print in `each` for each type and fail in the middle, also in `wasm_parity.rs`.

## Alternatives

- **The name `for_each`.** Longer; the names of these modules are single words (`map`, `fold`, `find`).
- **`f: fn(A) -> B`, discarding the values.** Accepts any function, including `map`-like ones by mistake. Rejected for the type mismatch above.
- **Stating the order of `map` instead.** Useful on its own (see Open questions), but it does not remove the collection of `()` or say that the result is unused.
- **Only List.** Leaves `fold` with `from: ()` for Map and Set, and a match for Option. The same name in every collection lets code move between them.

## Prior art

- Rust: `Iterator::for_each`, `Option::inspect`. Ruby: `Array#each`, `Hash#each`. Kotlin: `Iterable.forEach`, `Map.forEach`. Effect (TypeScript): `Effect.forEach` with `discard: true`.

## Open questions

- Whether this proposal also states that the other functions of these modules that take `f` (`map`, `filter`, `filter_map`, `flat_map`, `fold`, `any`, `all`, `find`, `map_values`, ...) call it in the order of the collection, and that `any`, `all` and `find` stop at the first element that decides the result.

## Decision
