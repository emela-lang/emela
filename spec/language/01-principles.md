# 1. Scope and design principles `[intro]`

Emela is a functional language with strict effect types. It targets JavaScript, WebAssembly and C (chapter 16), and this specification requires that the same program returns the same result on all of them.

## 1.1 Three pillars `[intro.pillars]`

Every language feature belongs to one of the following three. A feature that belongs to none of them is not added.

- Defining data: `type`, `enum`, `error`
- Functions that operate on data: `fn`, `match`, `|>`
- Making side effects explicit: `effect`, `use`, `with`

## 1.2 Two levels `[intro.layers]`

| Level | Who writes it | What it contains |
|---|---|---|
| Everyday level | People who write applications | `type` `enum` `error` `fn` `match` `use` `with` `fail` `escape` |
| Foundation level | People who write libraries and runtime boundaries | `effect` `handler` `layer` `suspend` `trait` `impl`, `fails` clauses and `use` clauses, constraints, `@external` |

If writing code at the everyday level requires knowledge of the foundation level, that is treated as a design flaw.

## 1.3 Fixed premises `[intro.premises]`

- Continuations are one-shot only. A suspended computation can be resumed at most once. This is a permanent premise and does not change in the future.
- Errors are not effects. They are handled in a separate channel.
- Expected errors (which appear in types) are distinct from defects (which do not appear in types).
- Variables are immutable. There is no loop syntax.
- There are no higher-kinded types, subtyping, null, implicit type conversions, or operator overloading.

## 1.4 How to read this document `[intro.reading]`

- Section names and the file layout are defined in `README.md`; terms and requirement keywords in `glossary.md`.
- Points that are not yet agreed on are collected in `open-questions.md`.
