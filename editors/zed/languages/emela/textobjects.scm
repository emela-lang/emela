(function_declaration
  body: (block
    "{"
    (_)* @function.inside
    "}")) @function.around

(lambda
  body: (block
    "{"
    (_)* @function.inside
    "}")) @function.around

(operation) @function.around

[
  (type_declaration)
  (enum_declaration)
  (error_declaration)
  (effect_declaration)
  (handler_declaration)
  (layer_declaration)
  (trait_declaration)
  (impl_declaration)
] @class.around

(declaration_list
  "{"
  (_)* @class.inside
  "}")

(variant_list
  "{"
  (_)* @class.inside
  "}")

(comment)+ @comment.around
