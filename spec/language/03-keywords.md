# 3. Reserved words and attributes `[kw]`

There are 25 reserved words, and none of them can be used as an identifier in any position. In addition, there are words that have meaning only in specific positions, and words reserved for the future.

## 3.1 Reserved words `[kw.reserved]`

| Category | Words |
|---|---|
| Declarations | `fn` `type` `enum` `error` `const` `trait` `impl` `effect` `handler` `layer` `import` |
| Modifiers | `pub` `opaque` `suspend` |
| Expressions | `if` `else` `match` `fail` `escape` `use` `with` `assert` |
| Special names | `self` `Self` |
| Reserved, not used yet | `as` |

`as` is reserved for import aliases. No syntax uses it yet.

## 3.2 Contextual keywords `[kw.contextual]`

The following words act as keywords only in the positions shown. In any other position they are ordinary identifiers.

| Word | Position |
|---|---|
| `fails` | Function signatures and function types |
| `derive` | The end of a type, enum, or error declaration, and the body of a trait |
| `init` `release` | The body of a handler |
| `of` `default` | Label declarations (8.9) |
| `by` | with items (8.9) |

## 3.3 Reserved for the future `[kw.future]`

The following words have no syntax, but they cannot be used as identifiers either.

`var` `return` `while` `loop` `let` `mut` `in` `where` `do` `try` `catch` `throw` `async` `yield` `defer` `macro` `mod`

## 3.4 Not keywords `[kw.non-keyword]`

- `True`, `False`, `Some`, and `None` are constructors of enums in the Prelude.
- `panic`, `todo`, and `dbg` are functions in the Prelude.
- `resume` is the conventional name given to a handler parameter.
- `Immediate`, `Async`, `Eq`, and similar names are the names of built-in Traits and effects.

## 3.5 Attributes `[kw.annotation]`

An attribute has the form `@name(arguments)` and is placed immediately before a declaration. Only attributes built into the compiler can be used. Users cannot define new attributes.

| Attribute | Target | Meaning |
|---|---|---|
| `@external(...)` | fn without a body | Points to an implementation on the host side. It can also be attached to a type without fields (chapter 12) |
| `@test` | fn | The function is a test function (chapter 13) |
| `@deprecated(...)` | Any declaration | Warns at each use site |
| `@intrinsic` | fn without a body and effect (only at the top level of the bundled core sources) | On a fn, the fn is a built-in function implemented by the compiler. The signature is written in Emela syntax, and the implementation is looked up by name in the compiler's table of built-in functions. A fn with this attribute satisfies the `@external` constraints in 17.3. On an effect, the effect becomes a built-in effect (8.1, 11.2). Outside the core sources it cannot be used (E0224), and an effect with it is checked as a user effect. Attaching it to a fn with a body, to any other declaration, or to a fn inside a trait, impl, or handler is a compile error (E0225) |
| `@internal` | Top-level fn (only in the bundled core sources) | The fn cannot be referenced from outside the core modules (E0228). It cannot be attached outside core or to anything other than a top-level fn (E0229) |
| Field attributes | Fields of types and enums | Specify details of deriving. Their names and arguments are not yet decided (chapter 18) |
