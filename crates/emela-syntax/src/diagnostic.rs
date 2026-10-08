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
    /// 来るはずのトークンがない．
    ExpectedToken,
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
            Self::ExpectedToken => "E0110",
        }
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
