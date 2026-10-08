//! 字句解析，パーサ，無損失構文木．エラー回復もここで行う．
//!
//! 構文木は rowan の green/red tree．空白，改行，コメントもトークンとして木に残す．

mod lexer;
mod parser;
mod syntax_kind;

use std::fmt::Write;

use rowan::{GreenNode, NodeOrToken};

pub use lexer::{Diagnostic, Lexed, Token, lex};
pub use syntax_kind::SyntaxKind;

/// パースの結果．木はどんな入力でも組めて，テキストは入力と一致する．
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    diagnostics: Vec<Diagnostic>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// 字句解析の診断のあとにパースの診断が並ぶ．
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// 木を1要素1行で書き出し，最後に診断を `error@start..end: メッセージ` で並べる．
    pub fn debug_dump(&self) -> String {
        let mut out = debug_tree(&self.syntax());
        for diagnostic in &self.diagnostics {
            writeln!(out, "error@{:?}: {}", diagnostic.range, diagnostic.message).unwrap();
        }
        out
    }
}

pub fn parse(src: &str) -> Parse {
    let lexed = lex(src);
    let mut p = parser::Parser::new(lexed.tokens.iter().map(|t| t.kind));
    parser::root(&mut p);
    let (green, parse_diagnostics) = parser::build(src, &lexed.tokens, p.finish());
    let mut diagnostics = lexed.diagnostics;
    diagnostics.extend(parse_diagnostics);
    Parse { green, diagnostics }
}

/// ノードは `KIND@start..end`，トークンは `KIND@start..end "テキスト"` で，深さ1につき2字下げる．
pub(crate) fn debug_tree(node: &SyntaxNode) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for event in node.preorder_with_tokens() {
        match event {
            rowan::WalkEvent::Enter(element) => {
                let indent = "  ".repeat(depth);
                match element {
                    NodeOrToken::Node(n) => {
                        writeln!(out, "{indent}{:?}@{:?}", n.kind(), n.text_range()).unwrap();
                        depth += 1;
                    }
                    NodeOrToken::Token(t) => {
                        writeln!(
                            out,
                            "{indent}{:?}@{:?} {:?}",
                            t.kind(),
                            t.text_range(),
                            t.text()
                        )
                        .unwrap();
                    }
                }
            }
            rowan::WalkEvent::Leave(NodeOrToken::Node(_)) => depth -= 1,
            rowan::WalkEvent::Leave(NodeOrToken::Token(_)) => {}
        }
    }
    out
}

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
