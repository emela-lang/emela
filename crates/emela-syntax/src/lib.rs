//! 字句解析，パーサ，無損失構文木．エラー回復もここで行う．
//!
//! 構文木は rowan の green/red tree．空白，改行，コメントもトークンとして木に残す．

mod lexer;
mod syntax_kind;

pub use lexer::{Diagnostic, Lexed, Token, lex};
pub use syntax_kind::SyntaxKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EmelaLanguage {}

impl rowan::Language for EmelaLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::from_raw(raw.0)
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<EmelaLanguage>;
pub type SyntaxToken = rowan::SyntaxToken<EmelaLanguage>;
pub type SyntaxElement = rowan::SyntaxElement<EmelaLanguage>;
