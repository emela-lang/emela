//! ファイルと宣言（17.3）．この段の宣言は `fn` だけ．

use emela_syntax::SyntaxKind::*;
use emela_syntax::SyntaxNode;
use pretty::RcDoc;

use super::{Doc, Printer, significant_children};

impl Printer {
    /// ファイル全体．宣言の間の空行は1つまで保つ．最後は改行で終える．
    pub(crate) fn root(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        let mut started = false;
        for decl in node.children() {
            if started {
                out = out.append(self.hard());
            }
            if let Some(first) = decl.first_token() {
                out = out.append(self.leading_lines(&first, !started));
            }
            out = out.append(self.decl(&decl));
            started = true;
        }
        let items = self.take_leading_eof();
        let has_comments = items
            .iter()
            .any(|l| matches!(l, crate::comments::Leading::Comment(_)));
        out = out.append(self.closing_comments(items, started));
        if started || has_comments {
            out = out.append(self.hard());
        }
        out
    }

    fn decl(&mut self, node: &SyntaxNode) -> Doc {
        match node.kind() {
            FN_DECL => self.fn_decl(node),
            _ => self.verbatim(node),
        }
    }

    /// `[pub] [suspend] fn name[A](x: Int) -> T fails E use R { ... }`
    fn fn_decl(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        let mut first = true;
        for child in significant_children(node) {
            // 型引数と引数の並びは名前に付ける．
            let attached = matches!(child.kind(), TYPE_PARAM_LIST | PARAM_LIST);
            if !first && !attached {
                out = out.append(self.space());
            }
            first = false;
            let doc = match child {
                rowan::NodeOrToken::Token(t) => self.tok(&t),
                rowan::NodeOrToken::Node(n) => match n.kind() {
                    BLOCK_EXPR => self.block(&n).assemble(false).group(),
                    _ => self.node(&n),
                },
            };
            out = out.append(doc);
        }
        out
    }
}
