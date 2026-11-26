---
title: Diagnostic codes in bands of 1000, and one code per kind of mistake
authors: [Alliana]
status: implemented
issue:
touches: [diag, diag.bands]
requires: []
related: []
pr: 837
decided: 2026-11-24
---

# Diagnostic codes in bands of 1000, and one code per kind of mistake

## Summary

The bands of diagnostic codes change from 100 codes per stage (E01xx to E09xx) to 1000 codes per stage (E1xxx to E9xxx), divided into subject bands of 100. A code names one kind of mistake: mistakes fixed in the same way share a code, and the message tells their forms apart. At the end of 0.29 the codes in `[diag.list]` are renumbered once into the new bands; codes of the same kind of mistake are merged, and retired codes are dropped.

## Motivation

A stage has only 100 codes, and E04xx is close to full.

| Band | 2026-10-21 | 2026-11-05 | 2026-11-24 | Left |
|---|---|---|---|---|
| E01xx | 46 | 49 | 53 | 36 (E0154 to E0189) |
| E02xx | 41 | 41 | 41 | 59 |
| E03xx | 34 | 36 | 42 | 58 |
| E04xx | 34 | 44 | 63 | 37 |

- E04xx grew by about 30 codes in five weeks. At that rate it is full in about six weeks.
- E04xx holds effects and handlers, and also effect labels, capabilities, `@export` and `@entry`, and the restrictions of the native target. The bands are divided by the stage that checks a program, and all of these are checked in the stage of types and effects.
- The reserved bands E05xx to E08xx have no subject, and only one code is in them.
- Ten codes are retired: E0116, E0327, E0408 to E0410, E0425, E0428, E0444, W0402 and E0905.
- Some kinds of mistake have one code per position or form. E0111, E0113, E0118 and E0126 all say that a type, a pattern, an expression or a declaration is missing where one is expected. There is no rule for how fine codes are, and newer diagnostics tend to be split finer.

Assigning subjects to the reserved bands would be enough for now. But when E04xx overflows into a second band, one subject is split between two distant bands. Renumbering once during 0.x keeps the table readable by subject.

## Proposal

### The form of a code

- An error code is still `E` followed by 4 digits, and a warning code `W` followed by 4 digits.
- The first digit names the stage, and the second the subject within the stage. The last two digits number the codes within a subject, from 01.
- The first digit of a code in the new bands is not 0, so the new codes and the codes of the earlier bands (`E0xxx`, `W0xxx`) never look alike.
- A code, once assigned, does not change, and the number of a retired code is not reused. This rule stays; the renumbering at the end of 0.29 is the one exception.

### Bands

| Band | Stage | Subjects |
|---|---|---|
| E1xxx | Lexing and parsing | E10xx lexical, E11xx syntax of expressions, patterns and types, E12xx syntax of declarations, E13xx syntax of attributes, E14xx forms that are no longer written |
| E2xxx | Modules, names, projects | E20xx files and modules, E21xx name resolution, E22xx definitions restricted to the bundled core or to tests, E23xx `Pome.toml` and dependencies |
| E3xxx | Types | E30xx types in general, E31xx calls and patterns, E32xx error types (E), E33xx traits, impls and derive |
| E4xxx | Effects | E40xx declarations of effects and handlers, E41xx with, layers and handler members, E42xx effect labels, E43xx R, Immediate and suspension, E44xx capabilities |
| E5xxx | External functions and targets | E50xx `@external`, E51xx `@export` and `@entry`, E52xx restrictions of a build target |
| E6xxx to E8xxx | Reserved | |
| E9xxx | Tools | E90xx input and output, E91xx external tools (the JS host, cc, wasmtime) |

Warnings use the same bands (W21xx are warnings of name resolution). When the codes of a subject reach 99, the subject continues in an unassigned subject band of the same stage.

### How fine a code is

- A code names one kind of mistake. Mistakes fixed in the same way have the same code even when they are written in different positions or forms, and the message tells the forms apart.
- Mistakes fixed in different ways, or for which the language server offers different code actions (`[lsp]`), have different codes.

### Codes merged by the renumbering

