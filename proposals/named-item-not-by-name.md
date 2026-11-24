---
title: A named item does not provide its effect by name
authors: [somnicattus]
status: draft
issue:
touches: [effect.with, effect.label, type.rules, task.group, diag.list]
requires: []
related: [handler-member, capability-identity, effect-alias, task]
---

# A named item does not provide its effect by name

## Summary

- An item of a with bound to a name (`with x = H`, `with x = L by P`) makes the capability x and nothing else. A named item no longer provides its effect by name. Only an item without a name provides its effect by name.
- An item that opens a handler member already provides nothing by name (`[effect.with]`). With this proposal, the same rule holds for every item: a named item makes a capability and provides nothing by name, and an item without a name provides its effect by name.
- A with that binds a capability and leaves the uses of the effect by name to the outer withs can then be written as one item (`with replica = ReplicaDb`). Under the current rule, the same program needs a label declared for that purpose and two withs.
- No form is added for a named item that also provides its effect by name. In std, examples and the tests, only tests of the current rule use a named item that way. Whether to add such a form in this proposal, and whether to warn where a program changes meaning without a diagnostic, are left to the language's author (Open questions).

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
- The rule holds for every item that can be bound: an item of a handler (`x = H`, `x = H(arguments)`), an item that opens a handler member (`x = E.M(...)`, `x = c.M(...)`, as now) and a label item (`x = L by P`). A named handler that implements several effects provides none of them by name.
- A named item does not meet the requirements of the other items of the same with. A requirement on its effect is met by an item without a name of the same with, or from outside the with. A handler that has to use the handler of a named item is listed in a with inside the with of the named item, and takes the capability as an argument of its init.
- A named item does not provide the labels of its effect declared `default` (`[effect.label]`). Those labels are provided together only by an item that provides the effect by name.
- Precedence applies only among the items without a name: a later item without a name takes precedence for the same effect, and the earlier item is not initialized. A named item is never overridden: it is always initialized, its arguments are always evaluated, and the E and R of its arguments and the E of its init are always in the E and R of the with.
- The type arguments of a named item are inferred from the arguments of its init and from how x is used, not from the R of the body (`[effect.with]`). E0459 for two or more effects of the body that unify with the effect of an item applies only to items without a name.
- A task of a group starts siblings through the capability of the group, which the R of the task MAY contain (`[task.group]`). A group without a name also provides TaskGroup by name, as now.

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

- `[effect.with]`: a named item provides nothing by name, whether it is an item of a handler, of a handler member or of a label. Precedence, the exception for an overridden named item, the requirements met in the same with, the inference of type arguments from the R of the body, and E0459 for effects that unify are stated for the items without a name.
- `[effect.label]`: the labels declared `default` are provided together only by an item without a name.
- `[type.rules]`: `provides(H)` in the row of `with H { e }` is the effects of the items without a name.
- `[task.group]`: TaskGroup by name comes from a group without a name; a task of a named group starts siblings through the capability of the group.
- `[diag.list]`: the note and the help of E0402, and the third help of W0403.

## Diagnostics

- [ ] E0402: when a with between the place where E0402 is reported and the origin of the effect (searched for as for the help about `Platform`) has a named item of that effect, a note at the named item says ``= note: `tasks` is a named item, so it does not provide `TaskGroup` by name``, and the help is ``= help: remove `tasks =` to provide it by name, or call through `tasks` ``. No new code. The search stays inside the function where E0402 is reported, so a named item inside a function whose R is inferred (a function without a uses clause) gets no note when E0402 is reported at a caller of that function.
- [ ] W0403 (0.29): the third help (``= help: write `x <- Logger`, or a named item `with x = H` ``, given for `x = use E` when the second help does not apply: x is used other than as the receiver of operations, or inside a later with) is left out when the body of the with that provides E also uses E by name. For such a body, rewriting into a named item keeps the meaning only under the current rule, and no form yet both binds a capability and provides its effect by name. The other two helps are unchanged.

## Compatibility

