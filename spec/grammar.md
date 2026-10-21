# 17. Grammar `[grammar]`

This chapter is the first version that turns the decisions so far into EBNF. Ambiguities are found while implementing the formatter, and the grammar is finalized together with the conformance tests.

## 17.1 Notation `[grammar.notation]`

- `{ x }` is zero or more repetitions, `[ x ]` is optional, and `|` is a choice. Quoted items are terminal symbols.
- NL is a newline that acts as a statement terminator (2.5). Newlines judged to be line continuations, newlines inside `(` and `[`, and newlines inside the braces of an effect set (`use { ... }`) are removed during lexical analysis.
- A trailing comma in a list is allowed everywhere. The rules omit it.

## 17.2 Lexical grammar `[grammar.lex]`

Tokens are split by longest match. `""""` is `"""` followed by `"`, not two empty strings `""`.

```ebnf
lower_name = ( "a".."z" | "_" ) { letter | digit | "_" }      (* excluding reserved words *)
type_name  = "A".."Z" { letter | digit }                      (* contains at least one lowercase letter *)
upper_name = "A".."Z" { "A".."Z" | digit | "_" }

dec_lit    = digit { digit | "_" }
int_lit    = dec_lit
           | "0x" hex_digit { hex_digit | "_" }
           | "0b" ( "0" | "1" ) { "0" | "1" | "_" }
float_lit  = dec_lit "." dec_lit [ ( "e" | "E" ) [ "+" | "-" ] dec_lit ]
string_lit = '"' { char | escape | "#{" expr "}" } '"'
           | '"""' NEWLINE { line } '"""'                (* multi-line; indentation rules in 2.4 *)
escape     = '\\' ( '"' | '\\' | "n" | "t" | "r" | "#" | "u{" hex_digit { hex_digit } "}" )
literal    = int_lit | float_lit | string_lit
```

- `letter` and `digit` are limited to ASCII.
- `char` is any character except `"`, `\`, and newline. `hex_digit` is `0` to `9`, `a` to `f`, and `A` to `F`.
- `1..2` reads as `dec_lit ".." dec_lit`, and `1.x` reads as `dec_lit "." lower_name`. If no digit follows the decimal point, the literal is not a floating-point number.

## 17.3 Modules and declarations `[grammar.decl]`

```ebnf
module       = { import NL } { decl NL }

import       = "import" module_path [ "." "{" import_item { "," import_item } "}" ]
module_path  = type_name { "." type_name }
import_item  = lower_name | type_name | upper_name

decl         = { annotation NL } [ "pub" [ "opaque" ] ] item
             | impl_decl
item         = fn_decl | type_decl | enum_decl | error_decl | const_decl
             | effect_decl | handler_decl | layer_decl | trait_decl

annotation   = "@" lower_name [ "(" annot_arg { "," annot_arg } ")" ]
annot_arg    = lower_name | literal | lower_name ":" literal

fn_decl      = [ "suspend" ] "fn" lower_name [ type_params ]
               "(" [ params ] ")" [ signature ] [ block ]
signature    = [ "->" type [ "fails" error_set ] ] [ "use" effect_set ]
params       = param { "," param }
param        = "self" | lower_name [ ":" type ]

type_params  = "[" type_param { "," type_param } "]"
type_param   = ( upper_name | type_name ) [ ":" bound { "+" bound } ]
bound        = type_ref

type_decl    = "type" type_name [ type_params ] [ "(" [ fields ] ")" ] [ derive ]
enum_decl    = "enum" type_name [ type_params ] "{" variant { sep variant } "}" [ derive ]
variant      = type_name [ "(" ( fields | type { "," type } ) ")" ]
error_decl   = "error" type_name [ "(" [ fields ] ")" ] [ derive ]
fields       = field { "," field }
field        = { annotation } lower_name ":" type
derive       = "derive" type_ref { "," type_ref }
sep          = "," | NL

const_decl   = "const" upper_name [ ":" type ] "=" expr

effect_decl  = "effect" type_name [ type_params ] "{" { op_sig NL } "}"
             | "effect" type_name "of" type_ref [ "[" type { "," type } "]" ] [ "default" ]   (* label; 8.9 *)
op_sig       = [ "suspend" ] "fn" lower_name [ type_params ] "(" [ params ] ")"
               "->" type [ "fails" error_set ]     (* type_params only for built-in effects; 8.1 *)

handler_decl = "handler" type_name [ "(" [ fields ] ")" ]
               ":" type_ref [ "[" type { "," type } "]" ] "{" { handler_item NL } "}"
handler_item = "init" block
             | "release" block
             | [ "suspend" ] "fn" lower_name "(" [ lower_name { "," lower_name } ] ")" block

layer_decl   = "layer" type_name "{" type_ref { sep type_ref } "}"

trait_decl   = "trait" type_name [ ":" bound { "+" bound } ] "{" { trait_item NL } "}"
trait_item   = "fn" lower_name "(" [ params ] ")" [ signature ] [ block ]
             | derive_rule                                    (* syntax not yet decided *)

