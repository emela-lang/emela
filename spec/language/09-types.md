# 9. The type and effect system `[type]`

Every expression has a triple: the value type T, the error set E, and the effect set R. T is inferred in the Hindley-Milner style, and E and R are computed with set union and difference.

## 9.1 Form of judgments `[type.judgment]`

```
Γ ⊢ e : T ! E ; R
```

- T: the type of the value.
- E: the set of error tags. If it is empty, the expression does not fail.
- R: the set of effects. If it is empty, the expression does not depend on the environment.
- A function whose E and R are both empty is called a pure function.

## 9.2 Function types `[type.fn]`

```
fn(A, B) -> C fails E uses { R }
```

In a function type written in a parameter position, an omitted clause means "anything, and it propagates to the caller".

| Form | Functions accepted |
|---|---|
| `fn(A) -> B` | Functions with any errors and effects. They are added to the E and R of the side that calls the received function |
| `fn(A) -> B fails {}` | Only functions that do not fail |
| `fn(A) -> B uses {}` | Only functions without effects |
| `fn(A) -> B fails {} uses {}` | Only pure functions |
| `fn(A) -> B uses { R }` (R is a type parameter) | Names the effects so that constraints can be put on them (9.5) |

By this rule, `List.map(xs, f)` passes on the errors and effects of f as they are. There is no need to provide both a pure version and an effectful version.

The meaning of an omitted clause in a function type depends on where the type is written.

| Position | Meaning when omitted |
|---|---|
| Inside a parameter type (at any depth: `f: fn(A) -> B`, `fs: List[fn(A) -> B]`) | Anything. A hidden set variable is placed for each omitted clause. Its errors and effects are added to the side that calls the function |
| Return type, type annotation of a binding, type annotation of an anonymous function's parameter | Inferred from the body. It can refer to hidden set variables of the parameters (anonymous functions are not generalized, so they cannot place hidden set variables) |
| Inside a declaration (fields of type, enum, error, and handler; effect operations; Trait functions; const) | Empty. Only functions that do not fail and have no effects |

To hold an effectful function inside a declaration, make it explicit with type parameters.

```emel
type Handler[E, R](run: fn(Request) -> Response fails E uses { R })

fn compose[A, B, C](f: fn(A) -> B, g: fn(B) -> C) -> fn(A) -> C {
  |x| g(f(x)) # the returned function is inferred to have the errors and effects of f and g
}
```

- Inside declarations, an omitted clause is empty so that no set variable is created that does not appear in the type's name. That `Handler` can have effects is visible in its type parameters, as `Handler[E, R]`.
- Whether a type parameter is an ordinary type, an error set, or an effect set is inferred from the position where it is used (`fails E`, `uses { R }`). There is no syntax for writing kinds. A type parameter passed to a set type parameter position of a named type (E in `type Server[E](inner: Handler[E, Db])`, E and R in `fn serve[E, R](h: Handler[E, R])`) and a type parameter in a set type parameter position of a built-in effect written in a uses clause (`uses { TaskGroup[E, R] }`) are also sets.
- The `uses` of a function type can contain capabilities as well as effects (`fn() -> Rows uses { primary }`, 9.4). They are inferred and shown by the language server, and are not written in source.
- A set can be passed to a set type parameter of a named type. The notation matches the fails clause (`A | B`) and the uses clause (`{ A, B }`): `Handler[NotFound | Timeout, { Db, Log }]`, `Handler[{}, {}]`, `Handler[E | Timeout, { R, Log }]`.
- A single element can be written without enclosing it (`Handler[NotFound, Db]`). Syntactically, it is read as a type. An empty `{}` can also be written in an error set position (the same as `fails {}`).
- Which type parameters accept sets is not decided by the grammar but checked by type checking. A set of the wrong kind or an element that is not a set is a compile error (E0323), and a set in a type parameter position that is not for sets (`List[{}]`) is a compile error (E0324).

## 9.3 Annotations and inference in declarations `[type.inference]`

| Part | When omitted | When written |
|---|---|---|
| Return type | T and E are inferred | T is checked, and E is enforced to match the fails clause (7.4) |
| fails clause | "No errors" if there is a return type | Enforces exactly that set |
| uses clause | R is always inferred (even when a return type is written) | Enforces exactly that set. The effects of calling parameter functions (hidden set variables) are added (as for E in 7.4), and so are the sets of the capabilities in the parameter types (9.4) |

A local fn (6.9) follows the same table. Its E and R are inferred as part of the enclosing body, and its type is not generalized.

E and R usually do not appear in source. The LSP displays them.

