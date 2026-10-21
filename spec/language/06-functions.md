# 6. Functions and expressions `[fn]`

There is one form of call, `name(arguments)`, and functions and constructors follow the same rules. Every syntactic element is an expression; the only statement is a binding.

## 6.1 Function declarations `[fn.decl]`

```emel
fn add(x: Int, y: Int) -> Int {
  x + y
}

fn map[A, B](xs: List[A], f: fn(A) -> B) -> List[B] { ... }

pub fn find(id: Int) -> User fails NotFound use Users { ... }
```

- In a top-level fn, the parameter types are written (omitting them is a compile error (E0316)). In an anonymous function they can be omitted. The return type, `fails`, and `use` can be omitted. The meaning when they are omitted is defined in 7.4 and 9.3.
- The body is a block, and the value of the last expression is the return value. There is no `return`.
- Anonymous functions have one form only, `fn(x) { x + 1 }`, with no shorthand. Parameter types can be omitted.
- A parameter of an anonymous function can be a pattern instead of a name (`fn((key, value)) { ... }`, `fn(acc, _) { acc }`, `fn(Request(url:, ..)) { url }`). A type, when written, comes after the pattern (`fn((a, b): (Int, Int)) { a + b }`).
  - A parameter pattern is limited, as for bindings, to patterns that always match (6.3, E0317).
  - `fn(p) { body }` means the same as `fn(t) { p = t; body }`. Parameters are matched from left to right. Binding the same name in two parameters is a compile error (E0220).
  - The parameters of top-level fns, fns in impls and handlers, and operations of traits and effects are limited to names. This is because a parameter name also serves as the label of a named argument (6.2) and as the link to the declaration. Values of anonymous functions have no parameter names and are called by position only (6.2), so making a parameter a pattern loses nothing.

- A top-level fn and a constructor with fields become function values when their names are written without calling them (`List.map(xs, Some)`).

## 6.2 Arguments and argument names `[fn.args]`

The name of a parameter is the argument name written at the call.

```emel
fn connect(host: String, port: Int) { ... }

connect("localhost", 5432)
connect("localhost", port: 5432)
connect(host: "localhost", port: 5432)
```

- Positional arguments come first and named arguments after.
- When the variable name and the argument name are the same, it can be shortened to `NotFound(id:)`. The same holds in patterns: `User(name:)` binds `name`.
- Constructors with named fields follow the same rule, so both `Circle(1.0)` and `Circle(radius: 1.0)` can be written.
- Parameter names are part of the public API, and renaming one is a breaking change.
- Named arguments can be used only when calling an fn or a constructor referred to by name, and when calling an operation on a capability value (`log.info(msg: "hi")`, where the argument names are the parameter names in the declaration of the effect operation). Function values such as local variables and lambdas have no parameter names (the function type `fn(A) -> B` has no names), so they can be called with positional arguments only (E0306).
- A wrong number of arguments is E0305, a label with no matching parameter is E0306, and passing to the same parameter twice is E0307. A call that has a label error (E0306, E0307) does not also get an error for the number of arguments.

## 6.3 Blocks and bindings `[fn.block]`

```emel
count = 0
count = count + 1 # rebinding with the same name
(evens, odds) = List.partition(xs, is_even)
pairs: List[(String, Int)] = List.zip(names, scores)
```

- A binding is written `pattern = expr`. There is no declaration keyword.
- Variables are immutable, and there is no reassignment. Binding the same name again makes the new binding visible from then on.
- A binding pattern is limited to patterns that always match. The same holds for parameter patterns of anonymous functions (6.1). Matching that can fail is written with match. The check is the same as the exhaustiveness check (6.5), and the error shows examples of the missing patterns.
- The value of a block `{ ... }` is the value of its last expression.
- Bindings inside a block are not generalized. Only top-level functions and consts are polymorphic.

## 6.4 if `[fn.if]`

```emel
label = if count > 0 { "some" } else { "none" }

if !valid { fail Invalid(input:) }
```

- if is an expression. The condition is limited to the type Bool, and no parentheses are needed around it.
- When used as a value, it MUST have an `else`. The type of an if without `else` is `()`, and the type of its body MUST also be `()` or `Never`.
- There is no ternary operator.

