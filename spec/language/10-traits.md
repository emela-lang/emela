# 10. Traits and deriving `[trait]`

A trait is a constraint on types, and a mechanism for checking which functions are available for a type. Behavior that changes with the environment is written with effects, not traits.

## 10.1 trait `[trait.decl]`

```emel
trait Describe {
  fn describe(self) -> String
}

trait Priority: Eq { # requires Eq
  fn compare_priority(self, other: Self) -> Ordering

  fn higher(self, other: Self) -> Self { # default implementation
    Priority.compare_priority(self, other) match {
      Less -> other
      _    -> self
    }
  }
}

trait Decode {
  fn decode(json: Json) -> Self fails DecodeError
}
```

- `self` is the first parameter, and its type is `Self` (the implementing type). Only trait and impl functions can take `self` as a parameter. `self` outside a handler and `Self` outside a trait or impl is a compile error (E0219).
- A trait function whose return type is omitted returns `()`.
- A function can also be written without `self`. The expected return type determines which type's implementation is used.
- A function with a body is a default implementation and can be omitted in an impl.
- `trait A: B` requires that every type implementing A also implements B. B is called a supertrait of A.
- Writing `impl Type: A` checks at that point that an implementation of `Type: B` exists (E0326 if not). For an impl with type parameters, the bounds that the impl of B requires MUST be derivable from the bounds of the impl of A (10.3). `impl Pair[A: Eq, B: Eq]: Eq` for `impl Pair[A: Ord, B: Ord]: Ord` is accepted, because `A: Eq` can be derived from `A: Ord`. When they cannot be derived, E0326 is reported at the head of the impl of A (for `impl Pair[A: Ord, B]: Ord` with `impl Pair[A: Eq, B: Eq]: Eq`, ``type `B` does not implement `Eq` ``).
- A declaration that returns to itself by following supertraits (`trait A: B` and `trait B: A`) is a compile error (E0329). The diagnostic is reported once per cycle, at the name of the trait in the cycle that is defined first, and lists the cycle starting from that trait.

## 10.2 impl `[trait.impl]`

```emel
impl User: Show {
  fn show(self) { "User(#{self.name})" }
}

impl Pair[A: Eq, B: Eq]: Eq {
  fn eq(self, other) { Eq.eq(self.first, other.first) && Eq.eq(self.second, other.second) }
}
```

- An impl is written `impl Type: Trait`. The colon means that the type on the left satisfies the trait on the right (the same as `[A: Ord]`, `trait Ord: Eq` and `handler H: E` (8.4)).
- An impl of a type with type parameters writes the bounds on the type parameters in its head (`impl Map[K: Eq, V: Eq]: Eq`). Inside the impl, the functions of a trait can be used only on values of type parameters that have that bound (E0326 if the bound is not written).
- The head can contain only the name of the type and a list of distinct type parameters. `impl List[Int]: Show` and `impl Pair[A, A]: Eq` cannot be written (E0328). A duplicate type parameter in the head, and a type parameter with the same name as an existing type (the Int in `impl List[Int]: Show`), are E0328, not E0213 or W0201, and the diagnostic points at the whole head. So for one pair of a type and a trait there is only one impl regardless of type arguments, and impls never overlap.
- Writing `impl Show for User` or `impl[A: Eq] Box[A]: Eq` is a compile error (E0144), which shows the forms `impl User: Show` and `impl Box[A: Eq]: Eq`. The diagnostic points from `impl` to the end of the head (before the `{` of the body). The bounds of the leading type parameters are shown moved onto the type parameters of the same name after the type. If the type has no list of type parameters, the leading list is attached as it is (`impl[A: Eq] Box: Eq` is shown as `impl Box[A: Eq]: Eq`). The implementation continues checking with the rewritten head.
- Parameter and return types are determined by the trait declaration, so they are not written. The number of parameters MUST be the same as in the declaration (E0332). Writing an fn with the same name twice in an impl is a compile error (E0213). Inside an impl fn, the type parameters of the impl head can be written, but the type parameters of the trait function itself cannot.
- Every trait method without a default implementation MUST be implemented. If any is missing, it is a compile error (E0325) that lists all the missing methods.
- The primary location of E0325 is the head of the impl (from `impl` to the end of the trait name, as in `impl User: Show`). It does not include the `{` of the body or the whitespace before it. The language server's quickfix (`[lsp.code-action]`) appears when the cursor is in this location.
- For one pair of a type and a trait, there is only one implementation in the whole program (a second one is E0330).
- An impl can be written only in the module that defines the type or in the module that defines the trait (E0331).
- If you own neither, wrap the type in your own type. A type with one field has no run-time cost.

```emel
type MyDate(inner: ThirdParty.Date)

impl MyDate: Encode { ... }
```

## 10.3 Calls and bounds `[trait.call]`

```emel
Show.show(user)
user |> Show.show

fn largest[A: Ord](xs: List[A]) -> Option[A] { ... }
```

