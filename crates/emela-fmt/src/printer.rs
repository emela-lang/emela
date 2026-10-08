//! 構文木から `pretty` の文書を組む．
//!
//! 文書は必ずソースの順に組む．トークンを出す `tok` と改行を出す `line` などが，行末の
//! コメントの後に改行を強制するための状態（`pending`）を順に受け渡すため．
//! 同じ部分木から形の違う文書を2つ作る（引数の抱え込み，腕の揃え）ときは，部分木に
//! コメントがないことを確かめてから，一度組んだ部品を使い回す．

mod exprs;
mod items;

use std::cell::Cell;
use std::rc::Rc;

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::{SyntaxElement, SyntaxNode, SyntaxToken};
use pretty::RcDoc;

use crate::comments::{Comments, Leading};

pub(crate) type Doc = RcDoc<'static, ()>;

/// 字下げの幅．
pub(crate) const INDENT: isize = 2;

/// 幅を測るときの描画幅．どの group も平らに出る．
const HUGE: usize = 1 << 30;

pub(crate) struct Printer {
    comments: Comments,
    width: usize,
    /// 行末のコメントを出した直後．次の区切りは必ず改行にする．
    pending: bool,
    /// 直前に出したものが強制の改行で終わっている．
    at_break: bool,
}

impl Printer {
    pub(crate) fn new(root: &SyntaxNode, width: usize) -> Self {
        Printer {
            comments: Comments::new(root),
            width,
            pending: false,
            at_break: true,
        }
    }

    pub(crate) fn remaining_comments(&self) -> usize {
        self.comments.remaining()
    }

    // ---- トークンと区切り ----

    /// トークンを出す．引き取られていない前置きのコメントと，行末のコメントもここで出す．
    pub(crate) fn tok(&mut self, token: &SyntaxToken) -> Doc {
        let text = text(token.text());
        self.tok_as(token, text)
    }

    /// トークンの位置に `doc` を出す．コメントの扱いは `tok` と同じ．
    pub(crate) fn tok_as(&mut self, token: &SyntaxToken, doc: Doc) -> Doc {
        let mut out = RcDoc::nil();
        if self.pending {
            out = out.append(RcDoc::hardline());
            self.pending = false;
            self.at_break = true;
        }
        let leading: Vec<String> = self
            .comments
            .take_leading(token)
            .into_iter()
            .filter_map(|l| match l {
                Leading::Comment(c) => Some(c),
                Leading::Blank => None,
            })
            .collect();
        if !leading.is_empty() {
            if !self.at_break {
                out = out.append(RcDoc::hardline());
            }
            for c in leading {
                out = out.append(text(&c)).append(RcDoc::hardline());
            }
        }
        out = out.append(doc);
        self.at_break = false;
        out.append(self.trailing(token))
    }

    /// 行末のコメント．出したら次の区切りを改行にする．
    fn trailing(&mut self, token: &SyntaxToken) -> Doc {
        let mut out = RcDoc::nil();
        for c in self.comments.take_trailing(token) {
            out = out.append(" ").append(text(&c));
            self.pending = true;
        }
        out
    }

    /// group が平らなら空白，折れたら改行．
    pub(crate) fn line(&mut self) -> Doc {
        self.soft(RcDoc::line())
    }

    /// group が平らなら何もなし，折れたら改行．
    pub(crate) fn line_(&mut self) -> Doc {
        self.soft(RcDoc::line_())
    }

    /// 空白1つ．行末のコメントの直後なら改行．
    pub(crate) fn space(&mut self) -> Doc {
        self.soft(text(" "))
    }

    fn soft(&mut self, doc: Doc) -> Doc {
        if self.pending {
            return self.hard();
        }
        self.at_break = false;
        doc
    }

    /// 必ず改行する．
    pub(crate) fn hard(&mut self) -> Doc {
        self.pending = false;
        self.at_break = true;
        RcDoc::hardline()
    }

    /// `hard` が真なら改行，偽なら `line`．
    pub(crate) fn brk(&mut self, hard: bool) -> Doc {
        if hard { self.hard() } else { self.line() }
    }

    // ---- 構造の境目のコメント ----

    /// 宣言・文・腕の前置き．行頭にいるところで呼ぶ．`first` なら先頭の空行を捨てる．
    pub(crate) fn leading_lines(&mut self, token: &SyntaxToken, first: bool) -> Doc {
        let items = self.comments.take_leading(token);
        self.leading_items(items, first)
    }

