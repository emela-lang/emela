# 6. Functions and expressions `[fn]`

There is one form of call, `name(arguments)`, and functions and constructors follow the same rules. Every syntactic element is an expression; the only statement is a binding.

## 6.1 Function declarations `[fn.decl]`

```emel
fn add(x: Int, y: Int) -> Int {
  x + y
}

fn map[A, B](xs: List[A], f: fn(A) -> B) -> List[B] { ... }

pub fn find(id: Int) -> User fails NotFound uses { Users } { ... }
```

- In a top-level fn and a local fn (6.9), the parameter types are written (omitting them is a compile error (E3006)). In a loop expression (6.8) they are not written; they are inferred. In an anonymous function they can be omitted. The return type, `fails`, and `uses` can be omitted. The meaning when they are omitted is defined in 7.4 and 9.3.
- A call determines the type parameters of a top-level fn only from the type of the fn: the parameter types, the return type, the fails clause and the uses clause, including inside function types and in the type arguments of named types. When the return type, the fails clause or the uses clause is omitted, the inferred one is used (in `fn f[A](x: Int) { none: Option[A] = None; none }` the return type is inferred as `Option[A]`, so A appears in it). A type parameter that does not appear in the type of the fn is never determined, so the compiler warns (W3002) at that type parameter in the list. A bound (`[A: Show]`) and type annotations inside the body do not count as appearing. Ordinary type parameters and set type parameters are treated alike.
- The body is a block, and the value of the last expression is the return value. There is no `return`.
- An anonymous function is written `|x| x + 1`. The parameters are written between `|` and `|`, separated by `,` (`|acc, x| acc + x`). An anonymous function without parameters is written with the token `||` (`|| fetch(1)`). Parameter types can be omitted. There is no annotation of the return type.
- The body is one expression and extends as far to the right as possible: it includes `|>`, binary operators, and postfix `match` and `escape`. It ends where an expression ends: at a `,` or a closing bracket, at the end of a statement, and at the end of an arm. A body of several statements is a block, `|x| { ... }`. A `{` after the parameter list always starts a block, because records and constructors are written with parentheses (5.1).
- Since the body takes everything to its right, an anonymous function that is called, piped, or used as the left operand of an operator is enclosed in parentheses (`(|x| x + 1)(2)`).
- `fn` is written only in declarations, local fns (6.9) and function types. `fn(...)` in an expression is a compile error (E1405).
- A parameter of an anonymous function can be a pattern instead of a name (`|(key, value)| ...`, `|acc, _| acc`, `|Request(url:, ..)| url`). A type, when written, comes after the pattern (`|(a, b): (Int, Int)| a + b`).
  - A parameter pattern is limited, as for bindings, to patterns that always match (6.3, E3108).
  - `|p| body` means the same as `|t| { p = t; body }`. Parameters are matched from left to right. Binding the same name in two parameters is a compile error (E2109).
  - Inside the parameter list, a `|` at bracket depth zero closes the list. A parameter type whose error set has two or more items is enclosed in parentheses (`|f: (fn() -> Int fails Timeout | Refused)| f()`).
  - The parameters of top-level fns, local fns, fns in impls and handlers, and operations of traits and effects are limited to names. This is because a parameter name also serves as the label of a named argument (6.2) and as the link to the declaration. Values of anonymous functions have no parameter names and are called by position only (6.2), so making a parameter a pattern loses nothing.

- A top-level fn, a local fn, and a constructor with fields become function values when their names are written without calling them (`List.map(xs, Some)`).

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
- Named arguments can be used only when calling an fn (a top-level fn or a local fn), the name of a loop expression inside its body (6.8), or a constructor referred to by name, and when calling an operation on a capability value (`log.info(msg: "hi")`, where the argument names are the parameter names in the declaration of the effect operation). Function values such as local variables and lambdas have no parameter names (the function type `fn(A) -> B` has no names), so they can be called with positional arguments only (E3103).
- A wrong number of arguments is E3101, a label with no matching parameter is E3103, and passing to the same parameter twice is E3104. A call that has a label error (E3103, E3104) does not also get an error for the number of arguments.

## 6.3 Blocks and bindings `[fn.block]`

