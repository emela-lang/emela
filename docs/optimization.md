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
  3 loops           unswitching; loop-invariant motion and induction
                    variables are reserved
  4 analyses        effects and defects, uses, call graph, ranges
Monomorphic Core IR (after monomorphisation)
  5 δ stage         one δ-only pass over the monomorphic copies: the rule of
                    1 with the same budget, run where a callee that was
                    effect-polymorphic before (a higher-order wrapper) is
                    concrete. The CoreFns identity map is rebuilt through
                    Monomorphized::origins, so the reserved bundled functions
                    stay calls. No other rule or fusion law runs here.
LIR (after lowering; heap passes after reference counting)
  5 LIR             analyses: escape, liveness, borrow, ownership;
                    passes: SRA of tuples, rc cleanup with the borrow
                    elision, DCE, borrow inference, switch normalisation,
                    and closure devirtualisation with closure SRA.
                    Drop specialisation is implemented but opt-in because
                    its measured size cost has no measured runtime gain;
                    reuse of a uniquely owned constructor (its old reference
                    fields released first) is C4 and is opt-in too; record
                    SRA rewrites nothing on the corpus and stays off.
                    Suspension frame pruning is reserved.
Lowering and code generation
  6 representation  string concat, value records (on by default in the
                    WASM backend since E1 step 6), unboxed arrays (opt-in,
                    E2), tagging, dead code and import pruning, switches
Runtime
  A fixed implementation of reference counting, allocation, strings and
  the scheduler. It is not an optimisation layer: no JIT, no runtime
  profiling, no self-modifying code.
```

The analyses in layer 4 and in layer 6 are shared: a rewrite consumes a tested analysis and
gives up when the analysis answers `Unknown`.

The WASM backend holds a record or tuple whose leaves are all scalars by value by default: a span
of locals or parameters, one per leaf, and a flattened call signature (E1 step 6). The measurement
that decided it left the WASM size unchanged (the median per-item delta is 0.0% and no item moves
by more than 0.7%) and cut the allocations of the native record loops from 20,002 to 2 and from
60,002 to 2. A value is still boxed where a
boundary stores one reference: a list head, an enum payload, a closure capture or environment
slot, `Any`, and an array element. The entry thunk, exports, externals, constants and error shows
keep the boxed form, so a type used there stays boxed everywhere, and two results stay boxed
because their runtime path carries one word: a suspending function's or operation's result of
more than one leaf, and a function value passed to a fiber operation together with its parameters
and result. Array elements stay boxed unless the opt-in E2 `aggregate_arrays` stores an all-scalar
element inline. `WasmOptions::boxed()` selects the representation from before E1 step 6.

### Default change: closure devirtualisation and closure SRA

Closure devirtualisation (E5/PR-037) and closure SRA (C5/PR-043, closure half) run by default. This
is a default change, not a rule change: their contracts, preconditions, deopts and counters are
unchanged, and a build that sets the flags off gets exactly the previous output. The default changed
because, with both on, the `effects` benchmark measured 0.86–0.91x on WASM and 0.85x on C across
the isolated and integrated runs,
the WASM output of the other 16 benchmark cases was byte-identical, and the passes are covered end to
end by `crates/emela-driver/tests/opt/closure_sra.rs`. Record SRA stays off because it rewrites
nothing on the corpus.

The same shape, `|n| n + 1` made and only called in one function, is what E5 turns into a direct call
and what closure SRA removes the record of; the lifted function is specialised on its scalar
captures and the record and its `env_get` reads disappear. Neither pass moves an allocation it
cannot prove, a drop point or a frame: see their rustdoc for the exact contract.

The measured code size follows the same split: the WASM component of `effects` shrinks by 96 bytes
and that of `mixed-app` by 90, and every other benchmark case keeps its exact bytes. The C source of
the two cases grows (by 6.2% and 3.3%) because closure SRA keeps the original lifted function and
adds the specialised copy, while the linked native binary shrinks slightly (104 and 96 bytes).

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
| [`crates/emela-core/src/opt/mod.rs`](../crates/emela-core/src/opt/mod.rs) | The Core IR pipeline: the order of the passes, the fixed point, `Stats`, the pass contract, and the δ-only stage after monomorphisation. |
| [`crates/emela-core/src/opt/reduce/`](../crates/emela-core/src/opt/reduce/) | The reduction kernel: `subst`, `zeta`, `beta`, `eta`, `iota`, `delta`, `fold`, `algebra`, `bounds` and `cse`. |
| [`crates/emela-core/src/opt/fusion/`](../crates/emela-core/src/opt/fusion/) | The fusion laws: the registry, their order and their budget. |
| [`crates/emela-core/src/opt/loop/`](../crates/emela-core/src/opt/loop/) | Loop transformations: the unswitch; LICM and induction variables are reserved. |
| [`crates/emela-core/src/opt/analysis/`](../crates/emela-core/src/opt/analysis/) | The shared Core IR analyses: effects and defects, uses, call graph, ranges. |
| [`crates/emela-lir/src/opt/mod.rs`](../crates/emela-lir/src/opt/mod.rs) | The LIR stages (`run_flow`, `run_mid`, `run_heap`), `Stats`, the shared analyses. |
| [`crates/emela-lir/src/opt/analysis/`](../crates/emela-lir/src/opt/analysis/) | The shared LIR analyses: escape, liveness, borrow, ownership. |
| [`crates/emela-lir/src/opt/`](../crates/emela-lir/src/opt/) | The LIR passes: SRA of tuples (records behind a flag), rc cleanup with the borrow elision, DCE, borrow inference and switch normalisation (E4), on by default. Switch normalisation writes the canonical case form the backends read: sorted and distinct keys, one case for the bodies that are the same sequence, and a direct block when every arm is that body. Drop specialisation is opt-in: with it on the M3 measurement grew the native binary by up to 7.5% and the C source by up to 3.7%, with no measured runtime gain. Reuse of a uniquely owned, same-constructor record, whose old reference fields it releases first (C4/PR-042) is opt-in, and record SRA rewrites nothing on the corpus and is off by default. Closure devirtualisation (E5/PR-037) and closure SRA of a record's scalar captures (C5/PR-043, after E5) are on by default as of the default change recorded above; suspension frame pruning is reserved. |
| [`crates/emela-lir/src/analysis/reach.rs`](../crates/emela-lir/src/analysis/reach.rs) | The target-independent reachability analysis: called and referenced functions, performed handler operations and reachable externals, on the Flow and Heap IR. `prune_functions` removes neither-called-nor-referenced functions behind `LirRules::prune` (off by default). Both backends read the same analysis for bodies and host imports. |
| [`crates/emela-codegen-c/`](../crates/emela-codegen-c/) | The C backend: representation and layout; a canonical switch is one C `switch` with a label per key. |
| [`crates/emela-codegen-wasm/`](../crates/emela-codegen-wasm/) | The WASM backend: representation (records and tuples by value by default, array elements inline only with E2), dead code elimination, and the switch encoding (`br_table` or the chain of `if`s, chosen from the canonical keys). `WasmOptions::boxed()` restores the pre-E1-step-6 representation. |

A pass's rustdoc states the contract; the module rustdoc states the pipeline and the fixed
point.

## Detailed procedures

The gates are the CI workflows in `.github/workflows/`; the full differential corpus is
`EMELA_OPT_DIFF_FULL=1 cargo test -p emela-driver --test tests opt_diff`. A pass's rustdoc states its
contract and the module rustdoc the pipeline and the fixed point.