    fn leading_items(&mut self, items: Vec<Leading>, first: bool) -> Doc {
        let mut out = RcDoc::nil();
        let mut emitted = !first;
        for item in items {
            match item {
                Leading::Blank if emitted => out = out.append(RcDoc::hardline()),
                Leading::Blank => {}
                Leading::Comment(c) => {
                    out = out.append(text(&c)).append(RcDoc::hardline());
                    emitted = true;
                    self.at_break = true;
                }
            }
        }
        out
    }

    /// 並びの要素の前置き．空行は捨てる．
    pub(crate) fn leading_in_list(&mut self, token: &SyntaxToken) -> Doc {
        let items = self
            .comments
            .take_leading(token)
            .into_iter()
            .filter(|l| matches!(l, Leading::Comment(_)))
            .collect();
        self.leading_items(items, true)
    }

    /// 閉じ括弧（または入力の終わり）の前のコメント．最後の要素の行末から呼ぶ．
    /// `started` が偽なら，まだ何も出していない（空のブロック）．
    pub(crate) fn closing_comments(&mut self, items: Vec<Leading>, started: bool) -> Doc {
        let mut out = RcDoc::nil();
        let mut blank = false;
        let mut any = started;
        for item in items {
            match item {
                Leading::Blank => blank = any,
                Leading::Comment(c) => {
                    if any {
                        out = out.append(RcDoc::hardline());
                        if blank {
                            out = out.append(RcDoc::hardline());
                        }
                    }
                    out = out.append(text(&c));
                    any = true;
                    blank = false;
                    self.pending = true;
                }
            }
        }
        out
    }

    pub(crate) fn take_leading(&mut self, token: &SyntaxToken) -> Vec<Leading> {
        self.comments.take_leading(token)
    }

    pub(crate) fn take_leading_eof(&mut self) -> Vec<Leading> {
        self.comments.take_leading_eof()
    }

    // ---- 幅 ----

    /// 平らに出したときの幅．強制の改行を含めば None．
    pub(crate) fn flat_width(doc: &Doc) -> Option<usize> {
        let s = render(doc, HUGE);
        if s.contains('\n') {
            None
        } else {
            Some(display_width(&s))
        }
    }