```emel
count = 0
count = count + 1 # rebinding with the same name
(evens, odds) = List.partition(xs, is_even)
pairs: List[(String, Int)] = List.zip(names, scores)
```

- A binding is written `pattern = expr`. There is no declaration keyword. A local fn (6.9) is also a binding of its name.
- Variables are immutable, and there is no reassignment. Binding the same name again makes the new binding visible from then on.
- A binding pattern is limited to patterns that always match. The same holds for parameter patterns of anonymous functions (6.1). Matching that can fail is written with match. The check is the same as the exhaustiveness check (6.5), and the error shows examples of the missing patterns.
- The value of a block `{ ... }` is the value of its last expression.
- Bindings inside a block are not generalized, and neither are local fns. Only top-level functions and consts are polymorphic.
- A binding of an anonymous function (`f = |x| x + 1`) is not generalized, is called with positional arguments only (6.2), and cannot refer to itself, because the name is visible only after the binding.
- An anonymous function written as a statement that is not the last statement of its block is a compile error (E1112). Its value is always discarded, so it is never useful. A line that starts with `||` starts such an anonymous function (2.5), so the rule also keeps `x = a⏎|| b`, which once bound `a || b`, from binding `a` and dropping `|| b`.

## 6.4 if `[fn.if]`

```emel
label = if count > 0 { "some" } else { "none" }

if !valid { fail Invalid(input:) }
```

- if is an expression. No parentheses are needed around its condition.
- The condition is a position with the expected type Bool (`[type.rules]`), so its type is Bool or Never.
- Apart from the type of the condition, `if c { a } else { b }` is the same as the following match (`[fn.match]`), where `c` is the condition and `a` and `b` are the contents of the blocks. In `else if`, the inner if is the body of the `False` arm. An if without `else` is the same as one whose `else` is `{ () }`.

```emel
c match {
  True  -> { a }
  False -> { b }
}
```

- The rewrite gives the following rules. The type of an if without `else` is `()`, and the type of its body MUST also be `()` or `Never` (`[type.rules]`), so an if used as a value of another type MUST have an `else`. The condition is evaluated first, and only the chosen branch is evaluated (`[dyn.order]`).
- There is no ternary operator.

## 6.5 match and patterns `[fn.match]`

```emel
shape match {
  Circle(radius:) where radius > 0.0 -> 3.14 * radius * radius
  Circle(_)                          -> 0.0
  Rect(w, h)                         -> w * h
  Empty                              -> 0.0
}
```

- match is postfix and is written `expr match { arms }`. It has the same form as escape (7.3) and the same precedence, the weakest level (6.6). Even when branching on the result of a pipe, the reading direction stays left to right. Because match is an expression, it can also be written on the right-hand side of a binding.

