# The Emela Language Specification (0.20 series)

Emela is a functional language that keeps three things simple: defining data, the functions that operate on it, and making side effects explicit. It compiles to JavaScript, WebAssembly and C. This directory is the specification of the 0.20 series: the language, the standard library, the targets and the tools.

The specification is changed through proposals (see [Changing the specification](#changing-the-specification)).

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED", "MAY", and "OPTIONAL" in this specification are to be interpreted as described in BCP 14 (RFC 2119, RFC 8174) when, and only when, they appear in all capitals. How they map to the wording of the specification is in [`glossary.md`](glossary.md).

## Layout

| File | Contents | Name prefix |
|---|---|---|
| `glossary.md` | Terms and requirement keywords | `glossary` |
| `language/01-principles.md` | 1. Positioning and design principles | `intro` |
| `language/02-lexical.md` | 2. Lexical structure | `lex` |
| `language/03-keywords.md` | 3. Reserved words and attributes | `kw` |
| `language/04-modules.md` | 4. Modules and visibility | `module` |
| `language/05-data.md` | 5. Data | `data` |
| `language/06-functions.md` | 6. Functions and expressions | `fn` |
| `language/07-errors.md` | 7. Errors | `error` |
| `language/08-effects.md` | 8. Effects | `effect` |
| `language/09-types.md` | 9. Types and effects | `type` |
| `language/10-traits.md` | 10. Traits and deriving | `trait` |
| `language/11-concurrency.md` | 11. Concurrency | `fiber` |
| `language/12-external.md` | 12. External functions | `ext` |
| `language/13-testing.md` | 13. Testing | `test` |
| `language/14-dynamics.md` | 14. Dynamic semantics | `dyn` |
| `library/15-stdlib.md` | 15. Scope of the standard library | `lib` |
| `library/api.md` | The APIs of the bundled core and std | `api` |
| `targets/16-targets.md` | 16. Targets | `target` |
| `targets/js.md`, `targets/c.md`, `targets/wasm.md` | Observable behavior of each target | `target.js` and so on |
| `grammar.md` | 17. Grammar | `grammar` |
| `diagnostics.md` | Appendix A. Diagnostic codes | `diag` |
| `tooling/` | Appendix B. Commands, the formatter, the language server, Pome, the test runner | `cmd` and so on |
| `open-questions.md` | 18. Open questions | `open` |
| `history.md` | Revision history, and the record of what drafts filled in | `history` |

## Section names

Besides its number, every section has a stable name: the `` `[effect.handler]` `` at the end of a heading.

- A number (8.4) gives the order and can change when chapters or sections are added. A name never changes once given.
- Refer to a section by its name, in the specification, in proposals, and in code comments (`[effect.handler]`). A number may be added for readers (`[effect.handler]` (8.4)).
- A name has the form `<chapter prefix>.<topic>`, in lower case, with words separated by `-`.
- To find a section, search for its heading: `rg -n '\[effect.handler\]' spec`, or `scripts/spec.py show effect.handler`.
- When a section is split, one of the parts keeps the old name and the others get new names. When a section is removed, record its name in `history.md` and never reuse the name for something else.

## Normative and informative text

- All text is normative unless it says otherwise. Informative text starts its section or paragraph with "Informative."
- "Is a compile error" means the program is rejected at compile time. Abnormal termination at run time is written "is a defect".
- Requirement keywords are used as described in [`glossary.md`](glossary.md). Statements that describe meaning are in the present tense and are normative without a keyword. Choices left to an implementation are marked "implementation-defined" (`[target.unspecified]`).
- Code examples use the extension `.emel` and match the output of `emela fmt`.

## The specification and the implementation

- The specification covers what users, hosts and tools can observe: syntax, the rules for types and effects, results and their order, diagnostic codes and messages, command input and output, the files a command writes, the host boundary, formatter output, and language server responses.
- Decisions inside the implementation (the shape of an IR, data layout, algorithms, internal names) are documented in the code that makes them, as rustdoc (`//!` and `///`) or comments in English. Refer to sections of the specification by name there.
- When the implementation decides something observable that the specification does not cover, it MUST NOT stay only in the code. Write a proposal, or, if the answer is already fixed by the rest of the specification, add it to the specification directly.

## Changing the specification

- A change that alters meaning (syntax, typing rules, semantics, adding or removing diagnostics, the commitments of the standard library, observable behavior of targets and tools) is made through a proposal: a draft is refined locally and reviewed in a pull request that adds it to `proposals/`; merging the pull request adopts it, and the specification changes are applied to `spec/`. The process is in `proposals/README.md`.
- A change that does not alter meaning (typos, wording, added examples, reordering sections, resolving an inconsistency whose answer is fixed by the rest of the specification) can be made directly without a proposal. It is not recorded in `history.md`.
- A change made through a proposal is recorded in `history.md` with the proposal's id and the names of the sections it changed.
- Run `scripts/spec.py check` after any change.
