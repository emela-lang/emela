//! Diagnostics. Messages are in English; codes follow spec appendix A.

use rowan::TextRange;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub code: DiagnosticCode,
    pub message: String,
}

/// Codes match the list in spec appendix A. A code once assigned is never reused.
///
/// E0101–E0109 are lexical, E0110–E0189 syntactic, and E0190–E0199 more lexical ones (added
/// after E0101–E0109 filled up).
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
    /// line, or a line is indented less than the closing line. These share one code because the
    /// lexical band E0101–E0109 had no room left.
    MalformedMultilineString,
    ExpectedToken,
    ExpectedType,
    /// `(Int)` `(a,)`
    SingleElementTuple,
    ExpectedPattern,
    /// Something follows `..`.
    RestNotLast,
    /// No element before `..` in a list. `[..rest]`
    RestWithoutElements,
    InterpolationInPattern,
    ExpectedExpression,
    /// `a < b < c`
    ChainedComparison,
    PositionalAfterNamed,
    /// Calling an operation of `use X` without parentheses. `use Clock.sleep(1)`
    UnparenthesizedUse,
    /// A statement not followed by a newline or `}`. `a b`
    ExpectedStatementEnd,
    UnderscoreOutsidePattern,
    /// A bare `..` (without an expression) outside a pattern.
    BareRestOutsidePattern,
    /// The left side of a binding cannot be read as a pattern. `a + b = 1`
    InvalidBindingTarget,
    ExpectedDeclaration,
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
    /// A return type, fails or use on an impl fn. The types come from the trait declaration.
    SignatureInImpl,
    /// A `use` clause on an effect operation (spec 8.1).
    UseOnEffectOp,
    /// A fn's `@external` arguments are not `(target, "a")` or `(target, "a", "b")` (spec 12.1):
    /// the target is missing or not a name, an argument is not a string, or there are not one or
    /// two strings.
    MalformedExternal,
    /// Two or more `@external` for the same target on one fn (spec 12.1).
    DuplicateExternalTarget,
    /// An `@external` target that is not a known name (spec 12.1).
    UnknownExternalTarget,
    /// Arguments on `@external type` (spec 12.3).
    ExternalTypeWithArgs,
    /// `match` written before the value. `match x { ... }` (spec 6.5)
    PrefixMatch,
    /// The old head of an impl. `impl Show for User`, `impl[A: Eq] Box[A]: Eq` (spec 10.2)
    OldImplHead,
    /// `implements` in a handler head. `handler H implements E` (spec 8.4)
    OldHandlerHead,
}

impl DiagnosticCode {
    /// A code of the form `E0101`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnterminatedString => "E0101",
            Self::UnterminatedInterpolation => "E0102",
            Self::InvalidName => "E0103",
            Self::InvalidNumber => "E0104",
            Self::UnrecognizedCharacter => "E0105",
            Self::MissingEscapedCharacter => "E0106",
            Self::UnknownEscape => "E0107",
            Self::InvalidUnicodeEscape => "E0108",
            Self::MalformedMultilineString => "E0109",
            Self::ExpectedToken => "E0110",
            Self::ExpectedType => "E0111",
            Self::SingleElementTuple => "E0112",
            Self::ExpectedPattern => "E0113",
            Self::RestNotLast => "E0114",
            Self::RestWithoutElements => "E0115",
            Self::InterpolationInPattern => "E0117",
            Self::ExpectedExpression => "E0118",
            Self::ChainedComparison => "E0119",
            Self::PositionalAfterNamed => "E0120",
            Self::UnparenthesizedUse => "E0121",
            Self::ExpectedStatementEnd => "E0122",
            Self::UnderscoreOutsidePattern => "E0123",
            Self::BareRestOutsidePattern => "E0124",
            Self::InvalidBindingTarget => "E0125",
            Self::ExpectedDeclaration => "E0126",
            Self::ImportAfterDeclaration => "E0127",
            Self::MixedVariantFields => "E0128",
            Self::DeriveRuleUnsupported => "E0129",
            Self::MissingExternal => "E0130",
            Self::MisplacedOpaque => "E0131",
            Self::EmptyEnum => "E0132",
            Self::ModifierOnImpl => "E0133",
            Self::TypedHandlerParam => "E0134",
            Self::MissingBody => "E0135",
            Self::SignatureInImpl => "E0136",
            Self::UseOnEffectOp => "E0137",
            Self::MalformedExternal => "E0138",
            Self::DuplicateExternalTarget => "E0139",
            Self::UnknownExternalTarget => "E0140",
            Self::ExternalTypeWithArgs => "E0141",
            Self::PrefixMatch => "E0142",
            Self::OldImplHead => "E0144",
            Self::OldHandlerHead => "E0145",
        }
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
