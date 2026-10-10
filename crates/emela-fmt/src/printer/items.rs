//! ファイルと宣言（17.3）．この段の宣言は `fn` だけ．

use emela_syntax::SyntaxKind::*;
use emela_syntax::SyntaxNode;
use pretty::RcDoc;

use super::exprs::{newline_before, unbreakable};
use super::{
    Doc, INDENT, Printer, child_token, first_significant_token, has_newline_child,
    significant_children, text,
};
use crate::comments::Leading;

impl Printer {
    /// ファイル全体．宣言の間の空行は1つまで保つ．最後は改行で終える．
    pub(crate) fn root(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        let mut started = false;
        for decl in node.children() {
            if started {
                out = out.append(self.hard());
            }
            if let Some(first) = first_significant_token(&decl) {
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
            FN_DECL | TYPE_DECL | ERROR_DECL | ENUM_DECL | CONST_DECL | EFFECT_DECL
            | HANDLER_DECL | LAYER_DECL | TRAIT_DECL | IMPL_DECL | IMPORT => self.decl_seq(node),
            _ => self.verbatim(node),
        }
    }

    /// 宣言．effect の操作，handler の init と release もこれで出す．
    ///
    /// `@external(...)⏎ [pub] [opaque] type Name[A](x: Int) derive Eq`
    /// `[pub] [suspend] fn name[A](x: Int) -> T fails E use R { ... }`
    ///
    /// 注釈は1つずつ自分の行に置く．型引数，引数，フィールドの並びは名前に付ける．
    pub(crate) fn decl_seq(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        let mut first = true;
        for child in significant_children(node) {
            if child.kind() == ANNOTATION {
                let doc = self.element(&child);
                out = out.append(doc).append(self.hard());
                continue;
            }
            let attached = matches!(
                child.kind(),
                TYPE_PARAM_LIST | PARAM_LIST | FIELD_LIST | TUPLE_FIELD_LIST | IMPORT_LIST | COLON
            );
            if child.kind() == DERIVE_CLAUSE {
                // 元のソースで次の行に書いてあれば次の行に置く．同じ行で収まらなくても次の行へ．
                let on_next_line = first_significant_token(child.as_node().unwrap())
                    .is_some_and(|t| newline_before(&t));
                let brk = self.brk(on_next_line);
                let doc = self.element(&child);
                out = out.append(brk.append(doc).nest(INDENT).group());
                continue;
            }
            if !first && !attached {
                out = out.append(self.space());
            }
            first = false;
            let doc = match child {
                rowan::NodeOrToken::Token(t) => self.tok(&t),
                rowan::NodeOrToken::Node(n) => match n.kind() {
                    BLOCK_EXPR => self.block(&n).assemble(false).group(),
                    // 戻り値と節は折らない．幅を超えるなら先に引数の並びが折れる．
                    RET_TYPE | FAILS_CLAUSE | USE_CLAUSE => {
                        let doc = self.node(&n);
                        unbreakable(&doc)
                    }
                    ITEM_LIST | VARIANT_LIST => self.item_list(&n),
                    _ => self.node(&n),
                },
            };
            out = out.append(doc);
        }
        out
    }

    /// 宣言の本体 `{ ... }`．enum のバリアント，layer のハンドラ，effect，handler，
    /// trait，impl の項目．
    ///
    /// 元が複数行なら1行に1項目で，区切りのカンマは消す（`enum Shape {⏎  Circle⏎  Empty⏎}`）．
    /// 元が1行なら，収まるかぎり `{ Red, Green }` のまま，収まらなければ複数行にする．
    fn item_list(&mut self, node: &SyntaxNode) -> Doc {
        let multi = has_newline_child(node);
        let open = match child_token(node, L_BRACE) {
            Some(t) => self.tok(&t),
            None => RcDoc::nil(),
        };
        let forced = self.pending;
        let first_break = self.brk(multi);
        let mut body = RcDoc::nil();
        let mut count = 0;
        let mut close_token = None;
        let children: Vec<_> = significant_children(node).collect();
        for child in &children {
            match child {
                rowan::NodeOrToken::Token(t) if t.kind() == COMMA => {
                    // 1行のときだけ，平らに出したところで `,` を出す．
                    let last = children
                        .iter()
                        .skip_while(|c| c.as_token() != Some(t))
                        .skip(1)
                        .all(|c| c.kind() == R_BRACE);
                    let doc = if multi || last {
                        RcDoc::nil()
                    } else {
                        RcDoc::nil().flat_alt(text(","))
                    };
                    body = body.append(self.tok_as(t, doc));
                }
                rowan::NodeOrToken::Token(t) if t.kind() == R_BRACE => {
                    close_token = Some(t.clone())
                }
                rowan::NodeOrToken::Token(t) if t.kind() == L_BRACE => {}
                rowan::NodeOrToken::Token(t) => body = body.append(self.tok(t)),
                rowan::NodeOrToken::Node(n) => {
                    if count > 0 {
                        body = body.append(self.brk(multi));
                    }
                    if let Some(first) = first_significant_token(n) {
                        body = body.append(self.leading_lines(&first, count == 0));
                    }
                    let doc = self.item(n);
                    body = body.append(doc);
                    count += 1;
                }
            }
        }
        let mut has_closing = false;
        if let Some(t) = &close_token {
            let items = self.take_leading(t);
            has_closing = items.iter().any(|l| matches!(l, Leading::Comment(_)));
            body = body.append(self.closing_comments(items, count > 0));
        }
        let last_break = self.brk(multi);
        let close = match &close_token {
            Some(t) => self.tok(t),
            None => RcDoc::nil(),
        };
        if count == 0 && !has_closing {
            let inner = if forced {
                RcDoc::hardline()
            } else {
                RcDoc::nil()
            };
            return open.append(inner).append(close);
        }
        open.append(first_break.append(body).nest(INDENT))
            .append(last_break)
            .append(close)
            .group()
    }

    /// 本体の項目．
    fn item(&mut self, node: &SyntaxNode) -> Doc {
        match node.kind() {
            FN_DECL | OP_SIG | HANDLER_INIT | HANDLER_RELEASE => self.decl_seq(node),
            _ => self.node(node),
        }
    }
}
