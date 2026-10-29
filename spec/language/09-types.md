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
fn(A, B) -> C fails E use R
```

In a function type written in a parameter position, an omitted clause means "anything, and it propagates to the caller".

| Form | Functions accepted |
|---|---|
| `fn(A) -> B` | Functions with any errors and effects. They are added to the E and R of the side that calls the received function |
| `fn(A) -> B fails {}` | Only functions that do not fail |
| `fn(A) -> B use {}` | Only functions without effects |
| `fn(A) -> B fails {} use {}` | Only pure functions |
| `fn(A) -> B use R` (R is a type parameter) | Names the effects so that constraints can be put on them (9.5) |

By this rule, `List.map(xs, f)` passes on the errors and effects of f as they are. There is no need to provide both a pure version and an effectful version.

The meaning of an omitted clause in a function type depends on where the type is written.

| Position | Meaning when omitted |
|---|---|
| Inside a parameter type (at any depth: `f: fn(A) -> B`, `fs: List[fn(A) -> B]`) | Anything. A hidden set variable is placed for each omitted clause. Its errors and effects are added to the side that calls the function |
| Return type, type annotation of a binding, type annotation of an anonymous function's parameter | Inferred from the body. It can refer to hidden set variables of the parameters (anonymous functions are not generalized, so they cannot place hidden set variables) |
| Inside a declaration (fields of type, enum, error, and handler; effect operations; Trait functions; const) | Empty. Only functions that do not fail and have no effects |

To hold an effectful function inside a declaration, make it explicit with type parameters.

```emel
type Handler[E, R](run: fn(Request) -> Response fails E use R)

