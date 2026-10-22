# Proposals

Proposals change the specification in `spec/`. A proposal is refined as a local draft, then reviewed in a pull request that adds its file to this directory. Merging the pull request adopts the proposal; closing it rejects the proposal. Either a person or an LLM can write one, and either can follow the same steps.

## When to write a proposal

Write one for a change that alters meaning (`spec/README.md`, "Changing the specification"):

- adding or changing syntax, typing rules or semantics
- adding or removing diagnostics, or changing their codes
- changing the scope or the commitments of the standard library (`[lib]`, `[api]`)
- changing observable behavior of targets and tools
- deciding a question in `open-questions.md`

A typo, a wording fix, or an inconsistency whose answer is fixed by the rest of the specification can be fixed directly.

A problem that is not yet shaped as a proposal (a gap, a divergence between the specification and the implementation, a request) can be filed as an issue first. When it becomes a proposal, its pull request refers to the issue.

## Language

Discussion may be in Japanese: the local draft, the pull request description and review comments. The "Specification changes" section is written in English, ready to paste into `spec/`. When the proposal is merged, the whole file is in English.

## Flow

```text
local draft ──> draft pull request (review) ──> merged (adopted)
                        │
                        └──> closed (rejected)
```

### 1. Local draft

- Copy `TEMPLATE.md` to `proposals/drafts/<slug>.md`. `proposals/drafts/` is not committed (`.gitignore`). The slug is lower case with `-`, and stays the proposal's file name.
- Before opening a pull request, refine the draft in discussion with an LLM. The LLM acts as the other side of the discussion: it points out weak motivation, alternatives, conflicts with other sections of the specification, counterexamples, and the results of prototypes (the `emela-proposal` skill).
- One draft holds one decision. When it depends on another decision, write that as a separate draft and refer to it in `requires`.
- Open questions may remain. They are what the review discusses.

### 2. Pull request

- When the draft is ready for review, move it to `proposals/<slug>.md` with `status: draft`, and open a draft pull request that adds only this file.
  - Title: `proposal: <title>`
  - Label: `proposal`
  - Commit type: `docs(proposal): ...`
- Write the pull request number in the file's `pr` field.
- The review happens in the pull request, with comments on lines of the proposal. Changes from the review are pushed as commits to the same pull request.
- Pull requests with the label `proposal` change only `proposals/` and `spec/`, so CI skips the build and the tests, and they do not appear in the changelog.

### 3. Decision

- When the language's author adopts the proposal:
  - set `status: accepted` and `decided`, empty "Open questions" (it MUST be empty), and fill in "Decision"
  - apply "Specification changes" to `spec/`, in the same pull request or in a following one
  - add an entry to `spec/history.md` (the date, the proposal's slug and title, and the names of the sections it changed)
  - mark the pull request ready for review and merge it
- When the author rejects the proposal, close the pull request with the reason in a comment.
- The author decides with a comment on the pull request whose first line is `/accept` or `/reject`, followed by the reasons. The `Proposal decision` workflow (`.github/workflows/proposal-decision.yml`) acts only on comments from the author. On `/accept` it sets `status` and `decided`, writes the reasons into "Decision", adds the entry to `spec/history.md` (`scripts/spec.py accept`), and marks the pull request ready for review; applying "Specification changes" and merging stay with the author. On `/reject` it closes the pull request with the reasons. It refuses a proposal that still has open questions or Japanese text, and a pull request from a fork, which is finished by hand.
- CI (`proposal-adopted`, part of the required `ci-ok`) fails on a pull request that adds a proposal, or changes one from `draft`, until the proposal is `status: accepted` and the author has commented `/accept` on the pull request. A proposal cannot be merged without the author's decision, including one finished by hand. Nothing is merged. Rejected proposals are listed by `is:pr label:proposal is:closed is:unmerged`.

### 4. Implementation

- An adopted proposal is implemented following `spec/` (the `emela-implement` skill). The pull request that completes the implementation starts its description with `Implements: proposals/<slug>.md` and sets the proposal's `status` to `implemented` itself.

## Status

| `status` | Meaning |
|---|---|
| `draft` | Under review in an open pull request |
| `accepted` | Merged. The specification includes it, or will in a following pull request |
| `implemented` | The implementation follows it |

Only the language's author (a person) adopts or rejects a proposal. An LLM does not set `accepted`, and opens or merges pull requests only when asked.

## Front matter

```yaml
---
title: Arguments for handler init
authors: [Alliana]
status: draft           # draft, accepted, implemented
pr: 380                 # once the pull request is open
issue: 374              # an issue the proposal came from, if any
decided:                # the date, when accepted
touches: [effect.handler, effect.with, effect.layer, grammar.decl]
requires: []            # slugs of proposals that must be accepted first
related: []             # slugs of related proposals
---
```

- `touches` lists the names of every section the proposal changes (`spec/README.md`, "Section names"). A new section is written with its planned name and `(new)` (`ext.c-host (new)`).

## Sections

In the order of `TEMPLATE.md`:

| Section | Contents |
|---|---|
| Summary | What changes, in two or three sentences |
| Motivation | What the current specification cannot express or makes hard, shown with real code |
| Proposal | Syntax, meaning and typing rules, with the code users write first |
| Specification changes | For each section in `touches`, the text to put into the specification, as it will be pasted |
| Diagnostics | Diagnostics added, changed or removed. "Code to be assigned" when not decided |
| Compatibility | Whether programs that compile now stop compiling or change meaning |
| Implementation | The crates to change, and the results of any prototype |
| Alternatives | Syntax and design alternatives, and why they were not chosen |
| Prior art | Related concepts, and the languages or papers that have them, named by concept |
| Open questions | Undecided points, one per item |
| Decision | The author's reasons, written when the proposal is accepted |

## Writing

- Use the requirement keywords and terms of `spec/glossary.md`.
- Do not explain by analogy with other languages or implementations ("the same as X", "like X"). Name the concept (row polymorphism, one-shot continuations, transparent type aliases, nominal types) and state the rule. Mention other languages only in "Prior art", and even there lead with the concept.
- Put what is not certain in "Open questions" rather than hedging in the text.
- Refer to sections of the specification by name (`[effect.handler]`).
- Code examples match the output of `emela fmt`.