## 6.5 match and patterns `[fn.match]`

```emel
shape match {
  Circle(radius:) if radius > 0.0 -> 3.14 * radius * radius
  Circle(_)                       -> 0.0
  Rect(w, h)                      -> w * h
  Empty                           -> 0.0
}
```

- match is postfix and is written `expr match { arms }`. It has the same form as escape (7.3) and the same precedence, the weakest level (6.6). Even when branching on the result of a pipe, the reading direction stays left to right. Because match is an expression, it can also be written on the right-hand side of a binding.

```emel
name = users |> List.find(fn(u) { u.id == id }) match {
  Some(u) -> u.name
  None -> "guest"
}

load(id) match {
  Some(user) -> user.name
  None -> "guest"
} escape {
  DbError(_) -> "unavailable"
}
```

- In a pattern, `-` can be written before a numeric literal (`-1`, `-0x10`, `-1.5`). The expression `-1` is unary minus applied to a literal, so this rule applies only to patterns. Whitespace MAY be placed between `-` and the number (fmt removes it). It can also be written on the left-hand side of a binding, but it is refutable, so it is E0317. When what follows `-` is not a numeric literal (`-x`), it is not a pattern: in the pattern of an arm it is E0113, and on the left-hand side of a binding (`--1`, `-x`) it is E0125. The range check treats `-` and the number as one value (the minimum Int value `-2147483648` can be written). `-0.0` matches with `==`, so it matches the same values as `0.0`. There are no range patterns (`1..10`) (exhaustiveness checking would have to handle intervals of numbers; use a guard instead).
- The prefix form (`match expr { arms }`) cannot be written. Writing it is a compile error (E0142) with help that suggests the postfix form. The diagnostic points at `match`. The same holds in an operand position of an operator (`1 + match x { ... }`). The implementation recovers by reading it in the prefix form and continues type checking.
- Arms are tried from top to bottom. A match that does not cover all cases is a compile error.
- A match with no arms is not exhaustive for a value of any type, including `Never` (E0303).
- Arms with a guard (`if condition`) are not counted when judging exhaustiveness. An arm with a guard also does not cover the arms below it when judging whether those arms are redundant.
- An arm fully covered by the arms above it is warned about as redundant (W0301).
- When a match is not exhaustive, examples of missing patterns are shown (`Rect(_, _)`, `[_, ..]`, `Some(None)`). The examples take the following form.
  - At a position with no constructor patterns, the constructors are not listed and only `_` is shown. At a position with constructor patterns, all missing constructors are listed.
  - The missing values at a position with only literals are shown as `_` (values other than the literals that appear).
  - Constant patterns are replaced with their values for the check, so the examples show values, not constant names.
  - Constructors with named fields are also shown by position (`Circle(_)`).
  - `[_, ..]` is a form that cannot be written in the grammar (17.6) (the rest needs a name), but it is shown as it is to describe the missing values.
  - Outside the defining module, constructor patterns of an opaque type cannot be written, so the example is `_` and the hidden constructors are not shown.
  - The examples are cut off at 64.
- An arm with a guard is also warned about as redundant (W0301) if it is covered by arms without guards above it.

| Pattern | Form | Meaning |
|---|---|---|
| Wildcard | `_` | Matches anything and binds nothing |
| Variable | `x` | Matches anything and binds it to `x` |
| Literal | `0` `-1` `"a"` | Matches when equal to the value. Float is matched with `==` (10.6), so `0.0` also matches `-0.0`, and a NaN value pattern matches no value (when judging redundant arms, NaN values are treated as the same value) |
| Constant | `MAX_SIZE` | Matches when equal to the value of the const |
| Constructor | `Empty` `Rect(w, h)` `Circle(radius: r)` | Matches values made with that constructor |
| Ignore the rest | `User(name:, ..)` | Ignores the fields not listed |
| Tuple | `(a, b)` | Matches element by element |
| List | `[]` `[x, ..rest]` `[x, ..]` | The empty list, the head and the rest, the head and the rest (not bound) |

