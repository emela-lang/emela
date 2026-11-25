---
title: Scopes, with items through which handler members are opened in expressions
authors: [somnicattus]
status: draft
issue: 723
touches: [effect.scope (new), effect.decl, effect.use, effect.with, effect.escape, type.rules, dyn.with, grammar.type, grammar.expr, kw.contextual, glossary.terms, glossary.context, lsp.hover, diag.list]
requires: [named-item-not-by-name]
related: [capability-identity, handler-member, task]
---

# Scopes, with items through which handler members are opened in expressions

## Summary

A named with item can be marked `scope` (`with wave = scope engine.Wave()`). The capability it makes is a scope, of type `scope E`. Through a scope, a handler member can be opened in any expression (`wave.SpawnEnemy(kind: Small)`), not only as a with item or in an init. What is opened through the scope in an expression records the scope and the member (`wave.SpawnEnemy`) in its type, cannot leave the with of the scope, and is released before the scope is, in the reverse order of opening.

## Motivation

Some resources are opened a number of times decided at run time: the enemies of a stage in a game, the files a job reads, the permits of a pool of workers. This proposal lets that number be decided at run time; the resources opened through a scope in an expression end when the scope ends. Ending each of them at a time decided at run time is left to a later proposal (Alternatives, "Closing at any time"). A handler member gives a resource the shape of a handler: the resource has its own fields, init and release, the provider decides its implementation, and code uses it as `use Enemy` without knowing the provider (`[effect.decl]`, `[effect.handler]`).

A handler member can be opened only as a with item or inside an init (`[effect.use]`: "It can be written only as a with item `x = c.M(arguments)` (8.3) and inside an init (8.4)", E0448). A with item opens one handler for one block, so the number of resources is fixed by the source:

```emel
fn stage(engine: use Engine) {
  with boss = engine.SpawnEnemy(kind: Large) {
    with a = engine.SpawnEnemy(kind: Small), b = engine.SpawnEnemy(kind: Small) {
      fight(boss, [a, b])
    }
  }
}
```

