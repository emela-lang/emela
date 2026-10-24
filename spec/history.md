# Revision history `[history]`

This is the record of changes to the specification. New entries are added at the top. Each entry names the sections it changed.

## 2026-10-23 (The WASM output becomes a WASI 0.3 component)

- `[target.wasm.output]`, `[target.wasm.run]`, `[target.wasm.external]`, `[cmd.test.out-dir]`, `[diag.list]`, `[open.list]` #56: `main.wasm` is a WASI 0.3 component exporting `wasi:cli/run@0.3.0` and importing only `wasi:cli/stdout`, `wasi:cli/stderr` and `wasi:cli/exit` (all `@0.3.0`), instead of a `wasi_snapshot_preview1` command exporting `_start`. It runs with `wasmtime run -S p3=y` and needs wasmtime 49 or later; an older wasmtime is E0909 before the program runs. A reachable fn with `@external(wasm, ...)` is a compile error until the host's WIT can be declared, so the boundary types of the earlier core module import no longer apply, and E0427 is not used on WASM. This follows the pull request #401.

## 2026-10-22 (Arguments for handler init)

- `[effect.handler]`, `[effect.with]`, `[effect.layer]`, `[effect.label]`, `[grammar.decl]`, `[grammar.expr]`, `[type.rules]`, `[dyn.with]`: The `init` of a handler can take parameters, and a value is passed to it by writing `H(x: ...)` as an item of a with or a layer, or on the right side of `by` for a label. This revises "The handler itself takes no arguments." in `[effect.handler]`. Adopted in the proposal `handler-init-arguments` (#389).

## 2026-10-22 (Float.max with NaN)

- `[target.number]`: `Float.min` and `Float.max` choose by the total order of `Float.compare`, in which NaN is the largest value. `Float.max` therefore returns NaN when one argument is NaN, instead of not propagating NaN. This follows `[api.float]` and the implementation. Decided while reviewing divergences between the specification and the implementation.

## 2026-10-21 (The WASM target catches up)

- `[target.wasm.unsupported]`, `[target.wasm.external]`, `[open.list]` #56: The WASM target supports every construct of the language (suspension, fibers, the Float conversions, the grapheme cluster operations, the notes for release defects), and the std platform handlers work on it. The boundary of external functions written by a user stays narrower than 12.4 until the WASM output moves to the Component Model and its canonical ABI (#56). This follows the draft pull requests #289 to #304.

## 2026-10-21 (dbg of an `@external type` value)

- `[trait.builtin]`: `dbg` shows a value of an `@external type` as `<external Name>` on every target, instead of leaving it unspecified (the C and WASM targets were a defect, the JS target printed `undefined`). Decided while reviewing divergences between the specification and the implementation.

## 2026-10-21 (self of a handler without state)

- `[effect.handler]`: In the operations of a handler without state, `self` is the handler value and its type is the handler type, as in a handler with state, instead of `()`. Decided while reviewing divergences between the specification and the implementation; the implementation already behaved this way.

## 2026-10-21 (E0412 walks effects with different type arguments separately)

- `[ext.decl]`: When deciding which operation bodies are reachable for E0412, effects with different type arguments are different effects, as `[effect.decl]` already states. Calling `put` of `Store[String]` no longer makes the `put` of a handler for `Store[Int]` reachable. Decided while reviewing divergences between the specification and the implementation; the implementation already behaved this way.

## 2026-10-21 (`add --allow-external` covers only Pomes this add brings in)

- `[cmd.pome]`: `emela pome add --allow-external` allows only the Pome being added and the Pomes that this `add` newly brings into the lock. A Pome already in the lock is not allowed even if its version changes, because agreeing to give the host to the Pome being added does not extend to a Pome the project already depended on. Decided while reviewing divergences between the specification and the implementation; the implementation already behaved this way.

## 2026-10-21 (Split the specification into `spec/` and promoted the observable decisions from NOTES.md)

- Split `docs/spec/emela-0.20.md` into per-chapter files under `spec/`, and gave each section a stable name (`[effect.handler]`). Chapter and section numbers are unchanged. `spec/README.md` defines the layout and naming rules, the distinction between normative and informative text, the boundary with NOTES.md, and the change procedure. Proposals to change the specification go in `proposals/`.
- Merged the draft that had progressed outside the repository (the revisions from 2026-10-13 to 2026-10-19; from "Pome" to "Test dependencies and the test root" below). `[module.pome]` (4.7), `[effect.label]` (8.9), `[ext.c-host]` (12.6), and `[cmd.pome]` are text from this draft. Where a section changed in both, the version with the newer revision date was taken. For the test layer in `[lib.layers]`, the draft's 2026-10-19 decision (distribute the value comparison helpers, generators, and property testing through brix) was taken, and the repository text of 2026-10-18 that bundled them in std (commit b6e4d6a, which had no entry in this history) was removed.
- Promoted the observable items among the (supplemented) items in each crate's NOTES.md into the specification. The newly created sections are `[target.js]`, `[target.c]`, `[target.wasm]`, `[api]`, `[cmd.test]`, `[pome.overview]`, `[fmt]`, `[lsp]`, and the sections under them. Items were added to existing sections in the chapters `[lex]`, `[kw]`, `[module]`, `[data]`, `[fn]`, `[error]`, `[effect]`, `[type]`, `[trait]`, `[fiber]`, `[ext]`, `[test]`, `[lib]`, `[target]`, `[diag]`, `[diag.list]`, `[cmd]`, and `[open.list]`. Each NOTES.md item was replaced with one line that points to the name of a specification section.
- Split `[api]` out of `[lib.prelude]`, and moved the API list in `crates/emela-core/lib/README.md` into `[api]`.
- Moved 18.2 (points filled in during drafting) to `[history.filled]`, leaving Chapter 18 with only `[open]` (open questions).

## 2026-10-20 (Show of errors and W0303)

- 10.5: The derived Show of an error no longer gets W0303. Displaying an error that reaches main or a test (14.3) uses Show, but this is not counted as a "requirement" in type checking, so merely letting `error DbError(message: String) derive Show` reach main produced W0303.

## 2026-10-20 (Patterns in anonymous function parameters)

- 6.1, 6.3, 17.5, Appendix A: Parameters of anonymous functions can now be patterns that always match (`fn((key, value)) { ... }`, `fn(acc, _) { acc }`). Folding a list of tuples, or a property with `Gen.pair`, needed a binding line only to destructure a parameter and a name for an unused parameter. Parameters of top-level fns and the like stay names, because they also serve as the labels of named arguments. No shorthand (such as `_ + 1`) is added.

## 2026-10-20 (inspect requires Show)

- 7.1, 10.3, 10.5, 10.6, 13.2, 13.3, 14.3, 15.1, 17.3, 18.1 #22, #23: Limited displaying values to types that satisfy Show. `inspect` requires `[A: Show]`. An assert failure shows the values only when the types of both sides satisfy Show; otherwise it shows only the source text of the expressions. Only dbg does not require Show, and it shows a type that does not satisfy Show in its construction form (values inside it that satisfy Show use their implementation). Error declarations can also have derive. An error that reaches main or a test is shown with its Show if it satisfies Show, and only by its name otherwise. If the display of a type without Show were guessed from its structure, a display the author never decided on would become a string through inspect and reach the program. Generic test helpers require `[A: Show]`.

## 2026-10-20 (Map and Set)

- 15.2, 18.1 #36: Decided the API of `Std.Map` and `Std.Set`. They are ordered by key and require `K: Ord` of keys. The contents are a balanced tree in pure Emela, and comparison and display go by the contents. The display is a source form that rebuilds the value (`Map.from_list([(1, "a")])`).

## 2026-10-19 (Syntax of impl and handler, scope of derive, Hash)

- 10.2, 10.5, Appendix A: Assigned codes to duplicate impls, impls outside the owner, and the number of parameters of fns in an impl (E0330 to E0332).
- 10.3, Appendix A: For a Trait function that has Self only in its return type, it is a compile error (E0333) when Self is not determined inside the function.
- 3.3, Appendix A: Withdrew the dedicated diagnostic for `let` and `var` bindings (E0146).
- 3.2, 8.1, 8.4, 10.2, 17.3, Appendix A: The heads of impl and handler are now written with a colon (`impl User: Show`, `handler ConsoleLogger: Logger`). As in `[A: Ord]` and `trait Ord: Eq`, the colon means that the left side satisfies the right side. There were two words for implementing, `implements` and `impl ... for`, in opposite word orders; this unifies them. The contextual keywords `implements` and `for` are no longer used. The old forms get E0144 and E0145, which show the rewrite.
- 10.2: An impl for a type with type parameters writes the bounds on the type parameters in the head (`impl Map[K: Eq, V: Eq]: Eq`). The head is limited to the type name and a list of mutually distinct type parameters (E0328), which keeps one impl per pair of a type and a Trait.
- 10.5, 18.1 #3, #24: Limited deriving to Eq, Ord, and Show, with the same bound on every type parameter. Supertraits are not derived implicitly. W0303 reports unused derivings on non-`pub` types.
- 10.6: The compiler holds the implementations for basic types and tuples, and List and Option are written as impls in the core source. Comparison and display of types made only of derivings MAY be executed by a routine that walks the structure.
- 10.5, 10.6, 15.1, 15.2, 18.1 #36: Removed Hash. If Map and Set are built in key order, it has no use.
- 8.4: Stated how `use` is written inside init.
- 5.6, 15.2, 18.1 #3, #36, 18.3: Changed the version that introduces Traits and collections from 0.21 to 0.24.
- 10.1, 10.3, Appendix A: Added the rules for supertraits. The implementations of the supertraits are checked at the impl, and a bound includes its supertraits (`==` is available with `[A: Ord]`). A cycle is a compile error (E0329).
- Appendix B, Appendix A: An fn with bounded type parameters cannot be exported from the entry (E0327). Monomorphization chooses the impl, so the implementation is not determined unless the types it is called with from JS are determined.

## 2026-10-19 (Eq, Show, and the order of Float)

- 10.3, 10.5, 18.1 #22, Appendix A: `==`, ordering comparison, and interpolation on a type that does not satisfy the Trait are now a compile error (E0326). Types other than basic types, tuples, List, and Option need a derive or an impl, and nothing is derived implicitly. `==` on function values is also rejected.
- 10.6, 13.2, 14.3, 15.1: Separated the display for users (Show, interpolation) from the display for debugging (inspect). Added `inspect[A](value: A) -> String` to the Prelude and removed the Show bound from `dbg`. Assert failures and errors that reach main are printed with inspect. This lets generic test helpers print values without requiring `[A: Show]`.
- 10.6: Stated the contract of Ord (a total order, with Equal agreeing with `==`), and removed Float, which cannot keep it, from Ord. Ordering comparison on Float stays the built-in IEEE 754 operators. For a total order, pass `Float.compare` as a function. This avoids `<` giving different answers inside and outside generic code.
- 10.5, 18.1 #23: Allowed derive on opaque types as well. inspect also prints the contents of opaque types.

## 2026-10-19 (Missing methods in an impl)

- 10.2, 18.2, Appendix B: Stated the rule that every Trait method without a default implementation is implemented. An incomplete impl is a compile error (E0325), which lists all the missing methods.

## 2026-10-19 (Test dependencies and the test root)

- 4.7, Appendix A, Appendix B: Added dependencies used only by tests, `[dev-dependencies]`. They are ignored when the Pome is depended on, and the test dependencies of a dependency Pome are not fetched. This keeps a library's test helpers out of its users' builds just because they depend on the library.
  - Version selection runs once, over the graph combined with `[dependencies]`. This keeps the version that was tested and the version that is distributed the same.
  - The lock marks with `dev = true` the Pomes that can be reached only from the test dependency graph, and `build` and `run` do not fetch them.
  - Listing the same Pome in both is a compile error (E0240). Added `emela pome add --dev`.
- 4.1, 13.1, Appendix A: Added the test root `<project>/test`.
  - Modules in the test root are used only by tests and are not visible to dependents. This is a place for helper functions that take arguments and for generators. Putting `@test` on helper functions was rejected, because helpers without parameters would run as tests.
  - Only the test root can import test dependencies; importing them from the source root is a compile error (E0239). The test dependencies of a dependency Pome are not available locally, so a dependency Pome is designed to be usable without reading its test root or test functions. Tests in the same file as the implementation use only std and `[dependencies]`. A marker for using test dependencies from there (an import with `@test`) was not added, since it is not worth extra syntax.
- 13.1, 15.2: Removed the value comparison helpers, generators, and property testing (formerly `Std.Test.Assert`, `Std.Test.Gen`, and `Std.Test.Property`) from std, and decided to distribute them as the Pome `github.com/emela-lang/brix`. Tastes differ in how they are written and running programs do not need them, so they evolve separately from compiler versions. Test handlers (TestClock and others) stay in std, because the convention puts them in the same module as their effect.

## 2026-10-18 (Pome proxy and sumdb)

- 4.7, Appendix B, 18.1 #40: Changed fetching dependencies from fetching directly from Git to fetching through a proxy and verifying with a sumdb. The source of truth for identity and versions stays in Git tags; the proxy is a copy, and the sumdb is an append-only record. There is still no central registry (accounts, name registration, uploads).
  - Fixed the content hash as `h1:`. It is computed from the git tree, so it does not depend on the archive format or the working tree. A hash of `Pome.toml` alone is also recorded and is used when walking the dependency graph.
  - Made the match between the tag and `[pome].version`, the rejection of symbolic links and submodules, and a size limit conditions for ingestion.
  - An ingested version does not change even if the tag is moved. The contents first recorded become the contents of that version.
  - `@latest` skips versions that cannot be ingested (up to 5). This keeps `emela pome add` without a version usable even when the latest tag is broken. Temporary failures are not skipped.
  - Added `EMELA_POMEPROXY`, `EMELA_POMESUMDB`, and `EMELA_POMEPRIVATE`. `direct` and `EMELA_POMEPRIVATE` keep fetching without the proxy and fetching repositories that are not public.
  - Added `emela pome verify` and `emela pome publish`. `publish` is not a step required for publishing. It lets the author find ingestion errors first and compare the local contents with the sumdb record.
  - `publish` compares the public API with the previous version and warns when the version bump is insufficient (W0901). It does not enforce it. Enforcing it would make publish a gate for publishing, which does not fit the policy of keeping the truth in Git. Because fails and use appear in types, added side effects and errors are also detected as incompatible changes. During v0, the required bump shifts down by one level. An ingested version cannot be withdrawn, so `--dry-run` was added to try it before pushing.
  - A major version of v2 or above is a separate Pome with `/vN` at the end of its source path (18.1 #40). The `[dependencies]` keys, the lock, the cache, proxy URLs, and sumdb records are all keyed by source path, so two incompatible majors can coexist as they are. Separating them only by tag was rejected, because the two cannot coexist and cannot be called "separate Pomes". A tag whose major does not match `/vN` is not considered a version of that Pome. The default root name is formed by removing `/vN`.
  - Allowed multiple Pomes in one repository. On github.com, the path up to `owner/repo` is the repository and the rest is a directory. Tags of a Pome in a directory are prefixed with the directory (`platform-cloudflare/v0.1.0`). This lets each Pome in a repository bump its version separately. Other hosts are not handled for now, because the repository boundary cannot be determined from the source path.
  - Added diagnostics for builds that use dependencies: `@external` in a dependency (E0233), a `[pome].emela` that is too new (E0234) and an old minor (W0203), a root name that does not form a module name (E0235), and a dependency that cannot be used (E0236). A root name that is a reserved name is a compile error (E0203), and one that collides with the Pome's own modules or with another dependency is a compile error (E0204).
  - A dependency's `@external` can be used only when the `allow` in `[external]` of the Pome being built permits it (E0233). An `@external` fn in a dependency cannot be pub (E0237). An external function that omits use can only be used inside a handler in the first place (12.2), so the only path to the host visible to dependents is effects. `[pome].targets` declares the targets the Pome runs on (E0238). Added `emela pome add --allow-external`, and the lock lists the files that contain `@external`. Target-specific parts are confined to handlers, and the entry chooses them. There is no way to write per-target dependencies.
  - 12.1, 18.2: Files that `@external` points to are copied to the output directory and read by relative path. The output works after being moved, and rewriting such as platform-cloudflare's build.sh is no longer needed.
  - The compatibility of changes that remove fails and use is covered in 18.1 #41.

## 2026-10-17 (Effect labels)

- 3.2, 8.3, 8.9, 9.4, 17.3, 17.5, Appendix A: Added effect labels (`effect L of E [default]`). A with can take `L` and `L by P`. This lets the caller provide the same effect with a different handler at each place (issue #255). Added E0429 to E0433 and W0401.
- 2.3, 6.2, 15.3, Appendix A: Renamed "label" in the sense of named arguments to "argument name", so that it is not confused with effect labels.

## 2026-10-15 (Walk only the operations in use)

- 12.1, 15.2: In the target annotation check (E0412), the body of an operation implementation of a handler written in a with is walked only when that operation is called or used as a value in reachable code. The bodies of init and release are still always walked. This lets a layer that bundles many handlers, like `with Platform`, be used on targets where some operations are not yet implemented.

## 2026-10-15 (Asynchronous host functions in C)

- 12.2, 12.6, 15.2, 18.1 #37, Appendix A: Allowed C external functions for suspend fns, in a form that reports completion later. The C function takes an `em_task *` as its last parameter, and reports completion with `em_complete_*` and a host error with `em_complete_defect`. The runtime provides, in `emela.h`, registration with the event loop (`em_after`, `em_on_readable`, `em_on_writable`) and notification of interruption (`em_on_cancel`). Users can write them in the same form, not only the bundled std. This makes extension through external functions possible in C, as it is in JS and WASM. E0428 is retired.

## 2026-10-15 (C target)

- Chapter 1, 4.1, 4.7, 12.1 to 12.5, 15.2, Chapter 16, 18.1 #37 to #39, Appendix A, Appendix B: Added `c` as a target. It emits C and builds a native executable with cc. It is Tier 2, has the same semantics as JS and WASM, and is subject to all of the rules of Chapter 16. Decided the table that maps boundary types to C types (a String is passed as a NUL-terminated UTF-8 copy, and a String received from C is `free`d as soon as it has been copied; `@external type` is `void *`). The first version of the C target (0.21) has no callbacks and no external functions for suspend fns (E0427, E0428), and the form of asynchronous host functions was left in #37. `Pome.toml` can specify the default build target `[build].target` (E0232). Added the cc diagnostics E0907 and E0908.
- 16.5: Changed polymorphic-R functions in WASM and C to monomorphization, and to propagating defects explicitly through return values (an informative section; the conclusion of `emela-0.20-backend-survey.md`).

## 2026-10-14 (Numeric comparison functions)

- 16.1: Described the behavior of `compare`, `min`, `max`, `abs`, and `Float.is_nan` added to core ((supplemented) from the implementation of the std expansion). `Float.compare` is a total order (NaN is the largest; NaN with NaN, and ±0, are Equal), and `min` and `max` do not propagate NaN. The Trait Ord in 0.21 also uses this order.

## 2026-10-13 (Pome)

- 4.1, 4.7, 12.1, 15.2, Appendix B, 18.1 #14: Decided Pome. Carried over the skeleton of spec 0032 from 0.1x (no central registry, source paths and git tags, publishing by pushing a tag) and revised it for 0.20.
  - Changed version selection from "the largest version that satisfies the requirements" to Minimal Version Selection. Requirements are only "at least", with no range syntax, and new tags do not change a build. The lock is used not to pin the selected versions but to verify the fetched contents. A major of v2 or above is a separate Pome.
  - Modules of a dependency are visible under its root name. `src/lib.emel` is the library root, and whether `main.emel` and `lib.emel` exist decides between application and library (no setting switches it). The depending side can rename the root name. `Std` and core names cannot be root names.
  - `[pome].emela` is the minimum compiler version, and a dependency for an older minor is built with a warning. There is no mechanism to switch the language per version.
  - Dependencies are fetched with `git` into the cache, and the lock records the commit and the content hash. build fetches missing dependencies but does not rewrite the lock. Added `[replace]`, which uses a local directory as a dependency.
  - A Pome used as a dependency cannot have `@external`. The only path to the host is the platform layer of std (the opening of Chapter 12).
  - std stays bundled. It can grow, but there is a criterion for what goes in. Components that are not settled are released first in `github.com/emela-lang/x`.
  - Integration with the capability manifest (0025) of 0032 is replaced by the R of public functions (a later version). The search index (Orchard) will be redesigned, and is postponed together with Bushel and vendor.

## 2026-10-13 (The host boundary)

- Opening of Chapter 12: The only path out to the host is the platform layer of std, and `@external` is positioned as a way out. A Pome used as a dependency cannot have `@external`. Missing host features (TCP sockets, Bytes, and so on) are added on the std side. This keeps every Pome running with the same semantics on both JS and WASM.

## 2026-10-13 (Negative numbers in patterns)

- 6.5, 17.6, 18.1 #17: Allowed `-` before a numeric literal, in patterns only (0.20.1). The range check treats `-` and the number as one value. Range patterns are not provided, because exhaustiveness checking would need to handle numeric intervals. Fixed the "open" note in 2.4 and retired the provisional E0116.

## 2026-10-13 (Directories outside the source root)

- Appendix B, 18.1 #32: Passing a directory outside the source root to check, build, run, or test is now a compile error (E0208), as with fmt. Previously the whole project (src) was checked and passed, which misled people into thinking the given directory had been checked. Passing a file was already a compile error (E0208).

## 2026-10-13 (Evaluation of const)

- 5.6, 18.1 #25: Dropped the rule that limits the right-hand side of a const to expressions that can be evaluated at compile time; any pure expression can be written (the implementation already did this). Evaluation happens once, on the first reference, and the result is remembered, rather than on every reference. The defect message includes the name of the const. A self-reference during evaluation is a defect, and a cycle made only of names is a compile error. A const in a pattern is checked for exhaustiveness as its value if it is made only of literals and constructors; otherwise it is treated like an arm with a guard.

## 2026-10-13 (Make match postfix)

- 6.5, 6.6, 2.5, 9.4, 17.5, 18.1 #2: Made match postfix only (`expr match { arms }`). It matches escape in form and precedence (the weakest, left-associative), so branching on the result of a pipe also reads from left to right. There is no prefix form (two ways of writing the same thing are not offered side by side). Writing it as prefix is a compile error (E0142). There is no migration period: from 0.20.0 on, only the postfix form exists (with no alpha in between; the author's policy). Added `match` to continuation rule 1 in 2.5 (since there is no prefix form, no statement starts with `match`). The form that follows a parenthesized expression with arms and no keyword was rejected, because it is hard to tell apart from a function call and hard to search for. Prefix uses in the repository are rewritten mechanically, once, in the implementation PR.

## 2026-10-13 (Postpone collections to 0.21)

- 15.2, 4.6, 6.3, 18.1 #36: Postponed Map, Set, and Array to 0.21. In 0.20, `==` and display work through built-in runtime routines rather than Traits, so if they were written in pure Emela, comparison and display could differ even with the same contents. Writing them with intrinsics would mean rebuilding them in 0.21, so they are decided together after Traits. Decided to place them in Std (`Std.Map` and so on). Placing them in core would make `map.emel` and others reserved names, which cannot be undone later.
- 15.2: Changed the criterion for core from "modules with `@intrinsic`" to "the modules of types the compiler knows as syntax, and the Prelude". State and Fiber have intrinsics but live in Std, so the old criterion no longer worked.
- 4.6, 6.3: Rewrote the examples that used Map to use core types and List.

## 2026-10-13 (Decisions for alpha.5)

- Chapter 11: Rewrote the fiber chapter. Decided the following after discussing with the author.
  - Opening of Chapter 11: Wrote down why the foundation is intrinsic (to avoid adding to the language a means to discard continuations and a means to receive defects. The approach of OCaml 5 and Koka, writing a scheduler on the user side, was rejected, because it cannot express interruption and unwinding).
  - 11.2, 18.1 #4: There are no restrictions on the closure passed to fork. A child inherits the handlers as of the call to Fiber.scope (changed from as of fork), and the child's R is included in the scope's R (the type checker implementation (types-fiber) changed this from unification to inclusion). This uses types to prevent a with inside the scope body from ending before a child does, which would leave the child hitting a released handler. Effect-TS and ZIO inherit the environment as of fork, and make users avoid this problem by choosing a kind of fork (forkScoped and so on).
  - 11.2: Made the scope the built-in effect `Scope[E, R]` (`s = use Scope`, `s.fork(f)`). If the scope were a value, `s.fork` could not be written in the grammar of 10.3, which has no method calls, and a separate leak check would be needed. As a capability, the capability rules (8.2, 8.6) apply as they are. Fiber handles stay values (`Fiber[A]`), because their number is decided at run time and they are collected into lists. join is `Fiber.join(h)`.
  - 8.1, 17.3: Allowed operation type parameters and set type parameters only for built-in effects. Added type parameters to op_sig in 17.3. Type parameters on operations of user effects are a compile error (E0426).
  - 11.1: Fairness yields are inserted at the positions of calls that may suspend, and are counted. Programs are written assuming that a switch can happen at any call that may suspend. This is implemented in alpha.5.
  - 11.4: Added `Fiber.interrupt`, which interrupts one child.
  - 11.5: Decided `join`, `await` (Exit), and `attempt` as the ways to receive results. As in 7.1, a value of a union of errors cannot be built, so the cause is represented by `Cause[E]`, whose contents cannot be extracted; it is raised again with `Cause.raise` and then received with escape. attempt is the boundary where a defect can be turned into a value (7.5; so that one panic on a server does not stop everything). An interruption of the fiber itself is not received by attempt. join on an interrupted child is a defect.
  - 11.2: When two or more children fail, the earlier failure becomes the result, and later failures are added as notes (aligned with 14.4). Composing Causes (Sequential and Parallel in Effect-TS and ZIO) was rejected, because it makes the receiving type heavy.
  - 11.3: race takes "the first to finish (with a value or an error)". A version that takes the first success can be written on top of attempt.
  - 11.2, 11.3: To reduce how much has to be written out, added `Fiber.fork` and `Fiber.deferred` (which wrap `use Scope`), and `Fiber.both` and `Fiber.map`. There is no dedicated syntax like async let and no short syntax for functions without parameters (the policy at the opening of Chapter 11 stands).
  - 11.7, 14.6: Added Deferreds that belong to a scope (`s.deferred()`). They are the minimal component for writing race in pure Emela. They also have `poll`, which looks without waiting (to check a stop signal at each step). Added them to the places where mutable state can live.
  - 11.8, 8.6, Appendix A: Leaking scopes, handles, and Deferreds is prevented by the capability rules, E0425 (the type of the value of Fiber.scope), and run-time checks. With capture sets in 0.22, all of this becomes static, and what is a compile error in 0.20 stays a compile error in 0.22. Tabulated the coverage of 0.20 and 0.22. Added E0425 and E0426, and extended E0413 to Scope.
  - 3.5, 8.1: A built-in effect is declared in the bundled source by putting `@intrinsic` on the effect ((supplemented) by types-fiber). Putting it on an effect outside the bundled source is a compile error (E0224).
  - 2.5: Added rule 4, which continues lines starting with `fails` and `use` onto the previous line, only inside signatures (reported by platform-fs; for wrapping the long fails of FileSystem and Http). Adding it to rule 3 was rejected, because it would be ambiguous with `fails` (an identifier) and `use` (an expression) inside bodies.
  - 3.3: Removed `await` from the words reserved for the future. There is no async/await syntax (the opening of Chapter 11), so there is no reason to reserve it, and it is used for the function names `Fiber.await` and `Deferred.await` (reported by types-fiber). `async` stays reserved.
  - 11.2, 11.5, 11.8, 14.5: Reflected the (supplemented) items of the JS runtime (js-fiber-runtime): fork is not a suspension point, how the result of a scope is decided, closing the scope on a child's failure, failed children visible during release, interrupting oneself, how unwinding continues, queuing a resume from another fiber, the continuation of the operation implementation after resume, handling when the event loop is empty, and the message for a finished scope. poll is also subject to the finished-scope check (the runtime's (supplemented) item excluded it, but this aligns with the leak prohibition in 11.8).
  - 15.2: Postponed Queue, Semaphore, Schedule, and Stream to later versions. A limit on the number of fibers running concurrently comes before Http.
- 15.2: Decided FileSystem and Std.Path (text only, suspend fns, one error per kind, String paths, bundled into Platform and a defect when called in the browser, and TestFileSystem kept in State).
- 15.2, 11.3, 12.2, 16: Decided the Http client (one operation, send; requests and responses as values; text-only bodies; headers as a List; 4xx and 5xx are not failures; connection failures are merged into one; timeouts through Fiber.timeout; an AbortSignal that stops the request on interruption; TestHttp, which decides responses with a function). The JS target assumes a host that runs on a server, and the browser was dropped from the scope of 0.20 (the author's policy). The browser handling of FileSystem was also dropped.
- 8.8: TestClock runs on virtual time (the same idea as ZIO's TestClock). In the form that resumes immediately, multiple fibers do not wake in time order, and tests of Fiber.timeout and race behave differently from the real thing. TestClock now depends on Async. The mechanism that knows when no fiber can make progress lives in the runtime, and it wakes the earliest sleeper when the host's event loop becomes empty (Node's beforeExit). Emela calls it through an `@external suspend fn` that points into the runtime (a suspending intrinsic would need new paths in type checking and lowering, while an external function can remove the registration of an interrupted sleeper through an AbortSignal; (supplemented) by js-fiber-runtime). In the JS output, yields are inserted before calls that may suspend inside generators.
- 15.2: Changed the errors of FileSystem and Http to `pub error` (so that they can be imported and received with escape). From the (supplemented) items of the Http implementation (platform-http), added to the text: schemes other than http and https, empty bodies, how header names are compared, and handling of the BOM.
- Coverage: alpha.5 covers FileSystem and the Http client (15.2), and all of Chapter 11 (Scope, fork, join, await, interrupt, attempt, Exit, Cause, Deferred, all, race, yields, and leak checks).

## 2026-10-12 (Dropped the labels for higher-order functions)

- 15.3: Dropped the convention of labeling function parameters `with` and predicates `where`. Both are reserved words and cannot be parameter names, and it is not worth adding syntax that separates the external label from the internal name. Function parameters stay named `f` and are passed by position. `from` for the initial value of a fold stays. To match the names in the std migration (#189), changed the examples in the text to `SystemClock` and so on.

## 2026-10-12 (Reflected from the State implementation)

- 8.8, 18.1 #35: From the lowering of State and `State.run` (#187), added to the text that the state carries over across a suspension, and the behavior of TestConsole and TestClock. Changed the TestConsole example in 8.8 to append in the order written. Added to #35 the approximation of the key used to look up handlers.

## 2026-10-12 (Reflected from the implementation of effect type parameters)

- 8.1, 17.4, 18.1 #35, Appendix A: From type checking of effect type parameters and State (#183), added to the text: omitted type arguments and a wrong number of them, use as a set, comparing down to the type arguments in R, unification of undetermined type arguments, and generalization. Added type parameters to effect_item in 17.4. Added to #35 the approximation when the same effect appears two or more times. Added E0423 and E0424.

## 2026-10-12 (Reflected from the implementation of stateful handlers)

- 8.3, 8.4, 8.5, 8.6, 14.4, 14.5, Appendix A: Reflected from type checking of stateful handlers and layers (#178) and from the runtime that runs release (#179): an overridden handler is not initialized; the init value of a handler without fields; self inside init; record update also cannot be written outside init; the initialization order is decided from the whole of requires(H) (revising "from the dependencies of init" in 8.5); the use-after-release check applies only to stateful handlers; the form of the note for a defect in release; interruption and defects in release; and with in a continuation after the with has finished. Added E0418 to E0422.

## 2026-10-12 (Split the standard library into core and Std)

- 4.1, 15.2: Split the standard library into core, which cannot be separated from the compiler (the Prelude and modules with `@intrinsic`; short names), and std, which can be (under `Std`). core is kept small and std rich. To avoid users' file names becoming reserved names each time a std module is added, only `std.emel` and `std/` are reserved. Letting user modules hide bundled modules was rejected, because writing `http.emel` would make the bundled Http unusable.
- 15.2: In 0.20, Map and Set are handled with structural comparison and a built-in hash, and Trait bounds are added in 0.21.
- 18.1: Added #33 (the names and bundling of platform handlers; target names are not exposed to users) and #34 (the conventions of the standard library API).
- 15.3, 18.1 #34: Decided the conventions of the standard library API and created 15.3. Parameters that take functions are `with`, predicates are `where`, and the initial value of a fold is `from`. Joining two is `append`, flattening is `List.flatten` and `String.concat`, and joining with a separator is `join`. Absence is an Option, and malformed input fails with an error. Conversions are gathered in the module of the depending side, except that conversion to String is each type's `to_string`. Changed `Option.unwrap_or` to `Option.or`, and removed `String.from_int`.
- 15.2, 18.1 #33: Decided to name platform handlers `System*` (system handlers) and `Test*` (test handlers), and to place them in the same module as their effect. Target names are not exposed; one handler has an `@external` for each target. There is a layer `Std.Platform.Platform` that bundles the System handlers. `Host*` is unnatural English, `Live*` has an unclear origin, and placing short names in each module would require telling modules and effects apart when reading, so these were rejected.

## 2026-10-12 (Made a way to create Instant public)

- 15.2: Made `Instant.from_unix_milliseconds` public (#172, decided after discussing with the author). Limiting where an Instant can be created to Clock guarantees nothing, since `now` and `Instant.add` can create any point in time, and it would prevent fixing the time in tests and users writing Clock handlers. The `@internal` mechanism (E0228, E0229) stays.

## 2026-10-12 (E0417 per call)

- 8.1, 9.5, Appendix A: Changed the E0417 check from whether the R of the body of an fn operation implementation is Immediate to whether the calls in the body may suspend. For an operation call this is decided by the operation's declaration; for calls of functions and function values, by the R of the type; and a call of an `@external suspend fn` always may suspend. Because Immediate is per effect, an fn operation implementation of an effect with suspend fns (Clock) could not even call an fn operation of the same effect (`now`). The R of types, handler requirements, the sets that with removes, `R: Immediate` (E0416), and function types are unchanged. Giving R the granularity of operations (`use { Clock.now }`) was rejected, because set elements, what with removes, and handler requirements would all become per operation, which would greatly change the type and effect system (Chapter 9). Inferring from a function's body that it does not suspend was rejected, because changing the body would change the checks on the caller, which breaks the rule in 8.1 that "the suspension marker is only on operation declarations, not on function signatures" (#173, decided after discussing with the author).
- 8.1, 9.5, Appendix A: With only the per-call check, a body that calls a suspend fn operation inside the with of a handler that resumes immediately (which passed before, because the body's R was empty) becomes a compile error (E0417). So the body passes if it satisfies either of two conditions: its R is Immediate, or it contains no call that may suspend. Each condition alone guarantees that the body does not suspend, so soundness holds. A body that mixes fn operations of an outer effect with suspend fn operations inside an inner with is left unaccepted, because accepting it would require looking at the with that provides each call, which makes the rule heavy (#173, decided after discussing with the author).

## 2026-10-12 (Reflected from the lowering of suspend)

- 8.1, 9.5, Appendix A: The R of the body of an fn operation implementation is required to be Immediate (E0417). Operations that wait are declared as suspend fn on the effect side. The caller can only learn whether a call suspends from the effect declaration, and if fn operations could suspend, an operation of an Immediate effect would suspend, which breaks 9.5 and 11.1. Treating every operation as possibly suspending (Immediate would lose nearly all meaning, and it is slow) and putting handler requirements into Immediate (which cannot be decided statically in functions that do not know the handler) were rejected (#167, decided after discussing with the author).
- 14.5: Stated that a continuation that is not resumed in a context that does not suspend unwinds as an interruption at that point.
- 16.5: Added to the informative table: emitting a separate suspending copy of polymorphic-R functions (specialization), and not putting Async in the evidence table.

## 2026-10-12 (Random and Env)

- 15.2, 18.2: Added to the text the operations of Random and Env added to the bundled core (#168), the random source of the JS handler, and the handling of empty values, and added 2 items to 18.2.

## 2026-10-12 (Mutable state as the State effect)

- 8.1, 8.8, 9.5, 14.6, 15.2, 17.3, 17.5, 18.1 #21: Decided to represent mutable state with the effect State[S], which has a type parameter, and `State.run`. Allowed type parameters on effects (ordinary types only), and effects with different type arguments are distinct. Handlers implement them for concrete types. There is no Ref. It was compared with a Ref closed inside a handler, requiring a built-in effect for Ref operations, swapping handler values, and having no mutable state at all; this option was chosen because it keeps the principle of passing side effects through effects (1.1) while letting reading test state and local mutable computation be written with the same mechanism.
- 9.5: Replaced the Ref.update example with an example of a callback passed to an external function.

## 2026-10-12 (Reflected from the JS runtime for suspend)

- 14.5, 12.2, 13.4: From the JS runtime for suspend (#159), added to the text: the order of execution after an immediate resume, the point a later resume returns to, handling when an operation body fails before and after resume, resume after finishing, non-Promise values and rejections, and reporting a test that ended while suspended.

## 2026-10-12 (Cross-checking the (supplemented) items in NOTES.md)

- 8.3, Appendix A: Changed `emela run` on an entry without main from the implementation's behavior, which did nothing and exited with code 0, to a compile error (E0230). This avoids nothing happening without anyone noticing.

Cross-checked the 559 (supplemented) items in the NOTES.md files of the crates against the specification, and added to the text those that affect semantics but were missing from it. Items that were only in 18.2 and not in the text were also added to the text here.

- Fixed contradictions: `_` and variables in escape arms in 4.6 (aligned with 7.3 and E0320), named arguments in 6.2 (operations on capability values can also be called with them), polymorphism in 6.3 (a const is also generalized), the type of escape in 9.4 (the arm type when the original expression is Never), file specification in 12.1 (`../` can also be used), and the display examples in Appendix A (replaced with the implementation's form).
- Added to the text: 2.2, 2.3, 2.4, 3.5 (`@internal`), 4.1, 4.6, 5.3, 5.6, 6.1, 6.5, 6.6, 7.3, 7.5 (example defect messages), 8.2, 8.3, 8.4, 9.2, 9.3 (inference groups and how types are displayed), 9.4, 9.5, 10.5, 10.6, 12.1, 12.4, 12.5, 13.2, 14.3, 15.1, 15.2 (Instant, Duration, Console), 16.1, 16.2, 17.1, 17.2, 17.3.
- Created 13.4 (`emela test`) and Appendix B (commands).
- 18.1: Added #22 to #32. 18.3: Added the range that the implementation does not yet handle.
- Appendix A: Widened the scope of E0106, and added E0138 to E0141, E0228, and E0229.

## 2026-10-12 (Reflected from type checking of suspend and Async)

- Appendix A: Added E0322 to E0324 and E0413 to E0416, and widened the scope of E0221 and E0407 (#156, #160).
- 9.2, 17.4: Sets can be written as type arguments of named types (`Handler[NotFound | Timeout, { Db, Log }]`). The syntax matches the fails clause and the use clause, and type checking verifies which type parameters accept sets (#160).

## 2026-10-12 (Decisions for alpha.4)

- 8.4, 14.4, 18.1 #9: The E of release is required to be empty, and decided how defects inside release are handled. suspend inside release is allowed.
- 8.4: init and release can be written even for handlers without fields. A handler value can be created only inside that handler's init.
- 8.5: Changed the rule so that requirements not satisfied inside a layer are not a compile error and remain in the R of the with. This aligns with the with rules in 8.3 and allows layers that take some requirements from outside.
- 8.6: Until capture sets arrive, using a released handler is a defect at run time.
- 14.5: release also runs when unwinding while still suspended.
- 18.1: Added #21 (the definition of mutable state).
- Coverage: alpha.4 covers fields, init, release, self, layer, the run-time check for use after release, and mutable state (once #21 is decided).

## 2026-10-12 (External files of the bundled core)

- 12.1, 18.2: Allowed `@external` that points to files from the bundled core sources (#155). The files are bundled with the compiler and written out to the output directory of the build.

## 2026-10-12 (Decisions for alpha.3)

- 8.7: Decided to determine syntactically whether resume may be called later, by whether the operation body uses resume in any way other than calling it directly, and if so, to add Async to the handler's requirements. Giving the type of resume an Async marker was rejected, because it would need nearly the same mechanism as capture sets (8.6, 0.22).
- 14.5: When the continuation of main ends while still suspended, `interrupted` is printed and the exit code is 130. In a test, that test fails.
- Coverage: alpha.3 covers suspend, Async, `@external suspend fn`, Immediate, Clock, and fails and use with set type parameters in declarations. Stateful handlers and layers were moved to alpha.4, and fibers (Chapter 11) to alpha.5. Interruption runs release (11.4), so stateful handlers are needed first.

## 2026-10-12 (Reflected from the implementation of effect lowering)

- 8.3, 12.1, 12.5: From the lowering of effects and external functions (#152), added to the text: what `use` inside an anonymous function refers to, handling of external functions that slipped past the check, and the message for exceptions from external functions.
- 18.2: Added 5 items supplemented by the implementation (1 from #150, 4 from #152).
- Appendix A: Added E0412.

## 2026-10-12 (Mixing within effect sets)

- 8.1, Appendix A: Decided that effect operations cannot have a use clause (E0137). Requirements of operation implementations are satisfied as handler requirements at the with. If a use clause could be written there, implementation details would leak into the effect declaration, and callers would need Db even with a test handler that does not use Db.
- 8.3, 17.4: An effect set can mix effects and set type parameters (`use { Logger, R }`). with removes nothing from hidden effect sets, so writing out the effects to be provided removes them exactly. This aligns with `fails Timeout | E` for errors (18.1 #19).
- 8.3: The example used the same name Quiet for a handler and an effect; changed it to the effect Logger and the handler ConsoleLogger (8.4). The same name is a duplicate in name resolution.

## 2026-10-12 (Reflected from the implementation of the effect set R and error lowering)

- 8.2, 8.3, 8.4, 9.3, 12.2, 14.3: From R in type inference (#147) and error lowering (#148), added to the text: the R and E of operation calls, the scope of the restrictions on capability values, with and hidden effect sets, requirements within the same with, the checks on handlers, deciding the restriction on external functions by reference, and the display of unhandled errors.
- 7.3, 9.4, 18.1 #20: Decided that a tag is removed from E only when the escape arms receive every value of that error. A value that does not match arms that receive only some values propagates as an error, not as a defect.
- 18.2: Added 10 items supplemented by the implementation.
- Appendix A: Added E0401 to E0411.

## 2026-10-12 (Reflected from the implementation of the error set E)

- 7.2, 7.3, 7.4, 9.4, 9.6: From the implementation of E in type inference (#144 to #146), added to the text: the range of set variables that escape does not remove, `-> T` and the errors of function parameters, and inclusion (subtyping) on E.
- 18.2: Added 9 items supplemented by the implementation (8 from emela-types, 1 from emela-core).
- Appendix A: Added E0318 to E0321 and W0302.
- 12.1: Decided that an fn without an `@external` for the target is a compile error only when it is reachable from the entry. The check runs in build, run, and test, and not in check or the LSP. This allows target-specific handlers (JsConsole) in the bundled core.
- 4.1, 15.2: Adding Console and JsConsole to the bundled core makes `console.emel` and `js_console.emel` directly under the source root reserved names (E0203). This follows the current rule that reserves core module names, and breaking existing programs that have files with these names is accepted.

## 2026-10-12 (escape and set variables)

- 7.3, 7.4, 9.4, 17.4, 18.1 #19: Decided that when the E that escape targets contains a set variable, escape removes nothing from the set variable. The tags to be received are removed exactly by writing them out, as in `fails Timeout | E`. For this, error_set was widened to mix tags and set type parameters. Keeping the difference symbolically was rejected because the rules get complex, and forbidding escape on set variables was rejected as too strict.

## 2026-10-08 (Reflected from the implementation)

Decided revisions:

- 17.6, 6.5: In list patterns, allow an unnamed rest `[x, ..]`. This aligns with `User(name:, ..)` for constructors and avoids naming an unused rest. The examples of missing patterns that exhaustiveness checking shows (`[_, ..]`) can also be written as source as they are.
- 17.5, 2.4, 9.4, 14.1: A list literal can end with `..expr`, once (`[x, y, ..rest]`). There was no construction syntax paired with the pattern `[x, ..rest]`.
- 18.1: Added #17 (negative numbers in patterns) and #18 (the Unicode version for graphemes).
- 4.1, 18.1 #14: A project is now determined by whether `Pome.toml` exists. Searching for an ancestor `src` from the input path string gave different results for absolute and relative paths, and even collected another project under `~/src`. The name follows the Pome of the 0.1x series (spec 0032). The format of its contents remains open.
- 2.3, 5.7, 17.3, 17.4: Type parameters can also be written as type names (`Item`). A type parameter hides an outer type of the same name, and the linter warns about it. This is the first item sorted in 18.2.
- 18.2: Added the "Verdict" column. Sorted items 1 to 21 (from the original draft), 6.1, and others.
- 18.2: Added 3 items supplemented by the built-in functions (#122).

## 2026-10-09 (Sorting 18.2)

- Decided all 41 items in 18.2. 2.3 (type parameters) and 5.5 (adding the type parenthesis `(T)`) were decided with changes. Items already implemented were decided as implemented. Decided items had the (supplemented) mark removed from the text.
- 14.3: Decided the details: an error that reaches main prints `error: ...` and exits with code 1.
- 9.2, 18.1 #7: Decided, per position, the meaning of omitting clauses in function types.
- 8.2, 18.1 #5: Decided that capability values have no type that can be written and cannot be passed as arguments. To make code use a specific handler, pass a closure.
- 7.1, 7.2, 18.1 #6: An error declaration defines a type of the same name, so error values can be treated as ordinary values.
- 4.6: Added the name resolution rules (from the name resolution implementation).
- 3.5, 15.2: Made the Prelude and core bundled `.emel` sources, with built-in functions declared by `@intrinsic`.
- 4.1, 4.6, Appendix A: Made core module names reserved names, and added E0222 to E0225 (from the continued name resolution implementation).
- 14.3: Decided that the return value of main is not printed. Output is done with `dbg`.
- 6.2, Appendix A: Stated that function values cannot be called with named arguments, and added E0304 to E0307 (from the type inference implementation).
- 13.1, Appendix A: Test functions can be referenced only from inside test functions. Added E0227.
- Appendix A: Added the type inference codes E0308 to E0317. Added to 2.4, as (supplemented), that the minimum Int cannot be written as a literal.
- 14.5, 18.1 #1: Decided that a second resume is checked at run time and is a defect.
- 2.4, 6.6, 16.2, 17.2, 18.1 #11: There is no character type, and bit operations are functions. Added `0x` and `0b` literals, seven escapes, and `"""` multi-line strings.
- 5.3, 14.1, 17.5, 9.4, 18.1 #10: Added record update in the form `User(name: "b", ..user)`. The type arguments can change.
- 2.4: Decided the type of integer literals. In type inference, a literal is a type variable constrained to "Int or Int64", and it becomes Int if it is still undetermined at the end.
- Appendix A: Added the system and the list of diagnostic codes. Messages are in English, and the display is close to rustc's. The syntax codes E0110 to E0126 were assigned in the parser.
- 18.2: Added 3 items supplemented in the parser (line breaks inside the braces of effect sets, interpolation in patterns, and parentheses in types and patterns).

Added (supplemented) items: the items in 18.2 whose "Source" is not the draft. The corresponding places in the text were marked (supplemented).

## 2026-10-07

- First version.

## Points filled in during drafting (formerly 18.2) `[history.filled]`

To the 21 items of the original draft, 50 items supplemented by the implementation (the `NOTES.md` of each 0.20 crate) were added. For the latter, "Source" names the place in the implementation. "Verdict" is one of: Open (not yet reviewed), Decided, Decided with changes (decided with different content), Sent back, or On hold. Decided items have the (supplemented) mark removed from the text.

| Section | What was filled in | Reason | Source | Verdict |
|---|---|---|---|---|
| 2.3 | Included type parameters (A, R) in the class of uppercase names | Since "a type name contains a lowercase letter" was decided, a single-letter type parameter would belong to no class | Draft | Decided with changes: can be written as either an uppercase name or a type name, and hides an outer type (2.3) |
| 2.4 | An integer literal becomes Int64 depending on context | How to write Int64 literals was undecided | Draft | Decided |
| 4.1 | File names are snake_case and are converted to PascalCase | The mapping of multi-word module names was undecided | Draft | Decided |
| 4.2 | import goes at the top of the file | There was no rule on its position | Draft | Decided |
| 5.4 | A single variant declaration cannot mix unnamed and named fields | Mixing was allowed at the call site, but the declaration side was not decided | Draft | Decided |
| 6.1 | Top-level fns write the types of their parameters | All examples are typed, but whether it is required was not decided | Draft | Decided |
| 6.3 | Binding patterns are limited to those that always succeed | The meaning on failure was undecided | Draft | Decided |
| 6.6 | Operator precedence. escape is weaker than the pipe | The precedence table was not decided | Draft | Decided |
| 8.1 | Effect operations can have fails | How to declare an operation that can fail was undecided. Aligned with the Trait rules | Draft | Decided |
| 9.3 | When a use clause is written, exactly that set is enforced | Aligned with the rules of the fails clause | Draft | Decided |
| 10.3 | Multiple bounds are joined with + | The syntax was undecided | Draft | Decided |
| 10.5 | Combining derive with a handwritten impl is a compile error | Derived from the rule that there is one implementation | Draft | Decided |
| 11.4 | release is not interrupted while it runs | Stopping in the middle of cleanup leaks resources | Draft | Decided |
| 12.3 | Host types are declared with @external type | How to write them after switching to the annotation form was not decided | Draft | Decided |
| 13.2 | assert can be used outside test functions | There was no rule on where it can be used | Draft | Decided |
| 14.1 | Evaluation is left to right. Named arguments are evaluated in the order written | The evaluation order was not decided | Draft | Decided |
| 14.2 | If init fails, only the handlers already initialized are released | Failure in the middle of initialization was not decided | Draft | Decided |
| 14.3 | An error that reaches main is abnormal termination | The handling of main's E was undecided | Draft | Decided |
| 14.5 | A continuation discarded without resume being called unwinds as an interruption | The meaning of a handler that does not resume was undecided | Draft | Decided |
| 16.1, 16.2 | Integer division truncates toward zero. Float division by zero follows IEEE. byte_size is the number of UTF-8 bytes | Results need rules to agree across both targets | Draft | Decided |
| 17.4 | Multiple effects are enclosed in braces, as in `use { A, B }` | How to annotate multiple effects was undecided | Draft | Decided |
| 2.1 | Whitespace is spaces and tabs. `\r\n` is one line break, and a lone `\r` is an unrecognized character | There was no definition of line breaks and whitespace | emela-syntax | Decided |
| 2.3, 17.2 | Names are ASCII only. Uppercase letters in the middle of lowercase names and consecutive uppercase letters in type names are lexically allowed, and the conventions are checked by the linter | The EBNF and the table disagreed | emela-syntax | Decided |
| 2.4, 17.2 | Strings cannot span line breaks. A letter right after a number is a lexical error. How `1..2` and `1.x` are read | There were no rules for the boundaries of strings and numbers | emela-syntax | Decided |
| 2.5 | `else`, `->`, and `|` at the start of a line continue the previous line. Comment-only lines and blank lines are skipped. The innermost bracket decides. Line breaks inside interpolation continue | Following the rules as written, `}⏎else` and the like become separate statements | emela-syntax | Decided |
| 4.1 | Checking converted names, digits after `_`, duplicates (including differences only in case), ignoring files that start with `.` and files other than `.emel`, and reserving `prelude.emel` directly under the root. The range of modules in single-file mode, entries outside the source root, the output directory, and paths in diagnostics | There were no rules for the details of conversion and discovery | emela-resolve, emela-driver | Decided |
| 6.3 | Bindings inside blocks are not generalized | The scope of let-polymorphism was undecided | emela-types | Decided |
| 6.5 | An arm with a guard does not cover the arms below it. Warnings for redundant arms, and display of examples of missing patterns. Omitting fields without `..` is a compile error. escape arms do not require exhaustiveness | The details of exhaustiveness checking were undecided | emela-types | Decided |
| 6.8, 16.3 | The definition of tail position (including the right side of `&&` / `||`; excluding the target of escape, the body of with, and the inside of anonymous functions) | There was no definition of tail position | emela-core | Decided |
| 7.5 | `emela run` prints a top-level defect as `defect: <message>` and exits with code 101 | How a defect looks at the boundary was undecided | emela-driver | Decided |
| 9.4 | Never becomes another type only at the joins of branches and at the top level of expected-type positions | The scope of the subtype-like treatment of Never was undecided | emela-types | Decided (implemented) |
| 10.6 | Show for basic types (Bool, Float, Int64), Ord (String in code point order), and Eq for Float | Results of interpolation and comparison need to agree across both targets | emela-codegen-js | Decided (implemented) |
| 16.1 | The remainder has the sign of the dividend. `%` on Float is fmod. Dividing the minimum value by -1 wraps around | There were no rules for integer and floating-point remainders | emela-codegen-js | Decided (implemented) |
| 17.1, 17.4 | Line breaks inside the braces of an effect set `use { ... }` are not terminators | 17.1 removes line breaks only inside `(` and `[`, so an effect set could not be split over multiple lines | emela-syntax | Decided (implemented) |
| 6.5, 17.6 | String literals in patterns cannot contain interpolation (E0117) | Interpolation is not constant, so it cannot be used for matching | emela-syntax | Decided (implemented) |
| 5.5, 17.4, 17.6 | Types and patterns have no parenthesized grouping form (`(Int)`, `(a)`). These are compile errors as one-element tuples (E0112). `(a)` in expressions is just parentheses | The grammars in 17.4 and 17.6 have no parenthesized form, and the statement in 5.5 that "`(x)` is just parentheses" reads as a statement about expressions | emela-syntax | Decided with changes: add the parenthesized grouping form `(T)` to types only. `(a)` in patterns stays a compile error |
| 10.5, 17.3 | `derive` that follows the line break right after a type or enum declaration continues the declaration | The example in 10.5 (`derive` on the next line) cannot be read under the rules of 17.3 and 2.5 | emela-syntax | Decided (implemented) |
| 17.3 | The constraint that requires `@external` on an fn without a body does not apply to trait fns | trait fns can omit default implementations (10.1), so the scope of the constraint is made clear | emela-syntax | Decided (implemented) |
| 16.1 | Converting a Float to an Int is a defect if the result is out of range or is NaN or ±Infinity (the same as the trap of WASM's `i32.trunc_f64_s`). `Float.round` rounds 0.5 away from zero. `Int64.to_int` wraps around to the low 32 bits | There were no rules for numeric conversion | emela-codegen-js (#122) | Decided (implemented) |
| 16.2 | `String.split(s, "")` splits into graphemes. Substring matching works on sequences of code points. The whitespace of `String.trim` is the 25 characters of Unicode White_Space (U+FEFF is not included) | There were no rules for string operations | emela-codegen-js (#122) | Decided (implemented) |
| 10.6 | Interpolation outputs only a top-level String as it is. A String inside a list or a constructor is enclosed in `"..."` and escaped | There were no rules for display in interpolation and dbg | emela-codegen-js (#122) | Decided (implemented) |
| 4.1 | In single-file mode, only the files reachable by import from the entry are read and checked | This keeps errors in unrelated sibling files out of the diagnostics | emela-driver | Decided (implemented) |
| 4.6 | Namespaces and tie-breaking by position, import and re-export, constructor collisions, hiding the Prelude, types of built-in modules, and where self is visible | There were no name resolution rules | emela-resolve | Decided (implemented) |
| 7.3, 9.4 | The "opaque set variables" that escape does not remove are set type parameters, hidden set variables, type parameters of a single error type, and set variables being generalized. From set variables being inferred in the same group, tags are removed exactly | The range of "set variable" in 18.1 #19 was undecided | emela-types (#145) | Decided (implemented) |
| 7.4, 9.2 | The E of an fn with a written return type is the union of the fails clause and the hidden set variables in positive positions in the parameter types. Passing a function that fails to a hidden set variable in a negative position is a compile error (E0321) | `-> T` means "no errors", but errors of function parameters propagate (9.2), so the relation between the two was undecided | emela-types (#145) | Decided (implemented) |
| 9.4, 9.6 | The E of function types is not unified but becomes an inclusion constraint (subtyping on E only). The direction is reversed in parameters, equal in type arguments of named types, and the same in list and tuple elements | Whether a function with a smaller E can be placed in an expected-type position was undecided | emela-types (#145) | Decided (implemented) |
| 9.2 | Omitting fails in a function type in the type annotation of a lambda parameter gives an inference set variable (since lambdas are not generalized) | Lambda parameters are "inside parameter types", but they cannot become hidden set type parameters | emela-types (#145) | Decided (implemented) |
| 9.3 | When a definition without a return type is generalized, a set variable in a negative position with no upper or lower bounds becomes a set type parameter. Other set variables take the least solution. Inclusion into a parameter of `fails E1 \| E2` is approximated by leaning on the first variable | The rules for generalizing set variables were undecided | emela-types (#145) | Decided (implemented) |
| 7.2 | If the type of e in `fail e` is a type variable, it is a single error type. A type that is not an error is a compile error (E0319) | fail on a value whose type is undetermined was undecided | emela-types (#145) | Decided (implemented) |
| 7.3 | escape arms are limited to error constructors (E0320). An arm for an error that the original expression cannot raise gets W0302 (only when E has no opaque set). An escape with an error in it is treated as receiving everything | The constraints on arm patterns and the handling of useless arms were undecided | emela-types (#145) | Decided (implemented) |
| 9.3 | Display omits an empty E, except inside the parameters of function types, where `fails {}` is shown. Hidden set variables are shown as `E(f)`, `E(f, 2)` | How E appears in the LSP and in diagnostics was undecided | emela-types, emela-lsp (#145, #146) | Decided (implemented) |
| 15.2 | Console has two operations, `print_line` and `print_error_line` (each adds a newline; no fails). The effect is in module `Console`, and the JS handler's module and handler are both `JsConsole` (`WasiConsole` for WASI). Later changed to `SystemConsole` in `Std.Console` (18.1 #33, #189) | The individual names in the platform layer were not decided | emela-core (#143) | Decided (implemented) |
| 8.2 | Calling an operation on a capability value puts the effect in R and the operation's fails in E. Capability value types unify only with the same effect, and are displayed as `use Logger` | The R of operation calls and the type of closures that capture capabilities were undecided | emela-types (#147) | Decided (implemented) |
| 8.2 | The restriction on capability values looks only at the top-level type of a value. Parameters are E0408, data structures E0409, and return values E0410. Values in the body of a with are not checked (8.6 is 0.22) | The scope of the "cannot pass, store, or return" check was undecided | emela-types (#147) | Decided (implemented) |
| 8.3, 14.2 | with removes only tags, and removes nothing from hidden effect sets. A handler of another effect in the same with can satisfy requirements, and a cycle is a compile error (E0411) | The treatment of R corresponding to escape (18.1 #19) and requirements within the same with were undecided | emela-types (#147) | Decided (implemented) |
| 8.3, 13.3, 5.6 | main is the `main` of the entry module. main and test functions have an empty R even if they have a use clause. The R of the value of a const is also empty (E0402) | The definition of main and the handling of a main with a use clause were undecided | emela-types, emela-driver (#147) | Decided (implemented) |
| 8.4 | A handler implements every operation exactly once (E0404, E0405) with the matching number of parameters (E0406), and the E of an operation body is included in the declared fails (E0318) | The details of handler checking were undecided | emela-types (#147) | Decided (implemented) |
| 9.3 | An fn without a use clause has its R inferred even if its return type is written (with no polymorphic recursion within a group). The R of an fn with a use clause is the union of the written set and the hidden effect sets in positive positions of its parameters | How "R is always inferred" in 9.3 combines with the rules in 7.4 was undecided | emela-types (#147) | Decided (implemented) |
| 9.3 | R is displayed as is if it has one element, as `{ A, B }` if it has two or more, and as `{}` if empty. In parameter positions, `use {}` is shown. Hidden effect sets are shown as `R(f)`, and generalized ones as `R`, `R1` | How R appears in the LSP and in diagnostics was undecided | emela-types (#147) | Decided (implemented) |
| 9.3, 17.4 | Effect sets are displayed not in the order written but with effects first and set type parameters after | The display order of mixed sets was undecided | emela-types (#150) | Decided (implemented) |
| 8.3 | `use X` inside an anonymous function first looks at the enclosing withs (within the same top-level function), and if there are none, refers to what the caller of the anonymous function passed | Whether "the same function" includes anonymous functions was undecided | emela-core (#152) | Decided (implemented) |
| 12.1 | An `@external` that points to a file is read after conversion to an absolute path from the directory of the source that contains the declaration (the output depends on the location of the source). An external function without a target annotation that was output although it is not reachable is a defect if called | The resolution of file specifications and the handling of external functions that slipped past the check were undecided | emela-core, emela-codegen-js (#152) | Decided with changes (2026-10-18): JS copies files other than `.emel` under the source root to the output directory and reads them by relative path (12.1). This keeps the output working after it is moved, and keeps the location of dependency files (the cache) out of the output |
| 12.5 | The defect message for an exception from an external function is prefixed with the exception's name, as in `TypeError: bad`. A value that is not an Error gives `uncaught exception: ...`. An external function that returns `()` discards the host's return value | The details of exceptions and return values at the boundary were undecided | emela-codegen-js (#152) | Decided (implemented) |
| 12.1, Appendix A | Placed E0412, the target check, in E04xx (external functions are the same side-effect path as effects; E05xx to E08xx are reserved) | The code band for external function diagnostics was undecided | emela-driver (#152) | Decided (implemented) |
| 12.1 | The `.mjs` files that the bundled core points to are embedded in the compiler, and only when used, all of them are written to `emela_core/` in the output directory, in the same layout as `lib/`. The specifier is rewritten to `./emela_core/x.mjs` | Bundled sources have no path on disk and cannot read the files next to them | emela-core, emela-driver (#155) | Decided (implemented) |
| 15.2 | Random has two operations, `int()` and `float()` (`bytes` from 0.1x is not included, because there is no Bytes). JsRandom draws with `crypto.getRandomValues` (later changed to `SystemRandom`; #189). Values in a range and seeded pseudo-random numbers are provided separately | The individual names in the platform layer were not decided | emela-core (#168) | Decided (implemented) |
| 15.2, 12.4 | Env has only `get(name) -> Option[String]`. Option cannot cross the boundary, so the host functions of JsEnv are split into `has` and `get`, and the result is assembled inside the handler. An empty value is `Some("")`. `random.emel`, `js_random.emel`, `env.emel`, and `js_env.emel` are reserved names (later no longer reserved, after std moved under `Std`; 4.1, #189) | The individual names in the platform layer were not decided | emela-core (#168) | Decided (implemented) |
| 12.2 | The restriction on external functions that omit use is decided by reference (they cannot be taken out as values). Anonymous functions inside an operation body count as inside. The host side effects of external functions are not put in R | The range of "where they can be called" and the R of external functions were undecided | emela-types (#147) | Decided (implemented) |
| 13.3, 14.3 | Errors that reach main or a test are displayed the same way as dbg (a String with quotes). An error with fields of function type shows only its name | The display of unhandled errors was undecided | emela-codegen-js (#148) | Decided (implemented) |
| 16.3 | Self tail calls that can fail are also turned into loops. The inside of escape (both the target and the arms) is not traversed | The handling of tail calls of functions that can fail was undecided | emela-core (#148) | Decided (implemented) |
| 10.2 | Every Trait method without a default implementation is implemented, and the missing ones are all listed with E0325 | The handling of incomplete impls was not stated | emela-driver | Decided (implemented) |
| 4.7 | The format of `Pome.lock`. TOML with `version = 1` and an array of `[[pome]]` tables (`path`, `version`, `commit`, `hash`, `manifest`, and the optional `external` and `dev`). It lists the whole dependency graph, sorted by source path | 4.7 decided only the fields to write, and the format and scope (whether transitive dependencies are included) were undecided | emela-pome | Open |
| 4.7 | If the major of `[pome].version` or of a version in `[dependencies]` does not match the `/vN` of the source path, it is an error in `Pome.toml` | This makes the rule that a mismatched tag is not a version an error earlier, at the file stage | emela-pome | Open |