An effect not included in the declared R is a compile error (E0402). The diagnostic points, for each effect, at the first expression (`use` or a call) that requires it. The first expression is determined in the same way as for E0318 (7.4).

Definitions whose types are inferred (a fn without a return type, a fn whose return type contains a function type with omitted clauses, a fn without a uses clause, a const without a type) are inferred for each group of mutually referencing definitions, starting from the groups that are referenced.

- Within a group, references use monomorphic types, and the group is generalized after its inference finishes. Using the same definition at different types within a group is a type mismatch (there is no polymorphic recursion). For a fn without a uses clause, T and E are as declared, and only R follows this rule.
- When generalizing, set variables in the E and R of parameter function types that have neither a lower bound nor an upper bound are quantified as set type parameters (`fn caller() { |g| g() }` is `∀ A, E. fn() -> fn(fn() -> A fails E) -> A fails E`). Other set variables are fixed to the smallest set that satisfies the constraints.

Types are displayed as follows.

- Generalized type parameters are named `A`, `B`, …, `Z` in order of appearance, and then `T26`, `T27`, …, and constraints are written on the quantifier in the form `∀ A: Ord + Show, B. T`.
- Generalized set type parameters are named `E`, `E1`, … (errors) and `R`, `R1`, … (effects), and are listed after ordinary type parameters. A hidden set in a parameter type is displayed by the parameter name as `E(f)` and `R(f)`, and the second and later ones of the same parameter as `E(f, 2)`.
- Empty E and R are omitted. However, inside the parameter types of a function type, omitting them would read as "anything", so `fails {}` and `uses {}` are written.
- An effect set in a uses clause is always written with braces (`uses { A }`, `uses { A, B }`, `uses {}`), with effects first and set type parameters after them. In a set type argument of a named type, a set of one element is written as is (`Handler[E, Db]`). The `uses` clause is written after the `fails` clause.
- If the return type of a function type with outer clauses is a function type, the return type is enclosed in parentheses (5.5). The type of a capability value is displayed as `use Logger`. `use` here names one capability value, and `uses` names a set; the two are not interchangeable.
- Diagnostics for type mismatches do not show set variables that are not yet determined.
- A type that could not be checked (a type that could not be read, or a type whose name could not be resolved) is displayed as `{unknown}`. This type is treated as matching every type, so that errors do not pile up.

## 9.4 Typing rules `[type.rules]`

The T, E, and R of each expression are determined as follows. Unless stated otherwise, the E and R of subexpressions flow into the parent expression.

