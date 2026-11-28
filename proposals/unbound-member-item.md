---
title: Handler members can be opened by items without a name and on the right of `by`
authors: [somnicattus]
status: accepted
issue: 874
touches: [grammar.expr, module.resolution, effect.decl, effect.use, effect.with, effect.label, type.rules, dyn.with, lsp.hover, diag.list]
requires: []
related: [handler-member, handler-member-gaps, handler-init-arguments, named-item-not-by-name]
pr: 895
decided: 2026-11-29
---

# Handler members can be opened by items without a name and on the right of `by`

## Summary

- An item of a with that opens a handler member (`Files.Open(p)`, `fs.Open(p)`, `Audit.Open(p)`) can be written without a name. It opens the handler as the named form does, and provides by name in the body the effects after the colon of the declaration of the handler member (ReadFile for `handler Open(path: String): ReadFile`). E4105 no longer applies to it.
- After `named-item-not-by-name` (#863), whether an item of a with provides its effect by name depends only on whether the item has a name, except that an item opening a handler member must have one. This proposal removes the exception: every item without a name provides its effects by name, and every named item makes a capability and provides nothing by name.
- The right side of `by` can open a handler member (`Source by Files.Open(p)`, `Source by fs.Open(p)`, and `x = Source by Files.Open(p)`), as it can open a handler (`Source by HostFile(path: p)`). The handler opened is provided under the label on the left (Source), as a handler H is in `L by H`. In the grammar, the right side of `by` becomes a supply, the same as the left side of an item.
- The other cases of E4105 (a handler member as an item of a layer, a bound layer, a bound label item that makes no handler) are unchanged. A handler member as an item of a layer is left to a change of layers (Alternatives).

## Motivation

### Whether a resource is provided by name depends on how it is opened

A handler whose init takes arguments (`handler-init-arguments`) can stand for one resource: `HostFile(path: ...)` below is one file, and the item `HostFile(path: "in.txt")` without a name provides ReadFile by name. An item that opens a handler member of the same effect (a handler member item) cannot be written without a name:

```emel
effect ReadFile {
  fn read() -> String
}

effect Files {
  handler Open(path: String): ReadFile
}

handler HostFile(path: String): ReadFile {
  init(path: String) { Self(path:) }

  fn read() { self.path }
}

handler MemFiles: Files {
  handler Open(path: String): ReadFile {
    init(path: String) { Self(path:) }

    fn read() { self.path }
  }
}

fn first_line() -> String uses { ReadFile } {
  f <- ReadFile
  f.read()
}

fn by_handler() -> String {
  with HostFile(path: "in.txt") { first_line() } # compiles
}

fn by_member() -> String {
  with MemFiles {
    with Files.Open("in.txt") { first_line() } # E4105
  }
}
```

(Checked at `5ad03e79`.) `by_handler` and `by_member` call `first_line`, which uses ReadFile by name. `with Files.Open("in.txt")` is E4105, and `with f = Files.Open("in.txt")` provides nothing by name. Whether ReadFile reaches `first_line` by name is decided by whether an item of a handler or a handler member item opens the handler, not by the effect.

### A handler can be plugged into a label, and a handler member cannot

A label tells apart two roles of the same effect (`[effect.label]`). The right side of `by` can open a handler, and cannot open a handler member:

```emel
effect Source of ReadFile

fn copy_source() -> String uses { Source } {
  f <- Source
  f.read()
}

fn by_handler_label() -> String {
  with Source by HostFile(path: "in.txt") { copy_source() } # compiles
}

fn by_member_label() -> String {
  with MemFiles {
    with Source by Files.Open("in.txt") { copy_source() } # E2106
  }
}

fn by_capability_label(fs: use Files) -> String {
  with Source by fs.Open("in.txt") { copy_source() } # E1101
}
```

(Checked at `5ad03e79`, with the declarations of the example above.) `Source by Files.Open("in.txt")` is E2106 (``expected handler or effect, found handler member `Open` ``), and `Source by fs.Open("in.txt")` does not parse (E1101), since `by_right` in the grammar takes a type_ref, not `lower_name "." type_name`. `x = Source by Files.Open("in.txt")` is E2106 as well. As with providing by name, whether a resource can be provided under a label is decided by how it is opened.

### The reasons `handler-member` gave

`handler-member` (#624) gave the principle "Resources are bound and passed; only services are looked up by name", and rejected "Looking resources up by name and telling them apart by labels (`with Source by Files.Open(...)`)" for two reasons: "a label has to be declared at the top level for each role", and "With both this and the bound form, libraries would mix `copy() uses { ReadFile }` and `copy(src: use ReadFile)`."

- The first reason is a cost of labels, not of handler members. A role told apart by a label needs a label declared at the top level whether a handler or a handler member opens the resource, and `with Source by HostFile(path: "in.txt")` above already pays it. Plugging a handler member into a label does not make labels necessary: the named form remains, and a with that does not need a role writes the item without a label.
- The mix that the second reason names exists without handler members. `first_line` above is `uses { ReadFile }`, and `with HostFile(path: ...)` serves it. For every effect, a library chooses between a parameter of type `use ReadFile` and `uses { ReadFile }`. Needing a name does not prevent the mix, and only keeps a handler member item from serving the functions that are `uses { ReadFile }`.
- Two resources of the same effect are told apart by binding them or by labels, whether an item of a handler or a handler member item opens them. Without names, a later item of the same with takes precedence for the same effect, and an inner with over an outer one: for items of a handler now, and for handler member items under this proposal.

### The help of E4301 leads to E4105

When a with between the place where E4301 is reported and the origin of the missing effect has a named item of that effect, E4301 adds a note at the item and the help ``= help: remove `tasks =` to provide it by name, or call through `tasks` ``, as `named-item-not-by-name` (#863) specified. #888 implemented it for an item of a handler; at `5ad03e79`, E4301 for `with f = Files.Open("in.txt") { first_line() }` in main has the help ``= help: provide it with `with` `` and no note at the item. For that with, removing `f =` as the help of `named-item-not-by-name` says gives `with Files.Open("in.txt") { first_line() }`, which is E4105 under the current rule. Under this proposal, removing the name of any named item makes the item provide its effect by name.

## Proposal

### Syntax

`with_item = [ lower_name "=" ] supply [ "by" by_right ]` already accepts `Files.Open(p)` and `fs.Open(p)` without a name; the name was required by a rule outside the grammar (E4105).

The right side of `by` becomes a supply (`[grammar.expr]`):

```text
by_right = supply
```

`supply` is the left side of an item: `type_ref [ "." type_name ] [ "[" type { "," type } "]" ] [ "(" [ args ] ")" ]`, or `lower_name "." type_name "(" [ args ] ")"` for a handler member of a capability. Before the change, `by_right` was `type_ref [ "[" type { "," type } "]" ] [ "(" [ args ] ")" ]`, which reads `Files.Open(p)` as a type_ref (and the resolution rejects it, E2106) and does not read `fs.Open(p)`.

```emel
fn by_member() -> String {
  with MemFiles {
    with Files.Open("in.txt") { first_line() }
  }
}

fn by_member_label() -> String {
  with MemFiles {
    with Source by Files.Open("in.txt") { copy_source() }
  }
}

fn by_capability_label(fs: use Files) -> String {
  with Source by fs.Open("in.txt") { copy_source() }
}
```

### Meaning

- An item `E.M(arguments)` without a name opens M of the innermost handler that provides E by name at that position, as `x = E.M(arguments)` does. An item `c.M(arguments)`, where c is a capability of E, opens M of the handler that provides c. An item `L.M(arguments)`, where L is a label of E, opens M of the innermost handler that provides L by name.
- The item provides by name, in the body, effects(M): the effects after the colon of the declaration of M (ReadFile for `handler Open(path: String): ReadFile`, and both Query and Execute for `handler Lock(path: String): Query + Execute`). The rules for an item without a name apply to the item as to an item of a handler:
  - It can meet the requirements of the other items of the same with on effects(M). It requires E, c or L, as the named form does, and is ordered with the other items by that requirement (`[dyn.with]`).
  - A later item without a name of the same with that provides the same effect takes precedence. A handler member item that a later item overrides is not opened: the init of M does not run, and the E of the init is not in the E of the with. The arguments of the item are not evaluated, and their E and R are not in the E and R of the with.
  - The labels of effects(M) declared `default` are provided with effects(M) (`[effect.label]`).
  - For each effect of effects(M), the with counts the name of the effect in types as its own capability (`[effect.escape]`), as for an item of a handler.
- A named handler member item (`x = E.M(arguments)`) is unchanged: it makes the capability x and provides nothing by name.
- `L by F.M(arguments)` and `L by c.M(arguments)`, where L is a label, F an effect or a label that has the handler member M, and c a capability, open M as the item `F.M(arguments)` or `c.M(arguments)` does, and plug the handler opened into L (`[effect.label]`). The rules of `L by H` apply:
  - The item provides {L} and requires {F} or {c}. It does not provide effects(M) by name, nor the labels of effects(M) declared `default`.
  - The handler opened serves L through the effect of effects(M) that is the underlying effect of L. If effects(M) does not contain the underlying effect of L, it is E4203. In `Source by Files.Open(p)`, effects(M) is ReadFile, the underlying effect of Source.
  - The arguments are treated as for the item: evaluated outside the with in the order the items are written, with their E and R in the E and R of the with.
  - E4204 (`by` written twice for the same label), the precedence over `default` labels, and W4201 (a label item that is never used) apply as for `L by H`.
  - `x = L by F.M(arguments)` is a named item, as `x = L by H` is: x is a capability of L, and the item provides nothing by name.
- The name of a built-in handler that has the name of its effect (`Semaphore`) refers to the handler in an item of a with and on the right of `by`, except as the E of `E.M(...)`, where it refers to the effect, since only an effect has handler members (`[module.resolution]`). The text listed `E.M(...)` among the positions of the effect without that exception, so it contradicted itself for an item of a with; the implementation reads the name as the effect there (`with p = Semaphore.Permits(n: 1)` compiles at `5ad03e79`). This proposal fixes the text along the way, since the right of `by` now takes `E.M(...)` too. On the right of `by`, no such item compiles: the handler members of the built-in effects open built-in effects (Permit), which cannot be the underlying effect of a label (E4201).
- The hover of a value whose type records capabilities names the handler of an item without a name (`capabilities: PrimaryDb`, `[lsp.hover]`). The inner handler of a handler member cannot be named outside its outer handler, so for a handler member item the hover names the handler member as written without its arguments (`capabilities: Files.Open`, `capabilities: fs.Open`). For `L by F.M(arguments)`, the hover names the right side of `by` in the same way, as it names H for `L by H`.

The named form remains the one to write when the body passes the resource to functions as a capability, or when it opens two resources of the same effect.

### Typing rules

Two rows are added to the table of `[type.rules]`, next to the rows of the named form:

| Expression | T | E | R |
|---|---|---|---|
| `with E.M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | (Re ∖ effects(M)) ∪ { E } ∪ Ra |
| `with c.M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | (Re ∖ effects(M)) ∪ { c } ∪ Ra |

In a with of several items, an item `E.M(a)` or `c.M(a)` without a name adds effects(M) to provides(H) and { E } or { c } to requires(H) in the row of `with H { e }`. As for every item without a name, effects(M) is not removed from opaque effect sets (`[effect.with]`). An item `L by E.M(a)` or `L by c.M(a)` without a name adds { L } to provides(H) and { E } or { c } to requires(H).

## Diagnostics

- [ ] E4104: a handler member with parameters written without arguments as an item of a with now covers `E.M` without a name and `L by E.M`, as well as `x = E.M`. The message and the help do not change.
- [ ] E2106 and E1101: no longer reported for a handler member on the right of `by` (`L by Files.Open(p)` and `L by fs.Open(p)`). Neither changes otherwise.
- [ ] E4203: covers `L by F.M(arguments)` whose effects(M) does not contain the underlying effect of L, with the message ``label `Source` stands for `ReadFile`, but `Files.Open` provides `Other` `` (the handler member as written without its arguments in place of the handler).
- [ ] E4105: the case of a handler member written as an item of a with without `x =` is removed, with its message ``handler member `Open` must be bound to a name in `with` `` and its help ``= help: bind it to a name: `x = Files.Open(...)` ``. The other cases (a handler member as an item of a layer, a layer bound to a name, a label item that makes no handler bound to a name) are unchanged.

No diagnostic is added. As for `with H` where H is a handler, no warning is given when the body does not use the effects of a handler member item: opening the handler runs its init and its release, which can be the purpose of the item (permits held while the body runs). `L by F.M(arguments)` is a label item, and W4201 applies to it when L is never used, as to `L by H`.

## Compatibility

The change only accepts items that were E4105, E2106 or E1101, and the right side of `by` reads every form it read before in the same way, so every program that compiles before the change compiles after it, with the same meaning. The change is an addition and can land in a patch release (`docs/release.md`).

## Implementation

- emela-syntax: `with_expr` in `crates/emela-syntax/src/parser/grammar/expressions.rs` reads the right side of `by` as the left side of an item: a SUPPLY, or a SUPPLY with a CAP_MEMBER for `c.M(...)` (`cap_member_supply`).
- emela-resolve: `with_item` in `crates/emela-resolve/src/resolve/lower/exprs.rs` resolves the right side of `by` as it resolves the left side of an item, so that it can name a handler member (now E2106 through `Expected::HandlerOrEffect`) or a capability, and `WithItem` in `crates/emela-resolve/src/hir.rs` records the member and the capability of the right side.
- emela-types
  - `infer_member_item` in `crates/emela-types/src/body/with.rs` stops reporting E4105 for an item without `x =`, and `DiagnosticKind::UnboundMemberItem` is removed (`crates/emela-types/src/diagnostic.rs`).
  - The providers of a with (`WithBind` in `crates/emela-types/src/decl/layer.rs`) record an item that opens a handler member with `named: false`, and with only the first of effects(M), since a named item needs only the type of its capability (`crates/emela-types/src/body/effects.rs`). Recorded with `named: true`, once for each effect of effects(M), an item without a name takes part in the precedence among the items without a name and can meet the requirements of the other items; effects(M) is removed from the R of the body, and the labels of effects(M) declared `default` are provided.
  - `L by F.M(...)` and `L by c.M(...)` record L with a member supplier, check that effects(M) contains the underlying effect of L (E4203), and require F or c.
- emela-core: `lower/with.rs` installs the handler opened by a handler member item without a name for effects(M), as for the handler of an item `H` without a name, so that the calls by name in the body and the inits of later items reach it. For `L by F.M(...)`, it installs the handler opened for L, as for `L by H`.
- tree-sitter: `with_item` in `editors/tree-sitter-emela/grammar.js` takes a supply or a member supply as the provider after `by`.
- emela-lsp: the `capabilities: ...` line of the hover names a handler member item without a name, and the right side of `L by F.M(...)`, as `Files.Open` or `fs.Open`.
- emela-driver: the rendering of E4105 for an item without `x =` (`crates/emela-driver/src/diagnostic.rs`) and its case in `crates/emela-driver/tests/check/diagnostics/effects.rs` are removed.
- Tests: `crates/emela-types/tests/handlers/member.rs` checks the effects removed from R, the precedence (a later item overrides a member item and the member item overrides an earlier one), the requirement of another item met by a member item, a label declared `default`, a member of several effects, and `L by F.M(...)` (provides only L, E4203, E4204, W4201, the named form). A program in `crates/emela-codegen-c/tests/programs/` runs a handler member item without a name on every target, with `c.M(...)`, `L.M(...)` and `L by F.M(...)` as well.

## Alternatives

- Keeping E4105 for an item without a name. Motivation shows what it costs and answers the reasons that `handler-member` gave.
- An item without a name that opens a handler member and provides nothing by name (only holding the handler open). An item without a name then has two meanings, depending on what its right side names. A with that opens a handler only to hold it open while the body runs can write the item without a name (`with s.Permits(n:) { f() }`), or with a name that the body does not use (`with _held = s.Permits(n:) { f() }`, as `with_permits` in std does now).
- Not adding `L by F.M(...)`, and providing the handler under a label with a second item: in `with Files.Open(p), Source by ReadFile { ... }`, `Source by ReadFile` provides under Source the handler that `Files.Open(p)` opens (`[effect.label]`). The body then also has ReadFile by name, which overrides the ReadFile of the outer withs in the body, and a with that wants only the role Source cannot say so. The named form has no such workaround, since `x = Source by ReadFile` makes no handler and cannot be bound (E4105). `Source by HostFile(path: p)` needs none.
- Allowing a handler member as an item of a layer (`layer App { MemFiles, Files.Open("app.conf") }`). The reason that `handler-member` gave for E4105 there is "a layer has no binding", which assumes that a handler member item has a name; under this proposal, it does not have to. This proposal still keeps E4105 for a layer and leaves the case to a change of layers (the issue #904, which builds on this proposal). A layer takes no parameters, so a handler member item of a layer could only be `E.M(...)` with arguments fixed in the declaration of the layer (`[effect.layer]`); `c.M(...)` needs a capability, which a layer cannot receive. #904 asks that a layer take parameters, from which such an item could take its arguments and its capability.

## Prior art

- Effect handlers found by name at each use: a handler installed for a scope handles the operations of its effect performed inside the scope, and the innermost handler of the effect is chosen (Eff, Koka). Emela's items without a name are of this kind.
- Handlers referred to by a value: capability passing, where a handler is a value passed to the code that uses it (Effekt, "Effects as Capabilities", OOPSLA 2020), and named handlers (Koka, "First-class names for effect handlers", OOPSLA 2022, cited by `handler-member`). Emela's named items are of this kind.
- Emela has both. With this proposal, whether an item of a with is found by name or referred to by a value is decided by whether it has a name, for every item that can be bound.

## Open questions

## Decision

Whether a resource was provided by name, or could be plugged into a label, depended on whether a handler or a handler member opened it, not on its effect or on the with. After `named-item-not-by-name`, whether an item has a name decides how its effect is reached, and this removes the one exception: an item without a name that opens a handler member provides the effects of the member by name, and the right side of `by` takes a handler member as it takes a handler. The reasons `handler-member` gave for requiring a name are costs of labels and a mix of `uses { E }` and `use E` parameters that exists without handler members, so they do not hold against this. A handler member as an item of a layer is left to #904, which lets a layer take parameters. A convention for the standard library on taking resources opened from handler members as capability parameters is left to a separate proposal.