```emel
name = users |> List.find(|u| u.id == id) match {
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

- In a pattern, `-` can be written before a numeric literal (`-1`, `-0x10`, `-1.5`). The expression `-1` is unary minus applied to a literal, so this rule applies only to patterns. Whitespace MAY be placed between `-` and the number (fmt removes it). It can also be written on the left-hand side of a binding, but it is refutable, so it is E3108. When what follows `-` is not a numeric literal (`-x`), it is not a pattern: in the pattern of an arm it is E1102, and on the left-hand side of a binding (`--1`, `-x`) it is E1111. The range check treats `-` and the number as one value (the minimum Int value `-2147483648` can be written). `-0.0` matches with `==`, so it matches the same values as `0.0`. There are no range patterns (`1..10`) (exhaustiveness checking would have to handle intervals of numbers; use a guard instead).
- The prefix form (`match expr { arms }`) cannot be written. Writing it is a compile error (E1401) with help that suggests the postfix form. The diagnostic points at `match`. The same holds in an operand position of an operator (`1 + match x { ... }`). The implementation recovers by reading it in the prefix form and continues type checking.
- Arms are tried from top to bottom. A match that does not cover all cases is a compile error.
- A match with no arms is not exhaustive for a value of any type, including `Never` (E3102).
- Arms with a guard (`where condition`) are not counted when judging exhaustiveness. An arm with a guard also does not cover the arms below it when judging whether those arms are redundant.
- The guard of an arm, in a match or in an escape (7.3), is a position with the expected type Bool (`[type.rules]`), so its type is Bool or Never.
- An arm fully covered by the arms above it is warned about as redundant (W3101).
- When a match is not exhaustive, examples of missing patterns are shown (`Rect(_, _)`, `[_, ..]`, `Some(None)`). The examples take the following form.
  - At a position with no constructor patterns, the constructors are not listed and only `_` is shown. At a position with constructor patterns, all missing constructors are listed.
  - The missing values at a position with only literals are shown as `_` (values other than the literals that appear).
  - Constant patterns are replaced with their values for the check, so the examples show values, not constant names.
  - Constructors with named fields are also shown by position (`Circle(_)`).
  - `[_, ..]` is a form that cannot be written in the grammar (17.6) (the rest needs a name), but it is shown as it is to describe the missing values.
  - Outside the defining module, constructor patterns of an opaque type cannot be written, so the example is `_` and the hidden constructors are not shown.
  - The examples are cut off at 64.
- An arm with a guard is also warned about as redundant (W3101) if it is covered by arms without guards above it.

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

- In a pattern for a constructor with named fields, omitting fields without writing `..` is a compile error (E3105, which lists the omitted fields). Writing more than the number of fields, or writing too few in a positional-only constructor without `..`, is E3101. Patterns for constructors with named fields MAY also be written by position.
- Interpolation cannot be written in a string in a pattern (E1105). An interpolation is not a constant, so it cannot be used for matching.
- A const pattern is matched with `==` but does not require Eq (it is not E3302).
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

- Arithmetic operators can be used only on Int, Int64, and Float (otherwise E3003). Both sides MUST have the same type, and there is no implicit conversion.
- If the type of an operand of an arithmetic operator or unary minus is still undetermined after inference of the definition finishes, it is Int. There are no type parameters that abstract over numeric types.
- There are no bitwise operators. They are written with core functions (`Int.bit_and`, `Int.bit_or`, `Int.bit_xor`, `Int.bit_not`, `Int.shift_left`, `Int.shift_right` (sign-preserving), and `Int.shift_right_unsigned`; Int64 has the same ones). They can be written with a pipe, as in `flags |> Int.bit_and(MASK)`.
- The shift amount of the shift functions is Int, even in the Int64 versions. A negative shift amount is a defect. A shift amount at least the width of the type (32 or 64) shifts out all bits: `shift_left` and `shift_right_unsigned` give 0, and `shift_right` gives 0 or -1 depending on the sign (the amount is not taken modulo the width). The result of `shift_right_unsigned` is read as a signed value (`Int.shift_right_unsigned(-1, 0)` is -1).
- `==` and `!=` can be used on types that satisfy Eq, and ordering comparisons on types that satisfy Ord.
- The operand of `!` is a position with the expected type Bool (`[type.rules]`), so its type is Bool or Never. The type of `!x` is Bool.
- `a && b` is the same as `if a { b } else { False }`, and `a || b` is the same as `if a { True } else { b }` (`[fn.if]`). So the type of the whole is Bool, and the right side is evaluated only when the left side does not determine the result.
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

- There is no for or while. Repetition is written with recursion, with a loop expression (below), or with functions such as `List.map` and `List.fold`.
- A tail call to the function itself is guaranteed not to consume stack. For a local fn, a call to its own name inside its body, where the name refers to that local fn, is a call to itself. For a loop expression, a call to its name inside its body is a call to itself. The same holds for functions that contain suspend operations.
- Tail calls between mutually recursive functions are not guaranteed. The compiler detects them and warns.

The tail positions are the following.

- The last expression of the block of a function body, and the last expression of the body of a loop expression
- The body of each arm of a match in tail position, and the last expression of a block in tail position

An if, `&&` and `||` are evaluated as a match (`[fn.if]`, `[fn.operator]`), so both branches of an if in tail position and the right-hand side of `&&` and `||` in tail position are also tail positions.

The following are not tail positions.

- The body of an anonymous function, the body of a local fn and the body of a loop expression, as positions of the enclosing function (a call there to the enclosing function is not a call to the function itself)
- The target expression of `escape` (the escape arms can run after it)
- The body of `with` (the handler's release runs after it)

The linter warns that a call to the function itself that looks like a tail call inside the target of an escape or the body of a with is "not guaranteed".

A loop expression defines a function that calls itself only in tail position, and calls it in place with initial values.

```emel
pub fn range(from: Int, to: Int) -> List[Int] {
  loop onto(to:, acc: []) {
    if to <= from { acc } else { onto(to - 1, [to - 1, ..acc]) }
  }
}
```

- A loop expression is written `loop name(p: e, ...) [signature] block`. Each element in the parentheses names a parameter and gives its initial value, in the form of a named argument (6.2): `p: e`, or `p:` for `p: p`. The same name written twice is a compile error (E3104), as for an argument given twice. The parentheses can be empty. The return type, `fails` and `uses` follow the rules of a top-level fn (9.3). A loop expression has no type parameters, attributes, `pub` or `suspend`, and `fn` is not written. Whether it suspends is decided by its R, as for any function.
- The initial values are evaluated from left to right, in the scope outside the loop expression, before the body runs. The parameters are not visible in them: in `loop go(a: 1, b: a)`, the `a` of `b: a` is the `a` outside. The body then runs with each parameter bound to its initial value, and the value of the loop expression is the value of the body.
- A call to the name runs the body again with the parameters bound to its arguments. It follows the rules of calls (6.2): the arguments are positional, in the order of the parentheses, or named by the parameter names.
- The name is visible only inside the body, not in the parentheses and not after the loop expression. The body can refer to the variables visible at the loop expression. A parameter, binding or local fn of the same name inside the body hides the name, as in 6.9, and the rules on the name below do not apply to what the hiding name refers to.
- The name can be written only as the callee of a call in tail position of the body. A call anywhere else is a compile error (E3111): a call not in tail position (`onto(..) + 1`, an argument of another call, the right-hand side of a binding, a statement that is not the last of its block), and a call inside the target or an arm of an `escape`, the body of a `with`, an anonymous function, a local fn or another loop expression that is itself in the body. An `escape` or `with` around the whole loop expression does not count: `loop go(..) { .. } escape { .. }` is allowed. The name written as a value (`List.map(xs, onto)`, `f = onto`) is a compile error (E3112). A pipe into the name (`acc |> onto(n)`, `n |> onto`) is a call to it (6.7). The name is not found by a method call (9.4).
- The type of each parameter is the monomorphic type that its initial value and the arguments of the calls to the name determine. It is not generalized (6.3). A type that they leave open, or that is needed before they determine it (a field access, 9.4), is given by the return type or by a binding with a type before the loop expression (`empty: List[Int] = []`, then `acc: empty`).

## 6.9 Local functions `[fn.local]`

```emel
pub fn range(from: Int, to: Int) -> List[Int] {
  ## Builds from the back, so `to - 1` cannot overflow.
  fn onto(to: Int, acc: List[Int]) -> List[Int] {
    if to <= from { acc } else { onto(to - 1, [to - 1, ..acc]) }
  }
  onto(to, [])
}
```

- A block can contain a local fn, a statement written `fn name(params) [signature] block`. Its parameters, return type, `fails` and `uses` follow the rules of a top-level fn.
- A local fn cannot have type parameters (E1213), attributes, `pub` or `suspend` (E1212).
- A local fn cannot be the last statement of its block (E1214). Nothing after it can call it.
- A doc comment (`##`) can be written before a local fn (2.2). The language server shows it on hover. Generated documentation does not include it.
- The declaration binds the name as a value. The name is visible from the declaration to the end of the block, and inside the local fn's own body. It is not visible before the declaration, so local fns cannot be mutually recursive.
- The body can refer to every variable visible at the declaration (it captures them, as an anonymous function does). A captured variable keeps the value it had at the declaration.
- A local fn is a binding, and follows the shadowing rules of 6.3. A later binding or local fn of the same name hides it. Inside its body, its name refers to itself unless a parameter of the same name hides it. It can hide a top-level fn or an imported name of the same name inside the block. None of these is warned about.
- A local fn is not generalized (6.3). The type parameters of the enclosing fn can be written in its signature and body.
- A local fn is not found by a method call (9.4), and cannot be called from a test (13.1), because its name is visible only inside its block.
