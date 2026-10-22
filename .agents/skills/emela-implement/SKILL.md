---
name: emela-implement
description: Implement an adopted proposal (proposals/<slug>.md with status accepted) in the Emela compiler, tools and standard library, following the specification in spec/. Writes the tests from the rules of the specification first, implements crate by crate, assigns diagnostic codes, and opens a pull request that sets the proposal to implemented. Use when asked to implement a proposal or a decided change of the specification. Triggers include "implement the proposal", "proposal を実装して", "実装して" with a proposal's slug or pull request number, and "emela-implement".
license: Apache-2.0
compatibility: Requires python3, ripgrep (rg), cargo, cargo-nextest, node, a C compiler, wasmtime and the GitHub CLI (gh). Run from the root of the emela repository (github.com/emela-lang/emela).
metadata:
  repository: github.com/emela-lang/emela
  version: "1"
---

# emela-implement: implement an adopted proposal

The specification in `spec/` is what is implemented. The proposal tells you why and where; `spec/` tells you what. Look up the specification with the `emela-spec` skill and `scripts/spec.py`. The process around proposals is in `proposals/README.md`.

Talk with the person who asked in the language they use. Code, comments, rustdoc, commit bodies that quote the specification, and anything written into `spec/` are in English.

## 0. Check that the proposal can be implemented

1. Find the proposal: by slug (`proposals/<slug>.md`) or by its pull request number (`rg -l '^pr: <N>$' proposals`).
2. It MUST be `status: accepted` and its "Specification changes" MUST already be in `spec/` on main. If it is `draft`, stop: an agent does not implement an undecided proposal. If the specification changes are not applied yet, stop and say so (the `emela-proposal` skill applies them).
3. Read `requires`. Every proposal listed there MUST be `implemented` first. Otherwise stop and name it.
4. Check that no one else is implementing it: `gh pr list --search "Implements: proposals/<slug>.md"` and `gh issue list --search "<slug>"`.
5. Start a branch from main: `feat/<slug>`.

## 1. Read the specification, not only the proposal

1. Read the proposal once for the motivation, "Implementation" (the crates to change) and "Diagnostics".
2. Read every section in `touches` from `spec/`, and the sections they refer to:

   ```sh
   scripts/spec.py show effect.handler effect.with
   scripts/spec.py refs effect.handler    # where the implementation already refers to it
   ```

3. Make a list of the rules: one line per normative sentence (MUST, MUST NOT, "is a compile error", an order of evaluation, a value). Each rule becomes at least one test in step 2. Keep the list in the pull request description.
4. If a rule is ambiguous, contradicts another section, or cannot be implemented as written, stop and report it with the section names. Do not choose an answer in the code: a change of meaning goes through a proposal, and a fix whose answer is fixed by the rest of the specification is made in `spec/` in its own commit.

## 2. Write the tests first

Write tests from the list of rules, and see them fail before implementing. Put each test where the crate that owns the rule keeps its tests. Look at the neighbouring tests and follow their form (most use `insta` snapshots).

| A rule about | Test in |
|---|---|
| Syntax, the parse tree | `crates/emela-syntax/tests/parser.rs` (and `ast.rs`) |
| Names and scopes | `crates/emela-resolve/tests/resolve.rs` |
| Types, E and R, a compile error | `crates/emela-types/tests/` (`effects.rs`, `errors.rs`, `body.rs`, ...); code that the specification shows goes in `spec_examples.rs` |
| A diagnostic as the user sees it (code, message, help, position) | `crates/emela-driver/tests/diagnostics.rs` |
| Lowering | `crates/emela-core/tests/lower.rs` |
| A result at run time, an order of evaluation | a program `crates/emela-codegen-c/tests/programs/<name>.emel` with its `.expected` output |
| Formatting of the new syntax | `crates/emela-fmt/tests/format.rs` |
| The language server | `crates/emela-lsp/tests/server.rs` |

- For every compile error in the rules, write the smallest program that MUST be rejected, and assert the code.
- For every construct that MUST be accepted, write a program that uses it in each place the grammar allows it.
- A result at run time MUST be the same on every target (spec chapter 16). A program under `emela-codegen-c/tests/programs/` runs on JS and C through `crates/emela-driver/tests/parity.rs`. Add a program that uses only the pure language and `dbg` to `PROGRAMS` in `crates/emela-driver/tests/wasm_parity.rs` as well, so that WASM is checked against JS. Do not leave a target out: a target that cannot run the new construct yet is a defect to report, not a reason to skip it.
- Code examples in `spec/` that use the new construct MUST type-check: add them to `spec_examples.rs`.

## 3. Implement, crate by crate

Follow the order of the pipeline, and make the tests of each crate pass before the next:

`emela-syntax` → `emela-resolve` → `emela-types` → `emela-core` → `emela-lir` → `emela-codegen-js`, `emela-codegen-c`, `emela-codegen-wasm` → `emela-fmt`, `emela-lsp`, `emela-cli`

Skip the crates the change does not reach, but check each one: new syntax almost always reaches `emela-fmt` and `emela-lsp`.

- Diagnostics:
  - A diagnostic marked "code to be assigned" gets the next free code of its stage (E01xx parsing, E02xx modules and names, E03xx types, E04xx effects; W for warnings). A number is never reused.
  - Add it to the table in `spec/diagnostics.md` (`[diag.list]`) with its message, help and the section, and to the copy in the code (`crates/emela-driver/src/code.rs`, or `DiagnosticCode` in `emela-syntax` for E01xx).
  - Write the code back into the proposal's "Diagnostics" in place of "code to be assigned", and tick the item.
- Decisions inside the implementation (a data structure, an approximation, an order that the specification leaves open) are written as rustdoc or comments in English where they are made. Refer to sections by name (`[effect.with]`). Observable behavior belongs in `spec/`, not only in the code.
- Do not explain by analogy with other languages in comments either. Name the concept.
- If the implementation shows that the specification is wrong or incomplete, stop and report it (step 1.4). Do not make the code differ from `spec/`.

## 4. Check

Run everything CI runs for code:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked
cargo build --bin emela --locked
scripts/spec.py check
```

And the examples and the laws, as in `.github/workflows/ci.yml` (`examples` and `laws` jobs): `emela fmt --check`, `emela check`, `emela test` on JS and C in each `examples/*` and in `tests/laws`.

The parity tests need node, a C compiler and wasmtime; without them they are skipped with a message. A skipped parity test is not a passed one: say which were skipped.

## 5. Open the pull request

1. Set the proposal's `status` to `implemented` in the same pull request. If the work is split into several pull requests, only the last one does this; the others say "Part of proposals/<slug>.md".
2. Commits follow the repository's convention (Conventional Commits, `feat:` for the language change, subject in Japanese). Do not put a bare `@name` in a commit message.
3. Open the pull request:
   - Title: `feat: <what users can now write>`
   - Body: the first line is `Implements: proposals/<slug>.md`, then `Closes #<issue>` if there is an implementation issue, the list of rules from step 1.3 with the test that checks each one, the diagnostic codes assigned, and anything skipped or reported.
4. Do not merge it. The author reviews and merges.

## Stopping and reporting

Stop and report, instead of guessing, when:

- the proposal is not `accepted`, or its specification changes are not in `spec/`
- a proposal in `requires` is not `implemented`
- the specification is ambiguous, contradicts itself, or cannot be implemented as written
- a target cannot support the construct

Report with the names of the sections, the smallest program that shows the problem, and what each target or the compiler does now.