impl_decl    = "impl" type_ref [ type_params ] ":" type_ref "{" { impl_item NL } "}"
impl_item    = "fn" lower_name "(" [ params ] ")" block
```

The following restrictions apply outside the grammar.

- An fn_decl without a body, and a top-level suspend fn, are allowed only when they have `@external`. A trait_item fn does not need a default implementation, and this restriction does not apply to it.
- `opaque` can be applied only to type and enum.
- A type_decl without fields is allowed only when it has `@external`.
- An enum_decl has at least one variant. Never is the only type with no values.
- When `derive` follows the newline right after a type_decl, enum_decl, or error_decl, that newline is not a terminator, and the derive continues the declaration (as written in the examples in 10.5). Blank lines and comment-only lines in between are skipped (as in the line continuation check in 2.5). Because `derive` is a contextual keyword, it is not part of the lexical line continuation rules in 2.5, and it is handled only at the position of a declaration.

## 17.4 Types `[grammar.type]`

```ebnf
type       = type_ref [ "[" type_arg { "," type_arg } "]" ]
           | upper_name                                       (* type parameter *)
                                                              (* a type parameter with a type name is written as a type_ref and told apart by name resolution *)
           | "Self"
           | "(" ")"                                          (* Unit *)
           | "(" type ")"                                     (* parentheses *)
           | "(" type "," type { "," type } ")"               (* tuple *)
           | "fn" "(" [ type { "," type } ] ")" "->" type
             [ "fails" error_set ] [ "use" effect_set ]
type_ref   = [ module_path "." ] type_name

type_arg   = type
           | "{" [ effect_item { "," effect_item } ] "}"          (* effect set; {} is the empty set *)
           | error_item "|" error_item { "|" error_item }          (* error set *)

error_set  = "{" "}" | error_item { "|" error_item }
error_item = type_ref | upper_name                            (* an error tag or a set type parameter; they can be mixed *)
effect_set = "{" [ effect_item { "," effect_item } ] "}" | type_ref | upper_name
effect_item = type_ref [ "[" type { "," type } "]" ] | upper_name   (* an effect (type arguments allowed) or a set type parameter; they can be mixed *)
```

## 17.5 Expressions `[grammar.expr]`

```ebnf
block      = "{" { stmt NL } "}"
stmt       = binding | expr
binding    = pattern [ ":" type ] "=" expr

expr       = "fail" expr
           | "assert" expr
           | pipe_expr { ( "escape" | "match" ) "{" { arm NL } "}" }

pipe_expr  = or_expr  { "|>" or_expr }
or_expr    = and_expr { "||" and_expr }
and_expr   = cmp_expr { "&&" cmp_expr }
cmp_expr   = add_expr [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) add_expr ]
add_expr   = mul_expr { ( "+" | "-" ) mul_expr }
mul_expr   = unary    { ( "*" | "/" | "%" ) unary }
unary      = ( "!" | "-" ) unary
           | "use" type_ref [ "[" type { "," type } "]" ]
           | postfix
postfix    = primary { "(" [ args ] ")" | "." lower_name }

primary    = literal | lower_name | upper_name | type_ref | "self"
           | "(" ")"
           | "(" expr ")"
           | "(" expr "," expr { "," expr } ")"
           | "[" [ expr { "," expr } [ "," ".." expr ] ] "]"
           | block | if_expr | with_expr | lambda

args       = arg { "," arg } [ "," ".." expr ]
           | ".." expr
arg        = expr | lower_name ":" [ expr ]

if_expr    = "if" expr block [ "else" ( block | if_expr ) ]
arm        = pattern [ "if" expr ] "->" expr
with_expr  = "with" with_item { "," with_item } block
with_item  = type_ref [ "by" type_ref [ "[" type { "," type } "]" ] ]   (* by is in 8.9 *)
lambda     = "fn" "(" [ lambda_param { "," lambda_param } ] ")" block
lambda_param = pattern [ ":" type ]
```

- When the last statement of a block is an expression, its value is the value of the block. When the last statement is a binding, the value of the block is `()`.
- Whether a statement is a binding or an expression is decided by whether a sequence that reads as a pattern is immediately followed by `=` or `:`.
- To call an operation of `use X` in place, wrap it in parentheses: `(use X).op(...)`. Writing `.` or `(` directly after `use X` is a compile error (E0121).
- In an argument list, a positional argument cannot follow a named argument.
- A trailing `..expr` in an argument list is a record update of a constructor (5.3). Writing it in a function call is a compile error (E0222). Writing an argument after `..` is a compile error (E0114).
- A `..expr` in a list literal can appear only once, at the end. `[x, y, ..rest]` is the list `rest` with `x` and `y` added in front. What follows `..` is not limited to a name and can be any expression (`[x, ..List.tail(xs)]`). `[..rest]` with no elements, and `..` in the middle or more than once (`[..a, ..b]`), cannot be written.

## 17.6 Patterns `[grammar.pattern]`

```ebnf
pattern  = "_"
         | lower_name
         | literal
         | "-" ( int_lit | float_lit )      (* negative number; patterns only (6.5) *)
         | upper_name
         | type_ref [ "(" pat_args ")" ]
         | "(" ")"
         | "(" pattern "," pattern { "," pattern } ")"
         | "[" "]"
         | "[" pattern { "," pattern } [ "," ".." [ lower_name ] ] "]"

pat_args = ".."
         | pat_arg { "," pat_arg } [ "," ".." ]
pat_arg  = pattern | lower_name ":" [ pattern ]
```
