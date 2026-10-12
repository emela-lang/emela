//! Builds a `pretty` document from the syntax tree.
//!
//! The document is always built in source order, because `tok`, `line` and friends hand along
//! the state (`pending`) that forces a newline after a trailing comment.
//! When two differently shaped documents are built from the same subtree (hugging the last
//! argument, aligning arms), the subtree is first checked to have no comments, and the parts
//! built once are reused.

mod exprs;
mod items;

use std::cell::Cell;
use std::rc::Rc;

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::{SyntaxElement, SyntaxNode, SyntaxToken};
use pretty::RcDoc;

use crate::comments::{Comments, Leading};

pub(crate) type Doc = RcDoc<'static, ()>;

pub(crate) const INDENT: isize = 2;

/// Rendering width used for measuring, wide enough that every group stays flat.
const HUGE: usize = 1 << 30;

pub(crate) struct Printer {
    comments: Comments,
    width: usize,
    /// A trailing comment was just printed, so the next separator must be a newline.
    pending: bool,
    /// The last thing printed ended with a forced newline.
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

    /// Also prints any unclaimed leading comments and the trailing comments of the token.
    pub(crate) fn tok(&mut self, token: &SyntaxToken) -> Doc {
        let text = text(token.text());
        self.tok_as(token, text)
    }

    /// A piece of a multiline string `"""`. The text is printed as is, without adding
    /// indentation after its newlines, because the indentation is part of the value.
    pub(crate) fn raw_tok(&mut self, token: &SyntaxToken) -> Doc {
        let mut doc = RcDoc::nil();
        for (i, line) in token.text().split('\n').enumerate() {
            let line = text(line.strip_suffix('\r').unwrap_or(line));
            if i == 0 {
                doc = doc.append(line);
                continue;
            }
            // Cancel the current indentation and write from column 0. `pretty` takes the
            // indentation after a newline from the next piece, so the newline and the line's
            // contents are un-nested together. An empty line still needs a piece right after the
            // newline, hence the zero-width marker.
            doc = doc.append(RcDoc::nesting(move |indent| {
                RcDoc::hardline()
                    .append(RcDoc::column(|_| RcDoc::nil()))
                    .append(line.clone())
                    .nest(-(indent as isize))
            }));
        }
        self.tok_as(token, doc)
    }

    /// Prints `doc` in place of `token`, handling comments as `tok` does.
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

    /// Forces the next separator to be a newline.
    fn trailing(&mut self, token: &SyntaxToken) -> Doc {
        let mut out = RcDoc::nil();
        for c in self.comments.take_trailing(token) {
            out = out.append(" ").append(text(&c));
            self.pending = true;
        }
        out
    }

    pub(crate) fn line(&mut self) -> Doc {
        self.soft(RcDoc::line())
    }

    pub(crate) fn line_(&mut self) -> Doc {
        self.soft(RcDoc::line_())
    }

    /// A newline instead if a trailing comment was just printed.
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

    pub(crate) fn hard(&mut self) -> Doc {
        self.pending = false;
        self.at_break = true;
        RcDoc::hardline()
    }

    pub(crate) fn brk(&mut self, hard: bool) -> Doc {
        if hard { self.hard() } else { self.line() }
    }

    /// Leading items of a declaration, statement or arm. Call at the start of a line. With
    /// `first`, leading blank lines are dropped.
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

    /// Leading comments of a list element. Blank lines are dropped.
    pub(crate) fn leading_in_list(&mut self, token: &SyntaxToken) -> Doc {
        let items = self
            .comments
            .take_leading(token)
            .into_iter()
            .filter(|l| matches!(l, Leading::Comment(_)))
            .collect();
        self.leading_items(items, true)
    }

    /// Comments before a closing bracket (or the end of the input). Called at the end of the
    /// last element's line. `started` is false when nothing has been printed yet (an empty
    /// block).
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

    /// `None` if the document contains a forced newline.
    pub(crate) fn flat_width(doc: &Doc) -> Option<usize> {
        let s = render(doc, HUGE);
        if s.contains('\n') {
            None
        } else {
            Some(display_width(&s))
        }
    }

    /// Width of the first line when rendered as flat as possible.
    pub(crate) fn first_line_width(doc: &Doc) -> usize {
        let s = render(doc, HUGE);
        display_width(s.split('\n').next().unwrap_or(""))
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn list(
        &mut self,
        node: &SyntaxNode,
        mut element: impl FnMut(&mut Self, &SyntaxNode) -> Doc,
    ) -> Doc {
        let parts = self.list_parts(node, |p, n, _| element(p, n));
        parts.grouped()
    }

    /// The third argument of `element` tells whether it is the last element.
    pub(crate) fn list_parts(
        &mut self,
        node: &SyntaxNode,
        mut element: impl FnMut(&mut Self, &SyntaxNode, bool) -> Doc,
    ) -> ListParts {
        let mut prefix = Vec::new();
        let mut open = None;
        let mut close = None;
        // An element is a node or a name token (the `.{Json, decode}` of an import).
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
                    // Tokens before the opening bracket (the `.` of an import).
                    _ if open.is_none() => prefix.push(t),
                    _ => elements.push((rowan::NodeOrToken::Token(t), None)),
                },
                node => elements.push((node, None)),
            }
        }
        // Effect-set braces get inner spaces (`{ Io, Clock }`); an import's `.{a, b}` does not.
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
                // A comma cannot follow a trailing comment (it would become part of the
                // comment). The comment forces the list to break anyway, so print no comma.
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

/// Every element except the last includes its separating comma.
pub(crate) struct ListParts {
    pub(crate) open: Doc,
    pub(crate) breaks: Vec<Doc>,
    pub(crate) elements: Vec<Doc>,
    /// Printed only when the list breaks.
    pub(crate) last_comma: Doc,
    /// Comments before the closing bracket.
    pub(crate) closing: Doc,
    pub(crate) end_break: Doc,
    pub(crate) close: Doc,
}

impl ListParts {
    /// One line if it fits; otherwise one element per line with a trailing comma.
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

/// Children other than trivia and NEWLINE.
pub(crate) fn significant_children(node: &SyntaxNode) -> impl Iterator<Item = SyntaxElement> {
    node.children_with_tokens()
        .filter(|e| !e.kind().is_trivia() && e.kind() != NEWLINE)
}

/// Declaration nodes contain the preceding `##` and newline, so leading items are looked up
/// by this token rather than by `first_token`.
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
        .expect("writing to a String does not fail");
    out
}

/// Display width in a terminal. Measured by `pretty` itself so it counts the same way.
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
