---
title: Guards, the operand of `!` and the expression of assert are positions with the expected type Bool
authors: [somnicattus]
status: accepted
issue:
touches: [fn.match, fn.operator, test.assert, type.rules]
requires: []
related: []
pr: 956
decided: 2026-11-29
---

# Guards, the operand of `!` and the expression of assert are positions with the expected type Bool

## Summary

The guard of a match or escape arm, the operand of `!` and the expression of `assert` are positions with the expected type Bool (`[type.rules]`). This rule already holds for the condition of an if (`[fn.if]`). The type of each of these positions is therefore Bool or Never. The E and R of a guard flow into the match or escape it belongs to. The compiler does all of this already, so no program changes meaning. A question raised in #908, whether a condition whose type is Never should be a compile error so that if, `&&` and `||` are defined by their rewrite into a match alone, this proposal answers no: a condition of type Never stays accepted (Alternatives).

## Motivation

### The specification does not give a type to a guard

`[fn.if]` says that "the condition is a position with the expected type Bool (`[type.rules]`), so its type is Bool or Never". No sentence gives a type to a guard. `[fn.match]` and `[error.escape]` say how guards count for exhaustiveness, redundancy and the removal of tags. `[dyn.order]` says when a guard is evaluated. `[grammar.expr]` writes a guard as `pattern [ "if" expr ]`. None of them says that the expression has to be a Bool.

The compiler checks a guard as a position with the expected type Bool (`check_expr(guard, &Ty::BOOL)` in `crates/emela-types/src/body.rs`, for match and for escape arms). Checked with emela 0.29.0+5ad03e7:

```emel
fn f(o: Option[Int]) -> Int {
  o match {
    Some(x) if x -> 1 # E3001: expected `Bool`, found `Int`
    _            -> 0
  }
}
```

A guard of type Never is accepted (`Some(x) if todo() -> x`, `Some(x) if fail Boom -> x`), as it is for the condition of an if.

### The same gap for `!` and assert

`[fn.operator]` lists `!x` among the unary operators but does not give the type of its operand or of the whole. The compiler checks the operand against Bool (`!5` is E3001 "expected `Bool`, found `Int`") and accepts `!(fail Boom)`.

`[test.assert]` says "the type of the expression is Bool". The compiler checks the expression against Bool and accepts `assert todo()` and `assert fail Boom`. Read literally, the sentence rejects a Never expression. The compiler also checks both sides of `&&` and `||` against Bool; the specification gets the same types from their rewrite into if (`[fn.operator]`), so they need no new rule.

### The E and R of a guard are left out of the table

`[type.rules]` says that "unless stated otherwise, the E and R of subexpressions flow into the parent expression", but its rows for match and escape state their E and R in full: `Ee ∪ Eei` and `Re ∪ Rei` for match, `(Ee ∖ {Ti}) ∪ Eei` and `Re ∪ Rei` for escape, where ei are the bodies of the arms. A guard is not among the terms, so the rows can be read as stating otherwise. The compiler adds the E and R of a guard: a guard `p(x)` where p has `fails Boom` gives E3201 "unhandled error `Boom`" in a function without `fails`, and a guard that calls an operation of Console makes the function require Console. A guard of an escape arm that fails with another error propagates that error to the caller.

### The question left by #908

#908 defined if by its rewrite into a match (`[fn.if]`) but kept the type of the condition as a rule of if: the scrutinee of a match is inferred without an expected type, so `(fail Boom) match { ... }` with the arms `True` and `False` is E3001 ("expected `Never`, found `Bool`") at the patterns, while `if fail Boom { 1 } else { 2 }` is accepted. A comment on #908 asked whether a condition of type Never should become a compile error, so that if, `&&` and `||` are defined by the rewrite alone, and noted that the guard would raise the same question. #908 was merged without an answer. Giving the guard its type takes a side on that question, since the comment named the guard. This proposal keeps Never accepted, at the guard and at the other conditions.

## Proposal

### Syntax

No change.

### Meaning

No change. The guard, the operand of `!` and the expression of assert are evaluated as they are today (`[dyn.order]`).

### Typing rules

