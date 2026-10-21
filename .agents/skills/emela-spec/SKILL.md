---
name: emela-spec
description: Look up the Emela specification (spec/) quickly and answer while keeping normative text, open questions and proposals under discussion apart. Use when asked how the Emela language, its standard library, targets or tools behave, before designing or implementing, and before writing a proposal. Triggers include "what does the spec say", "仕様では", "spec では", "仕様を確認", a section name such as "[effect.handler]", and "emela-spec".
license: Apache-2.0
compatibility: Requires python3, ripgrep (rg) and the emela CLI. Run from the root of the emela repository (github.com/emela-lang/emela); it reads spec/, proposals/ and scripts/spec.py there.
metadata:
  repository: github.com/emela-lang/emela
  version: "3"
---

# emela-spec: look up the specification

The specification is in `spec/`. Every section has a stable name (`[effect.handler]`). The rules are in `spec/README.md`, the terms and requirement keywords (RFC 2119) in `spec/glossary.md`. Look things up with `scripts/spec.py`, run from the repository root.

## Steps

1. Find the place. List a chapter by its prefix, or search for a word.

   ```sh
   scripts/spec.py index effect             # the sections of chapter 8
   rg -n 'handler' spec --glob '*.md' -l    # by word
   ```

   | To find | Look at |
   |---|---|
   | Syntax | `[grammar]` (grammar.md) and the examples in each chapter |
   | Typing and effects | `[type]`, `[effect]`, `[error]` |
   | Results and their order | `[dyn]`, `[fiber]` |
   | The standard library | `[lib]` (scope and commitments), `[api]` (each API) |
   | Differences between targets | `[target]`, `[target.js]`, `[target.c]`, `[target.wasm]` |
   | Diagnostic codes and messages | `[diag.list]` (`rg -n 'E0424' spec/diagnostics.md`) |
   | Commands, formatter, language server, Pome | `spec/tooling/` |
   | What is not decided yet | `[open.list]` |
   | A term | `[glossary]` |

2. Read only the sections that matter. Do not read everything.

   ```sh
   scripts/spec.py show effect.handler effect.with
   ```

3. Check the state of those sections.

   ```sh
   scripts/spec.py proposals effect.handler       # local drafts and decided proposals touching it
   gh pr list --label proposal --search "effect.handler"   # proposals under review
   ```

   - A decided proposal (`status: accepted`) that is not yet in the text is newer than the text.
   - Local drafts and open proposal pull requests (`status: draft`) are not decisions. Do not mix them into the answer.

4. Look at the implementation only when the question is about it: rustdoc and comments in the crates refer to sections by name.

   ```sh
   scripts/spec.py refs effect.handler    # where the name is referred to in crates and proposals
   ```

5. If unsure whether the specification or the implementation is right, check with the compiler: put an empty `Pome.toml` and `src/main.emel` in a temporary directory outside the repository and run `emela check` or `emela run`. If they differ, report both.

## Answering

- Cite sections by name (`[effect.handler]` (8.4)).
- Keep these four apart, and say which one a statement comes from:
  - normative: the text of the specification
  - informative: sections marked "Informative." (`[target.impl-hint]` and others)
  - open: questions in `[open.list]`
  - proposed: local drafts and open proposal pull requests
- When the specification does not cover something, say so. Do not fill the gap with the implementation's behavior; if you report that behavior, say that it is the implementation's.
- Do not explain by analogy with other languages. State the rule and name the concept.
- Answer the user in the language they use.

## When you find a gap

- Observable behavior that the specification does not cover, an inconsistency inside the specification, or a reference to a section that does not exist: mention it at the end of the answer. To fix it, use the `emela-proposal` skill, or fix it directly if the answer is already fixed by the rest of the specification (`spec/README.md`, "Changing the specification").
- `scripts/spec.py check` finds duplicate names, references to missing sections, and errors in proposal front matter.
