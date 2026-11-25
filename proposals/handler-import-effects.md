---
title: Importing a handler brings its effects
authors: [Satoru314]
status: draft
issue:
touches: [module.import, module.resolution]
requires: []
related: [handler-member]
---

# Importing a handler brings its effects

## Summary

Listing a handler in an import also imports the effects that the handler implements. `import Std.Console.{SystemConsole}` makes `Console` usable unqualified.

## Motivation

- std places an effect and its system and test handlers in the same module (`[lib.layers]`). A program therefore imports the declaration (the effect) and the implementation (the handler) side by side: `import Std.Console.{Console, SystemConsole}`. The two names have different roles, the effect names what the code asks for and the handler what provides it, but the import lists them as peers. This is hard to read.
- The declaration `handler SystemConsole: Console` already names the effect it implements. A program that uses a handler uses its effect, so the effect can be known from the handler and does not need to be imported separately.

```emel
import Std.Console.{SystemConsole}

fn main() {
  with SystemConsole {
    console <- Console
    console.print_line("Hello, world!")
  }
}
```

Current workaround: `import A.B.{x}` also brings B into scope as a module (`[module.resolution]`), so `import Std.Console.{Console}` alone allows `with Console.SystemConsole { ... }`. This relies on the module and the effect having the same name, and is not checked against the compiler yet.

## Proposal

### Meaning

- When an import lists a handler H, each effect named after `:` in the declaration of H is imported as if it were listed in the same import.
- The rest of the import is unchanged: names that are not listed and are not effects of a listed handler are not imported.

## Diagnostics

- [ ] Conflicts between an effect imported through a handler and another definition of the same name (see Open questions)

## Compatibility

A program that defines or imports another definition with the same name as an effect implemented by an imported handler may get E0213 (see Open questions). Other programs keep their meaning; listing the effect explicitly stays valid, and importing the same definition twice is not a duplicate.

## Implementation

Name resolution of imports (`emela-resolve`): after resolving a listed handler, add the effects of its declaration to the import's names.

## Alternatives

- Importing an effect brings every handler of the same module. More names come in than the import shows, and both `SystemConsole` and `TestConsole` are imported.
- Naming handlers under their effect (`with Console.System { ... }`). This collides with handler members (`E.M`, `[effect.with]`).
- No change, and documenting the workaround above as the recommended form.

## Prior art

## Open questions

- Is an exception to "only the names listed in braces" (`[module.import]`) acceptable?
- When the effect is in another module than the handler (a user handler implementing `Std.Console.Console`), is it imported too? What if the effect is not pub?
- A handler that implements several effects (`[effect.handler]`): are all of them imported?
- A layer (`Std.Platform.Platform`): are the effects of its handlers imported?
- When another definition of the same name is already in scope, is the implicit effect dropped silently, or is it E0213?
- Are the effects of handler members included?

## Decision
