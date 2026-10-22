# AGENTS.md

The entry point for coding agents (Claude Code, Codex, Copilot, Cursor and others) and people working on the specification of Emela in this repository.

## The specification

- The specification of the language, the standard library, the targets and the tools is in `spec/`. Its layout and rules are in `spec/README.md`; its terms and requirement keywords (RFC 2119) are in `spec/glossary.md`.
- Every section has a stable name (`[effect.handler]`) besides its number. Refer to sections by name.
- Look things up with `scripts/spec.py` (`index`, `show <name>`, `refs <name>`, `proposals [name]`, `check`).
- Decisions inside the implementation are documented as rustdoc or comments in English in the code that makes them. Observable behavior belongs in `spec/`, not only in the code.

## Changing the specification

- A change that alters meaning goes through a proposal: refine a draft in `proposals/drafts/` (not committed), then open a draft pull request (title `proposal: ...`, label `proposal`) that adds it as `proposals/<slug>.md`. Review happens in the pull request; merging it adopts the proposal, closing it rejects the proposal. The process is in `proposals/README.md`.
- Only the language's author adopts or rejects a proposal. An agent opens or merges a pull request only when asked.
- A change that does not alter meaning (typos, wording, an inconsistency whose answer is fixed) is made directly and is not recorded in `spec/history.md`.
- Run `scripts/spec.py check` after any change.

## Writing

- The specification, `proposals/` and code comments are in English. Discussions (local drafts, pull request descriptions and review comments) may be in Japanese, but the "Specification changes" section of a proposal is written in English, ready to paste into `spec/`.
- Do not explain behavior by analogy with other languages or implementations ("the same as X", "like X"). Name the concept and state the rule.
- Code examples in the specification match the output of `emela fmt`.

## Skills

Skills are in `.agents/skills/` in the [Agent Skills](https://agentskills.io/specification) format. The directories of the same names in `.claude/skills/` are links to them.

| Skill | When to use it |
|---|---|
| `emela-spec` | To check what the specification says, before designing, implementing or proposing |
| `emela-proposal` | To refine a proposal draft with the author, open its draft pull request, and finish an adopted proposal |
| `emela-implement` | To implement an adopted proposal in the compiler and tools, tests first, and open its pull request |

For an agent that does not read `.agents/skills/`, link its skills directory to `.agents/skills/<name>`.