- A trait function is called in the form `Trait.function(value)`. There is no method call syntax such as `user.show()`.
- A bound is written `[A: Ord]`. Multiple bounds are written `[A: Ord + Show]`. Supertraits are also written `trait A: B + C`.
- Where a definition with a bounded type parameter is referred to, the compiler checks that the instantiated type satisfies the bound. If not, E0326 is reported at the reference. The same applies when the definition is referred to as a value, not called.
- A bound includes its supertraits. Inside a function with `[A: Ord]`, `==` can be used on values of `A`. There is no need to also write `[A: Ord + Eq]`.
- `==` and `!=` use the implementation of Eq, comparisons use Ord, and string interpolation and inspect use Show. If the type does not satisfy the trait, it is E0326. The diagnostic points at the operator expression for `==` and comparisons, and at the embedded value for interpolation. Function types do not satisfy Eq, so `==` on function values is also a compile error. dbg, the display of an assert failure, and the display of an error that reaches main do not require Show (10.6).
- A function that has no Self in its parameters and has Self only in its return type (`Parse.parse(s)`) selects the impl from the result type. The result type MAY be inferred from how the result is used in the function (Int for `Parse.parse(s) + 1`). If it is not determined within the function, it is a compile error (E0333) that asks for a type annotation (`n: Int = Parse.parse(s)`). No default type is chosen. The diagnostic is reported at the reference. When the result of an fn without a written return type is `Parse.parse(text)` as it is, Self is not generalized to a type parameter and it is also E0333 (an unbounded type parameter cannot select an impl). The same applies when it is only referred to as a value (`f = Parse.parse`).

## 10.4 Errors and effects of trait functions `[trait.effects]`

- Trait functions are pure by default.
- If the declaration has `fails E` or `use R`, implementations can have errors and effects within that range. An implementation cannot go beyond the declared range.
- So the E and R of a function that uses `[A: Decode]` are determined by the trait declaration alone.

## 10.5 derive `[trait.derive]`

```emel
type User(id: Int, name: String, email: Email)
  derive Eq, Show
```

- The compiler generates the implementations of the traits listed in derive from the structure of the type. What it generates are ordinary impls.
- Each field goes through the implementation for that field's own type. If Email has a hand-written Show, it is used.
- Having both a derive and a hand-written impl for the same pair of type and trait is a compile error (E0330). The diagnostic points at the trait name written in derive. Listing the same trait twice in derive is also a compile error (E0330).
- A derived Ord orders variants by their order of declaration, smaller first, and compares the fields of the same variant from first to last.
- If the type of a field does not satisfy the trait, the trait cannot be derived (E0326). The diagnostic points at the trait name written in derive, and failures for the same type are combined into one. Ord cannot be derived for a type with a Float field, so if an order is needed, define it with a hand-written impl (10.6).
- Types other than primitive types, tuples, List and Option satisfy none of Eq, Ord and Show unless a derive or an impl is written. Nothing is derived implicitly.
- derive can be written on an opaque type. A derived Show shows the contents, so if you do not want to show them, use a hand-written impl (for a value of a type that does not satisfy Show, only dbg shows the contents. 10.6).
- Only Eq, Ord and Show can be derived, and the compiler owns the derivation rules. Writing any other trait in derive is a compile error (E0326).
- Deriving for a type with type parameters generates an impl that puts the same bound on every type parameter (`enum Tree[A] { ... } derive Eq` is `impl Tree[A: Eq]: Eq`). The bound is added even if the type parameter does not appear in any field.
- Supertraits are not derived implicitly. `derive Ord` needs a derive or an impl of Eq (E0326).
- If an implementation derived for a non-`pub` type is not required anywhere in the program, W0303 is reported. Requirements are `==`, comparisons, interpolation, passing to a function with a bound, and comparing and displaying fields when deriving for another type. inspect also requires Show. The display of dbg and assert (10.6) does not require it, so it is not counted. For a `pub` type, other modules can use it, so the warning is not reported. Being required as a supertrait counts as a requirement (the Eq in `derive Eq, Ord` is not warned about even if Ord is not used, because removing it makes Ord E0326). A derivation that goes through itself when checking its own fields (a recursive type) is not counted. The warning points at the trait name written in derive. The warning is not reported for the derived Show of an error either, because it can be used to display an error that reaches main or a test (14.3).

