//! コメントと空行を，意味のあるトークンに割り当てる．
//!
//! コメントは行末までなので，どれも「前のトークンと同じ行にある（行末のコメント）」か
//! 「自分の行にある（行だけのコメント）」かのどちらかになる．
//! - 行末のコメントは，直前の意味のあるトークンに付ける
//! - 行だけのコメントと空行は，直後の意味のあるトークン（なければ入力の終わり）に付ける
//!
//! ここで「意味のあるトークン」は，トリビアと文を区切る改行（NEWLINE）を除いたもの．
//! 整形は，宣言・文・腕・並びの要素といった構造の境目で前置きを引き取り，引き取られずに
//! 残ったものはトークンを出すときに必ず出す．どちらでも一度しか出さない．

use std::collections::HashMap;

use emela_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use rowan::TextSize;

/// トークンの前に置くもの．
#[derive(Debug, Clone)]
pub(crate) enum Leading {
    Comment(String),
    /// 空行．連続する空行は1つにまとめてある．
    Blank,
}

pub(crate) struct Comments {
    leading: HashMap<TextSize, Vec<Leading>>,
    trailing: HashMap<TextSize, Vec<String>>,
    /// 入力の終わりの位置．最後のトークンの後ろにあるものはこの位置に付ける．
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

    /// トークンの前置きを引き取る．二度目からは空．
    pub(crate) fn take_leading(&mut self, token: &SyntaxToken) -> Vec<Leading> {
        self.leading
            .remove(&token.text_range().start())
            .unwrap_or_default()
    }

    /// 入力の終わりの前置き（最後の宣言の後ろのコメント）を引き取る．
    pub(crate) fn take_leading_eof(&mut self) -> Vec<Leading> {
        self.leading.remove(&self.eof).unwrap_or_default()
    }

    /// トークンの前に行だけのコメントがあるか．引き取らない．
    pub(crate) fn has_leading_comment(&self, token: &SyntaxToken) -> bool {
        self.leading
            .get(&token.text_range().start())
            .is_some_and(|items| items.iter().any(|l| matches!(l, Leading::Comment(_))))
    }

    /// トークンの行末のコメントを引き取る．
    pub(crate) fn take_trailing(&mut self, token: &SyntaxToken) -> Vec<String> {
        self.trailing
            .remove(&token.text_range().start())
            .unwrap_or_default()
    }

    /// まだ出していないコメントの数．整形の終わりに0であることを確かめる．
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

/// ブロックと腕の並びの外にコメントがあるか．中のコメントの後の改行はブロックが
/// 自分で出すので，外側の区切りを空白に変えても壊れない．
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

/// ノードの中にコメントがあるか．最初のトークンの前と最後のトークンの後ろは含まない．
pub(crate) fn has_comments(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens()
        .any(|e| matches!(e.kind(), SyntaxKind::COMMENT | SyntaxKind::DOC_COMMENT))
}