- `[fn.match]`: the guard of an arm, in a match or in an escape (7.3), is a position with the expected type Bool (`[type.rules]`), so its type is Bool or Never.
- `[fn.operator]`: the operand of `!` is a position with the expected type Bool, so its type is Bool or Never. The type of `!x` is Bool.
- `[test.assert]`: in `assert expr`, the expression is a position with the expected type Bool, so its type is Bool or Never. (Replaces "the type of the expression is Bool".)
- `[type.rules]`: in the rows for match and escape, the E and R of the guards are added: `Ee ∪ Egi ∪ Eei` and `Re ∪ Rgi ∪ Rei` for match, and `(Ee ∖ {Ti}) ∪ Egi ∪ Eei` and `Re ∪ Rgi ∪ Rei` for escape, where gi are the guards of the arms (an arm without a guard adds nothing). When an escape has an arm `_`, its E is Egi ∪ Eei instead of Eei: `_` cannot have a guard, but the arms above it can.

`[fn.if]` is not changed: its sentence on the condition and the exception "apart from the type of the condition" stay as they are.

## Diagnostics

No new diagnostics. A guard, an operand of `!` or an expression of assert of another type is E3001, as the compiler reports it today.

## Compatibility

No program changes meaning. The rules state what the compiler checks today.

## Implementation

The compiler already follows these rules. Tests to add in `emela-types`, from the rules:

- a guard of type Int is E3001, in a match arm and in an escape arm; a guard of type Never is accepted in both
- `!` on an Int is E3001; `!` on Never is accepted
- `assert` on Never is accepted
- the E and R of a guard reach the function (E3201 for an error of the guard, E4301 at the caller for an effect of the guard), in a match arm and in an escape arm

## Alternatives

### A condition of type Never is a compile error

The question raised in #908, not taken. The condition of an if, a guard, the operands of `&&`, `||` and `!`, and the expression of assert would reject Never. The aim would be to let `[fn.if]` drop its exception, so that if, `&&` and `||` are defined by their rewrite into a match alone; the third point below is why it does not get there.

Against it:

- It is an exception to a general rule. `[type.rules]` says that Never is treated as another type "at the top level of a position with an expected type". Every position with an expected type accepts Never today: arguments, the return type, the annotation of a binding. The proposal applies that rule to the conditions; this alternative would add a position with an expected type that rejects Never, which no position does today.
- `todo()` is the way to leave an expression unwritten. `if todo() { ... }` and `Some(x) if todo() -> ...` are how one writes a branch before its condition is known, and they would stop compiling.
- The gain is small. The rewrite itself would reject Never (E3001 at the pattern `True`), so the rule "its type is Bool or Never" could go. But the condition would still be checked with the expected type Bool, which reaches the inside of the condition (a block's last expression, the branches of an inner if or match, `[type.rules]`), and the scrutinee of a match is checked without an expected type. Whether some program tells the two apart is not known, so `[fn.if]` would still keep the expected type of the condition as its own rule.

For it: a condition of type Never is almost never what the author meant, since the branch after it is never reached. No program in the repository has one (std, core, examples, tests and the Markdown files, searched for `if todo(`, `if fail `, `if panic(`, and `todo()` or `panic(...)` before `&&` and `||`). A warning would say this without breaking `todo()`, but Emela has no warning for unreachable code other than redundant arms (W3101), and adding one is a separate question.

### The scrutinee of a match accepts Never

Instead of changing if, change match: a scrutinee of type Never would be accepted with any patterns, because no arm can be reached. The rewrite of `if fail Boom { 1 } else { 2 }` would then be accepted, and `[fn.if]` could drop its exception without rejecting anything. This changes match for every program, not only for the rewrite of if, and it would also need an answer for `[fn.match]`'s rule that "a match with no arms is not exhaustive for a value of any type, including `Never`" (E3102). It does not make the rewrite exact either: the condition of an if is checked with the expected type Bool and the scrutinee without one, so the expected type that reaches the inside of the condition (`[type.rules]`, a block's last expression and the branches of an inner if or match) would still differ.

### Leave the guard unspecified

The compiler is the only description of the type of a guard. A second implementation, or a reader of the specification, has no rule to follow, and `[test.assert]` reads as rejecting `assert todo()`, which the compiler accepts.

## Prior art

None.

## Open questions

## Decision

A guard, the operand of `!` and the expression of assert are given the expected type Bool, which states what the compiler already checks; no program changes meaning. The question left by #908 is answered by keeping a condition of type Never accepted, as it is for the condition of an if.
