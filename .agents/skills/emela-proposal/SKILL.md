---
name: emela-proposal
description: Refine a proposal to change the Emela specification (spec/) with its author as a local draft (proposals/drafts/), open it as a draft pull request that adds proposals/<slug>.md, and finish it when the author adopts it. Use when someone wants to add or change a language feature, syntax, typing rule, diagnostic, standard library commitment, or target or tool behavior, or to decide an open question. Triggers include "proposal", "提案を書いて", "仕様を変えたい", "仕様に足したい", "下書きを詰めたい", "PR に出して", "採択したので仕上げて", and "emela-proposal".
license: Apache-2.0
compatibility: Requires python3, ripgrep (rg), the emela CLI and the GitHub CLI (gh). Run from the root of the emela repository (github.com/emela-lang/emela); it reads spec/, proposals/ and scripts/spec.py there.
metadata:
  repository: github.com/emela-lang/emela
  version: "4"
---

# emela-proposal: refine and submit a proposal

The process is defined in `proposals/README.md`, the template is `proposals/TEMPLATE.md`, the rules of the specification are in `spec/README.md`, and its terms and requirement keywords in `spec/glossary.md`. Read these first. Look up the specification with the `emela-spec` skill and `scripts/spec.py`.

The flow is local draft → draft pull request (review) → merged (adopted) or closed (rejected). First find out which stage the proposal is at, and do only that stage.

Discussion may be in Japanese. Talk with the author in the language they use. The "Specification changes" section is in English from the start; the whole file is in English by the time it is merged.

## 0. Decide whether a proposal is needed

- A change that alters meaning (syntax, typing rules, semantics, adding or removing diagnostics, standard library commitments, observable behavior) needs a proposal.
- A typo, a wording fix, or an inconsistency whose answer is fixed by the rest of the specification is fixed directly, without a proposal, and is not recorded in `spec/history.md`.
- When unsure, write a proposal.

## 1. Write the local draft and refine it in discussion

At this stage you are the author's discussion partner. Do not rush to conclusions; leave the author's decisions to the author.

1. Research.
   - List every section that changes (`scripts/spec.py index`, `scripts/spec.py show`). Always check the grammar (`[grammar.*]`), diagnostics (`[diag.list]`) and APIs (`[api.*]`).
   - Look for overlapping proposals: `scripts/spec.py proposals <name>`, `gh pr list --label proposal --state all`, and issues that describe the same problem.
   - Check whether `[open.list]` has the same question.
   - Check claims about current behavior with the compiler: put an empty `Pome.toml` and `src/main.emel` in a temporary directory outside the repository and run `emela check` or `emela run`.
2. Copy `TEMPLATE.md` to `proposals/drafts/<slug>.md` (not committed) and write it.
3. Discuss. Bring the author:
   - weak points in the motivation, and any workaround the current specification already allows
   - alternatives, and whether the reasons for rejecting them hold
   - conflicts with other sections of the specification, and programs that are counterexamples
   - the results of prototypes
   - each point that needs a decision, as an item in "Open questions"
4. Reflect the author's decisions in the draft. Do not write anything the author has not decided as if it were decided.

Writing:
- Use the requirement keywords and terms of `spec/glossary.md`.
- Do not explain by analogy with other languages or implementations ("the same as X", "like X"). Name the concept (row polymorphism, one-shot continuations, parameterised handlers, transparent type aliases) and state the rule. Mention other languages only in "Prior art".
- Write "Specification changes" for each section in `touches`, in English, as text that can be pasted into the specification. Write grammar changes as EBNF.
- Run code examples through `emela fmt`.

## 2. Open the draft pull request

Only when the author asks.

1. Move the draft to `proposals/<slug>.md` with `status: draft`. Run `scripts/spec.py check`.
2. Commit only that file with the type `docs(proposal)`, on a branch of its own.
3. Open a draft pull request:

   ```sh
   gh pr create --draft --title "proposal: <title>" --label proposal --body "<summary, and Refs #N if it came from an issue>"
   ```

4. Write the pull request number in the file's `pr` field and push.

- Do not write a bare `@name` in commit messages, the title or the description (it mentions a real user). Put code in backquotes.
- Changes from the review are pushed as commits to the same pull request, after checking them with the author.

## 3. Finish an adopted proposal

Only when the author has adopted the proposal and asks you to finish it.

1. In `proposals/<slug>.md`: set `status: accepted` and `decided: <date>`, empty "Open questions", and write the author's reasons in "Decision" (in the author's words). Make sure the whole file is in English.
2. Apply "Specification changes" to `spec/`, section by section (in the same pull request, or in a following one if the author prefers). Give new sections a name in the heading. Keep names even if numbers shift.
3. Add an entry at the top of `spec/history.md`: the date, the proposal's slug and title, and the names of the sections changed.
4. If an `[open.list]` question was decided, mark its row as decided.
5. Make `scripts/spec.py check` pass.
6. Mark the pull request ready for review (`gh pr ready <N>`). The author merges it.

When the author rejects the proposal, the author closes the pull request with the reason. Nothing else is needed.

## 4. After the implementation is merged

Set the proposal's `status` to `implemented`.
