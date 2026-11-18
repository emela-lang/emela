# Optimisation architecture

## Purpose and non-goals

Emela optimises at compile time, in layers: the Core IR is reduced and fused, the LIR chooses
representations and manages references, and the backends choose layouts and prune code. Every
rewrite must be classifiable as one of these layers and must state the equation it applies, the
observable behaviour it keeps and the case where it gives up.

This document is the map. It does not repeat the rules: a rewrite's preconditions, postconditions,
the behaviour it preserves and when it gives up are in the rustdoc of the pass that makes it.

The non-goals are a movement of work to run time (no JIT, no runtime profiling, no
self-modifying code), a general deforestation or supercompilation of user functions, and any change
to observable behaviour. Optimisation never appears in `spec/` as a guarantee.

## Layers

```text
Core IR (target-neutral, before monomorphisation)
  1 reductions      β ζ η ι δ, constant folding, algebra
  2 fusion          range / map / fold / filter, Option and Array.get, strings
  3 loops           hoisting, unswitching, induction variables
  4 analyses        effects and defects, uses, call graph, ranges
LIR (after lowering; heap passes after reference counting)
  5 LIR passes      escape, liveness, borrow, ownership, SRA, closure
                    devirtualisation, rc cleanup, drop specialisation,
                    reuse, suspension frame pruning
Lowering and code generation
  6 representation  string concat, value records, unboxed arrays, tagging,
                    dead code and import pruning, switches
Runtime
  A fixed implementation of reference counting, allocation, strings and
  the scheduler. It is not an optimisation layer: no JIT, no runtime
  profiling, no self-modifying code.
```

The analyses in layer 4 and in layer 5 are shared: a rewrite consumes a tested analysis and
gives up when the analysis answers `Unknown`.

## Terms

| Term | Meaning |
|---|---|
| β reduction | A function bound by a `let` and called once is replaced by its body, its argument bound by a `let`. |
| ζ reduction | A `let` of a variable or scalar literal is substituted, or removed when its binding is unused. |
| η reduction | `λx. f x` becomes `f` when `x` does not occur in `f` and the calling conventions agree. |
| ι reduction | A `match` on a known constructor takes the matching arm and binds its fields. |
| δ reduction | A direct call is replaced by the body of its definition, under a cost model and a budget. It is called *definition expansion*, never "δ-expansion". |
| fusion | A library law replaces a producer and a consumer by one loop, so no intermediate collection is built. |
| SRA | Scalar replacement of aggregates: a non-escaping tuple, record or closure is replaced by its fields. |
| RC | Reference counting: `dup` and `drop` make ownership explicit; rc cleanup removes a pair that only borrows. |
| Perceus | The reference-counting discipline that adds `dup`/`drop` and reuses a uniquely owned constructor; borrow inference, drop specialisation and reuse are its parts. |

Reductions and fusion are rewrites of equalities. SRA, rc cleanup, borrow inference, drop
specialisation, reuse and the representation choices are conversions: they change how a value is
held, not the expression by an equality. The two are kept apart.

## Invariants

Every rewrite and analysis keeps the following.

- Preserved: evaluation order (call by value, left to right), the order and count of effects, fail
  edges and where they are caught, suspension points, the order and count of the releases of
  `with`, drop points, the structure of evidence, and everything observable: stdout, stderr, the
  exit code and test results.
- May change: memory use, the number of allocations, when memory is released and when a program
  runs out of memory; the shape of the IR, code size and compile time within their budgets;
  running time.
- Forbidden: removing, moving or duplicating an effect; removing a defect (division by zero, an
  out-of-bounds access, a failed assertion); moving a computation across the release of a
  `with`; treating a value as pure when a defect does not appear in its type; stating a
  performance guarantee in the specification; choosing a rewrite from runtime state or a profile.

A rewrite that cannot meet these gives up on that expression. Giving up is always allowed;
changing behaviour is not.

## File map

| Where | What |
|---|---|
| [`crates/emela-core/src/opt/mod.rs`](../crates/emela-core/src/opt/mod.rs) | The Core IR pipeline: the order of the passes, the fixed point, `Stats`, the pass contract. |
| [`crates/emela-core/src/opt/`](../crates/emela-core/src/opt/) | The Core IR passes: `hoist`, the `reduce` kernel (including δ, the inliner), `option`, `range`. |
| [`crates/emela-lir/src/opt/mod.rs`](../crates/emela-lir/src/opt/mod.rs) | The LIR stages (`run_flow`, `run_mid`, `run_heap`), `Stats`, the shared analyses. |
| [`crates/emela-lir/src/opt/`](../crates/emela-lir/src/opt/) | The LIR passes and analyses: `escape`, `rc`, `sra`. |
| [`crates/emela-codegen-c/`](../crates/emela-codegen-c/) | The C backend: representation and layout. |
| [`crates/emela-codegen-wasm/`](../crates/emela-codegen-wasm/) | The WASM backend: representation, dead code elimination, switches. |

The Core IR passes are being grouped into `crates/emela-core/src/opt/reduce/` (the reduction
kernel), `fusion/` (the library laws), `loop/` (hoisting and unswitching) and `analysis/` (the
shared analyses). The LIR passes are being grouped into `analysis/`, `rc/` and `sra/` under
`crates/emela-lir/src/opt/`, with closure devirtualisation and suspension frame pruning beside
them. A pass's rustdoc states the contract; the module rustdoc states the pipeline and the fixed
point.

## Detailed procedures

The gates are the CI workflows in `.github/workflows/`; the full differential corpus is
`EMELA_OPT_DIFF_FULL=1 cargo test -p emela-driver --test opt_diff`. A pass's rustdoc states its
contract and the module rustdoc the pipeline and the fixed point.