- This proposal is made in 0.30, the version in which `use E` written as an expression becomes an error (#731), so that the rules for capabilities change in one version. The change of the help of W0403 is made in 0.29, the version that gives W0403.
- A program whose body uses by name an effect that only a named item of an enclosing with provides stops compiling (E0402, with the note above). The program is rewritten by removing the name when the capability is not used (`with Task.Group { spawn_tree(3) }`), or by calling the operations through the capability.
- A program whose body uses by name an effect that a named item provides, and that an outer with or an item without a name of the same with also provides, changes meaning without a diagnostic: the uses by name go to the handler of that outer with or of that item without a name. The typical case is a test that overrides a handler of a layer with a named item and calls the named item to control the fake handler: in `with AppLive, clock = FixedClock { run_jobs() }`, `run_jobs` now calls the Clock of `AppLive`. To keep its meaning, the test declares an effect for the operations that control the fake handler, implements that effect in the same handler, and lists the handler without a name: with `handler FixedClock(t: Int): Clock + ClockControl`, the test is `with AppLive, FixedClock { ctl <- ClockControl; ctl.advance(60); run_jobs() }` (checked at `ea45cbfb`). The measurement found no such program. Whether to warn about this case is an open question.
- A requirement of a handler on the effect of a named item of the same with is no longer met by the named item. The measurement found no such requirement.
- E0459 for several effects that fit, and for an item whose type arguments decide whether it overrides another item, is no longer given for a named item. The type arguments of a named item are determined by the arguments of its init and by how x is used, or are reported as not determined (E0459).
- A program that followed the help of W0403 in 0.29 and rewrote `x = use E` into a named item, in a with whose body also uses E by name, stops compiling in 0.30 (E0402), or calls an outer handler if an outer with provides E. The help is changed in 0.29 so that it no longer suggests that rewrite (Diagnostics).
- The tests to update are `binding_handlers`, `tasks_start_siblings` and `effect_alias_task.emel`, and four snapshots of `emela-core` whose evidence no longer provides the effect of a named item (`lower::handlers_of_several_effects`, `lower::semaphore_and_queue`, `lower::task_group_start_and_attempt`, `mono::tasks`).

## Implementation

- `keep_last` in `crates/emela-types/src/layer.rs`: a bound item that is not from a layer gets `named: false` whether or not a later item provides the same effect (the field documents "whether it provides its effect by name"). The rest of `with_effects` (the effects removed from the R of the body, the inference of type arguments from the R of the body, the order of initialization) and the lowering already follow `named`, except the two checks of E0459 below. The measurement above was made with the change of `keep_last` alone.
- E0402: the note and the help. The with is searched for between the place of E0402 and the origin of the effect, as `under_platform` in `crates/emela-types/src/body/errors.rs` does for `Platform`.
- E0459: `check_several_fits` and `check_item_overrides` in `crates/emela-types/src/body/items.rs` do not look at `named` now. Both skip named items, and `check_item_overrides` compares an item only with the items without a name.
- W0403 (0.29): leave out the third help when the body of the with that provides E also uses E by name.
- The comment of `Entry::named`, and the tests and snapshots listed in Compatibility.

## Alternatives

- **Keep the current rule and add `E by E`, so that the uses by name go to the outer handler.** `L by P` with an effect P provides "the outer handler (or that of an item initialized earlier in the same with)" (`[effect.label]`), so `Db by Db` written after `replica = ReplicaDb` would provide ReplicaDb, and another rule would be needed to skip the items of the same with. The item `Db by Db` would also be written to stop another item from providing Db by name, while every other item of a with provides an effect. A program that omits `Db by Db` still has its uses by name go to the named item without a diagnostic.
- **Keep the current rule and reject an overridden named item whose effect the body uses by name.** The order of the items in one with then no longer decides where the uses by name go, but the program of the motivation, where the named item is in an inner with, is not affected.
- **Add, in this proposal, a form for a named item that also provides its effect by name.** One candidate is an item that provides a capability by name (`with clock = FixedClock, Clock by clock`). Under the current rule `Clock by clock` is E0430, and `Clock by clock` would put a capability, not a handler, in an item of a with. Another candidate is a mark on the item (`with FixedClock as clock`), used only for this case, so that the other `with x = ...` are not rewritten; #732 rejected `as` for every named item because every `with x = ...` would be rewritten. The measurement found no program that needs such a form outside the tests of the current rule. Adding a form later does not change the meaning of programs written under this proposal. Whether to add a form in this proposal is an open question.
- **Two items of the same handler, one with a name and one without** (`with FixedClock, clock = FixedClock`). The program compiles, but the two items make two handlers with separate state, so an operation called through clock does not change the handler that the body uses by name.

## Prior art

- Named handlers (Xie, Cong, Ikemoto, Leijen, "First-class names for effect handlers", OOPSLA 2022), also cited by `handler-member`: a handler instance is a value through which its operations are called, and a rank-2 type keeps the instance inside its scope. Koka has named handlers.

## Open questions

The language's author decides the following. The measurement found no program that either question affects outside the tests of the current rule.

- Whether to give a warning, for one minor version, where the uses by name of an effect go to a different handler than under the current rule: a with has a named item of the effect, and an outer with or an item without a name of the same with also provides the effect (`with AppLive, clock = FixedClock { run_jobs() }`). The warning would also be given for the programs this proposal makes possible, which bind a capability inside a with of the same effect on purpose.
- Whether to add, in this proposal, a form for a named item that also provides its effect by name (the candidates in Alternatives), or to add a form when a program needs one.

## Decision
