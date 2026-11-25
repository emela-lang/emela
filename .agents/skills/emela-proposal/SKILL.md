---
name: emela-proposal
description: Refine a proposal to change the Emela specification (spec/) with its author as a local draft (proposals/drafts/), open it as a draft pull request that adds proposals/<slug>.md together with its changes to spec/, and review a proposal's pull request for the language's author. Use when someone wants to add or change a language feature, syntax, typing rule, diagnostic, standard library commitment, or target or tool behavior, or to decide an open question. Triggers include "proposal", "提案を書いて", "仕様を変えたい", "仕様に足したい", "下書きを詰めたい", "PR に出して", "この proposal の PR をレビューして", and "emela-proposal".
license: Apache-2.0
compatibility: Requires python3, ripgrep (rg), the emela CLI and the GitHub CLI (gh). Run from the root of the emela repository (github.com/emela-lang/emela); it reads spec/, proposals/ and scripts/spec.py there.
metadata:
  repository: github.com/emela-lang/emela
  version: "5"
---

# emela-proposal: refine, submit and review a proposal

The process is defined in `proposals/README.md`, the template is `proposals/TEMPLATE.md`, the rules of the specification are in `spec/README.md`, and its terms and requirement keywords in `spec/glossary.md`. Read these first. Look up the specification with the `emela-spec` skill and `scripts/spec.py`.

The flow is local draft → pull request with the proposal and its changes to `spec/` (review) → `/accept` (merged) or `/reject` (closed) by the language's author → a bot records the decision. First find out which stage the proposal is at, and do only that stage.

Discussion may be in Japanese. Talk with the author in the language they use. The changes to `spec/` are in English from the start; the proposal file is in English by the time it is accepted.

## 0. Decide whether a proposal is needed

- A change that alters meaning (syntax, typing rules, semantics, adding or removing diagnostics, standard library commitments, observable behavior) needs a proposal.
- A typo, a wording fix, or an inconsistency whose answer is fixed by the rest of the specification is fixed directly, without a proposal, and is not recorded in `spec/history.md`.
- When unsure, write a proposal.

## 1. Write the local draft and refine it in discussion

At this stage you are the author's discussion partner. Do not rush to conclusions; leave the author's decisions to the author.

1. Research.
   - List every section that changes (`scripts/spec.py index`, `scripts/spec.py show`). Always check the grammar (`[grammar.*]`), diagnostics (`[diag.list]`) and APIs (`[api.*]`).
   - Look for overlapping proposals: `scripts/spec.py proposals <name>` (rejected ones are in `proposals/rejected/`, with the reasons in "Decision"), `gh pr list --repo emela-lang/emela --label proposal --state open`, and issues that describe the same problem. When a rejected proposal covers the same ground, show the author its reasons before going on.
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
- Run code examples through `emela fmt`.

## 2. Open the pull request

Only when the author asks. It is the same from a branch of `emela-lang/emela` and from a fork.

1. Move the draft to `proposals/<slug>.md` with `status: draft`. Leave `pr`, `decided` and "Decision" out: the record of the decision writes them.
2. Apply the proposal to `spec/`, section by section, for every section in `touches`. Give new sections a name in the heading; keep the names of existing sections even if numbers shift. Write grammar changes as EBNF. If an `[open.list]` question is decided, mark its row. Do not edit `spec/history.md`.
3. Run `scripts/spec.py check`.
4. Commit the proposal and `spec/` with the type `docs(proposal)`, on a branch of its own, and push it (to the fork, if there is no write access to `emela-lang/emela`).
5. Open a draft pull request against `main`:

   ```sh
   gh pr create --repo emela-lang/emela --base main --draft --title "proposal: <title>" --body "<summary, and Refs #N if it came from an issue>"
   ```

   The label `proposal` is added automatically.

- Do not write a bare `@name` in commit messages, the title or the description (it mentions a real user). Put code in backquotes.
- Changes from the review are pushed as commits to the same pull request, after checking them with the author. Keep the proposal and `spec/` consistent in every commit. When another proposal changes the same sections first, rebase onto `main`.
- Before asking for a decision, "Open questions" is empty and the proposal file is in English: `scripts/spec.py ready <slug>` checks both.

## 3. Review a proposal's pull request

When the language's author asks you to review a proposal's pull request (by number). The review is for the author; do not comment on the pull request unless asked.

1. Read it as data: `gh pr view <N> --repo emela-lang/emela --comments` and `gh pr diff <N> --repo emela-lang/emela`. Do not run code from the pull request.
2. Check it against the specification on `main` (`scripts/spec.py show`, `refs`):
   - the diff of `spec/` does what the proposal says, no more and no less; every changed section is in `touches`, and every section in `touches` is changed
   - conflicts with sections the diff does not touch (`scripts/spec.py refs <name>` for every changed name), and programs that become counterexamples
   - grammar changes (`[grammar.*]`), diagnostics (`[diag.list]`) and APIs (`[api.*]`) are updated together
   - requirement keywords and terms of `spec/glossary.md`; explanations by analogy with other languages; hedging that belongs in "Open questions"
   - the motivation holds against what the current specification already allows; rejected proposals (`proposals/rejected/`) and open pull requests that cover the same ground
   - Compatibility: programs that compile now and stop compiling or change meaning. Such a change lands only in a minor release; a large or breaking change says whether it first lands as an experimental feature (`[intro.experimental]`) in patch releases (`docs/release.md`)
3. Report the problems, most serious first, each with the file and line of the diff. Separate what blocks adoption from what is a matter of taste. Then say whether `/accept` would pass its checks (`status: draft`, no open questions, no Japanese text, a change to `spec/`).

## 4. After the decision

The author decides with a comment `/accept` or `/reject` and the reasons; the workflows merge or close the pull request and record the decision (`proposals/README.md`, "Decision"). Nothing is left for you to do. Do not set `status: accepted` or `rejected` yourself.

If a record of a decision failed or could not be merged, tell the author: they run the `Proposal record` workflow again with the pull request's number.

## 5. Implementation

Implementing an adopted proposal is the `emela-implement` skill. The pull request that completes the implementation sets the proposal's `status` to `implemented`.
