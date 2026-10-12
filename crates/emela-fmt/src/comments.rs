//! Attaches comments and blank lines to significant tokens.
//!
//! A comment runs to the end of its line, so it either shares a line with the preceding token
//! (a trailing comment) or sits on its own line (a line comment).
//! - A trailing comment attaches to the preceding significant token.
//! - Line comments and blank lines attach to the following significant token, or to the end of
//!   the input if there is none.
//!
//! Significant tokens are those other than trivia and statement-separating NEWLINEs. The printer
//! claims leading items at structural boundaries (declarations, statements, arms, list elements);
//! anything left unclaimed is printed with its token. Either way, each item is printed once.

use std::collections::HashMap;

use emela_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use rowan::TextSize;

#[derive(Debug, Clone)]
pub(crate) enum Leading {
    Comment(String),
    /// Consecutive blank lines are collapsed into one.
    Blank,
}

pub(crate) struct Comments {
    leading: HashMap<TextSize, Vec<Leading>>,
    trailing: HashMap<TextSize, Vec<String>>,
    /// Items after the last token attach to this position.
    eof: TextSize,
}

impl Comments {
    pub(crate) fn new(root: &SyntaxNode) -> Self {
        let mut comments = Comments {
            leading: HashMap::new(),
            trailing: HashMap::new(),
            eof: root.text_range().end(),
        };
        let mut prev: Option<TextSize> = None;
        let mut newlines = 0usize;
        let mut items = Vec::new();
        let tokens = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token());
        for token in tokens {
            match token.kind() {
                SyntaxKind::WHITESPACE => {}
                SyntaxKind::NEWLINE | SyntaxKind::NEWLINE_CONT => newlines += 1,
                SyntaxKind::COMMENT | SyntaxKind::DOC_COMMENT => {
                    let text = token.text().trim_end().to_owned();
                    match prev {
                        Some(p) if newlines == 0 && items.is_empty() => {
                            comments.trailing.entry(p).or_default().push(text);
                        }
                        _ => {
                            if newlines >= 2 && (prev.is_some() || !items.is_empty()) {
                                items.push(Leading::Blank);
                            }
                            items.push(Leading::Comment(text));
                        }
                    }
                    newlines = 0;
                }
                _ => {
                    let start = token.text_range().start();
                    comments.flush(start, &mut items, newlines, prev.is_some());
                    prev = Some(start);
                    newlines = 0;
                }
            }
        }
        let eof = comments.eof;
        comments.flush(eof, &mut items, newlines, prev.is_some());
        comments
    }

    fn flush(&mut self, at: TextSize, items: &mut Vec<Leading>, newlines: usize, has_prev: bool) {
        if newlines >= 2 && (has_prev || !items.is_empty()) {
            items.push(Leading::Blank);
        }
        if !items.is_empty() {
            self.leading.insert(at, std::mem::take(items));
        }
    }

    /// Later calls for the same token return nothing.
    pub(crate) fn take_leading(&mut self, token: &SyntaxToken) -> Vec<Leading> {
        self.leading
            .remove(&token.text_range().start())
            .unwrap_or_default()
    }

    /// Comments after the last declaration.
    pub(crate) fn take_leading_eof(&mut self) -> Vec<Leading> {
        self.leading.remove(&self.eof).unwrap_or_default()
    }

    /// Does not claim them.
    pub(crate) fn has_leading_comment(&self, token: &SyntaxToken) -> bool {
        self.leading
            .get(&token.text_range().start())
            .is_some_and(|items| items.iter().any(|l| matches!(l, Leading::Comment(_))))
    }

    pub(crate) fn take_trailing(&mut self, token: &SyntaxToken) -> Vec<String> {
        self.trailing
            .remove(&token.text_range().start())
            .unwrap_or_default()
    }

    /// Checked to be zero after printing, so no comment is silently dropped.
    pub(crate) fn remaining(&self) -> usize {
        let leading = self
            .leading
            .values()
            .flatten()
            .filter(|l| matches!(l, Leading::Comment(_)))
            .count();
        leading + self.trailing.values().map(Vec::len).sum::<usize>()
    }
}

/// Whether a comment sits outside every block and arm list. Blocks print the newline after
/// their own comments, so turning an outer separator into a space cannot break them.
pub(crate) fn has_comments_outside_blocks(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens()
        .filter(|e| matches!(e.kind(), SyntaxKind::COMMENT | SyntaxKind::DOC_COMMENT))
        .any(|comment| {
            !comment.ancestors().take_while(|a| a != node).any(|a| {
                matches!(
                    a.kind(),
                    SyntaxKind::BLOCK_EXPR | SyntaxKind::MATCH_ARM_LIST
                )
            })
        })
}

/// Excludes comments before the first token and after the last one.
pub(crate) fn has_comments(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens()
        .any(|e| matches!(e.kind(), SyntaxKind::COMMENT | SyntaxKind::DOC_COMMENT))
}
