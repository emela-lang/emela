; Capture names follow nvim-treesitter. A later pattern overrides an earlier one, so general
; patterns come first. `editors/zed/sync-queries.sh` derives the Zed copy from this file.

; Names

(identifier) @variable

(constant_identifier) @constant

; A type name in an expression is a constructor (`None`, `NotFound(id:)`); in a type it is a
; type (below).
(_
  (type_identifier) @constructor)

((type_identifier) @boolean
  (#any-of? @boolean "True" "False"))

(type_reference
  name: (type_identifier) @type)

(type_reference
  module: (type_identifier) @module)

(type_variable
  (constant_identifier) @type)

(type_parameter
  name: (_) @type)

(self_type) @type.builtin

(self) @variable.builtin

(wildcard) @variable.builtin

; `List.map`, `Http.Client.get`: the type names on the left of `.` are modules.
(field_expression
  value: (type_identifier) @module)

(field_expression
  value: (field_expression
    field: (type_identifier) @module))

(module_path
  (type_identifier) @module)

(import_list
  (type_identifier) @type)

(import_list
  (identifier) @function)

; Declarations

(type_declaration
  name: (type_identifier) @type.definition)

(enum_declaration
  name: (type_identifier) @type.definition)

(error_declaration
  name: (type_identifier) @type.definition)

(effect_declaration
  name: (type_identifier) @type.definition)

(handler_declaration
  name: (type_identifier) @type.definition)

(layer_declaration
  name: (type_identifier) @type.definition)

(trait_declaration
  name: (type_identifier) @type.definition)

(variant
  name: (type_identifier) @constructor)

(const_declaration
  name: (constant_identifier) @constant)

(function_declaration
  name: (identifier) @function)

(operation
  name: (identifier) @function)

(parameter
  name: (identifier) @variable.parameter)

(lambda_parameter
  pattern: (identifier) @variable.parameter)

(field
  name: (identifier) @variable.member)

; Expressions

(field_expression
  field: (identifier) @variable.member)

(named_argument
  name: (identifier) @variable.member)

(field_pattern
  name: (identifier) @variable.member)

(call_expression
  function: (identifier) @function.call)

(call_expression
  function: (field_expression
    field: (identifier) @function.method.call))

(call_expression
  function: (type_identifier) @constructor)

(call_expression
  function: (field_expression
    field: (type_identifier) @constructor))

; `xs |> List.reverse`, `x |> dbg`
(binary_expression
  operator: "|>"
  right: (identifier) @function.call)

(binary_expression
  operator: "|>"
  right: (field_expression
    field: (identifier) @function.method.call))

(variant_pattern
  variant: (type_reference
    name: (type_identifier) @constructor))

(match_arm
  pattern: (type_reference
    name: (type_identifier) @constructor))

(annotation
  "@" @attribute
  name: (identifier) @attribute)

(annotation_path
  module: (type_identifier) @module
  name: (identifier) @function)

; Literals

(integer) @number

(float) @number.float

(negative_literal
  "-" @number)

(string) @string

(escape_sequence) @string.escape

(comment) @comment @spell

((comment) @comment.documentation
  (#match? @comment.documentation "^##"))

; Keywords

"fn" @keyword.function

[
  "type"
  "enum"
  "error"
  "trait"
  "effect"
  "handler"
  "layer"
] @keyword.type

[
  "const"
  "impl"
  "use"
  "with"
  "assert"
  "fails"
  "derive"
  "of"
  "default"
  "by"
  "init"
  "release"
] @keyword

[
  "pub"
  "opaque"
  "suspend"
] @keyword.modifier

"import" @keyword.import

[
  "if"
  "else"
  "match"
] @keyword.conditional

[
  "fail"
  "escape"
] @keyword.exception

; Operators and punctuation

[
  "|>"
  "||"
  "&&"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "+"
  "-"
  "*"
  "/"
  "%"
  "!"
  "="
  "->"
  "|"
  ".."
] @operator

[
  "."
  ","
  ":"
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

(interpolation
  [
    "#{"
    "}"
  ] @punctuation.special)