`capability-identity` (#732) answered resources opened a number of times decided at run time with "Recursion writes them, and capability-bound types let a pool stop its ids from leaving" (Alternatives), and left "Whether regions are needed for opening and closing at any time" to #651, which was withdrawn without a decision.

Recursion opens one with per resource. The resources can be used together only at the deepest level of the recursion, where a list of them can be passed down but not returned, and they end only as the recursion unwinds.

A pool returns ids of a capability-bound type:

```emel
effect Engine {
  fn spawn(hp: Int) -> EnemyId
  fn damage(id: EnemyId, amount: Int) -> Int
  fn despawn(id: EnemyId) -> ()
}

type EnemyId(n: Int) uses Engine

fn wave(engine: use Engine) {
  ids = [1, 2, 3].map(|hp| engine.spawn(hp))
  ids.each(|id| engine.despawn(id))
}
```

The ids cannot leave the with that made `engine` (E0455), and their number is decided at run time. What the pool cannot give:

- The resource is not a handler. Its operations (`damage(id, amount)`) and its end (`despawn(id)`) are operations of the pool, and a function that uses one enemy takes the pool and an id instead of `use Enemy`. Wrapping an id as a handler of Enemy (`handler View(engine: use Engine, id: EnemyId): Enemy`) does not type-check today (reported in #866).
- The pool writes its own cleanup. What the ids left at the end of the with stand for is cleaned up only if the release of the pool walks a table of them, and every pool keeps that table itself.

A scope keeps the resource a handler and gives the cleanup to the language:

```emel
fn summon_wave(wave: scope Spawner, count: Int) -> List[use Enemy] {
  List.range(0, count) |> List.map(|_| wave.SpawnEnemy(kind: Small))
}

fn stage(engine: use Engine) {
  with wave = scope engine.Wave() {
    enemies = summon_wave(wave, 3)
    enemies.each(|enemy| enemy.move_to(10, 0))
  } # the enemies, then the wave, are released here
}
```

## Proposal

### Syntax

```ebnf
with_item  = [ lower_name "=" ] [ "scope" ] supply [ "by" by_right ]
type       = ...
           | "scope" effect_item { "+" effect_item }   (* the type of a scope; 8.11 *)
postfix    = primary { "(" [ args ] ")" | "." lower_name | "." type_name "(" [ args ] ")" }
             (* "." type_name: a handler member, inside an init or through a scope; 8.4, 8.11 *)
```

- `scope` is a contextual keyword: before the right side of a with item, and at the head of a type. `with x = scope.Engine` (a module named `scope`) and `with x = scope Engine` differ by the dot, and `with scope = H` binds the name `scope`.
- `scope` can be written in a named item whose right side makes a handler: a handler (`x = scope H(arguments)`), a handler member (`x = scope E.M(arguments)`, `x = scope c.M(arguments)`) and a label plugged into a handler (`x = scope L by H`). `x = L by H` makes the handler H, provides it as L and binds it (`[effect.label]`), so `x = scope L by H` is a scope of L, with the handler members of the underlying effect. The grammar also accepts `scope` on an item without a name, so that marking one is reported as such (E0463, in place of E0447 for a handler member). A layer, or a label item that makes no handler, bound to a name is E0447 whether or not it is marked.

```emel
pub enum Kind {
  Small
  Large
}

pub effect Enemy {
  fn move_to(x: Int, y: Int) -> ()
}

pub effect Spawner {
  handler SpawnEnemy(kind: Kind): Enemy
}

pub effect Engine {
  handler SpawnEnemy(kind: Kind): Enemy
  handler Wave(): Spawner
}

fn stage() {
  with engine = scope SystemEngine {
    boss = engine.SpawnEnemy(kind: Large)
    with wave = scope engine.Wave() {
      enemies = summon_wave(wave, 3)
      boss.move_to(10, 0)
    } # the enemies, then the wave, are released here
  } # the boss, then the engine, are released here
}
```

### Meaning

- An item marked `scope` makes its handler as the item without the mark does, and its capability is a scope. A scope is always the capability of a handler that the item makes. A capability that is already bound cannot be made a scope (`with s = scope engine` is not a supply).
- Through a scope s of E, `s.M(arguments)` can be written outside a with item and an init, in any other expression. This is called opening in an expression. It runs the init of the handler member M of the handler of s, as `x = s.M(arguments)` does as a with item, and its value is the capability of the handler opened.
- A handler opened in an expression through s belongs to s. It is released when s is released, before the release of s, in the reverse order of opening. The order of opening is the order in which the inits of the opened handlers return. The handlers that the init of s opened (8.4) are released after the release of s, as now.
- A handler opened through s as a with item (`x = s.M(...)`) or inside an init is released at the end of that with, or after the handler of that init, as now. Only opening in an expression gives a handler that belongs to s: the with item and the init already give the handler a lifetime of their own, and an expression does not.
- If the init of a handler opened in an expression fails, nothing is opened and the error is the error of the expression.
- `E.M(arguments)` and `c.M(arguments)` through a capability that is not a scope remain limited to with items and inits: opening them in an expression is E0448.
- A handler that belongs to a scope is not released before the scope is. Closing one of them at any time is left to a later proposal (#723, Alternatives).
- A field of a handler can have the type `scope E`, as it can have `use E`. A handler opened in an expression in an operation body or in release through a scope in a field (`self.wave.SpawnEnemy(...)`) belongs to that scope: it lives until the scope is released, also after the operation returns. It cannot be returned from the operation (E0456) or put in a field, which is fixed at init. Opening through the field and calling an operation of the handler opened are treated as calling an operation of a capability in a field (`[effect.use]`, including its rule for release): they do not make a requirement of the handler, since the scope and what it opens outlive the handler, and they count for whether the body may suspend.

### Typing rules

- A set of a capability type can also have an element `s.M`: the handlers opened in an expression through a scope s with a handler member M. All of them are released when s is, so one element stands for them, and the language server names it so (`wave.SpawnEnemy`). Each capability in a set is either a scope or not; an element `s.M` is not a scope. `scope E` is a capability type of E whose set has only scopes. It can be written wherever `use E` can.
- A scope can be passed to a parameter of type `use E`. A parameter of type `scope E` ranges over sets of scopes; passing a capability that is not a scope to it is a compile error (E0464). Unification takes the union of sets as now; the union of a scope and a capability that is not a scope is a `use E`, through which members cannot be opened in an expression (E0448).
- `s.M(arguments)` opened in an expression adds the set of s to R, as calling an operation of s does. Its type is `use F1 + ... + Fn`, where the Fi are the effects after the colon of M, and its set has the element `t.M` for each scope t in the set of s. Calling an operation of the opened handler adds those elements to R.
- An element `s.M` counts as a capability made by the with that made s. The rules on such capabilities apply to it unchanged: that with removes it from R (the row of `with` in `[type.rules]`), and it cannot leave that with (`[effect.escape]`, below).
- The set of a capability type `use F` (or of a capability-bound type of F) written in the result type of a function is the union of the sets of every capability of F in the parameter types, and, for each scope s of an effect E in the parameter types (including those inside other types and the hidden sets of fields) and each handler member M of E that has F after its colon, the element `s.M`. When neither gives an element, it is the name F. `summon_wave` above returns `List[use Enemy]` whose set has `wave.SpawnEnemy`.
- Opening in an expression may suspend if M is a `suspend handler` (`[effect.decl]`), and so may the end of the with of the scope, where the handler opened is released. The `fails` of M is in the E of the expression.

### Capabilities must not leak

- Since an element `s.M` counts as a capability made by the with that made s, every rule of `[effect.escape]` applies to it: the type of the value of the with body, the error type that leaves the body and the R of the with cannot contain it (E0455), and neither can a type fixed outside the with.
- A capability-bound type returned by an operation of a handler opened through s records the element `s.M`, and so cannot leave the with of s either.

## Diagnostics

- [ ] E0448: the condition becomes "a handler member is opened in an expression (outside an item of with and an init) through an effect or a capability that is not a scope". When it is opened through a capability, the help is ``= help: open it as an item of `with`, or through a scope: `with s = scope H { s.Open(...) }` ``
- [ ] E0455: also covers the elements `s.M` of a scope made by the with, as capabilities of that with
- [ ] E0463 (new): `scope` on an item without a name, reported in place of E0447 for a handler member. ``only a named item can be a scope``
- [ ] E0464 (new): a capability that is not a scope passed where `scope E` is expected. ``expected a scope of `Engine`, found a capability that is not a scope``

## Compatibility

- `scope` is a contextual keyword. An identifier named `scope` keeps its meaning everywhere else; in std, the examples and the tests it occurs only inside a string and a comment.
- No program that compiles now changes meaning. `x = scope H` and `scope E` in a type are compile errors now.

## Implementation

- emela-syntax: the mark in with items, the type `scope E`, and `.M(...)` in expressions outside an init.
- emela-types: the elements of sets for scopes and handlers opened through them, the result rule, E0448, E0455 and the new codes.
- emela-core and the runtimes (JS, C, WASM): a scope keeps the list of the handlers opened through it in an expression, and its release releases them first, in reverse order. An unpublished prototype of region members (#660) had the list and the release order; it was not built.
- emela-lsp: hover names a handler opened through a scope by the scope and the member (`wave.SpawnEnemy`).

## Alternatives

- **Any capability opens in an expression.** Opening through every capability, with the opened handler belonging to it, needs no mark. But a capability made at the top of a program (`with engine = SystemEngine`) would collect every handler opened through it until the program ends, every handler would keep a list, and the result rule would count every capability whose effect has a member. A scope makes this a choice at the item, and the result rule looks only at the scopes in the parameter types.
- **A separate region over an existing capability** (`with wave = Region(engine)`). The handler that implements the members (that of `engine`) and the owner (`wave`) are different, so `wave` forwards to `engine`. A handler written in Emela cannot forward members: an inner handler cannot refer to the `self` of its outer handler, and its init takes exactly the parameters of the member, so the only way to reach `engine` is to open `Engine.SpawnEnemy` by name, which `named-item-not-by-name` (#863) removes. A built-in `Region` would take the effect it provides as a type argument, while the type parameters of a handler are limited to ordinary types (`[effect.handler]`), so it would need an exception to that rule.
- **Regions as an effect** (`Std.Region`, #651) and **region members** (`handler Wave: Region[use Engine]`, #660). Both added an effect with a close operation and a new kind of root of capture sets. Capability identity (#732) removed capture sets; the scope reuses the sets of capability types.
- **Nesting through a member that provides its own effect** (`handler Wave(): Engine`, #723). A handler of an effect with such a member cannot be written as a finite program: the inner handler of `Wave` is a handler of Engine and implements `Wave` again, and the innermost one is E0404. A member that opens another effect (`Wave(): Spawner`) is finite.
- **Closing at any time** (`dispose`, #723). It closes the capabilities held in fields of handlers (`[effect.use]`: "both outlive the handler") and needs a check at run time on every capability. Restricted to handlers opened through a scope, the check covers only them, and a capability opened through a scope that goes into a field can be told by its type at the with item. It is a goal, left to a later proposal.
- **A handler opening through itself** (`self.M(...)` in an operation body, what it opens living as long as the handler). A handler does not know whether its item is marked `scope`, so `self` would be a scope in every handler, and every handler would keep a list of what it opened, which is the cost the mark avoids ("Any capability opens in an expression"). What it opened could not be returned from an operation (E0456) or put in a field. Opening through `self` in an expression is not allowed now, so this can be added later without changing the meaning of any program.
- **A warning when handlers accumulate** in a long-lived scope (opening inside a loop through a scope of `main`). How long a scope lives and how many times a closure runs (`each`, recursion) are not known when checking, so a warning would rest on guesses. The mark at the item is where the choice is made. A warning can be added later.
- **A postfix mark** (`SystemEngine.Regioned`). A handler without parameters is written without parentheses, so the mark reads as opening a member `E.M`, and a built-in handler can have the name of its effect (`Deferred`).
- **Other words.** `region` is the word of #651 and #660, chosen while `scope` was taken by `Scope`, the effect of fibers, which Task (#834) removes. `with` in a type position reads as the with expression.

## Prior art

- Region-based memory management (Tofte and Talpin): values are allocated in a region that is a lexical scope, and the type system keeps references from outliving it. Here the region is a with item, and the sets of capability types play the part of region variables.
- Arenas and nurseries: an owner that frees or awaits everything made in it at the end of a lexical scope.

## Open questions

## Decision
