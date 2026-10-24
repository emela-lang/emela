(import
  "import" @context
  module: (_) @name) @item

(function_declaration
  (visibility)? @context
  "suspend"? @context
  "fn" @context
  name: (_) @name) @item

(operation
  "suspend"? @context
  "fn" @context
  name: (_) @name) @item

(type_declaration
  (visibility)? @context
  "type" @context
  name: (_) @name) @item

(enum_declaration
  (visibility)? @context
  "enum" @context
  name: (_) @name) @item

(variant
  name: (_) @name) @item

(error_declaration
  (visibility)? @context
  "error" @context
  name: (_) @name) @item

(const_declaration
  (visibility)? @context
  "const" @context
  name: (_) @name) @item

(effect_declaration
  (visibility)? @context
  "effect" @context
  name: (_) @name) @item

(handler_declaration
  (visibility)? @context
  "handler" @context
  name: (_) @name
  ":" @context
  effect: (_) @context) @item

(handler_init
  "init" @name) @item

(handler_release
  "release" @name) @item

(layer_declaration
  (visibility)? @context
  "layer" @context
  name: (_) @name) @item

(trait_declaration
  (visibility)? @context
  "trait" @context
  name: (_) @name) @item

(impl_declaration
  "impl" @context
  type: (_) @name
  ":" @context
  trait: (_) @context) @item

(comment) @annotation
