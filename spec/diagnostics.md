# Appendix A. Diagnostic codes `[diag]`

Diagnostics of the compiler and the tools have codes.

- An error code is `E` followed by 4 digits, and a warning code is `W` followed by 4 digits. A number, once assigned, is not reused even after the diagnostic is retired.
- Numbers are divided into bands by stage. Warnings use the same bands (`W03xx` are type warnings).
- Messages are written in English. Messages are not part of the specification, and an implementation MAY change them to improve them. Codes do not change.
- Diagnostics are displayed in the following form. A diagnostic without a code is printed as `error: ...`.

```
error[E0211]: cannot find value `usr` in this scope
   ╭─[ src/main.emel:2:7 ]
   │
 2 │   x = usr
   │       ─┬─
   │        ╰───
───╯

error[E0204]: duplicate module `Http.Client`
   ─[ src/http/Client.emel ]
   = note: `src/http/client.emel` already defines this module

2 errors
```

- Diagnostics are written to standard error, separated by blank lines. A location is shown in a `╭─[ path:line:column ]` frame, and when the location is a whole file, in the single line `─[ path ]`.
- A note is printed as `= note: ...`, and help as `= help: ...`.
- The last line is `N errors`, or `N errors, M warnings` when there are also warnings (singular for a count of 1).
- Color is used only when standard error is a terminal and the environment variable `NO_COLOR` is empty or unset.
- Diagnostics are ordered by stage: module collection (errors in file and directory names, for example), lexing and parsing per file (in module path order), the import graph, name resolution, and type checking. Within name resolution and within type checking, diagnostics are ordered by file and location.
- A "did you mean" candidate (`= help: did you mean ...`) is the single closest name among the names that can be looked up at the same location and whose edit distance is at most one third of the length of the name (at least 1).
- ``error: not supported yet: <what>`` without a code is emitted for forms the implementation cannot handle yet (such as a trait bound on an effect set type parameter). It is used only until the implementation can handle them.

## Bands `[diag.bands]`

| Band | Stage |
|---|---|
| E01xx | Lexing and parsing (E0101 to E0109 are lexical, E0110 to E0189 are syntax, E0190 to E0199 are additional lexical codes) |
| E02xx | Modules, imports, project structure |
| E03xx | Types |
| E04xx | effect, handler, layer, capture set |
| E05xx to E08xx | Reserved (assigned when needed, for traits and derive, external functions, tests, and so on) |
| E09xx | Tools (I/O, node, and so on) |

## List `[diag.list]`

