---
title: Remove the compare functions that Ord already gives
authors: [Alliana]
status: accepted
pr: 468
issue:
decided: 2026-10-28
touches: [api.int, api.int64, api.std.duration, api.std.instant, target.number, open.list]
requires: [method-call]
related: [method-call]
---

# Remove the compare functions that Ord already gives

## Summary

`Int.compare`, `Int64.compare`, `Duration.compare` and `Instant.compare` are removed. These types satisfy Ord, and `Ord.compare` gives the same order. With method calls (`[type.rules]`), `n.compare(m)` finds both the fn of the module and the function of Ord, and is E0335. `Float.compare` stays: Float does not satisfy Ord.

## Motivation

- A method call looks up the fns of the module of the type and the functions of the visible traits, and more than one candidate is E0335 (`[type.rules]`). Ord is in the Prelude, so it is visible in every module.
- Int, Int64, Duration and Instant satisfy Ord (Int and Int64 by the built-in impls, `[trait.builtin]`; Duration and Instant by `derive Ord`), and their modules also have a pub fn `compare` whose first parameter has the type. So for these four types, `x.compare(y)` is always E0335.

  ```emel
  fn order(a: Int, b: Int) -> Ordering {
    a.compare(b)  # E0335: `Int.compare` and `Ord.compare`
  }
  ```

- The two functions give the same result. The built-in Ord of Int and Int64 is the order of integers, and `derive Ord` of Duration and Instant compares the one field `milliseconds: Int64`, which is the order the module functions implement.
- Today the only way around it is to write `Int.compare(a, b)` or `Ord.compare(a, b)`, which method calls were meant to make unnecessary for the most common comparison.

## Proposal

### Meaning

- Remove `compare` from `[api.int]`, `[api.int64]`, `[api.std.duration]` and `[api.std.instant]`.
- `Ord.compare(a, b)`, `a.compare(b)` and the operators `<`, `<=`, `>`, `>=` compare values of these types. The order does not change.
- `min` and `max` of Int and Int64 stay, with the same results.
- `Float.compare` stays. Float does not satisfy Ord (`[trait.builtin]`), so `x.compare(y)` with `x: Float` finds only `Float.compare`.

## Specification changes

### `[api.int]`

Remove the row of `compare`.

### `[api.int64]`

Remove the row of `compare`.

### `[api.std.duration]`

Remove the row of `compare`.

### `[api.std.instant]`

Remove the row of `compare`.

### `[target.number]`

Replace "The comparison functions (`Int.compare`, `Float.compare` and so on), and `min` and `max` built on them, compare by a total order, unlike the operator `<`." with:

> `Float.compare`, and `min` and `max` of Int, Int64 and Float, compare by a total order. For Float, this order differs from the operator `<`.

### `[open.list]`

In question 44, replace "Int and Float have `min`, `max`, `abs`, and `compare`" with "Int has `min`, `max` and `abs`, and Float has `min`, `max`, `abs` and `compare`".

## Diagnostics

None. A call of a removed function is the existing E0212 (no such member).

## Compatibility

- `Int.compare(a, b)`, `Int64.compare(a, b)`, `Duration.compare(a, b)` and `Instant.compare(a, b)` stop compiling (E0212). Each is replaced by `Ord.compare(a, b)` or `a.compare(b)`, with the same result. This is a breaking change without a deprecation period: a deprecated fn would still be found by method calls, and `x.compare(y)` would stay E0335 for that period.
- In the repository: `std/clock.emel`, the internal uses in `core/int.emel` and `core/int64.emel`, and tests (`core_numeric.emel`, `run.rs`, `release_0_24/helpers.emel`).

## Implementation

- emela-core: remove the four fns; `min`, `max` and `std/clock.emel` call `Ord.compare`.
- Update the tests that call the removed functions.

## Alternatives

- Keep the functions and let the fn of the module win over the trait in a method call. Which function `x.f()` calls would then depend on whether the module declares `f`, and adding a fn to a module would silently change calls that used the trait. `method-call` chose E0335 for this reason.
- Keep the functions as `@deprecated` for one minor version. They would keep `x.compare(y)` at E0335 for that period, which is the problem this proposal removes.
- Keep them and leave `x.compare(y)` an error. Comparing integers is among the most common calls, and both qualified forms already exist.

- Forbid, as a compile error, a fn of the module of a type that has the name of a function taking `self` of a trait the type implements, as E0334 does for fields. An impl can also be written in the module of the trait (`[trait.impl]`), so the check could only cover the traits of the same module and of the Prelude, and the call already reports E0335.

## Prior art

## Open questions

## Decision

With method calls, `n.compare(m)` found both the module's fn and `Ord.compare` for Int, Int64, Duration and Instant, so the most common comparison was always E0335. The two functions give the same order, so removing the module's fn loses nothing, and `Ord.compare(a, b)` and `a.compare(b)` replace it. Letting the module's fn win over the trait would make adding a fn to a module silently change calls that used the trait, which is what `method-call` chose E0335 to prevent. A deprecation period would keep the ambiguity, so the change breaks code at once; before 1.0 that is acceptable. `Float.compare` stays because Float does not satisfy Ord. A check at declaration could only cover the traits of the same module and the Prelude, and the call already reports E0335, so it is not added.
