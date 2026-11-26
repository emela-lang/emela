//! Diagnostics. Messages are in English; codes follow spec appendix A.

use rowan::TextRange;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub code: DiagnosticCode,
    pub message: String,
    /// Printed as `= help: …` by the driver.
    pub help: Option<String>,
}

/// Codes match the list in spec appendix A. A code once assigned is never reused.
///
/// Bands (spec `[diag.bands]`): E10xx lexical, E11xx syntax of expressions, patterns and types,
/// E12xx syntax of declarations, E13xx syntax of attributes, E14xx forms that are no longer
/// written. A code names one kind of mistake; the message tells its forms apart.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    UnterminatedString,
    /// An interpolation `#{` is not closed.
    UnterminatedInterpolation,
    /// Matches none of the name classes (spec 2.3).
    InvalidName,
    InvalidNumber,
    UnrecognizedCharacter,
    /// Nothing follows `\`.
    MissingEscapedCharacter,
    UnknownEscape,
    /// `\u{...}` is malformed, or its value is a surrogate or above 10FFFF.
    InvalidUnicodeEscape,
    /// The opening `"""` is not followed by a newline, the closing `"""` is not alone on its
    /// line, or a line is indented less than the closing line. These are one kind of mistake.
    MalformedMultilineString,
    ExpectedToken,
    /// No type, pattern, expression or declaration where one is expected. The message names
    /// which.
    ExpectedNode,
    /// `(Int)` `(a,)`
    SingleElementTuple,
    /// `..` in a list is not last, or has no element before it (`[..rest]`).
    MisplacedListRest,
    InterpolationInPattern,
    /// `a < b < c`
    ChainedComparison,
    PositionalAfterNamed,
    /// Calling an operation of `use X` without parentheses. `use Clock.sleep(1)`
    UnparenthesizedUse,
    /// `<-` after an operand (`a<-1`), or the right side of `x <- R` that is not an effect name
    /// (`x <- 1`, `[grammar.lex]`).
    LeftArrow,
    /// A statement not followed by a newline or `}`. `a b`
    ExpectedStatementEnd,
    /// `_`, or a bare `..` (without an expression), outside a pattern.
    PatternFormOutsidePattern,
    /// The left side of a binding cannot be read as a pattern. `a + b = 1`
    InvalidBindingTarget,
    /// An import after a declaration (spec 4.2).
    ImportAfterDeclaration,
    /// Named and unnamed fields mixed in one variant (spec 5.4).
    MixedVariantFields,
    /// Trait derive rules; the syntax is undecided (spec 18.1 #3).
    DeriveRuleUnsupported,
    /// A declaration that needs `@external` lacks it: a fn without a body, a top-level
    /// suspend fn, or a type without fields.
    MissingExternal,
    /// `opaque` on something other than type and enum, or without `pub`.
    MisplacedOpaque,
    /// An enum with no variants.
    EmptyEnum,
    /// `pub` or an annotation on an impl.
    ModifierOnImpl,
    /// A type on a handler operation's parameter. The type comes from the effect declaration.
    TypedHandlerParam,
    /// An impl or handler fn without a body.
    MissingBody,
    /// A return type, fails or uses on an impl fn. The types come from the trait declaration.
    SignatureInImpl,
    /// A `uses` clause on an effect operation (spec 8.1).
    UseOnEffectOp,
    /// Malformed arguments of an attribute: a fn's `@external` arguments are not
    /// `(target, "a")` or `(target, "a", "b")` (spec 12.1), `@external type` has arguments
    /// (spec 12.3), or `@export` has malformed arguments or is on something other than a
    /// top-level fn with a body (spec 12.7, 16.6.2).
    MalformedAttributeArgs,
    /// Two or more `@external` for the same target on one fn (spec 12.1).
    DuplicateExternalTarget,
    /// An `@external` target that is not a known name (spec 12.1).
    UnknownExternalTarget,
    /// `match` written before the value. `match x { ... }` (spec 6.5)
    PrefixMatch,
    /// The old head of an impl. `impl Show for User`, `impl[A: Eq] Box[A]: Eq` (spec 10.2)
    OldImplHead,
    /// `implements` in a handler head. `handler H implements E` (spec 8.4)
    OldHandlerHead,
    /// `fn(...)` written as an expression, the old form of an anonymous function (`[fn.decl]`).
    OldLambda,
    /// An anonymous function as a statement that is not the last of its block (`[fn.block]`).
    UnusedLambda,
    /// `use` written in the place of the uses clause. `fn f() use Db` (`[grammar.decl]`)
    OldUseClause,
    /// An attribute, `pub` or `suspend` on a local fn (`[fn.local]`).
    LocalFnModifier,
    /// Type parameters where they cannot be written: on a local fn (`[fn.local]`). The type
    /// checker uses the same code for an operation of a user-defined effect and an inner
    /// handler.
    MisplacedTypeParams,
    /// A local fn as the last statement of its block, which nothing can call (`[fn.local]`).
    UnusedLocalFn,
}

impl DiagnosticCode {
    /// A code of the form `E1001`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnterminatedString => "E1001",
            Self::UnterminatedInterpolation => "E1002",
            Self::InvalidName => "E1003",
            Self::InvalidNumber => "E1004",
            Self::UnrecognizedCharacter => "E1005",
            Self::MissingEscapedCharacter => "E1006",
            Self::UnknownEscape => "E1007",
            Self::InvalidUnicodeEscape => "E1008",
            Self::MalformedMultilineString => "E1009",
            Self::ExpectedToken => "E1101",
            Self::ExpectedNode => "E1102",
            Self::SingleElementTuple => "E1103",
            Self::MisplacedListRest => "E1104",
            Self::InterpolationInPattern => "E1105",
            Self::ChainedComparison => "E1106",
            Self::PositionalAfterNamed => "E1107",
            Self::UnparenthesizedUse => "E1108",
            Self::ExpectedStatementEnd => "E1109",
            Self::PatternFormOutsidePattern => "E1110",
            Self::InvalidBindingTarget => "E1111",
            Self::ImportAfterDeclaration => "E1201",
            Self::MixedVariantFields => "E1202",
            Self::DeriveRuleUnsupported => "E1203",
            Self::MissingExternal => "E1204",
            Self::MisplacedOpaque => "E1205",
            Self::EmptyEnum => "E1206",
            Self::ModifierOnImpl => "E1207",
            Self::TypedHandlerParam => "E1208",
            Self::MissingBody => "E1209",
            Self::SignatureInImpl => "E1210",
            Self::UseOnEffectOp => "E1211",
            Self::MalformedAttributeArgs => "E1301",
            Self::DuplicateExternalTarget => "E1302",
            Self::UnknownExternalTarget => "E1303",
            Self::PrefixMatch => "E1401",
            Self::OldImplHead => "E1402",
            Self::OldHandlerHead => "E1403",
            Self::OldLambda => "E1405",
            Self::UnusedLambda => "E1112",
            Self::OldUseClause => "E1404",
            Self::LocalFnModifier => "E1212",
            Self::MisplacedTypeParams => "E1213",
            Self::UnusedLocalFn => "E1214",
            Self::LeftArrow => "E1113",
        }
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
