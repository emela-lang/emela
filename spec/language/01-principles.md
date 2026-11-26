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
| Foundation level | People who write libraries and runtime boundaries | `effect` `handler` `layer` `suspend` `trait` `impl`, `fails` clauses and `uses` clauses, constraints, `@external` |

If writing code at the everyday level requires knowledge of the foundation level, that is treated as a design flaw.

## 1.3 Fixed premises `[intro.premises]`

- Continuations are one-shot only. A suspended computation can be resumed at most once. This is a permanent premise and does not change in the future.
- Errors are not effects. They are handled in a separate channel.
- Expected errors (which appear in types) are distinct from defects (which do not appear in types).
- Variables are immutable. There is no loop syntax.
- There are no higher-kinded types, null, implicit type conversions, or operator overloading. The only subtyping is the containment of E and R (`[type.rules]`).

## 1.4 How to read this document `[intro.reading]`

- Section names and the file layout are defined in `README.md`; terms and requirement keywords in `glossary.md`.
- Points that are not yet agreed on are collected in `open-questions.md`.
- An item marked "Experimental (`name`)." applies only when that feature is enabled (`[intro.experimental]`).

## 1.5 Experimental features `[intro.experimental]`

An experimental feature is a part of the specification that is not yet stable. A Pome enables it in the `[experimental]` table of its `Pome.toml` (`[module.pome]`), and a command enables it for one run with a flag `--experimental-<name>` (`[cmd.build]`).

- The name of a feature is lower-case English words joined by `-`.
- The rules of an experimental feature are written where they belong. Each item that belongs to one starts with "Experimental (`name`)." Within the section of a feature, an item without the mark applies always, and an item with the mark applies only when the feature is enabled.
- Experimental features are outside the compatibility guarantees of this document. Any version MAY change or remove any of them. A Pome that enables one declares only the minimum compiler version in `[pome].emela`.
- When an experimental feature becomes stable, its rules lose the mark, and its name is accepted in `[experimental]` and as a flag for at least one minor version, with a warning (W2304) that it has no effect. The name of a removed feature is accepted with the same warning.
- Diagnostics produced by an experimental feature carry the note ``= note: experimental feature `name` ``.

| Name | Since | What it changes |
|---|---|---|

Each proposal that adds an experimental feature adds a row.
