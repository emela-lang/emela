; For nvim-treesitter-textobjects.

(function_declaration
  body: (block
    .
    "{"
    _+ @function.inner
    "}")) @function.outer

(lambda
  body: (block
    .
    "{"
    _+ @function.inner
    "}")) @function.outer

(operation) @function.outer

[
  (type_declaration)
  (enum_declaration)
  (error_declaration)
  (effect_declaration)
  (handler_declaration)
  (layer_declaration)
  (trait_declaration)
  (impl_declaration)
] @class.outer

(declaration_list
  .
  "{"
  _+ @class.inner
  "}")

(variant_list
  .
  "{"
  _+ @class.inner
  "}")

(parameters
  "," @parameter.outer
  .
  (parameter) @parameter.inner @parameter.outer)

(parameters
  .
  (parameter) @parameter.inner @parameter.outer
  .
  ","? @parameter.outer)

(lambda_parameters
  (lambda_parameter) @parameter.inner @parameter.outer)

(arguments
  (_) @parameter.inner @parameter.outer)

(call_expression) @call.outer

(call_expression
  arguments: (arguments
    .
    "("
    _+ @call.inner
    ")"))

(if_expression) @conditional.outer

(if_expression
  consequence: (block) @conditional.inner)

(match_expression) @conditional.outer

(match_arm) @conditional.inner

(block) @block.outer

(comment) @comment.outer

(string) @string.outer

(binding) @assignment.outer

(binding
  pattern: (_) @assignment.lhs @assignment.inner
  value: (_) @assignment.rhs)

(integer) @number.inner

(float) @number.inner