- In a pattern for a constructor with named fields, omitting fields without writing `..` is a compile error (E0308, which lists the omitted fields). Writing more than the number of fields, or writing too few in a positional-only constructor without `..`, is E0309. Patterns for constructors with named fields MAY also be written by position.
- Interpolation cannot be written in a string in a pattern (E0117). An interpolation is not a constant, so it cannot be used for matching.
- A const pattern is matched with `==` but does not require Eq (it is not E0326).
- In a list pattern, the forms with only the rest (`[..]`, `[..rest]`) cannot be written. They mean the same as `_` and `rest`, so they are not needed.
- The arms of escape (7.3) MAY handle only some of the errors, so exhaustiveness is not required. Only redundant arms are warned about.

## 6.6 Operators `[fn.operator]`

The precedence is as follows. Operators higher in the table bind more tightly.

| Rank | Kind | Operators | Associativity |
|---|---|---|---|
| 1 | Postfix | `f(x)` `x.name` | |
| 2 | Unary | `!x` `-x` | |
| 3 | Multiplicative | `*` `/` `%` | Left associative |
| 4 | Additive | `+` `-` | Left associative |
| 5 | Comparison | `==` `!=` `<` `<=` `>` `>=` | Non-associative |
| 6 | Logical and | `&&` | Left associative, short-circuit evaluation |
| 7 | Logical or | `\|\|` | Left associative, short-circuit evaluation |
| 8 | Pipe | `\|>` | Left associative |
| 9 | escape and match | `expr escape { ... }` `expr match { ... }` | Left associative |

- Arithmetic operators can be used only on Int, Int64, and Float (otherwise E0310). Both sides MUST have the same type, and there is no implicit conversion.
- If the type of an operand of an arithmetic operator or unary minus is still undetermined after inference of the definition finishes, it is Int. There are no type parameters that abstract over numeric types.
- There are no bitwise operators. They are written with core functions (`Int.bit_and`, `Int.bit_or`, `Int.bit_xor`, `Int.bit_not`, `Int.shift_left`, `Int.shift_right` (sign-preserving), and `Int.shift_right_unsigned`; Int64 has the same ones). They can be written with a pipe, as in `flags |> Int.bit_and(MASK)`.
- The shift amount of the shift functions is Int, even in the Int64 versions. A negative shift amount is a defect. A shift amount at least the width of the type (32 or 64) shifts out all bits: `shift_left` and `shift_right_unsigned` give 0, and `shift_right` gives 0 or -1 depending on the sign (the amount is not taken modulo the width). The result of `shift_right_unsigned` is read as a signed value (`Int.shift_right_unsigned(-1, 0)` is -1).
- `==` and `!=` can be used on types that satisfy Eq, and ordering comparisons on types that satisfy Ord.
- Operators cannot be defined for user-defined types. Write a function instead, such as `Money.add(a, b)`.
- `+` cannot be used to concatenate strings. Use interpolation (`"#{a}#{b}"`) or `String.concat`.

## 6.7 Pipe `[fn.pipe]`

`x |> f(a)` is the same as `f(x, a)`. The value on the left goes into the first positional argument of the call on the right. If the right side is not a call, `x |> f` is `f(x)`.

```emel
scores
  |> List.filter(is_valid)
  |> List.fold(from: 0, f: add)
```

## 6.8 Repetition `[fn.loop]`

- There is no for or while. Repetition is written with recursion or with functions such as `List.map` and `List.fold`.
- A tail call to the function itself is guaranteed not to consume stack. The same holds for functions that contain suspend operations.
- Tail calls between mutually recursive functions are not guaranteed. The compiler detects them and warns.

The tail positions are the following.

- The last expression of the block of a function body
- Both branches of an if in tail position, the body of each arm of a match in tail position, and the last expression of a block in tail position
- The right-hand side of `&&` and `||` in tail position (`a && b` is the same as `if a { b } else { False }`, and `a || b` is the same as `if a { True } else { b }`)

The following are not tail positions.

- The body of an anonymous function (a call there is not a call to the function itself)
- The target expression of `escape` (the escape arms can run after it)
- The body of `with` (the handler's release runs after it)

The linter warns that a call to the function itself that looks like a tail call inside the target of an escape or the body of a with is "not guaranteed".
