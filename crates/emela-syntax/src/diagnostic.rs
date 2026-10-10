//! 診断．文面は英語で，コードは仕様の付録 A に従う．

use rowan::TextRange;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub code: DiagnosticCode,
    pub message: String,
}

/// 診断の種類．番号は付録 A の一覧と一致させ，一度振った番号は使い回さない．
///
/// E0101〜E0109 は字句，E0110〜E0199 は構文．
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    /// 文字列が閉じていない．
    UnterminatedString,
    /// 補間 `#{` が閉じていない．
    UnterminatedInterpolation,
    /// どの名前のクラスにも合わない（2.3）．
    InvalidName,
    /// 数値の形になっていない．
    InvalidNumber,
    /// 認識できない文字．
    UnrecognizedCharacter,
    /// `\` の後ろに文字がない．
    MissingEscapedCharacter,
    /// 使えないエスケープ．
    UnknownEscape,
    /// `\u{...}` の形が崩れている，または値がサロゲートか 10FFFF を超える．
    InvalidUnicodeEscape,
    /// 来るはずのトークンがない．
    ExpectedToken,
    /// 型が来るはずの位置に型がない．
    ExpectedType,
    /// 要素が1つのタプル．`(Int)` `(a,)`
    SingleElementTuple,
    /// パターンが来るはずの位置にパターンがない．
    ExpectedPattern,
    /// `..` の後ろに要素がある．
    RestNotLast,
    /// リストの `..` の前に要素がない．`[..rest]`
    RestWithoutElements,
    /// パターンの負の数（18.1 #17 で未決）．
    NegativeNumberPattern,
    /// パターンの文字列に補間がある．
    InterpolationInPattern,
    /// 式が来るはずの位置に式がない．
    ExpectedExpression,
    /// 比較演算子の連鎖．`a < b < c`
    ChainedComparison,
    /// 名前付き引数の後ろの位置引数．
    PositionalAfterNamed,
    /// 括弧で囲まずに `use X` の操作を呼んでいる．`use Clock.sleep(1)`
    UnparenthesizedUse,
    /// 文の後ろに改行か `}` がない．`a b`
    ExpectedStatementEnd,
    /// パターンの外の `_`．
    UnderscoreOutsidePattern,
    /// パターンの外の，式のない `..`．
    BareRestOutsidePattern,
    /// 束縛の左辺がパターンとして読めない．`a + b = 1`
    InvalidBindingTarget,
    /// 宣言が来るはずの位置に宣言がない．
    ExpectedDeclaration,
    /// 宣言の後ろの import（4.2）．
    ImportAfterDeclaration,
    /// 1つのバリアントで名前付きと名前なしのフィールドを混ぜている（5.4）．
    MixedVariantFields,
    /// trait の導出規則．構文が未確定（18.1 #3）．
    DeriveRuleUnsupported,
    /// `@external` が要る宣言に付いていない．本体のない fn，トップレベルの suspend fn，
    /// フィールドのない type．
    MissingExternal,
    /// `opaque` を type と enum 以外に付けた，または `pub` なしで付けた．
    MisplacedOpaque,
    /// バリアントのない enum．
    EmptyEnum,
    /// impl に `pub` や注釈を付けた．
    ModifierOnImpl,
    /// handler の操作の引数に型を書いた．型は effect の宣言から決まる．
    TypedHandlerParam,
    /// impl と handler の fn に本体がない．
    MissingBody,
    /// impl の fn に戻り値，fails，use を書いた．型は trait の宣言から決まる．
    SignatureInImpl,
}

impl DiagnosticCode {
    /// `E0101` の形のコード．
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
            Self::ExpectedToken => "E0110",
            Self::ExpectedType => "E0111",
            Self::SingleElementTuple => "E0112",
            Self::ExpectedPattern => "E0113",
            Self::RestNotLast => "E0114",
            Self::RestWithoutElements => "E0115",
            Self::NegativeNumberPattern => "E0116",
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
        }
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
