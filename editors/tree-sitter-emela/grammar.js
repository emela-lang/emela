/**
 * @file Tree-sitter grammar for Emela
 * @license Apache-2.0
 *
 * Follows `spec/grammar.md` ([grammar]). Where the specification decides something in the
 * lexer (line continuation, [lex.continuation]), the external scanner in `src/scanner.c`
 * decides it the same way.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const PREC = {
  pipe: 1,
  or: 2,
  and: 3,
  compare: 4,
  add: 5,
  multiply: 6,
  unary: 7,
  postfix: 8,
};

// The reserved words of [kw.reserved] that have syntax. `as` and the words of [kw.future] have
// none, so the grammar has no token for them.
const RESERVED = [
  'fn', 'type', 'enum', 'error', 'const', 'trait', 'impl', 'effect', 'handler', 'layer', 'import',
  'pub', 'opaque', 'suspend',
  'if', 'else', 'match', 'fail', 'escape', 'use', 'with', 'assert',
  'self',
];

module.exports = grammar({
  name: 'emela',

  externals: $ => [
    // A newline that ends a statement, a declaration or an item ([lex.continuation]).
    $._newline,
    $._string_content,
    $._multiline_string_content,
    '#{',
    // Never produced. They are valid only where a newline followed by `fails` or `use`
    // (rule 4), or by `derive` ([grammar.decl]), continues the line, and tell the scanner so.
    $._signature_continues,
    $._derive_continues,
    // Valid in every state only during error recovery.
    $._error_sentinel,
  ],

  extras: $ => [/[ \t\r\n]/, $.comment],

  word: $ => $.identifier,

  reserved: {
    global: _ => RESERVED,
  },

  conflicts: $ => [
    // `import A.B.{...}`: the token after `.` tells whether the path goes on.
    [$.module_path],
  ],

  supertypes: $ => [$._expression, $._pattern, $._type, $._declaration],

  rules: {
    source_file: $ => optional(seq(
      $._top_level_item,
      repeat(seq($._newline, $._top_level_item)),
      optional($._newline),
    )),

    _top_level_item: $ => choice($.import, $._declaration),

    comment: _ => token(/#[^\r\n]*/),

    // Names ([lex.name]).

    identifier: _ => /[a-z_][A-Za-z0-9_]*/,
    type_identifier: _ => /[A-Z][A-Z0-9]*[a-z][A-Za-z0-9]*/,
    constant_identifier: _ => /[A-Z][A-Z0-9_]*/,

    // Modules and declarations ([grammar.decl]).

    import: $ => seq(
      'import',
      field('module', $.module_path),
      optional(seq('.', $.import_list)),
    ),

    module_path: $ => seq(
      $.type_identifier,
      repeat(seq('.', $.type_identifier)),
    ),

    import_list: $ => seq(
      '{',
      commaSep(choice($.identifier, $.type_identifier, $.constant_identifier)),
      '}',
    ),

    _declaration: $ => choice(
      $.function_declaration,
      $.type_declaration,
      $.enum_declaration,
      $.error_declaration,
      $.const_declaration,
      $.effect_declaration,
      $.handler_declaration,
      $.layer_declaration,
      $.trait_declaration,
      $.impl_declaration,
    ),

    annotation: $ => seq(
      '@',
      field('name', $.identifier),
      optional(seq('(', commaSep($.annotation_argument), ')')),
    ),

    annotation_argument: $ => choice(
      $.identifier,
      $._literal,
      seq(field('name', $.identifier), ':', field('value', $._literal)),
      $.annotation_path,
    ),

    // `@entry(Http.serve)`: the qualified name of a function.
    annotation_path: $ => seq(
      repeat1(seq(field('module', $.type_identifier), '.')),
      field('name', $.identifier),
    ),

    _modifiers: $ => seq(
      repeat1(choice(field('annotation', $.annotation), $.visibility)),
    ),

    visibility: _ => seq('pub', optional('opaque')),

    function_declaration: $ => seq(
      optional($._modifiers),
      $._function_head,
      optional(field('body', $.block)),
    ),

    _function_head: $ => seq(
      optional('suspend'),
      'fn',
      field('name', $.identifier),
      optional(field('type_parameters', $.type_parameters)),
      field('parameters', $.parameters),
      optional(seq(optional($._signature_continues), '->', field('return_type', $._type))),
      optional(seq(optional($._signature_continues), $.fails_clause)),
      optional(seq(optional($._signature_continues), $.use_clause)),
    ),

    parameters: $ => seq('(', commaSep($.parameter), ')'),

    parameter: $ => choice(
      $.self,
      seq(field('name', $.identifier), optional(seq(':', field('type', $._type)))),
    ),

    type_parameters: $ => seq('[', commaSep1($.type_parameter), ']'),

    type_parameter: $ => seq(
      field('name', choice($.constant_identifier, $.type_identifier)),
      optional(seq(':', $._bounds)),
    ),

    _bounds: $ => seq($.type_reference, repeat(seq('+', $.type_reference))),

    type_declaration: $ => seq(
      optional($._modifiers),
      'type',
      field('name', $.type_identifier),
      optional(field('type_parameters', $.type_parameters)),
      optional(field('fields', $.field_list)),
      optional($._derive),
    ),

    enum_declaration: $ => seq(
      optional($._modifiers),
      'enum',
      field('name', $.type_identifier),
      optional(field('type_parameters', $.type_parameters)),
      field('body', $.variant_list),
      optional($._derive),
    ),

    variant_list: $ => seq(
      '{',
      optional(seq(
        $.variant,
        repeat(seq(choice(',', $._newline), $.variant)),
        optional(choice(',', $._newline)),
      )),
      '}',
    ),

    variant: $ => seq(
      field('name', $.type_identifier),
      optional(field('fields', choice($.field_list, $.tuple_field_list))),
    ),

    tuple_field_list: $ => seq('(', commaSep1($._type), ')'),

    error_declaration: $ => seq(
      optional($._modifiers),
      'error',
      field('name', $.type_identifier),
      optional(field('fields', $.field_list)),
      optional($._derive),
    ),

    field_list: $ => seq('(', commaSep($.field), ')'),

    field: $ => seq(
      repeat($.annotation),
      field('name', $.identifier),
      ':',
      field('type', $._type),
    ),

    _derive: $ => seq(optional($._derive_continues), $.derive_clause),

    derive_clause: $ => seq('derive', commaSep1($.type_reference)),

    const_declaration: $ => seq(
      optional($._modifiers),
      'const',
      field('name', $.constant_identifier),
      optional(seq(':', field('type', $._type))),
      '=',
      field('value', $._expression),
    ),

    effect_declaration: $ => seq(
      optional($._modifiers),
      'effect',
      field('name', $.type_identifier),
      choice(
        seq(
          optional(field('type_parameters', $.type_parameters)),
          field('body', alias($._operation_list, $.declaration_list)),
        ),
        $.effect_label,
      ),
    ),

    // A label (8.9).
    effect_label: $ => seq(
      'of',
      field('effect', $.type_reference),
      optional(field('type_arguments', $.type_arguments)),
      optional('default'),
    ),

    _operation_list: $ => itemList($, $.operation),

    operation: $ => seq(repeat($.annotation), $._function_head),

    handler_declaration: $ => seq(
      optional($._modifiers),
      'handler',
      field('name', $.type_identifier),
      optional(field('fields', $.field_list)),
      ':',
      field('effect', $.type_reference),
      optional(field('type_arguments', $.type_arguments)),
      field('body', alias($._handler_item_list, $.declaration_list)),
    ),

    _handler_item_list: $ => itemList($, choice(
      $.handler_init,
      $.handler_release,
      $.function_declaration,
    )),

    handler_init: $ => seq(
      'init',
      optional(field('parameters', $.parameters)),
      field('body', $.block),
    ),

    handler_release: $ => seq('release', field('body', $.block)),

    layer_declaration: $ => seq(
      optional($._modifiers),
      'layer',
      field('name', $.type_identifier),
      field('body', $.supply_list),
    ),

    supply_list: $ => seq(
      '{',
      optional(seq(
        $.supply,
        repeat(seq(choice(',', $._newline), $.supply)),
        optional(choice(',', $._newline)),
      )),
      '}',
    ),

    supply: $ => seq(
      field('handler', $.type_reference),
      optional(field('arguments', $.arguments)),
    ),

    trait_declaration: $ => seq(
      optional($._modifiers),
      'trait',
      field('name', $.type_identifier),
      optional(seq(':', $._bounds)),
      field('body', alias($._method_list, $.declaration_list)),
    ),

    impl_declaration: $ => seq(
      'impl',
      field('type', $.type_reference),
      optional(field('type_parameters', $.type_parameters)),
      ':',
      field('trait', $.type_reference),
      field('body', alias($._method_list, $.declaration_list)),
    ),

    _method_list: $ => itemList($, $.function_declaration),

    // Types ([grammar.type]).

    _type: $ => choice(
      $.type_reference,
      $.generic_type,
      $.type_variable,
      $.self_type,
      $.unit_type,
      $.parenthesized_type,
      $.tuple_type,
      $.function_type,
    ),

    type_reference: $ => prec.right(seq(
      repeat(seq(field('module', $.type_identifier), '.')),
      field('name', $.type_identifier),
    )),

    generic_type: $ => seq(
      field('type', $.type_reference),
      field('type_arguments', $.type_arguments),
    ),

    type_arguments: $ => seq('[', commaSep1($._type_argument), ']'),

    _type_argument: $ => choice($._type, $.effect_set, $.error_set),

    type_variable: $ => $.constant_identifier,

    self_type: _ => 'Self',

    unit_type: _ => seq('(', ')'),

    parenthesized_type: $ => seq('(', $._type, ')'),

    tuple_type: $ => seq('(', $._type, repeat1(seq(',', $._type)), optional(','), ')'),

    function_type: $ => prec.right(seq(
      'fn',
      field('parameters', alias($._parameter_types, $.parameter_types)),
      '->',
      field('return_type', $._type),
      optional($.fails_clause),
      optional($.use_clause),
    )),

    _parameter_types: $ => seq('(', commaSep($._type), ')'),

    fails_clause: $ => seq('fails', $._error_set),

    use_clause: $ => seq('use', $._effect_set),

    _error_set: $ => choice(
      alias(seq('{', '}'), $.error_set),
      $._error_item,
      $.error_set,
    ),

    error_set: $ => seq($._error_item, repeat1(seq('|', $._error_item))),

    _error_item: $ => choice($.type_reference, $.type_variable),

    _effect_set: $ => choice($.effect_set, $._effect_item),

    effect_set: $ => seq('{', commaSep($._effect_item), '}'),

    _effect_item: $ => choice($.type_reference, $.generic_type, $.type_variable),

    // Expressions ([grammar.expr]).

    block: $ => seq(
      '{',
      optional(seq(
        $._statement,
        repeat(seq($._newline, $._statement)),
        optional($._newline),
      )),
      '}',
    ),

    _statement: $ => choice($.binding, $._expression),

    // The left side is read as an expression and checked as a pattern later, as the compiler
    // does.
    binding: $ => seq(
      field('pattern', $._expression),
      optional(seq(':', field('type', $._type))),
      '=',
      field('value', $._expression),
    ),

    _expression: $ => choice(
      $.fail_expression,
      $.assert_expression,
      $.match_expression,
      $.escape_expression,
      $.binary_expression,
      $.unary_expression,
      $.use_expression,
      $.call_expression,
      $.field_expression,
      $._primary_expression,
    ),

    fail_expression: $ => prec.right(-1, seq('fail', $._expression)),

    assert_expression: $ => prec.right(-1, seq('assert', $._expression)),

    match_expression: $ => prec.left(0, seq(
      field('value', $._expression),
      'match',
      field('body', $.match_arms),
    )),

    escape_expression: $ => prec.left(0, seq(
      field('value', $._expression),
      'escape',
      field('body', $.match_arms),
    )),

    match_arms: $ => seq(
      '{',
      optional(seq(
        $.match_arm,
        repeat(seq($._newline, $.match_arm)),
        optional($._newline),
      )),
      '}',
    ),

    match_arm: $ => seq(
      field('pattern', $._pattern),
      optional(seq('if', field('guard', $._expression))),
      '->',
      field('value', $._expression),
    ),

    binary_expression: $ => {
      const table = [
        [PREC.pipe, '|>'],
        [PREC.or, '||'],
        [PREC.and, '&&'],
        [PREC.compare, choice('==', '!=', '<', '<=', '>', '>=')],
        [PREC.add, choice('+', '-')],
        [PREC.multiply, choice('*', '/', '%')],
      ];
      return choice(...table.map(([p, operator]) => prec.left(/** @type {number} */ (p), seq(
        field('left', $._expression),
        field('operator', operator),
        field('right', $._expression),
      ))));
    },

    unary_expression: $ => prec(PREC.unary, seq(
      field('operator', choice('!', '-')),
      field('operand', $._expression),
    )),

    use_expression: $ => prec.right(PREC.unary, seq(
      'use',
      field('effect', $.type_reference),
      optional(field('type_arguments', $.type_arguments)),
    )),

    call_expression: $ => prec(PREC.postfix, seq(
      field('function', $._expression),
      field('arguments', $.arguments),
    )),

    field_expression: $ => prec(PREC.postfix, seq(
      field('value', $._expression),
      '.',
      field('field', choice($.identifier, $.type_identifier)),
    )),

    arguments: $ => seq(
      '(',
      commaSep(choice($._expression, $.named_argument, $.spread)),
      ')',
    ),

    named_argument: $ => seq(
      field('name', $.identifier),
      ':',
      optional(field('value', $._expression)),
    ),

    spread: $ => prec.right(seq('..', optional($._expression))),

    _primary_expression: $ => choice(
      $._literal,
      $.identifier,
      $.constant_identifier,
      $.type_identifier,
      $.self,
      $.wildcard,
      $.unit,
      $.parenthesized_expression,
      $.tuple_expression,
      $.list_expression,
      $.block,
      $.if_expression,
      $.with_expression,
      $.lambda,
    ),

    self: _ => 'self',

    wildcard: _ => '_',

    unit: _ => seq('(', ')'),

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    tuple_expression: $ => seq(
      '(',
      $._expression,
      repeat1(seq(',', $._expression)),
      optional(','),
      ')',
    ),

    list_expression: $ => seq('[', commaSep(choice($._expression, $.spread)), ']'),

    if_expression: $ => prec.right(seq(
      'if',
      field('condition', $._expression),
      field('consequence', $.block),
      optional(seq('else', field('alternative', choice($.block, $.if_expression)))),
    )),

    with_expression: $ => seq(
      'with',
      commaSep1($.with_item),
      field('body', $.block),
    ),

    with_item: $ => seq(
      $.supply,
      optional(seq(
        'by',
        field('provider', $.type_reference),
        optional(choice(
          field('type_arguments', $.type_arguments),
          field('arguments', $.arguments),
        )),
      )),
    ),

    lambda: $ => seq(
      'fn',
      field('parameters', $.lambda_parameters),
      field('body', $.block),
    ),

    lambda_parameters: $ => seq('(', commaSep($.lambda_parameter), ')'),

    lambda_parameter: $ => seq(
      field('pattern', choice($.self, $._pattern)),
      optional(seq(':', field('type', $._type))),
    ),

    // Literals ([grammar.lex]).

    _literal: $ => choice($.integer, $.float, $.string),

    integer: _ => token(choice(
      /0x[0-9a-fA-F][0-9a-fA-F_]*/,
      /0b[01][01_]*/,
      /[0-9][0-9_]*/,
    )),

    float: _ => token(/[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?/),

    string: $ => choice(
      seq(
        '"',
        repeat(choice(
          alias($._string_content, $.string_content),
          $.escape_sequence,
          $.interpolation,
        )),
        '"',
      ),
      seq(
        '"""',
        repeat(choice(
          alias($._multiline_string_content, $.string_content),
          $.escape_sequence,
          $.interpolation,
        )),
        '"""',
      ),
    ),

    escape_sequence: _ => token.immediate(/\\(u\{[0-9a-fA-F]*\}|[^\r\n])/),

    interpolation: $ => seq('#{', $._expression, '}'),

    // Patterns ([grammar.pattern]).

    _pattern: $ => choice(
      $.wildcard,
      $.identifier,
      $._literal,
      $.negative_literal,
      $.constant_identifier,
      $.type_reference,
      $.variant_pattern,
      alias($.unit, $.unit_pattern),
      $.tuple_pattern,
      $.list_pattern,
    ),

    negative_literal: $ => seq('-', choice($.integer, $.float)),

    variant_pattern: $ => seq(
      field('variant', $.type_reference),
      '(',
      commaSep(choice($._pattern, $.field_pattern, $.rest_pattern)),
      ')',
    ),

    field_pattern: $ => seq(
      field('name', $.identifier),
      ':',
      optional(field('pattern', $._pattern)),
    ),

    rest_pattern: $ => seq('..', optional($.identifier)),

    tuple_pattern: $ => seq(
      '(',
      $._pattern,
      repeat1(seq(',', $._pattern)),
      optional(','),
      ')',
    ),

    list_pattern: $ => seq('[', commaSep(choice($._pattern, $.rest_pattern)), ']'),
  },
});

/**
 * Items of a declaration body, one per line: `"{" { item NL } "}"`.
 *
 * @param {GrammarSymbols<string>} $
 * @param {RuleOrLiteral} item
 */
function itemList($, item) {
  return seq(
    '{',
    optional(seq(
      item,
      repeat(seq($._newline, item)),
      optional($._newline),
    )),
    '}',
  );
}

/**
 * @param {RuleOrLiteral} rule
 */
function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)), optional(','));
}

/**
 * @param {RuleOrLiteral} rule
 */
function commaSep(rule) {
  return optional(commaSep1(rule));
}