Letting trait authors write derivation rules is for a later version (18.1 #3). In 0.20, writing an item that starts with `derive` in the body of a trait is a compile error (E0129). The rules are expected to come in the following three kinds.

| Kind of derivation | Shape | Examples |
|---|---|---|
| Fold | Takes one value and combines the results for each field | Show Encode |
| Comparison | Takes two values and compares them field by field | Eq Ord |
| Construction | Builds a value from the results generated for each field | Decode Default |

The syntax of derivation rules is expected to take the following form, but it is not settled (chapter 18).

```emel
trait Encode {
  fn encode(self) -> Json

  derive fold {
    record(name, fields) { Json.object(fields) }
    variant(name, tag, payload) {
      Json.object([("tag", Json.string(tag)), ("value", payload)])
    }
  }
}
```

## 10.6 Built-in traits `[trait.builtin]`

| Trait | Purpose | User impl |
|---|---|---|
| Eq | `==` `!=` | Allowed |
| Ord | `<` `<=` `>` `>=` | Allowed |
| Show | String interpolation | Allowed |
| Immediate | The effects do not suspend (9.5) | Not allowed (determined automatically) |

The compiler owns the declarations of Eq, Ord and Show. Written in source form, they are as follows. `==` and `!=` call `eq`, `<`, `<=`, `>` and `>=` call `compare`, and interpolation calls `show`.

```emel
trait Eq {
  fn eq(self, other: Self) -> Bool
}

trait Ord: Eq {
  fn compare(self, other: Self) -> Ordering
}

trait Show {
  fn show(self) -> String
}
```

Function types do not satisfy Eq, Ord or Show.

Ord's `compare` is a total order, and it returns `Equal` only when `==` is True. Float cannot keep this promise, so it does not satisfy Ord.

- Float's `<` `<=` `>` `>=` are built-in operators that do not go through Ord, and they follow IEEE 754 comparison (every comparison with `NaN` is False). Comparisons on Int, Int64, String and Bool give the same results as their Ord implementations.
- Comparisons inside generic functions go through Ord. Float cannot be passed to a type parameter with `[A: Ord]`, so the same `<` never gives different answers inside and outside a generic function.
- When a total order on Float is needed, pass `Float.compare` as a function (`List.sort_by(xs, Float.compare)`). It makes `NaN` the largest, and treats two `NaN`s, and `0.0` and `-0.0`, as Equal.

The traits of primitive types, tuples, List and Option are as follows.

- Int, Int64, String and Bool satisfy Eq, Ord and Show. Float satisfies Eq and Show, and does not satisfy Ord.
- Tuples, List and Option satisfy a trait when their element types satisfy it (Eq of `List[A]` requires `A: Eq`). Ord compares lexicographically.
- The compiler owns the implementations for primitive types and tuples. The implementations for List and Option are written as impls in the core source (`impl List[A: Eq]: Eq`).
- Comparing and displaying a type that consists only of derived implementations MAY be executed by a process that walks the structure of the value instead of calling the generated impls. The result is the same.
- Bool's Show is `True` / `False` (the constructor names).
- The digits of Float's Show are the shortest sequence of decimal digits d₁d₂…dₖ that reads back into binary64 as the original value. If several candidates have the same length, the one closest to the original value is chosen. Writing the value as 0.d₁d₂…dₖ × 10ⁿ, decimal-point notation is used when -6 < n ≤ 21, and exponent notation is used otherwise (d₁.d₂…dₖ followed by `e` and the signed n − 1, with no decimal point when k is 1). Finite integral values get `.0` (`1.0`). Other values take forms such as `1e+21`, `-2.5e-8`, `Infinity` and `NaN`. WASM produces the same strings.
- Int64's Show has no suffix.
- String's Ord is the lexicographic order of Unicode code points (not the order of the encoding. 16.2).
- Float's Eq follows IEEE 754 comparison. `NaN` is not equal even to itself, and `0.0 == -0.0`.
- Float's `-0.0` is displayed as `0.0`.

A derived Show represents a value in the form that constructs it in source (`Circle(radius: 1.0)`, `Rect(1.0, 2.0)`, `Empty`, `(1, "a")`, `[1, 2, 3]`, `()`).

- Only a top-level String is embedded without quotes in string interpolation. A String inside a list or a constructor, and a String in inspect, are enclosed in `"`.
- When enclosed, it escapes with `\"`, `\\`, `\n`, `\t`, `\r`, and `\#` before `#{`. Other control characters (U+0000 to U+001F, U+007F) are written as `\u{...}`.

A value can be displayed only when its type satisfies Show. The debug display `inspect[A: Show](value: A) -> String` (Prelude, 15.1) also requires Show and returns the result of that implementation, except that a String is enclosed in `"` even at the top level. Generic test support (such as counterexamples of property checks) also requires `[A: Show]` when it shows values.

dbg (15.1) is the only exception: it does not require Show and shows a value of any type.

- A type that satisfies Show is displayed the same as with inspect.
- A type that does not satisfy Show is represented in the same constructing form as a derived Show. Where an inner value's type satisfies Show, that part uses the Show implementation. The contents of opaque types are also shown.
- A function value is shown as `<fn>`.
- A value of an `@external type` is shown as `<external Name>`, where `Name` is the name of the type, on every target. Its contents belong to the host and are not shown.
- This constructing form is only written to standard error, and a program has no way to receive it as a string. Strings shown to users are made with interpolation and Show.

## 10.7 What traits do not have `[trait.not]`

There are no associated types, traits over higher-kinded types, run-time dynamic dispatch, traits corresponding to operators, or user-defined automatically determined traits.
