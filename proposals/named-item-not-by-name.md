---
title: A named item does not provide its effect by name
authors: [somnicattus]
status: draft
issue:
touches: [effect.use, effect.with, effect.handler, effect.label, type.rules, task.group, dyn.with, diag.list]
requires: []
related: [handler-member, capability-identity, effect-alias, task]
---

# A named item does not provide its effect by name

## Summary

- An item of a with bound to a name (`with x = H`, `with x = L by H`) makes the capability x and nothing else. A named item no longer provides its effect by name. Only an item without a name provides its effect by name.
- An item that opens a handler member already provides nothing by name (`[effect.with]`). With this proposal, the same rule holds for every item: a named item makes a capability and provides nothing by name, and an item without a name provides its effect by name.
- A label item that makes no handler (`x = L`, and `x = L by P` where P is an effect) can no longer be bound (E0447). Under this proposal it would provide nothing and only fix x to a handler that it does not make. Two named items of the same label in one with are no longer E0432.
- A with that binds a capability and leaves the uses of the effect by name to the outer withs can then be written as one item (`with replica = ReplicaDb`). Under the current rule, the same program needs a label declared for that purpose and two withs.
- No form is added for a named item that also provides its effect by name. In std, examples and the tests, only tests of the current rule use a named item that way, and such a program is rewritten by removing the name and declaring an effect alias in the body (Compatibility).
- The change lands in 0.29, with `capability-identity` (#732), so that the rules for capabilities change in one version. No warning is given ahead of it.

## Motivation

### The uses by name go to an inner named item, not to the outer handler

```emel
fn write_side() -> String uses { Db } {
  db <- Db
  db.name()
}

fn now_taken() -> String {
  with PrimaryDb {
    with replica = ReplicaDb {
      "#{replica.name()} #{write_side()}"
    }
  }
}
```

`now_taken()` is `"replica replica"` (checked at `ea45cbfb`). The writer of `now_taken` binds replica to call its operations directly, but `write_side`, which uses Db by name, also calls the operation of ReplicaDb. The source does not show that `write_side` calls ReplicaDb, and no diagnostic is given.

### Leaving the uses by name to the outer handler needs a label and two withs

The only way to bind a capability and keep the uses of Db by name on the outer handler is to provide the outer handler of Db under a label, and to provide that label as Db again with the last item of the inner with:

```emel
effect Saved of Db

fn compare(a: use Db, b: use Db) -> String { "#{a.name()} #{b.name()}" }

fn now_kept() -> String {
  with PrimaryDb {
    with Saved by Db {
      with a = ReplicaDb, b = ReplicaDb, Saved {
        "#{compare(a, b)} #{write_side()}"
      }
    }
  }
}
```

- `now_kept()` is `"replica replica primary"`. The label `Saved` is declared only to give the innermost with the handler of Db that the outermost with provides.
- The two inner withs cannot be merged into one: in `with Saved by Db, a = ReplicaDb, b = ReplicaDb, Saved`, the Db that the item `Saved by Db` requires is met by the item `Saved` of the same with, and the requirements form a cycle (E0411).
- In `with replica = Side by ReplicaDb` (a label item whose right side is a handler), the uses of Db by name go to PrimaryDb, but replica is a capability of `Side`, and passing replica where `use Db` is expected is E0452.
- An effect that is not a label cannot be written on the left of `by` (`Db by Db` is E0430).

### Two items of one effect

- In `with a = Num(n: 1), b = Num(n: 2)`, the operations of Tag called by name go to b, because the later item takes precedence. Swapping the two items changes which handler the uses by name go to, while a and b still refer to their own handlers.
- An overridden named item is still initialized, so that x can refer to its handler: its arguments are evaluated, the R of the arguments and the E of its init are in the E and R of the with, and the handler is released. `[effect.with]` used to say that an overridden handler is not initialized and that its arguments are not evaluated, with no exception for named items. The exception was added to `[effect.with]` without a proposal (#862), because the rule that a named item refers to its own handler determines it. Under this proposal a named item is never overridden, and `[effect.with]` no longer needs the exception.

### A named label item that makes no handler

```emel
effect Side of Db

fn show(side: use Side) -> String { side.name() }

fn fixed() -> String {
  with PrimaryDb {
    with side = Side by Db {
      with ReplicaDb {
        show(side)
      }
    }
  }
}
```

- `fixed()` is `"primary"` (checked at `ea45cbfb`). The item `side = Side by Db` runs no init and makes no handler: side refers to the handler of PrimaryDb, which the outer with provides, and keeps referring to it inside the with of ReplicaDb. `x = Side by Other`, where `Other` is another label of Db, and `x = Side`, which gives a capability of Db served by the provider of Side, behave the same way (checked at `ea45cbfb`).
- Under the current rule, these items also provide their effect by name. Under this proposal they would provide nothing by name, and would do nothing but fix x to a handler that an outer with provides. That is what `x = use E` did, and `effect-alias` (#731) replaced it with an effect alias, which finds the handler at each call (`[effect.use]`).
- `[effect.with]` says that each item of a with runs its init and makes a capability. A label item whose right side is an effect has no init to run.

### Two named items of one label

- `with a = Side by PrimaryDb, b = Side by ReplicaDb` is E0432 (checked at `ea45cbfb`), because the two items would provide Side by name. Two named items of one effect (`with a = Num(n: 1), b = Num(n: 2)`) are not an error. Under this proposal neither pair provides anything by name.

### Where the current rule came from

- `handler-member` (#624) allowed an item that is not a handler member to be bound, in addition to providing its effect by name: `with log = ConsoleLogger { ... }` "provides Logger by name as usual, and binds its capability to log as well". The same proposal made an item that opens a handler member provide nothing by name, on the principle that resources are bound and passed and only services are looked up by name.
- `capability-identity` (#732) gave names to the two (named item, provide by name) and kept the rule. Neither proposal compares the rule with one in which a named item only makes its capability.
- When `handler-member` was adopted, a named item was one of two ways to get a fixed capability; the other was `x = use E`. `effect-alias` (#731) deprecates `use E` written as an expression (W0403 in 0.29, an error from 0.30). A named item is then the only way to make a fixed capability of a handler, and under the current rule every such capability also provides its effect by name.

### What uses the current rule

The measurement below was made at `ea45cbfb` with a compiler changed so that a named item does not provide its effect by name. The measurement type checked `examples/*`, `tests/laws`, `crates/emela-codegen-c/tests/programs/*.emel`, `crates/emela-cli/tests/process/*.emel`, `crates/emela-driver/tests/*/*.emel` and the std that these programs use, and ran the tests of `emela-types` and `emela-core`.

- There are 137 named items that do not open a handler member (TaskGroup 65, Deferred 23, Semaphore 16, Queue 10, Spawn 8, others 15). Every named item of std (in `Task.all`, `race`, `timeout` and the others) is used only through its capability.
- The body of a with uses the effect of its named item by name in three places, and all three are tests of the current rule: `effect_alias_task.emel` (`s <- TaskGroup` inside `with outer = Task.Group`), and `binding_handlers` and `tasks_start_siblings` in `crates/emela-types/tests`. In `tasks_start_siblings`, `with tasks = Task.Group { spawn_tree(3) }` never uses tasks through its capability. In `handler_members.emel`, the body uses by name the effect of an overridden named item, but a later item without a name provides that effect by name, so `handler_members.emel` compiles and keeps its meaning.
- In 9 places in the programs type checked above, an outer with provides the same effect as a named item. In none of the 9 places does the body use that effect by name, so the meaning of these programs does not change.
- No named item meets a requirement of another item of the same with.
- The spec examples (`spec_examples.rs`) are not affected.

## Proposal

### Syntax

No change.

### Meaning

- An item `x = Item` binds x to the capability of the item and provides nothing by name. The effects that the body uses by name (through the R of a function the body calls, or through an effect alias `e <- E`) are provided by the withs that enclose this with, or by the caller.
- The rule holds for every item that can be bound: an item of a handler (`x = H`, `x = H(arguments)`), an item that opens a handler member (`x = E.M(...)`, `x = c.M(...)`, as now) and a label item plugged into a handler (`x = L by H`, `x = L by H(arguments)`). A named handler that implements several effects provides none of them by name.
- A named item does not meet the requirements of the other items of the same with. A requirement on its effect is met by an item without a name of the same with, or from outside the with. A handler that has to use the handler of a named item is listed in a with inside the with of the named item, and takes the capability as an argument of its init.
- A named item does not provide the labels of its effect declared `default` (`[effect.label]`). Those labels are provided together only by an item that provides the effect by name.
- Every rule that looks for the innermost handler of an effect looks only at the withs that provide it by name: an effect alias (`x.op(...)`), `(use E).M(...)` and `use E` (`[effect.use]`), an item `x = E.M(...)` (`[effect.with]`, `[dyn.with]`), an item `x = L.M(...)` and the provider of an item `L` or `L by P` where P is an effect or a label (`[effect.label]`), and `E.M(...)` inside an init (`[effect.handler]`). In `with r = ReplicaDb { with c = Db.Conn() { ... } }`, `Db.Conn()` opens the handler member of the handler that an outer with provides by name, not of ReplicaDb; `r.Conn()` opens that of ReplicaDb.
- Precedence applies only among the items without a name: a later item without a name takes precedence for the same effect, and the earlier item is not initialized. A named item is never overridden: it is always initialized, its arguments are always evaluated, and the E and R of its arguments and the E of its init are always in the E and R of the with.
- The type arguments of a named item are inferred from the arguments of its init and from how x is used, not from the R of the body (`[effect.with]`). E0459 for two or more effects of the body that unify with the effect of an item applies only to items without a name.
- A task of a group starts siblings through the capability of the group, which the R of the task MAY contain (`[task.group]`). A group without a name also provides TaskGroup by name, as now.
- A label item that makes no handler cannot be bound (E0447): `x = L`, and `x = L by P` where P is the underlying effect of L or another label of it. Written without a name, these items are unchanged. To use the label in the body, the body declares an effect alias (`with Side by Db { side <- Side; side.name() }`).
- W0401 (a label item that is never used) is not given for a named label item. It provides nothing by name, so the label never appears in the R of the body; it is used through x.
- E0432 (`by` written twice for the same label in one with) counts only the items without a name. `with a = Side by PrimaryDb, b = Side by ReplicaDb` makes two capabilities of Side, as two named items of one effect do.

```emel
fn taken_no_more() -> String {
  with PrimaryDb {
    with replica = ReplicaDb {
      "#{replica.name()} #{write_side()}" # "replica primary"
    }
  }
}

fn recursive() {
  with Task.Group { spawn_tree(3) } # spawn_tree uses TaskGroup by name; no name is bound
}
```

### Typing rules

- In the table of `[type.rules]`, `provides(H)` in the row of `with H { e }` is the effects provided by the items without a name. The capability of a named item is removed from the R of the body as now, as one of the capabilities of the items.

## Specification changes

The changes are applied to `spec/` in this pull request.

- `[effect.use]`: an effect alias, `(use E).M(...)` and `use E` find the innermost handler that provides the effect by name.
- `[effect.handler]`: `E.M(...)` inside an init opens the handler member of the innermost handler that provides E by name.
- `[effect.with]`: a named item provides nothing by name, whether it is an item of a handler, of a handler member or of a label plugged into a handler. A label item that makes no handler cannot be bound (E0447). Precedence, the exception for an overridden named item, the requirements met in the same with, the inference of type arguments from the R of the body, and E0459 for effects that unify are stated for the items without a name. `use X` and an item `x = E.M(...)` find the innermost handler that provides the effect by name.
- `[effect.label]`: the labels declared `default` are provided together only by an item without a name. E0432 counts only the items without a name, and W0401 is not given for a named label item. An item `x = L.M(...)` opens M of the innermost handler that provides L by name, and an item `L` or `L by P` where P is an effect or a label provides again the handler that an outer with provides by name, or that of an item without a name initialized earlier in the same with.
- `[type.rules]`: `provides(H)` in the row of `with H { e }` is the effects of the items without a name.
- `[task.group]`: TaskGroup by name comes from a group without a name; a task of a named group starts siblings through the capability of the group.
- `[dyn.with]`: an item `x = E.M(...)` runs the init of M of the innermost handler that provides E by name, and the effects used in the operation bodies of a handler are provided by the handlers outside the with or by the handlers of the items without a name initialized earlier in the same with.
- `[diag.list]`: the note and the help of E0402, the third help of W0403, the help of E0453, the help of E0454 for an effect, the cases of E0432 and W0401, and the new case, message and help of E0447.

## Diagnostics

- [ ] E0402: when a with between the place where E0402 is reported and the origin of the effect (searched for as for the help about `Platform`) has a named item of that effect, a note at the named item says ``= note: `tasks` is a named item, so it does not provide `TaskGroup` by name``, and the help is ``= help: remove `tasks =` to provide it by name, or call through `tasks` ``. No new code. The search stays inside the function where E0402 is reported, so a named item inside a function whose R is inferred (a function without a uses clause) gets no note when E0402 is reported at a caller of that function.
- [ ] E0447: a label item that makes no handler bound to a name (`x = L`, `x = L by P` where P is an effect). The message is ``label item `Side by Db` cannot be bound to a name: it makes no handler``, and the help is ``= help: list it without a name, and write `side <- Side` in the body``, with the name of the item and the effect that the item provides when it has no name (L for `L by P`, the underlying effect for `L`). No new code: E0447 already covers the items that cannot be bound (a layer).
- [ ] E0432: not given when one of the two items is named.
- [ ] W0401: not given for a named label item.
- [ ] W0403: the third help (``= help: write `x <- Logger`, or a named item `with x = H` ``, given for `x = use E` when the second help does not apply: x is used other than as the receiver of operations, or inside a later with) is left out when the body of the with that provides E also uses E by name. Rewriting that with into a named item would send the uses by name to an outer handler, and no form both binds a capability and provides its effect by name. The other two helps are unchanged.
- [ ] E0453: the help ``= help: for a fixed value, use a named item `with x = H` `` is left out, for the same reason, when the body of the with that provides E also uses E by name.
- [ ] E0454: for an effect E, the help ``= help: capture the capability of a named item (`with g = H { ... }`) and call its operations`` is left out, for the same reason, when the body of the with that provides E also uses E by name outside the callback. The uses inside the callback are the ones that the help rewrites into calls through g.

## Compatibility

- This proposal is made in 0.29, the version of `capability-identity` (#732), so that the rules for capabilities change in one version. 0.29 is a minor version, which may change the meaning of programs (`docs/release.md`). No warning is given ahead of the change: 0.28 has been released, and the measurement found no program affected outside the tests of the current rule.
- A program whose body uses by name an effect that only a named item of an enclosing with provides stops compiling (E0402, with the note above). The program is rewritten by removing the name when the capability is not used (`with Task.Group { spawn_tree(3) }`), or by calling the operations through the capability.
- A program whose body uses by name an effect that a named item provides, and that an outer with or an item without a name of the same with also provides, changes meaning without a diagnostic: the uses by name go to the handler of that outer with or of that item without a name. The typical case is a test that overrides a handler of a layer with a named item and calls the named item to control the fake handler: in `with AppLive, clock = FixedClock { run_jobs() }`, `run_jobs` now calls the Clock of `AppLive`. To keep its meaning, the test declares an effect for the operations that control the fake handler, implements that effect in the same handler, and lists the handler without a name: with `handler FixedClock(t: Int): Clock + ClockControl`, the test is `with AppLive, FixedClock { ctl <- ClockControl; ctl.advance(60); run_jobs() }` (checked at `ea45cbfb`). When the body calls only operations of the effect through x, and no with inside the body provides the effect again, removing the name and declaring an effect alias keeps the meaning without a new effect: `with AppLive, FixedClock { clock <- Clock; ...; run_jobs() }`. The measurement found no such program.
- A requirement of a handler on the effect of a named item of the same with is no longer met by the named item. The measurement found no such requirement.
- A named label item that makes no handler (`x = L`, `x = L by P` where P is an effect) stops compiling (E0447). It is rewritten by removing the name and declaring an effect alias in the body. The alias finds the handler at each call, so it differs from the bound x only when a with inside the body provides the effect of the alias again: L for `L by P`, and the underlying effect for `L` (in `with Side { db <- Db; with ReplicaDb { db.name() } }`, db calls ReplicaDb). An effect alias cannot be an argument (E0453), so a function that took x as a parameter `use L` declares `uses { L }` instead. No program in std, examples and the tests binds such an item: the only named label item is `s = Source by Plug(n: 1)` in a test of the parser (`crates/emela-syntax/test_data/parser/ok/handler_member.emel`), whose right side is a handler.
- Two named items of the same label in one with are no longer E0432. This only accepts programs that were errors.
- E0459 for several effects that fit, and for an item whose type arguments decide whether it overrides another item, is no longer given for a named item. The type arguments of a named item are determined by the arguments of its init and by how x is used, or are reported as not determined (E0459).
- The tests to update are `binding_handlers`, `tasks_start_siblings` and `effect_alias_task.emel`, and four snapshots of `emela-core` whose evidence no longer provides the effect of a named item (`lower::handlers_of_several_effects`, `lower::semaphore_and_queue`, `lower::task_group_start_and_attempt`, `mono::tasks`).

## Implementation

- `keep_last` in `crates/emela-types/src/layer.rs`: a bound item that is not from a layer gets `named: false` whether or not a later item provides the same effect (the field documents "whether it provides its effect by name"). The rest of `with_effects` (the effects removed from the R of the body, the inference of type arguments from the R of the body, the order of initialization) and the lowering already follow `named`, except the two checks of E0459 below. The measurement above was made with the change of `keep_last` alone.
- E0402: the note and the help. The with is searched for between the place of E0402 and the origin of the effect, as `under_platform` in `crates/emela-types/src/body/errors.rs` does for `Platform`.
- E0459: `check_several_fits` and `check_item_overrides` in `crates/emela-types/src/body/items.rs` do not look at `named` now. Both skip named items, and `check_item_overrides` compares an item only with the items without a name.
- W0403, E0453 and E0454: leave out the help that suggests a named item when the body of the with that provides E also uses E by name (for E0454, outside the callback).
- The lookups of the innermost handler for an effect alias, `(use E).M(...)`, `use E`, `x = E.M(...)`, `x = L.M(...)`, the provider of an item `L` or `L by P` where P is an effect or a label, and `E.M(...)` in an init skip named items. A test checks each of them.
- E0447 and E0432: `check_with_items` in `crates/emela-types/src/layer.rs` reports E0447 for a bound label item that has no `by` or whose `by` resolves to an effect (`Supplier::Effect` of `by_tag`), and leaves out the items with `bind` when it looks for a label given a provider twice.
- W0401: `unused_label_providers` in `crates/emela-types/src/body/effects.rs` skips the named items. Today it gives no W0401 for a named label item whose x is used, but only because the capability x in the R of the body makes it return early.
- The comment of `Entry::named`, and the tests and snapshots listed in Compatibility.

## Alternatives

- **Keep the current rule and add `E by E`, so that the uses by name go to the outer handler.** `L by P` with an effect P provides "the outer handler (or that of an item initialized earlier in the same with)" (`[effect.label]`), so `Db by Db` written after `replica = ReplicaDb` would provide ReplicaDb, and another rule would be needed to skip the items of the same with. The item `Db by Db` would also be written to stop another item from providing Db by name, while every other item of a with provides an effect. A program that omits `Db by Db` still has its uses by name go to the named item without a diagnostic.
- **Keep the current rule and reject an overridden named item whose effect the body uses by name.** The order of the items in one with then no longer decides where the uses by name go, but the program of the motivation, where the named item is in an inner with, is not affected.
- **Add, in this proposal, a form for a named item that also provides its effect by name.** One candidate is an item that provides a capability by name (`with clock = FixedClock, Clock by clock`). Under the current rule `Clock by clock` is E0430, and `Clock by clock` would put a capability, not a handler, in an item of a with. Another candidate is a mark on the item (`with FixedClock as clock`), used only for this case, so that the other `with x = ...` are not rewritten; #732 rejected `as` for every named item because every `with x = ...` would be rewritten. The measurement found no program that needs such a form outside the tests of the current rule. Adding a form later does not change the meaning of programs written under this proposal. This proposal adds no form: a program that needs one is rewritten by removing the name and declaring an effect alias (Compatibility), except when x is passed as an argument.
- **Keep `x = L` and `x = L by P` with an effect P, as a capability fixed to the handler that an outer with provides.** It would be the only fixed capability of a handler that the with does not make. `effect-alias` (#731) removed the same thing for effects (`x = use E`), and an effect alias covers the uses inside the body. Keeping it for labels only would also make the uses differ by whether the effect is a label.
- **Two items of the same handler, one with a name and one without** (`with FixedClock, clock = FixedClock`). The program compiles, but the two items make two handlers with separate state, so an operation called through clock does not change the handler that the body uses by name.

## Prior art

- Named handlers (Xie, Cong, Ikemoto, Leijen, "First-class names for effect handlers", OOPSLA 2022), also cited by `handler-member`: a handler instance is a value through which its operations are called, and a rank-2 type keeps the instance inside its scope. Koka has named handlers.

## Open questions

None.

## Decision
