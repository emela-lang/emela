# Proposals

Proposals change the specification in `spec/`. A proposal is refined as a local draft, then reviewed in a pull request that adds its file to this directory together with its changes to `spec/`. The language's author decides it with a comment: `/accept` merges the pull request, `/reject` closes it, and a bot records the decision in the repository. Either a person or an LLM can write one, from a branch of this repository or from a fork, and every step is the same for both.

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

Discussion may be in Japanese: the local draft, the pull request description and review comments. The changes to `spec/` are in English from the start. The proposal file is in English by the time it is accepted; `/accept` refuses one with Japanese text.

## Flow

```text
local draft ──> pull request: proposals/<slug>.md + spec/ (review)
                   ├── /accept ──> merged ──> record of the adoption ──> implementation issue
                   └── /reject ──> closed ──> record of the rejection
```

### 1. Local draft

- Copy `TEMPLATE.md` to `proposals/drafts/<slug>.md`. `proposals/drafts/` is not committed (`.gitignore`). The slug is lower case with `-`, and stays the proposal's file name.
- Before opening a pull request, refine the draft in discussion with an LLM. The LLM acts as the other side of the discussion: it points out weak motivation, alternatives, conflicts with other sections of the specification, counterexamples, and the results of prototypes (the `emela-proposal` skill).
- One draft holds one decision. When it depends on another decision, write that as a separate draft and refer to it in `requires`.
- Open questions may remain. They are what the review discusses.

### 2. Pull request

- When the draft is ready for review, move it to `proposals/<slug>.md` with `status: draft`, apply its changes to `spec/`, and open a draft pull request against `main` with both. From a fork, open it the same way (`gh pr create --repo emela-lang/emela`).
  - Title: `proposal: <title>`
  - Commit type: `docs(proposal): ...`
- A pull request that adds a proposal MUST change `spec/` (CI fails otherwise). The proposal holds the reasons (motivation, alternatives, prior art, open questions); the diff of `spec/` is the change itself, and the review comments on its lines. Give new sections a name in the heading, and keep the names of existing sections even if numbers shift. Do not edit `spec/history.md`: the record of the adoption writes it.
- Do not fill in `pr`, `decided` or "Decision". The record of the decision writes them.
- The label `proposal` is added automatically (`.github/workflows/proposal-label.yml`), so the author of a pull request from a fork does not need to set it.
- Changes from the review are pushed as commits to the same pull request. When another proposal changes the same sections first, rebase onto `main`.
- Pull requests that change only `proposals/` and `spec/` skip the build and the tests in CI.

### 3. Decision

The language's author decides with a comment on the pull request whose first line is `/accept` or `/reject`, followed by the reasons. The `Proposal decision` workflow (`.github/workflows/proposal-decision.yml`) acts only on comments from the author. Neither command pushes to the pull request's branch, so a pull request from a fork is decided the same way.

- `/accept` checks the proposal as the head commit has it: `status: draft`, no open questions, no Japanese text (`scripts/spec.py ready`), `scripts/spec.py check`, and a change to `spec/`. It then comments a marker with that commit, marks the pull request ready for review, runs CI again, and adds the pull request to the merge queue.
- `/reject` closes the pull request with the reasons.
- CI (`proposal-adopted`, part of the required `ci-ok`) fails on a pull request that adds a proposal until the marker names its head commit. A commit pushed after `/accept` needs a new `/accept`. Nothing else merges a proposal.

After the pull request is closed, the `Proposal record` workflow (`.github/workflows/proposal-record.yml`) opens a pull request with the label `proposal-record` and adds it to the merge queue once its checks pass:

- adopted: `scripts/spec.py accept` sets `status: accepted`, `pr` and `decided`, writes the reasons of `/accept` into "Decision", and adds an entry to `spec/history.md` (the date, the proposal's slug and title, and the names of the sections in `touches`)
- rejected: `scripts/spec.py reject` keeps the proposal as it stood, with `status: rejected` and the reasons in "Decision", in `proposals/rejected/<slug>.md`

A pull request closed without `/reject` (withdrawn by its author) records nothing. When a record cannot be merged (two records changed `spec/history.md` at once), run the workflow again by hand with the pull request's number.

### 4. Implementation

- When the record of an adoption is merged, the `Proposal implementation issue` workflow (`.github/workflows/proposal-implementation.yml`) opens an issue `Implement: <title>` with the label `implementation` and no assignee. The person who implements the proposal, who may not be its author, assigns themselves to the issue. An issue with no assignee is open for anyone to take.
- An adopted proposal is implemented following `spec/` (the `emela-implement` skill). The pull request that completes the implementation starts its description with `Implements: proposals/<slug>.md`, closes the issue with `Closes #<issue>`, and sets the proposal's `status` to `implemented` itself.

## Status

| `status` | Meaning |
|---|---|
| `draft` | Under review in an open pull request, or merged and waiting for the record of its adoption |
| `accepted` | Adopted. Its pull request changed the specification |
| `implemented` | The implementation follows it |
| `rejected` | Rejected. Only in `proposals/rejected/` |

Only the language's author (a person) adopts or rejects a proposal. An LLM does not set `accepted` or `rejected`, and opens or merges pull requests only when asked.

## Front matter

```yaml
---
title: Arguments for handler init
authors: [Alliana]
status: draft           # draft, accepted, implemented; rejected in proposals/rejected/
issue: 374              # an issue the proposal came from, if any
touches: [effect.handler, effect.with, effect.layer, grammar.decl]
requires: []            # slugs of proposals that must be accepted first
related: []             # slugs of related proposals
pr: 380                 # written by the record of the decision
decided: 2026-01-01     # written by the record of the decision
---
```

- `touches` lists the names of every section the proposal changes (`spec/README.md`, "Section names"). A new section is written with its planned name and `(new)` (`ext.c-host (new)`).

## Sections

In the order of `TEMPLATE.md`:

| Section | Contents |
|---|---|
| Summary | What changes, in two or three sentences. It becomes the entry in `spec/history.md` |
| Motivation | What the current specification cannot express or makes hard, shown with real code |
| Proposal | Syntax, meaning and typing rules, with the code users write first |
| Diagnostics | Diagnostics added, changed or removed. "Code to be assigned" when not decided |
| Compatibility | Whether programs that compile now stop compiling or change meaning. If they do, the change lands in a minor release; until then it is an experimental feature or a deprecation warning (`docs/release.md`) |
| Implementation | The crates to change, and the results of any prototype |
| Alternatives | Syntax and design alternatives, and why they were not chosen |
| Prior art | Related concepts, and the languages or papers that have them, named by concept |
| Open questions | Undecided points, one per item. Empty before `/accept` |
| Decision | The author's reasons, written by the record of the decision |

Proposals adopted before changes to `spec/` were part of their pull request also have a "Specification changes" section, with the text that was pasted into the specification.

## Writing

- Use the requirement keywords and terms of `spec/glossary.md`.
- Do not explain by analogy with other languages or implementations ("the same as X", "like X"). Name the concept (row polymorphism, one-shot continuations, transparent type aliases, nominal types) and state the rule. Mention other languages only in "Prior art", and even there lead with the concept.
- Put what is not certain in "Open questions" rather than hedging in the text.
- Refer to sections of the specification by name (`[effect.handler]`).
- Code examples match the output of `emela fmt`.
