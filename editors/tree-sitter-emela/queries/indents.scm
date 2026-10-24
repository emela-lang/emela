[
  (block)
  (match_arms)
  (declaration_list)
  (variant_list)
  (supply_list)
  (field_list)
  (tuple_field_list)
  (parameters)
  (lambda_parameters)
  (arguments)
  (list_expression)
  (tuple_expression)
  (parenthesized_expression)
  (list_pattern)
  (tuple_pattern)
  (variant_pattern)
  (import_list)
  (effect_set)
  (type_arguments)
  (type_parameters)
] @indent.begin

[
  "}"
  ")"
  "]"
] @indent.branch @indent.end

; `fails` and `use` on the lines after a signature, and a line that continues with an operator.
(fails_clause) @indent.begin

(binary_expression
  operator: "|>") @indent.begin

(string) @indent.auto

(comment) @indent.auto
