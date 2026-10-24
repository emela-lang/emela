; Scopes

[
  (source_file)
  (function_declaration)
  (handler_init)
  (lambda)
  (block)
  (match_arm)
] @local.scope

; Definitions

(function_declaration
  name: (identifier) @local.definition.function)

(const_declaration
  name: (constant_identifier) @local.definition.constant)

(parameter
  name: (identifier) @local.definition.parameter)

(lambda_parameter
  pattern: (identifier) @local.definition.parameter)

(lambda_parameter
  pattern: (_
    (identifier) @local.definition.parameter))

(binding
  pattern: (identifier) @local.definition.var)

(binding
  pattern: (_
    (identifier) @local.definition.var))

(match_arm
  pattern: (identifier) @local.definition.var)

(match_arm
  pattern: (_
    (identifier) @local.definition.var))

(field_pattern
  name: (identifier) @local.definition.var
  .)

(rest_pattern
  (identifier) @local.definition.var)

[
  (type_declaration
    name: (type_identifier) @local.definition.type)
  (enum_declaration
    name: (type_identifier) @local.definition.type)
  (error_declaration
    name: (type_identifier) @local.definition.type)
  (effect_declaration
    name: (type_identifier) @local.definition.type)
  (handler_declaration
    name: (type_identifier) @local.definition.type)
  (layer_declaration
    name: (type_identifier) @local.definition.type)
  (trait_declaration
    name: (type_identifier) @local.definition.type)
]

; References

(identifier) @local.reference

(constant_identifier) @local.reference

(type_identifier) @local.reference