    /// できるだけ平らに出したときの1行目の幅．
    pub(crate) fn first_line_width(doc: &Doc) -> usize {
        let s = render(doc, HUGE);
        display_width(s.split('\n').next().unwrap_or(""))
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    // ---- 並び ----

    /// 括弧で囲んだカンマ区切りの並び．
    pub(crate) fn list(
        &mut self,
        node: &SyntaxNode,
        mut element: impl FnMut(&mut Self, &SyntaxNode) -> Doc,
    ) -> Doc {
        let parts = self.list_parts(node, |p, n, _| element(p, n));
        parts.grouped()
    }

    /// 並びを部品に分けて組む．`element` の3つ目の引数は最後の要素か．
    pub(crate) fn list_parts(
        &mut self,
        node: &SyntaxNode,
        mut element: impl FnMut(&mut Self, &SyntaxNode, bool) -> Doc,
    ) -> ListParts {
        let mut prefix = Vec::new();
        let mut open = None;
        let mut close = None;
        // 要素はノードか，名前のトークン（import の `.{Json, decode}`）．
        let mut elements: Vec<(SyntaxElement, Option<SyntaxToken>)> = Vec::new();
        for child in significant_children(node) {
            match child {
                rowan::NodeOrToken::Token(t) => match t.kind() {
                    COMMA => {
                        if let Some(last) = elements.last_mut() {
                            last.1 = Some(t);
                        }
                    }
                    L_PAREN | L_BRACK | L_BRACE if open.is_none() => open = Some(t),
                    R_PAREN | R_BRACK | R_BRACE => close = Some(t),
                    // 開き括弧の前のトークン（import の `.`）．
                    _ if open.is_none() => prefix.push(t),
                    _ => elements.push((rowan::NodeOrToken::Token(t), None)),
                },
                node => elements.push((node, None)),
            }
        }
        // 効果の集合の波括弧は内側に空白を置く．`{ Io, Clock }`．import の `.{a, b}` は置かない．
        let braces =
            open.as_ref().is_some_and(|t| t.kind() == L_BRACE) && node.kind() != IMPORT_LIST;
        let mut open_doc = RcDoc::nil();
        for t in &prefix {
            open_doc = open_doc.append(self.tok(t));
        }
        if let Some(t) = &open {
            open_doc = open_doc.append(self.tok(t));
        }
        let n = elements.len();
        let mut breaks = Vec::with_capacity(n);
        let mut docs = Vec::with_capacity(n);
        let mut last_comma = RcDoc::nil();
        for (i, (elem, comma)) in elements.iter().enumerate() {
            let brk = if i == 0 && !braces {
                self.line_()
            } else {
                self.line()
            };
            breaks.push(brk);
            let first = match elem {
                rowan::NodeOrToken::Node(n) => first_significant_token(n),
                rowan::NodeOrToken::Token(t) => Some(t.clone()),
            };
            let lead = match first {
                Some(t) => self.leading_in_list(&t),
                None => RcDoc::nil(),
            };
            let last = i + 1 == n;
            let elem_doc = match elem {
                rowan::NodeOrToken::Node(n) => element(self, n, last),
                rowan::NodeOrToken::Token(t) => self.tok(t),
            };
            let mut doc = lead.append(elem_doc);
            let trailing_comma = text(",").flat_alt(RcDoc::nil());
            match (comma, last) {
                (Some(c), false) => doc = doc.append(self.tok(c)),
                (Some(c), true) => last_comma = self.tok_as(c, trailing_comma),
                // 行末のコメントの後ろにはカンマを足せない（コメントの一部になる）．
                // 並びはそのコメントで必ず折れるので，カンマなしで出す．
                (None, true) if self.pending => {}
                (None, true) => last_comma = trailing_comma,
                (None, false) => {}
            }
            docs.push(doc);
        }
        let closing = match &close {
            Some(t) => {
                let items = self.take_leading(t);
                let has = items.iter().any(|l| matches!(l, Leading::Comment(_)));
                let start = if n == 0 && has {
                    self.hard()
                } else {
                    RcDoc::nil()
                };
                start.append(self.closing_comments(items, n > 0))
            }
            None => RcDoc::nil(),
        };
        let end_break = if braces && n > 0 {
            self.line()
        } else {
            self.line_()
        };
        let close_doc = close.as_ref().map_or_else(RcDoc::nil, |t| self.tok(t));
        ListParts {
            open: open_doc,
            breaks,
            elements: docs,
            last_comma,
            closing,
            end_break,
            close: close_doc,
        }
    }
}

/// 並びの部品．`elements` の最後以外は区切りのカンマを含む．
pub(crate) struct ListParts {
    pub(crate) open: Doc,
    pub(crate) breaks: Vec<Doc>,
    pub(crate) elements: Vec<Doc>,
    /// 最後の要素の後ろのカンマ．折れたときだけ出る．
    pub(crate) last_comma: Doc,
    /// 閉じ括弧の前のコメント．
    pub(crate) closing: Doc,
    pub(crate) end_break: Doc,
    pub(crate) close: Doc,
}

impl ListParts {
    /// 収まれば1行，収まらなければ1要素1行で末尾にカンマ．
    pub(crate) fn grouped(&self) -> Doc {
        if self.elements.is_empty() {
            let inner = self.closing.clone();
            return self
                .open
                .clone()
                .append(inner.nest(INDENT))
                .append(self.end_break.clone())
                .append(self.close.clone())
                .group();
        }
        let mut inner = RcDoc::nil();
        for (b, e) in self.breaks.iter().zip(&self.elements) {
            inner = inner.append(b.clone()).append(e.clone());
        }
        inner = inner
            .append(self.last_comma.clone())
            .append(self.closing.clone());
        self.open
            .clone()
            .append(inner.nest(INDENT))
            .append(self.end_break.clone())
            .append(self.close.clone())
            .group()
    }
}

// ---- 木の補助 ----

/// トリビアと NEWLINE を除いた子．
pub(crate) fn significant_children(node: &SyntaxNode) -> impl Iterator<Item = SyntaxElement> {
    node.children_with_tokens()
        .filter(|e| !e.kind().is_trivia() && e.kind() != NEWLINE)
}

/// ノードの最初の意味のあるトークン．宣言のノードは前の `##` と改行を中に含むので，
/// `first_token` ではなくこれで前置きを引く．
pub(crate) fn first_significant_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia() && t.kind() != NEWLINE)
}

pub(crate) fn child_token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}

pub(crate) fn has_newline_child(node: &SyntaxNode) -> bool {
    child_token(node, NEWLINE).is_some()
}

pub(crate) fn text(s: &str) -> Doc {
    RcDoc::text(s.to_owned())
}

pub(crate) fn render(doc: &Doc, width: usize) -> String {
    let mut out = String::new();
    doc.render_fmt(width, &mut out)
        .expect("String への書き込みは失敗しない");
    out
}

/// 端末での表示幅．`pretty` と同じ数え方にするため，`pretty` に測らせる．
fn display_width(s: &str) -> usize {
    let col = Rc::new(Cell::new(0));
    let sink = col.clone();
    let doc: Doc = text(s).append(RcDoc::column(move |c| {
        sink.set(c);
        RcDoc::nil()
    }));
    render(&doc, HUGE);
    col.get()
}