| Merged code | Earlier codes |
|---|---|
| A type, a pattern, an expression or a declaration is expected | E0111, E0113, E0118, E0126 |
| `_` or `..` outside a pattern | E0123, E0124 |
| The position of `..` in a list | E0114, E0115 |
| Malformed arguments of an attribute | E0138, E0141, E0146 |
| A file name or a directory name does not form a module name | E0201, E0202 |
| An opaque type constructed, matched or accessed outside its defining module | E0215, E0216, E0315 |
| The wrong number of type arguments, arguments, fields of a constructor pattern, or parameters of an fn in an impl or an operation of a handler | E0304, E0305, E0309, E0332, E0406 |
| Arguments of an init missing or not expected | E0434, E0435 |
| The E of a body or of a function value is not included | E0318, E0321 |
| The R of a body or of a function value is not included | E0402, E0403 |
| An effect or a name repeated after the `:` of a handler | E0450, E0451 |
| Type parameters where they cannot be written | E0426, E0460, and E0150 for type parameters |
| A handler for something that cannot be implemented by a handler | E0413, E0433 |
| An external tool is not found, cannot be started, or is too old | E0904, E0907, E0909 |

With the ten retired codes dropped, about 210 codes become about 175.

The row `E0153 and up | Syntax errors` of `[diag.list]` is dropped by the renumbering. A new syntax error gets its own code in E11xx or E12xx.

### The earlier codes

`[diag.list]` gets a column with the earlier code of each row, which stays until 1.0. A merged code lists every earlier code it replaces. Diagnostics are printed with the new code only.

## Diagnostics

- [x] `[diag]`: the first two digits name the stage and the subject; a code names one kind of mistake.
- [x] `[diag.bands]`: the new bands, and a note that `[diag.list]` stays in the earlier bands until the end of 0.29.
- [x] `[diag.list]`: renumbered into the new bands with a column of earlier codes, the merged rows and the retired rows dropped (implementation, end of 0.29).
- [x] The references to codes elsewhere in `spec/` (about 300) and the code actions of `[lsp]` (E0303, E0325, E0211, E0212, E0318, E0402, E0144, E0145, E0147, W0305) move to the new codes (implementation, end of 0.29).

## Compatibility

- No program changes meaning, and no program starts or stops compiling.
- Every code printed changes at the end of 0.29. Continuous integration, editor settings and language server clients that filter by code need the new codes.
- The map from earlier codes to new codes is many to one where codes are merged.

## Implementation

At the end of 0.29, after the proposals that are under review and add or retire codes (#834, #820, `pome-toml-diagnostics`) have landed:

- Make one table from earlier codes to new codes, and replace the codes with it in `crates/`, `spec/`, `editors/` and the documentation comments of `crates/emela-core/lib/std/`. The proposals in `proposals/` are records and keep the earlier codes.
- A merged code is one constant in the implementation, and the message tells the forms apart.
- Regenerate the snapshots, and check in review that only the code strings change.
- Update the descriptions of the bands in `crates/emela-driver/src/code.rs` and in the `DiagnosticCode` of emela-syntax.
- Remove the note about the earlier bands from `[diag.bands]`.

## Alternatives

- Assign subjects to the reserved bands E05xx to E08xx, and put only new codes there. Existing codes stay. The subjects of E04xx (labels, capabilities, external functions) are then split between an earlier band and a new one, and the table cannot be read by subject.
- Drop the bands and number codes in sequence. No band fills up, but a code no longer tells its stage or subject.
- Five digits (E00101). The new codes are told apart from the earlier ones, but the bands keep the same division, and the same overflow comes later.
- Merge codes without renumbering. About 15 codes in E04xx become free, which lasts three or four weeks at the current rate.

## Prior art

- Diagnostic codes numbered in one sequence, with no stage or subject in the number: Rust (E0001 and up). Retired codes keep their numbers.
- Diagnostic codes in bands of 1000 by broad category, numbered in sequence within a band: TypeScript (1xxx syntactic, 2xxx semantic).

## Open questions

## Decision

Adopted. A stage had only 100 codes, and E04xx, which holds effects, handlers, labels, capabilities and the restrictions of external functions and targets, would have been full within weeks. Overflowing into a reserved band would have split one subject between two distant bands, so the codes are renumbered once, during 0.x, into bands of 1000 per stage with subject bands of 100.

- The first digit of a new code is never 0, so earlier and new codes cannot be mistaken for each other, and `[diag.list]` keeps the earlier codes in a column until 1.0.
- A code names one kind of mistake. Codes that only told positions or forms apart are merged, and the message tells them apart instead; codes stay separate where the fix or the code action of the language server differs.
- Only the rules change now. The renumbering, the merges and the references in `spec/` and `[lsp]` are done at the end of 0.29, after the proposals that add or retire codes have landed, so that none of them has to be redone.