fn compose[A, B, C](f: fn(A) -> B, g: fn(B) -> C) -> fn(A) -> C {
  fn(x) { g(f(x)) } # the returned function is inferred to have the errors and effects of f and g
}
```

- Inside declarations, an omitted clause is empty so that no set variable is created that does not appear in the type's name. That `Handler` can have effects is visible in its type parameters, as `Handler[E, R]`.
- Whether a type parameter is an ordinary type, an error set, or an effect set is inferred from the position where it is used (`fails E`, `use R`). There is no syntax for writing kinds. A type parameter passed to a set type parameter position of a named type (E in `type Server[E](inner: Handler[E, Db])`, E and R in `fn serve[E, R](h: Handler[E, R])`) and a type parameter in a set type parameter position of a built-in effect written in a use clause (`use Scope[E, R]`) are also sets.
- A set can be passed to a set type parameter of a named type. The notation matches the fails clause (`A | B`) and the use clause (`{ A, B }`): `Handler[NotFound | Timeout, { Db, Log }]`, `Handler[{}, {}]`, `Handler[E | Timeout, { R, Log }]`.
- A single element can be written without enclosing it (`Handler[NotFound, Db]`). Syntactically, it is read as a type. An empty `{}` can also be written in an error set position (the same as `fails {}`).
- Which type parameters accept sets is not decided by the grammar but checked by type checking. A set of the wrong kind or an element that is not a set is a compile error (E0323), and a set in a type parameter position that is not for sets (`List[{}]`) is a compile error (E0324).

## 9.3 Annotations and inference in declarations `[type.inference]`

| Part | When omitted | When written |
|---|---|---|
| Return type | T and E are inferred | T is checked, and E is enforced to match the fails clause (7.4) |
| fails clause | "No errors" if there is a return type | Enforces exactly that set |
| use clause | R is always inferred (even when a return type is written) | Enforces exactly that set. The effects of calling parameter functions (hidden set variables) are added (as for E in 7.4) |

E and R usually do not appear in source. The LSP displays them.

An effect not included in the declared R is a compile error (E0402). The diagnostic points, for each effect, at the first expression (`use` or a call) that requires it. The first expression is determined in the same way as for E0318 (7.4).

Definitions whose types are inferred (a fn without a return type, a fn whose return type contains a function type with omitted clauses, a fn without a use clause, a const without a type) are inferred for each group of mutually referencing definitions, starting from the groups that are referenced.

- Within a group, references use monomorphic types, and the group is generalized after its inference finishes. Using the same definition at different types within a group is a type mismatch (there is no polymorphic recursion). For a fn without a use clause, T and E are as declared, and only R follows this rule.
- When generalizing, set variables in the E and R of parameter function types that have neither a lower bound nor an upper bound are quantified as set type parameters (`fn caller() { fn(g) { g() } }` is `∀ A, E. fn() -> fn(fn() -> A fails E) -> A fails E`). Other set variables are fixed to the smallest set that satisfies the constraints.

Types are displayed as follows.

- Generalized type parameters are named `A`, `B`, …, `Z` in order of appearance, and then `T26`, `T27`, …, and constraints are written on the quantifier in the form `∀ A: Ord + Show, B. T`.
- Generalized set type parameters are named `E`, `E1`, … (errors) and `R`, `R1`, … (effects), and are listed after ordinary type parameters. A hidden set in a parameter type is displayed by the parameter name as `E(f)` and `R(f)`, and the second and later ones of the same parameter as `E(f, 2)`.
- Empty E and R are omitted. However, inside the parameter types of a function type, omitting them would read as "anything", so `fails {}` and `use {}` are written.
- An effect set is written as is when it has one element, as `{ A, B }` when it has two or more, and as `{}` when it is empty, with effects first and set type parameters after them. The `use` clause is written after the `fails` clause.
- If the return type of a function type with outer clauses is a function type, the return type is enclosed in parentheses (5.5). The type of a capability value is displayed as `use Logger`.
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
| `e escape { Ti -> ei }` | The common type of the type of e and the types of the arms (Never disappears in the join) | (Ee ∖ {Ti}) ∪ Eei (Ti are the tags whose arm accepts every value. They are not removed from set variables. 7.3) | Re ∪ Rei |
| `use X` | The capability of X | {} | { X } |
| `with H { e }` | The type of e | Ee ∪ (E of init) ∪ (E of the arguments of init) | (Re ∖ provides(H)) ∪ requires(H) ∪ (R of the arguments of init) |
| `fn(x) { e }` | `fn(..) -> T fails Ee use Re` | {} | {} |
| `if c { a } else { b }` | The common type of a and b | Ec ∪ Ea ∪ Eb | Rc ∪ Ra ∪ Rb |
| `e match { pi -> ei }` | The common type of the arms | Ee ∪ Eei | Re ∪ Rei |

- An anonymous function encloses the E and R of its body in its function type. They surface when the function is called.
- Never can form a common type with any type. Even if one branch of an if or a match is a fail, the type of the whole is the type of the other branch.
- When there is no expected type and the types of the branches do not match, the type of the first branch is reported as the expected type and the type of a later branch as the actual type (E0301), and checking continues with the type of the first branch as the type of the whole.
- For a field access `x.name`, the type of x MUST already be determined to be a named type when checking reaches it. Otherwise it is a compile error (E0314). The type is not worked out backward from the field name (there is no row polymorphism).
- A method call `x.f(a1, ..., an)` applies when the type of x has a named type T at its head and x is not a capability value. f is looked up among: (1) the fields of T; (2) the fns of the module that defines T whose first parameter has T at the head of its type (only pub fns outside that module), without looking at the scope; and (3) the functions that take `self` of the traits that T implements, among the traits visible by name in the calling module (the traits of the Prelude, the traits imported by name in braces, and the traits defined in that module; a trait reached only as `L.Trait` is not looked up). When x has a type parameter A, only the functions of the bounds of A are looked up. A field is called as a value of function type. A function is called as `M.f(x, a1, ..., an)`, with the same named arguments, order of evaluation, E and R. Only the head of the type of x is needed: the type arguments are checked as for the first argument of that call.
- When more than one candidate is found, it is a compile error (E0335). A field and a function of the module cannot both be found, because declaring them is E0334. When none is found, it is E0312 (E0313 for a type without fields).
- `x.f` without a call, where f is a function found by (2) or (3) and not a field, is E0312 with ``= help: call it as `x.f(...)` ``. It does not make a function value.
- The head of the type of x MUST already be determined when checking reaches the method call (E0314), as for a field access.
- The arguments of a call are checked in parameter order. At a position with an expected type (an argument, the return type, the type annotation of a binding), that type is passed first to the last expression of a block, each branch of if and match, list elements, and the parameters and body of an anonymous function. In `List.map(users, fn(u) { u.name })`, the type of u is determined from users, which is checked first.
- Never is treated as another type only at branch joins and at the top level of a position with an expected type. `List[Never]` and `List[Int]` do not match.
- provides(H) is the effects that H implements, and requires(H) is the union of the Rs of the init, the release, and the operation bodies of H. The provision and requirements of label items (`L`, `L by P`) and the provision of `default` labels are defined in 8.9.
- The E of a function value only needs to be contained in the E of the expected function type. A function with a smaller E can be used as a function with a larger E. When functions with different Es are put together at a branch join or as list elements, the E is their union. The direction of containment is reversed inside the parameters of function types, and goes both ways (equality) inside the type arguments of named types.
- When a function is passed to a parameter whose function type has two or more set type parameters in its fails (`fails E1 | E2`), the errors of the passed function that are not among the written-out tags go into the first set type parameter. This is an approximation, and later checks can report a compile error. The same applies to the use clause (`use { Quiet, R1, R2 }`).
- E is represented as a union of tags and set variables. `∖ {Ti}` acts only on the tag part and leaves set variables as they are. Even if a set variable is later determined to be a concrete set, tags are not removed from it retroactively (an approximation that errs on the safe side).

## 9.5 Constraints on effects `[type.effect-bound]`

Effect sets can be written as type parameters, and constraints can be put on them with the same syntax as for Traits.

```emel
@external(js, "./dom.mjs", "onClick")
fn on_click[R: Immediate](id: String, handler: fn() -> () use R) -> ()
```

Immediate is a built-in constraint that means "does not suspend, and completes on the spot when called". The compiler determines it automatically from declarations, and users never write implementations of it.

- An effect is Immediate when it has no suspend fn and is not Async.
- An effect set is Immediate when all its elements are Immediate. The empty set is Immediate.

Only the places where suspension would cause problems (such as callbacks passed to external functions) require this constraint. Instead of marking call sites, type checking forbids suspension there.

- Immediate can be put only on type parameters written in a use clause (effect sets). Putting it on an ordinary type parameter or an error set type parameter is a compile error (E0322). Trait constraints cannot be put on effect set type parameters (in 0.20, this produces a "not supported yet" diagnostic).
- An `R: Immediate` constraint is checked against the instantiated set where the definition (a fn or a constructor) is referenced (E0416). The diagnostic is reported at the reference (the name of the callee), and the missing elements in the same reference are combined into one diagnostic. It is not checked in type annotations (`Ticker[Clock]`), because values can be made only by constructors.
- A hidden effect set in a parameter type has no Immediate constraint, so it cannot be passed to an `R: Immediate` parameter. When the constraint is needed, write `: Immediate` on a named type parameter.
- A hidden effect set in a parameter type of an external function has the Immediate constraint. A use clause written in a function type of a parameter of an external function declaration is a compile error unless it is Immediate.
- In a definition whose type is inferred, when a set variable passed to a position that requires Immediate is generalized, that type parameter gets the Immediate constraint.
- Whether an effect is Immediate is determined only by its declaration and does not depend on handlers. In the implementation of a fn operation, the body's R is Immediate or the body contains no call that may suspend (8.1, E0417), so a call of a fn operation does not suspend whichever handler it reaches, and an operation of an Immediate effect never suspends.

## 9.6 What the type system does not have `[type.not]`

There is no subtyping (except the containment of E in 9.4), no general union or intersection types, no higher-kinded types, no null, no implicit type conversion, and no dynamic typing.
