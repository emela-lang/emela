# 5. Data `[data]`

Data is defined in three ways: type (product), enum (sum), and tuple. Declarations, construction, and patterns all take the same `()` form.

## 5.1 The role of brackets `[data.parens]`

| Brackets | Role |
|---|---|
| `( )` | The world of values: function calls, construction, patterns, field declarations, tuples |
| `[ ]` | Type arguments and list literals |
| `{ }` | Blocks and declaration bodies |

There is no index access syntax (`xs[0]`). An element is taken with a function such as `List.get(xs, 0)`, which returns an Option.

## 5.2 Basic types `[data.basic]`

| Type | Contents |
|---|---|
| `Int` | 32-bit signed integer |
| `Int64` | 64-bit signed integer |
| `Float` | 64-bit floating-point number (IEEE 754 binary64) |
| `Bool` | `True` or `False` |
| `String` | A sequence of Unicode scalar values (code points other than surrogates) (Chapter 16) |
| `()` | Unit. Its only value is `()` |
| `Never` | A type with no values. The type of `fail` and `panic` |

## 5.3 type `[data.type]`

```emel
type User(id: Int, name: String)

type Pair[A, B](first: A, second: B)

type Todo(id: Int, title: String, completed: Bool)
```

- A type defines a type and one constructor with the same name. It is constructed and matched in patterns in the same way as an enum with one variant. Unlike an enum, its fields can be accessed with `.`.
- Construction is written `User(id: 1, name: "a")`, and field access is written `user.name`.
- A function of the module that defines a type, whose first parameter has that type, can also be called as `user.f(...)` (`[type.rules]`). Such a function MUST NOT have the name of a field of that type (E0334).
- Fields can be accessed only on values of a type, an error, or a handler with fields. Fields of an enum value cannot be accessed, even when the enum has only one variant. The contents are taken out with match (E0313).
- Types are distinguished by name (nominal). Two types with the same fields but different names are different types.
- There are no anonymous record types.

A value with only some fields changed is made by writing `..expr` at the end of a constructor call. The fields not written are taken from the value after `..`.

```emel
renamed = User(name: "b", ..user)
fixed = Todo(title: "Buy milk", completed: True, ..todo)
```

- This is available only for things with a single constructor (type, error, and handler with fields). It is not available for enum variants (because the type cannot guarantee that the value after `..` is that variant). Writing `..` on an enum variant, a function, a local variable, or any other expression is a compile error (E0222). When the name cannot be resolved, E0222 is not reported in addition.
- The fields to change are written by name. They cannot be written by position (E0223). A field that the constructor does not have is E0312, and writing the same field twice is E0307.
- The type of the expression after `..` MUST be the type of that constructor. The type arguments can differ. Making `Pair(first: "s", ..p)` from `p: Pair[Int, Bool]` gives `Pair[String, Bool]`. It is enough that the types of the inherited fields do not conflict.
- An opaque type cannot be updated outside its defining module either (the same as construction, E0215).
- The linter warns when all fields are written together with `..`, or when only `..` is written with no fields.

## 5.4 enum `[data.enum]`

```emel
enum Shape {
  Circle(radius: Float) # named
  Rect(Float, Float) # positional
  Empty # no contents
}

enum Option[A] {
  Some(A)
  None
}
```

- An enum is a closed sum type. Variants are not types; they are constructors that make values of the parent type. `fn f(c: Circle)` cannot be written.
- The contents of a variant take one of three forms: none, positional, or named. One enum MAY list variants of different forms.
- One variant declaration cannot mix unnamed fields and named fields. If any field has a name, all fields have names (E0128). The rules at the call site (6.2) do not change.
- Variants are separated by newlines or commas.
- There are no general union types. Unions appear only in error sets (Chapter 7).

## 5.5 Tuples `[data.tuple]`

- The type is written `(Int, String)`, the value `(1, "a")`, and the pattern `(n, s)`.
- The tuple with zero elements, `()`, is Unit. There is no tuple with one element; `(x)` in an expression and `(T)` in a type are just parentheses. The pattern `(a)` cannot be written (E0112). The type `(Int,)` and the expression and pattern `(a,)` are also E0112, and the implementation reads them as tuples and continues checking.
- Parentheses in types are used in function types that return functions, to say which `fn` the fails and use clauses belong to. In `fn make() -> (fn(Int) -> String) fails ParseError`, the fails clause belongs to make. Without the parentheses it belongs to the inner `fn`.

## 5.6 const `[data.const]`

```emel
const MAX_RETRY = 3
const DEFAULT_CONFIG = Config(port: 8080)
```

- The name is written as an uppercase name.
- The right-hand side is limited to pure expressions (E and R are empty). `use` and `fail` cannot be written. Function calls can be written (`const DEFAULT_TIMEOUT = Duration.seconds(5)`). Values of opaque types can only be made by functions (4.4), so limiting the right-hand side to expressions that can be evaluated at compile time would make such consts impossible to write.
- The right-hand side is evaluated only once, when it is first referenced, and its value is remembered. Because it is pure, the value does not change. Not all consts are evaluated at startup, so that a defect in an unused const does not stop the program or other tests from running.
- A defect during evaluation happens at the first reference. The message includes the name of the const (`while evaluating const X: division by zero`). Referring to itself during evaluation is a defect (`const A refers to itself while being evaluated`). Consts that form a cycle through names alone (`const A = B` and `const B = A`) are a compile error (E0231).
- A defect during evaluation is also remembered, and the next reference raises the same defect (it is not evaluated again). It is shared between tests too. For a defect in nested consts, the name of each const passed through is prepended (`while evaluating const A: while evaluating const B: division by zero`). The self-reference defect gets no names prepended and is not remembered by any const (so that the message does not change depending on where the first reference came from).
- E0231 looks for cycles only through references that are always evaluated when the value is evaluated. References in the arms of if and match, on the right-hand side of `&&` and `||`, in lambda bodies, and inside called functions are not counted; these are left to the self-reference defect at run time. A cycle through a structure (`const P = (Q, 1)` and `const Q = [P]`) is E0231.
- The type of a const without a written type is generalized. The type of `const EMPTY = []` is `∀ A. List[A]`.
- Written in a pattern, a const matches by comparison with its value (`==`). If the right-hand side consists only of literals, numeric literals with unary minus, constructors, tuples, lists, and other such consts, it is treated as that value when judging exhaustiveness and redundancy. Otherwise, like an arm with a guard, it does not count toward exhaustiveness and does not cover the arms below it (6.5). When a const that cannot be expanded is in a nested position of a pattern (`Some(LIMIT)`), the whole arm is also treated as an arm with a guard. It cannot be written in a binding pattern (E0317).
- A generalized const (`const EMPTY = []`) also remembers only one value, because in 0.20 types are erased at run time. Whether to allow consts with Trait bounds is not decided (18.1 #57).

## 5.7 Type parameters `[data.type-param]`

- Type parameters are declared as `[A, B]` or `[Key, Value]`, and applied as in `List[Int]` and `Map[String, List[User]]`.
- A bound is written `[A: Ord]` (Chapter 10).
- A type constructor itself cannot be passed as a type argument (no higher-kinded types).
