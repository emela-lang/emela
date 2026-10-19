//! Lexer, parser and lossless syntax tree, including error recovery.
//!
//! The tree is a rowan green/red tree. Whitespace, newlines and comments stay in it as tokens.

pub mod ast;
mod diagnostic;
mod lexer;
mod parser;
pub mod rewrite;
mod syntax_kind;
mod validation;

use std::fmt::Write;

use rowan::{GreenNode, NodeOrToken};

pub use diagnostic::{Diagnostic, DiagnosticCode};
pub use lexer::{Lexed, Token, lex};
pub use syntax_kind::SyntaxKind;

/// A tree can be built for any input, and its text always equals the input.
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    diagnostics: Vec<Diagnostic>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn tree(&self) -> ast::SourceFile {
        ast::AstNode::cast(self.syntax()).expect("the root is always ROOT")
    }

    /// Lexer diagnostics come first, then parser diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Dumps the tree one element per line, followed by the diagnostics as
    /// `error[E0101]@start..end: message`.
    pub fn debug_dump(&self) -> String {
        let mut out = debug_tree(&self.syntax());
        for diagnostic in &self.diagnostics {
            writeln!(
                out,
                "error[{}]@{:?}: {}",
                diagnostic.code, diagnostic.range, diagnostic.message
            )
            .unwrap();
        }
        out
    }
}

pub fn parse(src: &str) -> Parse {
    let lexed = lex(src);
    let mut p = parser::Parser::new(src, &lexed.tokens);
    parser::root(&mut p);
    let (green, mut parse_diagnostics) = parser::build(src, &lexed.tokens, p.finish());
    parse_diagnostics.extend(validation::validate(&SyntaxNode::new_root(green.clone())));
    // Parser and validation diagnostics are sorted by position, keeping emission order for ties.
    parse_diagnostics.sort_by_key(|d| (d.range.start(), d.range.end()));
    let mut diagnostics = lexed.diagnostics;
    diagnostics.extend(parse_diagnostics);
    Parse { green, diagnostics }
}

/// Nodes as `KIND@start..end`, tokens as `KIND@start..end "text"`, indented two spaces per level.
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