| Code | Meaning | Example message |
|---|---|---|
| E0101 | A string is not closed | `unterminated string literal` |
| E0102 | An interpolation `#{` is not closed | `unterminated interpolation` |
| E0103 | Matches no name class (2.3) | ``invalid name `Http_client`: not a lower, type, or upper name`` |
| E0104 | Not a valid number form | ``invalid number literal `1e9` `` |
| E0105 | An unrecognized character | ``unrecognized character `é` `` |
| E0106 | No character after `\` (a `\` at the end of a line, including inside a multi-line string) | ``missing character after `\` `` |
| E0107 | An unknown escape | ``unknown escape sequence `\q` `` |
| E0108 | An invalid `\u{...}` (malformed, a surrogate, or above 10FFFF) | ``` `\u{D800}` is a surrogate, not a Unicode scalar value ``` |
| E0109 | A malformed multi-line string (the opening `"""` is not followed by a newline, the closing `"""` is not alone on its line, or indentation shallower than the closing line) | ``expected a newline after the opening `"""` `` |
| E0110 | An expected token is missing. The message contains the display name of the expected token (symbols and reserved words in backquotes, names and literals by kind such as `lower name` or `integer literal`, and the end of input as `end of file`) | ``expected `)` `` |
| E0111 | No type where a type is expected | `expected type` |
| E0112 | A tuple with one element (`(a,)` in types, patterns, and expressions; `(a)` in types and patterns) | `a tuple type needs at least two elements` |
| E0113 | No pattern where a pattern is expected | `expected pattern` |
| E0114 | An element after `..` | ``` `..` must come last ``` |
| E0115 | No element before `..` in a list (`[..rest]`) | ``` `..` needs at least one element before it ``` |
| E0116 | Unused (was a temporary diagnostic for negative numbers in patterns; retired in 0.20.1, when negative numbers became writable) | |
| E0117 | Interpolation in a string in a pattern | `string interpolation is not allowed in a pattern` |
| E0118 | No expression where an expression is expected | `expected expression` |
| E0119 | Chained comparison operators (`a < b < c`) | `comparison operators cannot be chained` |
| E0120 | A positional argument after a named argument | `positional arguments cannot follow named arguments` |
| E0121 | `use X.op()` without parentheses | ``wrap `use` in parentheses to call its operations: `(use X).op()` `` |
| E0122 | No newline or `}` after a statement, arm, or declaration | ``expected newline or `}` after statement`` |
| E0123 | `_` outside a pattern | ``` `_` can only be used in patterns ``` |
| E0124 | `..` without an expression outside a pattern | ``` `..` without an expression can only be used in patterns ``` |
| E0125 | The left-hand side of a binding does not read as a pattern (`a + b = 1`) | `invalid left-hand side of binding: expected a pattern` |
| E0126 | No declaration where a declaration is expected | `expected declaration` |
| E0127 | An import after a declaration (4.2) | `imports must come before declarations` |
| E0128 | A variant mixes named and positional fields (5.4) | `a variant cannot mix named and positional fields` |
| E0129 | A derive rule in a trait body (temporary until 18.1 #3 is decided) | `derive rules in traits are not supported yet` |
| E0130 | Missing `@external`: an fn without a body, a top-level suspend fn, or a type without fields (17.3) | ``a function without a body must be `@external` `` |
| E0131 | `opaque` on something other than type or enum, or without `pub` | ``` `opaque` can only be used on `type` and `enum` ``` |
| E0132 | An enum with no variants | `an enum needs at least one variant` |
| E0133 | `pub` or an attribute on an impl (impl_decl in 17.3) | ``` `impl` cannot have `pub` or annotations ``` |
| E0134 | A type on a parameter of a handler operation (handler_item) | `handler operation parameters take their types from the effect` |
| E0135 | An fn in an impl or handler without a body | `this function needs a body` |
| E0136 | A return type, fails, or use on an fn in an impl (impl_item) | `impl methods take their signature from the trait` |
| E0137 | A `use` clause on an effect operation (8.1) | ``effect operations cannot have a `use` clause`` |
| E0138 | Malformed arguments of `@external` (no target name, an argument that is not a string, not one or two strings, or string interpolation) (12.1) | ``expected `@external(target, "name")` `` |
| E0139 | Two or more `@external` for the same target on one fn (12.1) | ``duplicate `@external` for target `js` `` |
| E0140 | An unknown target name (currently js, wasm, c) (12.1) | ``unknown target `node` `` |
| E0141 | Arguments on the `@external type` attribute (12.3) | ``` `@external` on a type takes no arguments ``` |
| E0142 | match written in prefix form (6.5) | ``` `match` comes after the value: write `value match { ... }` ``` |
| E0144 | An impl written in the form `impl Trait for Type` or `impl[...] Type: Trait` (10.2) | ``write `impl User: Show` instead of `impl Show for User` `` |
| E0145 | A handler written with `implements` (8.4) | ``write `handler ConsoleLogger: Logger` instead of `implements` `` |
| E0143 and up | Syntax errors | Assigned in the parser and added to this table |
| E0201 | A file name does not form a module name (4.1) | ``file name `Foo` cannot be a module name: not lower snake_case`` |
| E0202 | A directory name does not form a module name | ``directory name `My-Dir` cannot be a module name: not lower snake_case`` |
| E0203 | A reserved module name (`prelude.emel` directly under the root, the module names of core, `std`, and `main.emel` and `lib.emel` directly under the test root). Includes a dependency whose root name is a reserved name (4.7) | ``module name `Prelude` is reserved`` |
| E0204 | Duplicate modules (including those that differ only in case, and those across the source root and the test root). Includes a dependency whose root name collides with a module of its own or with the root name of another dependency (4.7) | ``duplicate module `Http.Client` `` |
| E0205 | The imported module does not exist | ``undefined module `Http.Clinet` `` |
| E0206 | An import cycle (including a self-import) | ``import cycle: `A` -> `B` -> `A` `` |
| E0207 | A directory was given, but no `Pome.toml` was found | ``no `Pome.toml` found in `proj` or its ancestors`` |
| E0208 | The given file or directory is outside the source root. A file inside the test root was given to `build` or `run` (4.1, Appendix B) | ``file `scripts/x.emel` is outside the source root `src` `` / ``directory `scripts` is outside the source root `src` `` / ``` `test/a.emel` is under the test root, which only check and test read ``` |
| E0209 | The entry is not found (build and run only) | ``entry `src/main.emel` not found`` |
| E0210 | The entry does not form a module | ``entry `src/Main.emel` is not a module`` |
| E0211 | An undefined name | ``undefined name `usr` `` |
| E0212 | The module, trait, or effect has no item with that name | ``module `Http.Client` has no item `gett` `` |
| E0213 | Duplicate definitions in the same namespace. The diagnostic points at the later definition and attaches the location of the earlier definition as a label | ``duplicate definition of `User` `` |
| E0214 | A definition that is not pub is referred to from another module | `` `parse_raw` is private to `Email` `` |
| E0215 | An opaque type is constructed outside its defining module (4.4) | ``cannot construct opaque type `Email` outside its module`` |
| E0216 | An opaque type is written in a pattern outside its defining module (4.4) | ``cannot match on opaque type `Email` outside its module`` |
| E0217 | The name was found, but it is of a kind that cannot be written at that position | ``expected an error, found type `User` `` |
| E0218 | The name to the left of `.` refers to both a module and a trait | ``ambiguous name `Show`: both a module and a trait`` |
| E0219 | `self` or `Self` where it cannot be used | `` `self` is only available in handlers, traits, and impls `` |
| E0220 | The same name is bound twice in one pattern or one parameter list | `` `x` is bound more than once `` |
| E0221 | An effect name in a type position. Includes a position of a type argument that is not a set (`List[Db]`) (8.2, 9.2) | ``effect `Logger` cannot be used as a type`` |
| E0222 | The callee of a record update is not something with a single constructor (type, error, or handler with fields) (5.3) | ``cannot update enum variant `Circle` with `..` `` |
| E0223 | A field to update is written by position in a record update (5.3) | ``fields to update must be named: `field: value` `` |
| E0224 | `@intrinsic` used outside the source of the bundled core (3.5) | ``` `@intrinsic` can only be used in the bundled core library ``` |
| E0225 | `@intrinsic` on something other than an fn without a body (3.5) | ``` `@intrinsic` can only be used on a function without a body ``` |
| E0226 | `@test` on an fn with parameters, an fn without a body, or something other than a top-level fn, or `@test` with arguments (13.1) | ``test function `takes` cannot take parameters`` |
| E0227 | A test function is referred to from an fn or const that is not a test function (13.1). The note is ``= note: test functions are removed from normal builds (13.1)`` | ``test function `adds` can only be used from test functions`` |
| E0228 | An `@internal` fn is referred to from outside the modules of the bundled core (from the body of an fn, const, or handler). Both calls and references as values count; listing it in an import alone does not trigger it (3.5). The note is ``= note: it is marked `@internal` `` | ``function `Secret.peek` can only be used from the bundled core library`` |
| E0229 | `@internal` outside the bundled core, or on something other than a top-level fn (3.5) | ``` `@internal` can only be used in the bundled core library ``` / ``` `@internal` can only be used on top-level functions ``` |
| E0230 | The entry of `emela run` has no fn named main. Also `run` on a library-only project (8.3, Appendix B). The location is the entry file, and it is reported only when there are no other errors | ``entry `src/main.emel` has no `main` function`` |
| E0231 | Consts form a cycle through their names alone (5.6) | ``constants refer to each other: `A` -> `B` -> `A` `` |
| E0232 | `[build].target` in `Pome.toml` is an unknown target (4.7). The note is ``= note: expected one of `js`, `c`, `wasm` `` | ``unknown target `node` in `[build].target` `` |
| E0233 | An `@external` in a dependency Pome is not allowed by `allow` in `[external]` (4.7, Chapter 12). Reported only for the first `@external` in each module of a Pome that is not allowed. The help is ``= help: add it to `[external] allow` in Pome.toml, or run `emela pome add --allow-external <source path>` `` | `` `github.com/acme/x` uses `@external`, but it is not allowed `` |
| E0234 | `[pome].emela` of a dependency is newer than the current compiler, or does not read as a version (4.7) | `` `github.com/acme/x` needs emela 0.22.0 or newer; this is 0.21.0 `` / `` `[pome].emela` of `github.com/acme/x` is not a version: `next` `` |
| E0235 | The root name of a dependency does not form a module name, or a root name cannot be made from the source path (4.7). A note suggests renaming it with `module` | `` `Json-codec` (the root name of `github.com/acme/x`) is not a module name `` / ``cannot make a root name from `Json-Codec` of `github.com/acme/Json-Codec` `` |
| E0236 | The language server cannot use the dependencies in `Pome.toml` (no lock, or not in the cache; 4.7). The location is `Pome.toml`, and the note is ``= note: run `emela pome install` to get them`` | ``the dependencies cannot be used: no Pome.lock; run `emela pome install` `` |
| E0237 | An `@external` fn in a dependency Pome is pub (4.7). The note is ``= note: `<source path>` reaches the host through its handlers; expose an effect, or a function that calls this one`` | ``an `@external` function cannot be `pub` in a dependency`` |
| E0238 | `[pome].targets` of a Pome in the dependency graph does not include the build target (4.7). With the note ``= note: required by <Pome>`` (`this Pome` for the Pome being built) and the help ``= help: build for js, or move the wasm entry to a Pome that does not depend on it`` | `` `github.com/emela-lang/x/platform-cloudflare` supports only js, not wasm `` |
| E0239 | A module in the source root imports a module in the test root or a Pome in `[dev-dependencies]` (4.7, 13.1). The note is ``= note: the test root and [dev-dependencies] are only for the tests; move the tests that use it under test/ (13.1)`` | `` `Brix.Pbt.Property` can only be imported under `test/` `` |
| E0240 | The same Pome is in both `[dependencies]` and `[dev-dependencies]` (4.7) | `` `github.com/emela-lang/brix` is in both [dependencies] and [dev-dependencies] `` |
| W0201 | A type parameter with a type name shadows an outer type of the same name (2.3) | ``type parameter `Item` shadows type `Item` `` |
| W0202 | A definition or import in a module shadows a Prelude name of the same name | `` `Some` shadows a Prelude name `` |
| W0203 | `[pome].emela` of a dependency is an older minor than the current compiler (4.7). The note is ``= note: it is built anyway; errors inside it may come from changes between the versions`` | `` `github.com/acme/x` is written for emela 0.20.1; this is 0.21.0 `` |
| W0204 | A Pome in `allow` of `[external]` is not in the dependency graph (4.7). The location is `Pome.toml` | `` `github.com/acme/old` is allowed in [external] but not depended on `` |
| E0301 | A type mismatch | ``type mismatch: expected `Int`, found `String` `` |
| E0302 | An infinite type | ``infinite type: `?0` occurs in `List[?0]` `` |
| E0303 | A match is not exhaustive (6.5). With several missing examples, ``non-exhaustive match: patterns `A`, `B` not covered`` | ``non-exhaustive match: `Rect(_, _)` not covered`` |
| E0304 | The wrong number of type arguments (built-in types, type / enum / error, type parameters) | ``wrong number of type arguments: `List` takes 1, found 2`` |
| E0305 | The wrong number of arguments in a call (6.2). For a call of a function value, ``this function takes 2 arguments, found 3`` | ``wrong number of arguments: `connect` takes 2, found 3`` |
| E0306 | No parameter with that name. Includes calling a function value, which has no argument names, with named arguments (6.2). When a function value is called with named arguments, ``no parameter named `x`: a function value has no parameter names`` | ``no parameter named `prot` in `connect` `` |
| E0307 | The same argument is given twice. Includes a positional argument and a named argument that refer to the same parameter (6.2) | ``argument `port` given more than once`` |
| E0308 | A constructor pattern with named fields omits fields without `..` (6.5). The help is ``= help: add `..` to ignore the remaining fields``. Four or more fields are shown as the first three and ``and N more`` | ``pattern `User` does not mention fields `id`, `email` `` |
| E0309 | The wrong number of fields in a constructor pattern (too many, or too few without `..`) (6.5) | ``pattern `Rect` has 2 fields, found 1`` |
| E0310 | A non-numeric type where a number is expected (arithmetic operators, unary minus) (6.6). Where an integer is required, ``expected an integer (`Int` or `Int64`), found `Float` `` | ``expected a number (`Int`, `Int64` or `Float`), found `String` `` |
| E0311 | An integer literal is out of the range of its type (2.4) | ``integer literal `3000000000` is out of range for `Int` `` |
| E0312 | No field with that name | ``no field `nmae` on type `User` `` |
| E0313 | A field access on a type without fields | ``type `Int` has no fields`` |
| E0314 | A field access on a value whose type is not determined. The help is ``= help: add a type annotation`` | ``the type of this value must be known to access field `name` `` |
| E0315 | A field access on an opaque type outside its defining module (4.4) | ``cannot access field `value` of opaque type `Email` outside its module`` |
| E0316 | A parameter of a top-level fn has no type (6.1) | ``parameter `x` of a top-level function needs a type`` |
| E0317 | The pattern of a binding or of an anonymous function parameter is refutable (6.1, 6.3). The help is ``= help: use `match` to handle the other cases`` | ``refutable pattern in binding: `None` not covered`` |
| E0318 | An error of the body is not included in the declared E (an unhandled error with `-> T`, an error not in `fails`, a failure in the value of a const) (7.4). The help is ``= help: handle it with `escape`, or declare it with `fails` `` without a fails clause, and ``= help: add it to `fails` or handle it with `escape` `` with one. For several errors, ``unhandled errors `A`, `B` `` and ``errors `A`, `B` are not declared in `fails` ``. A failure in the value of a const is reported in the form without a fails clause (``unhandled error ...``) | ``unhandled error `NotFound` `` / ``error `DbError` is not declared in `fails` `` |
| E0319 | A value that is not of an error type is passed to `fail`. A type parameter that stands for one error type is instantiated with a type that is not an error (7.2) | ``expected an error value, found `Int` `` |
| E0320 | The pattern of an escape arm is not an error constructor (7.3) | `escape arms must match an error constructor` |
| E0321 | The E of a function value is not included in the E of the expected function type (9.4) | ``error set mismatch: expected `fails {}`, found `fails Timeout` `` |
| E0322 | Immediate on a type parameter that is not an effect set (9.5) | `` `Immediate` can only constrain effect sets, but `A` is not used in `use` `` |
| E0323 | A set type parameter of a named type is given something that is neither a tag of that kind nor a set type parameter (9.2). The help is ``= help: write an error or an error set type parameter`` (for an effect set, ``an effect or an effect set type parameter``) | `` `E` of `Handler` is an error set, found type `Int` `` |
| E0324 | A set in the position of a type argument that is not a set (9.2) | `expected a type, found a set: sets can only be passed to set type parameters` |
| E0325 | An impl lacks methods of the trait that have no default implementation (10.2) | ``incomplete impl: missing methods `show`, `merge` `` |
| E0326 | A type does not satisfy a trait (`==`, ordering comparisons, interpolation, bounds, the fields and supertraits for derive, a trait that cannot be derived; 10.2, 10.3, 10.5) | ``type `User` does not implement `Eq` `` |
| E0327 | An fn with a type parameter that has a bound is exported from the entry (Appendix B). Reported only by `build` and `run` for the JS target. The help is ``= help: make a `pub` fn that calls it with concrete types, and export that instead`` | ``cannot export `largest`: its type parameter `A` has a trait bound `` |
| E0328 | The head of an impl is not a type name with a list of distinct type parameters (10.2) | ``impl head must be a type with distinct type parameters: `List[Int]` `` |
| E0329 | Supertraits form a cycle (10.1) | ``trait `A` is its own supertrait: `A` -> `B` -> `A` `` |
| E0330 | Two impls for the same pair of type and trait (including an overlap between derive and a handwritten impl; 10.2, 10.5) | `` `Show` is already implemented for `User` `` |
| E0331 | An impl outside both the module that defines the type and the module that defines the trait (10.2) | `` `Show` for `User` can only be implemented in the module of `Show` or of `User` `` |
| E0332 | An fn in an impl has a different number of parameters from the trait declaration (10.2) | ``wrong number of parameters: `show` of `Show` takes 1, found 2`` |
| E0333 | The Self of a trait function is not determined within the function (10.3) | ``cannot infer which impl of `Parse` to use: add a type annotation`` |
| W0301 | A redundant arm (6.5) | `unreachable match arm` |
| W0302 | The error an escape arm receives is never raised by the original expression (7.3) | ``error `Timeout` is never raised here`` |
| W0303 | An implementation derived for a type that is not `pub` is never required anywhere (10.5). The help is ``= help: remove it from `derive` ``. For Show, ``= help: remove it from `derive`; `dbg` then shows `Point` as constructed, and a failed `assert` no longer shows its values`` | ``derived `Ord` for `Point` is never used`` |
| W0901 | In `emela pome publish`, the version bump is too small for the changes to the public API (Appendix B) | ``removing `Quince.Router.mount` needs a new major version (v1.2.0 → v2.0.0)`` |
| E0401 | The effect of a capability value has no such operation (8.2) | ``effect `Logger` has no operation `infoo` `` |
| E0402 | The R of a body is not included in the R that the declaration or the context allows (use clause, const, main, test function) (8.3, 9.3, 13.3). The help is ``= help: add it to `use` or provide it with `with` `` for a use clause, and ``= help: provide it with `with` `` for main and test functions. For effects that come from forked children (`[fiber.scope]`), the note ``= note: fibers forked in `Fiber.scope` get their handlers from where `Fiber.scope` is called, so `with` inside it does not reach them`` is added, and the help becomes ``provide it with `with` outside `Fiber.scope` `` (``add it to `use` or provide it with `with` outside `Fiber.scope` `` for a use clause). For several effects, ``effects `A`, `B` are not declared in `use` ``. For a const, ``constant values cannot use effect `Logger` ``; for a test function, ``effect `Logger` is not provided in test function `logs` `` | ``effect `Logger` is not declared in `use` `` / ``effect `Clock` is not provided in `main` `` |
| E0403 | The R of a function value is not included in the R of the expected function type | ``effect set mismatch: expected `use {}`, found `use Logger` `` |
| E0404 | A handler does not implement all operations (8.4). For several, ``operations `a`, `b` `` | ``handler `Half` does not implement operation `warn` `` |
| E0405 | A handler implements the same operation twice (8.4) | ``operation `info` is implemented more than once`` |
| E0406 | An operation of a handler has a different number of parameters from the declaration (8.4) | ``wrong number of parameters: operation `info` takes 1, found 2`` |
| E0407 | An `@external` fn with use omitted, or an `@external suspend fn`, is used outside handlers (12.2). The help is ``= help: declare it with `use {}` if it has no side effects`` (not for a `suspend fn`) | ``external function `console_log` can only be used in handlers`` |
| E0408 | A capability value is passed as an argument (8.2). The help is ``= help: pass a closure that uses the capability instead`` | `a capability cannot be passed as an argument` |
| E0409 | A capability value is stored in a data structure (8.2) | `a capability cannot be stored in a data structure` |
| E0410 | A capability value is returned from a function (8.2) | `a capability cannot be returned from a function` |
| E0411 | The requirements of the handlers in the same with form a cycle (8.3) | ``handlers depend on each other: `Stamped` -> `Ticking` -> `Stamped` `` |
| E0412 | An external function reachable from the entry has no `@external` for the build target (12.1). Reported by `build`, `run`, and `test`, and not by `check` or the LSP. The help is ``= help: add `@external(js, ...)`, or avoid calling it from the entry`` | ``external function `wasm_now` has no `@external` for target `js` `` |
| E0413 | A handler written for Async or Scope (8.7, 11.2) | ``handler `Runtime` cannot implement `Async`: only the runtime provides it`` / ``handler `MyScope` cannot implement `Scope`: only `Fiber.scope` provides it`` |
| E0414 | `use Async` is written (8.7) | `` `Async` has no operations and cannot be used with `use` `` |
| E0415 | Whether an operation of a handler is suspend differs from the effect declaration (8.1, 8.4). The help is ``= help: implement it with `suspend fn sleep(..., resume)` `` when the implementation lacks `suspend`, and ``= help: remove `suspend` `` when it is extra | ``operation `now` is not a `suspend fn` in effect `Clock` `` |
| E0416 | An effect set that is required to be Immediate is not Immediate (9.5). For a set type parameter, the message is ``effect set `R(f)` may suspend, ...``, with the help ``= help: constrain the effect set with `: Immediate` ``. For a callback of an external function, ``effect `Async` may suspend, but callbacks passed to external function `set_timeout` must be `Immediate` `` | ``effect `Clock` may suspend, but `R` of `update` must be `Immediate` `` |
| E0417 | The R of the body that implements an fn operation (not a suspend fn) is not Immediate and contains a call that can suspend (8.1, 9.5). The message depends on the reason: ``operation `sleep` of `Clock` is a `suspend fn` and may suspend, but ...``, ``external function `fetch_text` may suspend, but ...``, ``calling `stamp` may suspend because it uses effect `Clock`, but ...``, and ``this call may suspend because ...`` for an unnamed callee. The help is ``= help: declare `now` as a `suspend fn` in the effect ``, and ``, or call the `fn` operations of `Clock` directly instead of through a function`` is appended only for a function call whose R contains an effect other than Async that can suspend | ``calling `stamp` may suspend because it uses effect `Clock`, but operation `now` is not a `suspend fn` `` |
| E0418 | A handler value is constructed outside the init of that handler (8.4). The help is ``= help: use `with Counter { ... }`; `init` and `release` run there`` | ``handler `Counter` can only be constructed in its own `init` `` |
| E0419 | A handler with fields has no init (8.4). The help is ``= help: add `init { ... }` that returns `NoInit(...)` `` | ``handler `NoInit` has fields but no `init` `` |
| E0420 | release can fail (8.4). The help is ``= help: handle it with `escape`, or turn it into a defect with `panic` `` | ``` `release` of handler `Counter` cannot fail, but this may fail with error `Boom` ``` |
| E0421 | Two handlers in a layer provide the same effect (8.5). The help is ``= help: keep one of them, or override it with `with Loud, Quiet { ... }` ``. When the same handler is listed twice, the message is ``handler `Quiet` appears more than once in layer `Twice` `` and has no help. An effect with type arguments is shown in the form `State[Int]` | ``effect `Logger` is provided by both `Counter` and `Quiet` in layer `Loud` `` |
| E0422 | Layers include each other (8.5) | ``layers include each other: `LoopA` -> `LoopB` -> `LoopA` `` |
| E0423 | The type arguments of an effect with type parameters are not written (`use State`, a use clause, the head of a handler) (8.1). The help is ``= help: write the types, like `State[...]` ``, and ``= help: write the types, like `handler Fixed: State[...]`; a handler cannot be generic`` in the head of a handler | ``missing type arguments for effect `State[S]` `` / ``handler `Fixed` must implement `State[S]` with concrete types`` |
| E0424 | A type parameter of an effect is used as a set (8.1; built-in effects are excluded) | ``type parameter `E` of effect `Raise` is used as a set, but effect type parameters must be ordinary types`` |
| E0425 | The type of the value of `Fiber.scope` contains `Fiber` or `Deferred` (11.8). The note is ``= note: every fiber has ended once `Fiber.scope` returns, so the value would be of no use outside it`` | ``a `Fiber` cannot leave the `Fiber.scope` that started it`` |
| E0426 | Type parameters on an operation of a user-defined effect (8.1) | ``operation `wrap` cannot have type parameters: only built-in effects may declare them`` |
| E0427 | The build target is `c`, and an external function reachable from the entry takes or returns a callback (12.4). The `wasm` target does not use it: an `@external(wasm, ...)` fn is reported with the error of `[target.wasm.external]` instead | ``external function `on_data` takes a callback, which target `c` does not support yet`` |
| E0428 | (Unused, because external functions that are suspend fn in C were allowed in the form of 12.6) | |
| E0429 | The underlying effect of an effect label is a label or a built-in effect (8.9). When the underlying effect is a label, the help is ``= help: write `effect X of Console`; `AuditLog` can still be plugged into it with `with X by AuditLog` `` | ``label `Nested` cannot stand for `Audit`: `Audit` is a label`` |
| E0430 | An effect that is not a label is written in with, or on the left of `by` (8.9). For an item of with, the help is ``= help: list a handler of `Console`, or declare a label with `effect L of Console` `` | ``effect `Log` is not a label: `with` takes a handler, a layer or a label`` / ``only a label can take `by`, but `Log` is not a label`` |
| E0431 | The right side of `by` provides an effect other than the underlying effect of the label (8.9) | ``label `Audit` stands for `Log`, but `OtherH` provides `Other` `` |
| E0432 | `by` is written twice for the same label in the same with (8.9) | ``label `Audit` is given a provider more than once in this `with` `` |
| E0433 | A handler written for a label (8.9). The help is ``= help: implement `Console` instead, and plug it in with `with AuditLog by HandlerA` `` | ``handler `AuditH` cannot implement label `Audit` `` |
| W0401 | A label item in with (`L`, `L by P`) is never used (8.9) | ``label `Audit` is listed here but never used`` |
| E0901 | A path is not found | ``path `app` not found`` |
| E0902 | A file or directory cannot be read | ``cannot read `src/a.emel`: permission denied`` |
| E0903 | Output cannot be written | ``cannot write `target/emela/js/main.mjs` `` |
| E0904 | node is not found or cannot be started | ``cannot start `node` `` |
| E0905 | No JS output yet (only in intermediate stages of 0.20) | `JS backend is not available yet` |
| E0906 | The JS output stage returned invalid output (an internal error of the implementation) | `invalid output from the JS backend` |
| E0907 | cc is not found or cannot be started (Appendix B) | ``cannot start `cc` `` |
| E0908 | Compiling the output C failed (an error in the implementation or in the C of the host) | `` `cc` failed to compile `target/emela/c/main.c` `` |
| E0909 | wasmtime is not found, cannot be started, or is older than 49 with `--target wasm` (`[target.wasm.run]`) | ``` `wasmtime` not found ``` |