| Expression | T | E | R |
|---|---|---|---|
| Literal, variable, const | Its type | {} | {} |
| `[e1, ..., en]` / `[e1, ..., en, ..r]` | `List[T]` (each ei is T, and r is `List[T]`) | E1 ∪ … ∪ En ∪ Er | R1 ∪ … ∪ Rn ∪ Rr |
| `f(a)` | The return type of f | Ef ∪ Ea | Rf ∪ Ra |
| `x.f(a)` (method call) | The return type of the function found, called as `M.f(x, a)` | Ex ∪ Ef ∪ Ea | Rx ∪ Rf ∪ Ra |
| `C(fi: ei, ..r)` | The type of C (type arguments are determined from ei and the fields carried over from r) | Ei ∪ Er | Ri ∪ Rr |
| `fail e` | Never | { the tag of e } | {} |
| `e escape { Ti -> ei }` | The common type of the type of e and the types of the arms (Never disappears in the join) | (Ee ∖ {Ti}) ∪ Eei (Ti are the tags whose arm accepts every value. They are not removed from set variables. When there is an arm `_`, the E is Eei alone. 7.3) | Re ∪ Rei |
| `use X` | `use X`, a capability of the name X (0.29 only, 8.2) | {} | { X } |
| `with H { e }` | The type of e | Ee ∪ (E of init) ∪ (E of the arguments of init) | (Re ∖ provides(H) ∖ the capabilities of the items) ∪ requires(H) ∪ (R of the arguments of init) |
| `with x = E.M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | Re ∪ { E } ∪ Ra |
| `with x = c.M(a) { e }` | The type of e | Ee ∪ Ea ∪ (E of the init of M) | Re ∪ { c } ∪ Ra |
| `E.M(a)`, `c.M(a)` inside an init | `use F` (F is the effect of M) | Ea ∪ (E of the init of M) | { E } or { c }, ∪ Ra |
| `\|x\| e` | `fn(..) -> T fails Ee uses { Re }`, where Re also has the capabilities in the types of the captured values | {} | {} |
| `fn f(x: A) -> T { e }` (local fn, a statement) | Binds f to `fn(A) -> T fails Ee uses { Re }` | {} | {} |
| `if c { a } else { b }` | The common type of a and b | Ec ∪ Ea ∪ Eb | Rc ∪ Ra ∪ Rb |
| `e match { pi -> ei }` | The common type of the arms | Ee ∪ Eei | Re ∪ Rei |

- A call `f(a)` adds to R the elements of the R of f, with the set type parameter of each capability parameter replaced by the set of the corresponding argument.
- An anonymous function and a local fn enclose the E and R of their bodies in their function types. They surface when the function is called. The R of the function is provided where the function is called, not where it is made. A reference to a local fn inside its own body has the type being inferred, and is not generalized.
- Never can form a common type with any type. Even if one branch of an if or a match is a fail, the type of the whole is the type of the other branch.
- When there is no expected type and the types of the branches do not match, the type of the first branch is reported as the expected type and the type of a later branch as the actual type (E0301), and checking continues with the type of the first branch as the type of the whole.
- For a field access `x.name`, the type of x MUST already be determined to be a named type when checking reaches it. Otherwise it is a compile error (E0314). The type is not worked out backward from the field name (there is no row polymorphism).
- A method call `x.f(a1, ..., an)` applies when the type of x has a named type T at its head and x is not a capability value. f is looked up among: (1) the fields of T; (2) the fns of the module that defines T whose first parameter has T at the head of its type (only pub fns outside that module), without looking at the scope; and (3) the functions that take `self` of the traits that T implements, among the traits visible by name in the calling module (the traits of the Prelude, the traits imported by name in braces, and the traits defined in that module; a trait reached only as `L.Trait` is not looked up). When x has a type parameter A, only the functions of the bounds of A are looked up. A field is called as a value of function type. A function is called as `M.f(x, a1, ..., an)`, with the same named arguments, order of evaluation, E and R. Only the head of the type of x is needed: the type arguments are checked as for the first argument of that call.
- When more than one candidate is found, it is a compile error (E0335). A field and a function of the module cannot both be found, because declaring them is E0334. When none is found, it is E0312 (E0313 for a type without fields).
- `x.f` without a call, where f is a function found by (2) or (3) and not a field, is E0312 with ``= help: call it as `x.f(...)` ``. It does not make a function value.
- The head of the type of x MUST already be determined when checking reaches the method call (E0314), as for a field access.
- The arguments of a call are checked in parameter order. At a position with an expected type (an argument, the return type, the type annotation of a binding), that type is passed first to the last expression of a block, each branch of if and match, list elements, and the parameters and body of an anonymous function. In `List.map(users, |u| u.name)`, the type of u is determined from users, which is checked first.
- Never is treated as another type only at branch joins and at the top level of a position with an expected type. `List[Never]` and `List[Int]` do not match.
- provides(H) is all the effects that H implements, and requires(H) is the union of the Rs of the init, the release, and the operation bodies of H. The provision and requirements of label items (`L`, `L by P`) and the provision of `default` labels are defined in 8.9. For a handler with type parameters, provides(H) and requires(H) are instantiated with the type arguments of the item (`[effect.with]`).
- The E and the R of a function value only need to be contained in the E and the R of the expected function type. A function with a smaller E or R can be used as a function with a larger E or R. When functions with different Es or Rs are put together at a branch join or as list elements, the E and the R are their unions. The direction of containment is reversed inside the parameters of function types, is kept inside the elements of lists and tuples, and goes both ways (equality) inside the type arguments of named types.
- When a function is passed to a parameter whose function type has two or more set type parameters in its fails (`fails E1 | E2`), the errors of the passed function that are not among the written-out tags go into the first set type parameter. This is an approximation, and later checks can report a compile error. The same applies to the uses clause (`uses { Quiet, R1, R2 }`).
- E is represented as a union of tags and set variables. `∖ {Ti}` acts only on the tag part and leaves set variables as they are. Even if a set variable is later determined to be a concrete set, tags are not removed from it retroactively (an approximation that errs on the safe side).

Capabilities (8.2, 8.3) are told apart in types and in R.

- The elements of R are effects, declared labels and capabilities. Calling an operation of a capability adds that capability to R, and the with that made the capability removes it. A capability is not widened to its effect: an effect set that contains a capability is not included in an effect set that contains only the effect of the capability.
- A capability type `use E` records a set of capabilities of E. The set is not written in source. Unification takes the union of sets, as for R; there is no general subtyping. Calling an operation of a value whose type has a set adds every capability of the set to R.
- A parameter of type `use E` gives the function a type parameter that ranges over sets of capabilities of E, decided at each call by the argument. Two parameters give two type parameters. The same holds for a capability-bound type of E written in a parameter type (`task: Task[A]`, `ts: List[Task[A]]`). The result of a function can be a capability type; its set comes from the parameters.
- The set of a capability type `use E` or a capability-bound type of E written in the result type of a function is the union of the sets of every capability of E in the parameter types (including those inside other types, `acc: List[use Db]`, and the hidden sets of fields). When no parameter has one, it is the name E. The set is decided by the signature, not by the body.
- A capability type or a capability-bound type written in a field of a type, enum or error declaration (`type Pair(read: use Db, write: use Db)`, `error Leak(f: Task[Int])`) gives the declaration a hidden set type parameter for each place it is written. It is decided when a value is constructed, as a type argument is. An error records it in its tag in E, so an error whose field records a capability cannot leave the with that made the capability (8.6). The language server shows such a set as `C(read)`, by the name of the field.
- `use E` or a capability-bound type written as a parameter of a function type (`fn(use ReadFile) -> String`, `fn(Task[Int]) -> ()`) is quantified at that function type. A closure checked against such a type receives an unknown set, which cannot be in its result. The set may be in its R, and is removed on the side that calls the closure. A closure without an expected type is not quantified: the set of its parameter is an ordinary set variable decided by its uses. A quantified type is never inferred; it is only checked against a written type.
- If the type of a value that a closure or a local fn captures contains capabilities, including through capability-bound types, those capabilities are in the R of the function.
- A type declaration or an enum declaration may have `uses E` after its fields, before `derive` (a capability-bound type, `@external pub type Task[A] uses TaskGroup`, `enum Handle uses Files { ... }`): its values live only while a capability of E does. An error declaration cannot have it. When an operation of a capability c of E returns a value of such a type, the type of the value records c; an operation called through the name of E records the name E. A value of such a type is treated as containing the capability.
- A value of a capability-bound type of E can be constructed only inside a handler that implements E: in its init, its release and its operation bodies, including the anonymous functions inside them (E0458). The value records `self`, the capability of the handler (8.4). A constructor pattern does not make a value, so it can be written anywhere. An `@external` type cannot be constructed in Emela.
- In 0.29, `use E` written as an expression is a capability of the name E in types (W0403). A with that provides E by name counts the name E in types as its own capability (8.6).

## 9.5 Constraints on effects `[type.effect-bound]`

Effect sets can be written as type parameters, and constraints can be put on them with the same syntax as for Traits.

```emel
@external(js, "./dom.mjs", "onClick")
fn on_click[R: Immediate](id: String, handler: fn() -> () uses { R }) -> ()
```

Immediate is a built-in constraint that means "does not suspend, and completes on the spot when called". The compiler determines it automatically from declarations, and users never write implementations of it.

- An effect is Immediate when it has no suspend fn, no suspend handler member, and is not Async.
- An effect set is Immediate when all its elements are Immediate. The empty set is Immediate.

Only the places where suspension would cause problems (such as callbacks passed to external functions) require this constraint. Instead of marking call sites, type checking forbids suspension there.

- Immediate can be put only on type parameters written in a uses clause (effect sets). Putting it on an ordinary type parameter or an error set type parameter is a compile error (E0322). Trait constraints cannot be put on effect set type parameters (in 0.20, this produces a "not supported yet" diagnostic).
- An `R: Immediate` constraint is checked against the instantiated set where the definition (a fn or a constructor) is referenced (E0416). The diagnostic is reported at the reference (the name of the callee), and the missing elements in the same reference are combined into one diagnostic. It is not checked in type annotations (`Ticker[Clock]`), because values can be made only by constructors.
- A hidden effect set in a parameter type has no Immediate constraint, so it cannot be passed to an `R: Immediate` parameter. When the constraint is needed, write `: Immediate` on a named type parameter.
- A hidden effect set in a parameter type of an external function has the Immediate constraint. A uses clause written in a function type of a parameter of an external function declaration is a compile error unless it is Immediate.
- In a definition whose type is inferred, when a set variable passed to a position that requires Immediate is generalized, that type parameter gets the Immediate constraint.
- Whether an effect is Immediate is determined only by its declaration and does not depend on handlers. In the implementation of a fn operation, and in the init and the release of a handler that implements a handler member without `suspend`, the R is Immediate or the body contains no call that may suspend (8.1, E0417), so a call of a fn operation, or opening a handler member without `suspend`, does not suspend whichever handler it reaches, and an operation or a handler member of an Immediate effect never suspends.

## 9.6 What the type system does not have `[type.not]`

There is no subtyping except the containment of E and R in 9.4, which is decided by type checking and inference; there is no syntax for stating it. There are no general union or intersection types, no higher-kinded types, no null, no implicit type conversion, and no dynamic typing.
