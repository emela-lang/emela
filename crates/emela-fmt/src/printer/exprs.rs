//! Expressions (spec 17.5), types (spec 17.4) and patterns (spec 17.6).

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::{SyntaxElement, SyntaxNode, SyntaxToken, rewrite};
use pretty::RcDoc;

use super::{
    Doc, HUGE, INDENT, Printer, child_token, first_significant_token, has_newline_child, render,
    significant_children, text,
};
use crate::comments::{has_comments, has_comments_outside_blocks};

/// The parts of a block `{ ... }`. The caller decides whether to wrap it in a group.
pub(crate) struct BlockDoc {
    open: Doc,
    first_break: Doc,
    body: Doc,
    last_break: Doc,
    close: Doc,
    /// No statements and no comments: printed as `{}`.
    empty: bool,
    /// Nothing inside, but `{` has a trailing comment: printed as `{ # c⏎}`.
    comment_only: bool,
}

impl BlockDoc {
    /// With `force`, the block breaks even if the source had it on one line.
    pub(crate) fn assemble(&self, force: bool) -> Doc {
        if self.empty {
            return self.open.clone().append(self.close.clone());
        }
        if self.comment_only {
            // Two newlines in a row would make a blank line, so print only one.
            return self
                .open
                .clone()
                .append(RcDoc::hardline())
                .append(self.close.clone());
        }
        let (first, last) = if force {
            (RcDoc::hardline(), RcDoc::hardline())
        } else {
            (self.first_break.clone(), self.last_break.clone())
        };
        self.open
            .clone()
            .append(first.append(self.body.clone()).nest(INDENT))
            .append(last)
            .append(self.close.clone())
    }
}

/// The body of an anonymous function: a block, which an argument list may hug, or an
/// expression.
pub(crate) enum LambdaBody {
    Block(BlockDoc),
    Expr(Doc),
}

/// The parts of a match arm. Head and body are kept apart so that `->` can be aligned.
#[derive(Clone)]
struct ArmDoc {
    /// The separator from the previous arm, and leading comments.
    pre: Doc,
    head: Doc,
    /// Between the head and `->` when the arm is not aligned.
    gap: Doc,
    arrow: Doc,
    after_arrow: Doc,
    body: Doc,
    /// Widths of the head and body, if the arm may be aligned.
    widths: Option<(usize, usize)>,
}

/// Binding strength of a binary operator. A run of operators of equal strength is formatted as
/// one chain.
fn op_class(kind: SyntaxKind) -> u8 {
    match kind {
        PIPE_GT => 1,
        PIPE2 => 2,
        AMP2 => 3,
        EQ2 | NEQ | LT | LTEQ | GT | GTEQ => 4,
        PLUS | MINUS => 5,
        STAR | SLASH | PERCENT => 6,
        _ => 0,
    }
}

impl Printer {
    pub(crate) fn node(&mut self, node: &SyntaxNode) -> Doc {
        match node.kind() {
            PAREN_EXPR if paren_or_chain(node).is_some() => self.paren_or_chain(node),
            PATH | PATH_TYPE | TYPE_VAR | SELF_TYPE | NAME_REF | PATH_EXPR | UNDERSCORE_EXPR
            | PAREN_EXPR | FIELD_EXPR | PREFIX_EXPR | REST_EXPR | LITERAL | PAREN_TYPE
            | SPREAD_ARG | WILDCARD_PAT | IDENT_PAT | CONST_PAT | LITERAL_PAT | VARIANT_PAT
            | REST_PAT | ANNOTATION | ANNOT_PATH => self.seq(node, |_, _| false),
            TYPE_ARG_LIST | PARAM_TYPE_LIST | UNIT_TYPE | TUPLE_TYPE | TYPE_PARAM_LIST
            | PARAM_LIST | UNIT_EXPR | TUPLE_EXPR | LIST_EXPR | PAT_ARG_LIST | UNIT_PAT
            | TUPLE_PAT | LIST_PAT | ANNOT_ARG_LIST | FIELD_LIST | TUPLE_FIELD_LIST
            | IMPORT_LIST => self.list(node, |p, n| p.node(n)),
            VARIANT => self.variant(node),
            FIELD => self.seq(node, |_, cur| cur != COLON),
            DERIVE_CLAUSE => self.seq(node, |_, cur| cur != COMMA),
            // Effect type arguments (`handler H: State[Int]`, `use State[Int]`, `of State[Int]`)
            // attach to the name.
            HANDLER_EFFECT | USE_EXPR | EFFECT_LABEL => {
                self.seq(node, |_, cur| cur != TYPE_ARG_LIST)
            }
            WITH_BY => self.seq(node, |_, cur| cur != TYPE_ARG_LIST),
            // `Map[K: Eq, V: Eq]` in `impl Map[K: Eq, V: Eq]: Eq`.
            IMPL_TYPE => self.seq(node, |_, cur| cur != TYPE_PARAM_LIST),
            RET_TYPE | FAILS_CLAUSE | FAIL_EXPR | ASSERT_EXPR | MATCH_GUARD => {
                self.seq(node, |_, _| true)
            }
            USES_CLAUSE => self.uses_clause(node),
            // No space before `:`.
            PARAM | TYPE_PARAM | NAMED_ARG | FIELD_PAT | BINDING | ANNOT_ARG => {
                self.seq(node, |_, cur| cur != COLON)
            }
            FN_TYPE => self.seq(node, |_, cur| cur != PARAM_TYPE_LIST),
            ERROR_SET => self.seq(node, |prev, cur| prev != L_BRACE && cur != R_BRACE),
            EFFECT_SET if child_token(node, L_BRACE).is_some() => self.list(node, |p, n| p.node(n)),
            EFFECT_SET => self.seq(node, |_, _| true),
            STRING => self.string(node),
            INTERP => self.interp(node),
            // `WorkerKv(binding: "KV")` in `with` and `layer` reads as a call.
            CALL_EXPR | SUPPLY => self.call(node),
            BIN_EXPR => self.bin_chain(node),
            BLOCK_EXPR => self.block(node).assemble(false).group(),
            // A local fn is laid out as a top-level fn, indented in its block (`[fn.local]`).
            LOCAL_FN => self.decl_seq(node),
            IF_EXPR => self.if_chain(node).group(),
            MATCH_EXPR | ESCAPE_EXPR => self.arms_expr(node),
            MATCH_ARM_LIST => self.arm_list(node),
            WITH_EXPR => self.with_expr(node),
            LAMBDA_EXPR => self.lambda(node),
            _ => self.verbatim(node),
        }
    }

    pub(crate) fn element(&mut self, element: &SyntaxElement) -> Doc {
        match element {
            rowan::NodeOrToken::Token(t) => self.tok(t),
            rowan::NodeOrToken::Node(n) => self.node(n),
        }
    }

    /// `expr match { ... }` and `expr escape { ... }`. When the left side is a pipe that breaks
    /// across lines, the keyword starts the next line so the arms do not hang off the last link.
    fn arms_expr(&mut self, node: &SyntaxNode) -> Doc {
        let mut children = significant_children(node);
        let Some(first) = children.next() else {
            return RcDoc::nil();
        };
        let pipe = first.as_node().is_some_and(is_pipe);
        let lhs = self.element(&first);
        let mut rest = RcDoc::nil();
        let mut gap = None;
        for child in children {
            let space = self.space();
            if gap.is_none() {
                gap = Some(space);
            } else {
                rest = rest.append(space);
            }
            rest = rest.append(self.element(&child));
        }
        let gap = gap.unwrap_or_else(RcDoc::nil);
        if !pipe {
            return lhs.append(gap).append(rest);
        }
        let keyword = significant_children(node)
            .nth(1)
            .and_then(|e| e.into_token())
            .map_or(String::new(), |t| t.text().to_owned());
        let inline = lhs.clone().append(gap).append(rest.clone());
        let broken = lhs
            .clone()
            .append(RcDoc::hardline().append(rest).nest(INDENT));
        let width = self.width();
        RcDoc::column(move |col| {
            let (lhs, inline, broken, keyword) =
                (lhs.clone(), inline.clone(), broken.clone(), keyword.clone());
            RcDoc::nesting(move |indent| {
                // Render the left side where it stands, followed by the keyword, and see whether
                // the pipe breaks.
                let probe = text(&" ".repeat(col))
                    .append(lhs.clone())
                    .append(text(&format!(" {keyword} {{")))
                    .nest(indent as isize);
                let link = " ".repeat(indent + INDENT as usize) + "|>";
                let broke = render(&probe, width)
                    .lines()
                    .skip(1)
                    .any(|l| l.starts_with(&link));
                if broke {
                    broken.clone()
                } else {
                    inline.clone()
                }
            })
        })
    }

    /// Puts a space between children when `space(prev, cur)` is true.
    /// A variant. Each attribute goes on its own line above the name (`@tag("open")⏎Open`).
    fn variant(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        for child in significant_children(node) {
            let doc = match &child {
                rowan::NodeOrToken::Token(t) => self.tok(t),
                rowan::NodeOrToken::Node(n) => self.node(n),
            };
            out = out.append(doc);
            if child.kind() == ANNOTATION {
                out = out.append(self.hard());
            }
        }
        out
    }

    fn seq(&mut self, node: &SyntaxNode, space: impl Fn(SyntaxKind, SyntaxKind) -> bool) -> Doc {
        let mut out = RcDoc::nil();
        let mut prev = None;
        for child in significant_children(node) {
            if let Some(p) = prev
                && space(p, child.kind())
            {
                out = out.append(self.space());
            }
            prev = Some(child.kind());
            let doc = match child {
                rowan::NodeOrToken::Token(t) => self.tok(&t),
                rowan::NodeOrToken::Node(n) => self.node(&n),
            };
            out = out.append(doc);
        }
        out
    }

    /// For nodes the printer does not expect. Prints the source text as is and marks the
    /// comments inside as printed.
    pub(crate) fn verbatim(&mut self, node: &SyntaxNode) -> Doc {
        // The caller has already printed the leading `##` and newline, so start from the
        // first significant token.
        let Some(first) = first_significant_token(node) else {
            return RcDoc::nil();
        };
        let last = node.last_token();
        for token in node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            self.take_leading(&token);
            // The trailing comment of the last token lies outside the node; it is printed below.
            if Some(&token) != last.as_ref() {
                self.comments.take_trailing(&token);
            }
        }
        let start = first.text_range().start() - node.text_range().start();
        let source = node.text().to_string();
        let mut out = RcDoc::nil();
        for (i, line) in source[usize::from(start)..].split('\n').enumerate() {
            if i > 0 {
                out = out.append(RcDoc::hardline());
            }
            out = out.append(text(line.trim_end_matches('\r')));
        }
        self.at_break = false;
        match last {
            Some(last) => out.append(self.trailing(&last)),
            None => out,
        }
    }

    /// `{ stmt NL ... }`. A one-line source (at most one statement) stays on one line while it
    /// fits.
    pub(crate) fn block(&mut self, node: &SyntaxNode) -> BlockDoc {
        let multi = has_newline_child(node);
        let open = match child_token(node, L_BRACE) {
            Some(t) => self.tok(&t),
            None => RcDoc::nil(),
        };
        // A trailing comment after `{` forces a newline here.
        let forced = self.pending;
        let first_break = self.brk(multi);
        let mut body = RcDoc::nil();
        let stmts: Vec<_> = node.children().collect();
        for (i, stmt) in stmts.iter().enumerate() {
            if i > 0 {
                body = body.append(self.hard());
            }
            if let Some(first) = first_significant_token(stmt) {
                body = body.append(self.leading_lines(&first, i == 0));
            }
            body = body.append(self.node(stmt));
        }
        let close_token = child_token(node, R_BRACE);
        let mut has_closing = false;
        if let Some(t) = &close_token {
            let items = self.take_leading(t);
            has_closing = items
                .iter()
                .any(|l| matches!(l, crate::comments::Leading::Comment(_)));
            body = body.append(self.closing_comments(items, !stmts.is_empty()));
        }
        let last_break = self.brk(multi);
        let close = match &close_token {
            Some(t) => self.tok(t),
            None => RcDoc::nil(),
        };
        BlockDoc {
            open,
            first_break,
            body,
            last_break,
            close,
            empty: stmts.is_empty() && !has_closing && !forced,
            comment_only: stmts.is_empty() && !has_closing && forced,
        }
    }

    /// Arms of `match` and `escape`. One-line arms get their `->` aligned.
    fn arm_list(&mut self, node: &SyntaxNode) -> Doc {
        let multi = has_newline_child(node);
        let open = match child_token(node, L_BRACE) {
            Some(t) => self.tok(&t),
            None => RcDoc::nil(),
        };
        let forced = self.pending;
        let first_break = self.brk(multi);
        let arms: Vec<_> = node.children().collect();
        let mut docs = Vec::with_capacity(arms.len());
        for (i, arm) in arms.iter().enumerate() {
            let mut pre = RcDoc::nil();
            if i > 0 {
                pre = pre.append(self.hard());
            }
            if let Some(first) = first_significant_token(arm) {
                pre = pre.append(self.leading_lines(&first, i == 0));
            }
            docs.push(self.arm(arm, pre));
        }
        let close_token = child_token(node, R_BRACE);
        let mut closing = RcDoc::nil();
        let mut has_closing = false;
        if let Some(t) = &close_token {
            let items = self.take_leading(t);
            has_closing = items
                .iter()
                .any(|l| matches!(l, crate::comments::Leading::Comment(_)));
            closing = self.closing_comments(items, !arms.is_empty());
        }
        let last_break = self.brk(multi);
        let close = match &close_token {
            Some(t) => self.tok(t),
            None => RcDoc::nil(),
        };
        if arms.is_empty() && !has_closing {
            // Even when `{` has a trailing comment, print only one newline.
            let inner = if forced {
                RcDoc::hardline()
            } else {
                RcDoc::nil()
            };
            return open.append(inner).append(close);
        }
        let width = self.width();
        let arms_doc = RcDoc::nesting(move |indent| aligned_arms(&docs, indent, width));
        open.append(first_break.append(arms_doc).append(closing).nest(INDENT))
            .append(last_break)
            .append(close)
            .group()
    }

    /// `pattern [ "if" expr ] "->" expr`
    fn arm(&mut self, node: &SyntaxNode, pre: Doc) -> ArmDoc {
        let mut head = RcDoc::nil();
        let mut gap = RcDoc::nil();
        let mut arrow = RcDoc::nil();
        let mut after_arrow = RcDoc::nil();
        let mut body = RcDoc::nil();
        let mut seen_arrow = false;
        for child in significant_children(node) {
            match child {
                rowan::NodeOrToken::Token(t) if t.kind() == THIN_ARROW => {
                    gap = self.space();
                    arrow = self.tok(&t);
                    seen_arrow = true;
                }
                rowan::NodeOrToken::Token(t) => {
                    let doc = self.tok(&t);
                    head = head.append(doc);
                }
                rowan::NodeOrToken::Node(n) if seen_arrow => {
                    after_arrow = self.space();
                    body = self.node(&n);
                }
                rowan::NodeOrToken::Node(n) => {
                    if n.kind() == MATCH_GUARD {
                        head = head.append(self.space());
                    }
                    let doc = self.node(&n);
                    head = head.append(doc);
                }
            }
        }
        let widths = if has_comments(node) {
            None
        } else {
            Self::flat_width(&head).zip(Self::flat_width(&body))
        };
        ArmDoc {
            pre,
            head,
            gap,
            arrow,
            after_arrow,
            body,
            widths,
        }
    }

    /// `if c { a } else if d { b } else { c }`. All blocks share one group for their newlines,
    /// so if one breaks, all break.
    fn if_chain(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        let mut nodes = 0;
        for child in significant_children(node) {
            match child {
                rowan::NodeOrToken::Token(t) if t.kind() == ELSE_KW => {
                    out = out.append(self.space());
                    out = out.append(self.tok(&t));
                    out = out.append(self.space());
                }
                rowan::NodeOrToken::Token(t) => {
                    out = out.append(self.tok(&t));
                    out = out.append(self.space());
                }
                rowan::NodeOrToken::Node(n) => {
                    nodes += 1;
                    let doc = match n.kind() {
                        _ if nodes == 1 => {
                            let cond = self.node(&n);
                            cond.append(self.space())
                        }
                        BLOCK_EXPR => self.block(&n).assemble(false),
                        IF_EXPR => self.if_chain(&n),
                        _ => self.node(&n),
                    };
                    out = out.append(doc);
                }
            }
        }
        out
    }

    /// `with A, B { ... }`
    fn with_expr(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        let mut prev = None;
        for child in significant_children(node) {
            if prev.is_some_and(|p| p != COMMA) && child.kind() == COMMA {
                // No space before `,`.
            } else if prev.is_some() {
                out = out.append(self.space());
            }
            prev = Some(child.kind());
            let doc = match child {
                rowan::NodeOrToken::Token(t) => self.tok(&t),
                rowan::NodeOrToken::Node(n) if n.kind() == BLOCK_EXPR => {
                    self.block(&n).assemble(false).group()
                }
                rowan::NodeOrToken::Node(n) => self.node(&n),
            };
            out = out.append(doc);
        }
        out
    }

    /// An anonymous function (`[fmt.closure]`), in parentheses where it is an operand.
    fn lambda(&mut self, node: &SyntaxNode) -> Doc {
        let parens = rewrite::needs_parens(node);
        // A trailing comment at the end of the function goes after the closing parenthesis.
        let held = match node.last_token().filter(|_| parens) {
            Some(last) => self.hold_trailing(&last),
            None => Vec::new(),
        };
        let (head, body) = self.lambda_parts(node);
        let doc = match body {
            LambdaBody::Block(block) => head.append(block.assemble(false).group()),
            LambdaBody::Expr(expr) => head.append(expr),
        };
        if parens {
            let close = self.print_trailing(held);
            text("(").append(doc).append(")").append(close)
        } else {
            doc
        }
    }

    /// `(a || b)`, laid out as the parentheses the formatter adds around a chain of `||`.
    fn paren_or_chain(&mut self, node: &SyntaxNode) -> Doc {
        let chain = paren_or_chain(node).expect("checked by the caller");
        let open = child_token(node, L_PAREN).map_or_else(RcDoc::nil, |t| self.tok(&t));
        let chain = self.node(&chain);
        // After a trailing comment, `)` already starts a line of its own.
        let close_breaks = self.pending;
        let close = child_token(node, R_PAREN).map_or_else(RcDoc::nil, |t| self.tok(&t));
        or_chain_in_parens(open, chain, close, close_breaks, 1, self.width())
    }

    /// The parameter list and the space after it, and the body. The old form `fn(x) { ... }`
    /// (E0148) is printed in the new form. A body block written on one line with one expression
    /// loses its braces; any other block keeps them.
    fn lambda_parts(&mut self, node: &SyntaxNode) -> (Doc, LambdaBody) {
        let mut head = RcDoc::nil();
        let mut body = None;
        for child in significant_children(node) {
            match child {
                // The `fn` of the old form.
                rowan::NodeOrToken::Token(t) => head = head.append(self.tok_as(&t, RcDoc::nil())),
                rowan::NodeOrToken::Node(n) if n.kind() == PARAM_LIST => {
                    head = head.append(self.closure_params(&n));
                }
                rowan::NodeOrToken::Node(n) => {
                    head = head.append(self.space());
                    let stripped = strip_one_line_blocks(n.clone());
                    body = Some(if stripped.kind() == BLOCK_EXPR {
                        LambdaBody::Block(self.block(&stripped))
                    } else {
                        // The braces removed: a comment before `{` is printed before the
                        // expression, and one after `}` after it.
                        let removed: Vec<SyntaxNode> = stripped
                            .ancestors()
                            .skip(1)
                            .take_while(|a| a != node)
                            .collect();
                        let mut doc = RcDoc::nil();
                        for block in removed.iter().rev() {
                            if let Some(open) = child_token(block, L_BRACE) {
                                doc = doc.append(self.tok_as(&open, RcDoc::nil()));
                            }
                        }
                        doc = doc.append(self.node(&stripped));
                        for block in &removed {
                            if let Some(close) = child_token(block, R_BRACE) {
                                doc = doc.append(self.tok_as(&close, RcDoc::nil()));
                            }
                        }
                        LambdaBody::Expr(doc)
                    });
                }
            }
        }
        let body = body.unwrap_or(LambdaBody::Expr(RcDoc::nil()));
        (head, body)
    }

    /// `|a, b|` or `||`, on one line: outside brackets a newline would end the statement. The
    /// brackets of the old form become `|`. A type with `|` outside brackets (only possible in
    /// the old form) is parenthesized, since there `|` closes the list.
    fn closure_params(&mut self, node: &SyntaxNode) -> Doc {
        let children: Vec<SyntaxElement> = significant_children(node).collect();
        let empty = !children.iter().any(|c| c.kind() == PARAM);
        let last_param = children.iter().rposition(|c| c.kind() == PARAM);
        let last = children.len().saturating_sub(1);
        let mut out = RcDoc::nil();
        for (i, child) in children.iter().enumerate() {
            match child {
                rowan::NodeOrToken::Token(t) => {
                    let closing = i == last && i > 0 && matches!(t.kind(), R_PAREN | PIPE);
                    let doc = match t.kind() {
                        COMMA if last_param.is_some_and(|p| i > p) => RcDoc::nil(),
                        COMMA => text(", "),
                        _ if closing && empty => RcDoc::nil(),
                        _ if closing => text("|"),
                        _ if empty => text("||"),
                        _ => text("|"),
                    };
                    out = out.append(self.tok_as(t, doc));
                }
                rowan::NodeOrToken::Node(n) => out = out.append(self.closure_param(n)),
            }
        }
        out
    }

    fn closure_param(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        for child in significant_children(node) {
            match child {
                rowan::NodeOrToken::Token(t) if t.kind() == COLON => {
                    out = out.append(self.tok(&t)).append(" ");
                }
                rowan::NodeOrToken::Token(t) => out = out.append(self.tok(&t)),
                rowan::NodeOrToken::Node(n) if pipe_outside_brackets(&n) => {
                    out = out.append("(").append(self.node(&n)).append(")");
                }
                rowan::NodeOrToken::Node(n) => out = out.append(self.node(&n)),
            }
        }
        out
    }

    fn call(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        for child in significant_children(node) {
            let doc = match child {
                rowan::NodeOrToken::Token(t) => self.tok(&t),
                rowan::NodeOrToken::Node(n) if n.kind() == ARG_LIST => self.arg_list(&n),
                rowan::NodeOrToken::Node(n) => self.node(&n),
            };
            out = out.append(doc);
        }
        out
    }

    /// If the last argument is a lambda with a block, tries the hugging form that keeps the
    /// parenthesis open around its body (`f(xs, |x| {` ... `})`).
    fn arg_list(&mut self, node: &SyntaxNode) -> Doc {
        let args: Vec<_> = node.children().collect();
        let huggable = !has_comments_outside_blocks(node) && args.last().is_some_and(is_lambda_arg);
        if !huggable {
            return self.list(node, |p, n| p.node(n));
        }
        let mut hugged = None;
        let parts = self.list_parts(node, |p, n, last| {
            if !last {
                return p.node(n);
            }
            let (normal, hug) = p.lambda_arg(n);
            hugged = Some(hug);
            normal
        });
        let normal = parts.grouped();
        let Some(hugged) = hugged else { return normal };
        let (init, _) = parts.elements.split_at(parts.elements.len() - 1);
        if init.iter().any(|e| Self::flat_width(e).is_none()) {
            return normal;
        }
        let mut hug = parts.open.clone();
        for e in init {
            hug = hug.append(e.clone()).append(" ");
        }
        hug = hug.append(hugged).append(parts.close.clone());
        let flat = Self::flat_width(&normal);
        let head = Self::first_line_width(&hug);
        let width = self.width();
        RcDoc::column(move |col| {
            if flat.is_some_and(|w| col + w <= width) || col + head > width {
                normal.clone()
            } else {
                hug.clone()
            }
        })
    }

    /// Returns (the plain form, the hugging form) of the trailing lambda argument.
    fn lambda_arg(&mut self, node: &SyntaxNode) -> (Doc, Doc) {
        let mut prefix = RcDoc::nil();
        let mut lambda = node.clone();
        if node.kind() == NAMED_ARG {
            for child in significant_children(node) {
                match child {
                    rowan::NodeOrToken::Token(t) => prefix = prefix.append(self.tok(&t)),
                    rowan::NodeOrToken::Node(n) => {
                        prefix = prefix.append(self.space());
                        lambda = n;
                    }
                }
            }
        }
        let (head, body) = self.lambda_parts(&lambda);
        let head = prefix.append(head);
        match body {
            LambdaBody::Block(block) => (
                head.clone().append(block.assemble(false).group()),
                head.append(block.assemble(true)),
            ),
            // `is_lambda_arg` admits only a body printed as a block.
            LambdaBody::Expr(expr) => {
                let doc = head.append(expr);
                (doc.clone(), doc)
            }
        }
    }

    /// String interpolation. The contents never break by width; only forced newlines from
    /// comments and multiline blocks remain.
    fn interp(&mut self, node: &SyntaxNode) -> Doc {
        // Hugging and arm alignment record the width at build time, so build the contents
        // with no width limit.
        let width = std::mem::replace(&mut self.width, HUGE);
        let doc = self.seq(node, |_, _| false);
        self.width = width;
        // A multiline string inside must not be re-indented, so do not freeze the shape.
        if node
            .descendants_with_tokens()
            .any(|e| e.kind() == TRIPLE_QUOTE)
        {
            return doc;
        }
        unbreakable(&doc)
    }

    /// The pieces of a multiline string `"""` are printed verbatim.
    fn string(&mut self, node: &SyntaxNode) -> Doc {
        let multiline = child_token(node, TRIPLE_QUOTE).is_some();
        let mut out = RcDoc::nil();
        for child in significant_children(node) {
            let doc = match child {
                rowan::NodeOrToken::Token(t) if multiline => self.raw_tok(&t),
                rowan::NodeOrToken::Token(t) => self.tok(&t),
                rowan::NodeOrToken::Node(n) => self.node(&n),
            };
            out = out.append(doc);
        }
        out
    }

    /// A chain of operators of equal strength. When it breaks, each operator starts a line,
    /// indented one level.
    ///
    /// Never break before `-`: a `-` at the start of a line is a unary minus that starts a new
    /// statement (spec 2.5). A pipe chain breaks entirely if the source had a newline before
    /// any of its `|>`.
    ///
    /// A `||` at the start of a line starts an anonymous function (`[lex.continuation]`), so a
    /// chain of `||` that breaks outside brackets is put in parentheses and broken inside them
    /// (`[fmt.wrap]`).
    fn bin_chain(&mut self, node: &SyntaxNode) -> Doc {
        let class = bin_op(node).map_or(0, |t| op_class(t.kind()));
        // Left-associative, so collect the chain by following left children.
        let mut links = Vec::new();
        let mut cur = node.clone();
        let first = loop {
            let mut operands = cur.children();
            let (Some(lhs), Some(rhs), Some(op)) = (operands.next(), operands.next(), bin_op(&cur))
            else {
                return self.verbatim(node);
            };
            links.push((op, rhs));
            if lhs.kind() == BIN_EXPR && bin_op(&lhs).is_some_and(|t| op_class(t.kind()) == class) {
                cur = lhs;
            } else {
                break lhs;
            }
        };
        links.reverse();
        let pipe = class == op_class(PIPE_GT);
        let source_broken = pipe && links.iter().any(|(op, _)| newline_before(op));

        let first_doc = self.node(&first);
        let mut ops = Vec::with_capacity(links.len());
        let mut tail = RcDoc::nil();
        for (op, rhs) in &links {
            // Breaking before `-` is only safe inside parentheses. A line comment before it
            // keeps it at the start of a line.
            let brk = if op.kind() == MINUS && self.comments.has_leading_comment(op) {
                self.hard()
            } else if op.kind() == MINUS {
                self.space()
            } else {
                self.brk(source_broken)
            };
            let lead = self.leading_in_list(op);
            let op_doc = self.tok(op);
            let gap = self.space();
            let rhs_doc = self.node(rhs);
            let link = op_doc.append(gap).append(rhs_doc);
            tail = tail.append(brk).append(lead).append(link.clone());
            ops.push(link);
        }
        let normal = first_doc.clone().append(tail.nest(INDENT)).group();
        if class == op_class(PIPE2) && !inside_brackets(node) {
            return or_chain_in_parens(text("("), normal, text(")"), false, 0, self.width());
        }
        // When a single pipe breaks only inside a hugged body (`xs |> List.map(|x| {`), do
        // not break the pipe if the first line fits.
        if pipe
            && !source_broken
            && ops.len() == 1
            && !has_comments_outside_blocks(node)
            && Self::flat_width(&normal).is_none()
        {
            let spaced = first_doc.clone().append(" ").append(ops[0].clone());
            let broken = first_doc.append(RcDoc::hardline().append(ops[0].clone()).nest(INDENT));
            let head = Self::first_line_width(&spaced);
            let width = self.width();
            return RcDoc::column(move |col| {
                if col + head <= width {
                    spaced.clone()
                } else {
                    broken.clone()
                }
            });
        }
        normal
    }
}

/// A chain of `||` that breaks before a `||` is written with `open` and `close` on lines of
/// their own around it (`[fmt.wrap]`); with `close_breaks`, `close` already starts a line (after
/// a trailing comment). Otherwise the chain is printed as is: alone for a chain outside brackets
/// (`shift` 0: `open` and `close` are the parentheses the formatter adds), or in its parentheses,
/// `shift` being the width of `(`.
fn or_chain_in_parens(
    open: Doc,
    chain: Doc,
    close: Doc,
    close_breaks: bool,
    shift: usize,
    width: usize,
) -> Doc {
    let inline = if shift == 0 {
        chain.clone()
    } else {
        open.clone().append(chain.clone()).append(close.clone())
    };
    let close_line = if close_breaks {
        close
    } else {
        RcDoc::hardline().append(close)
    };
    let broken = open
        .append(RcDoc::hardline().append(chain.clone()).nest(INDENT))
        .append(close_line);
    RcDoc::column(move |col| {
        let (chain, inline, broken) = (chain.clone(), inline.clone(), broken.clone());
        RcDoc::nesting(move |indent| {
            let probe = text(&" ".repeat(col + shift))
                .append(chain.clone())
                .nest(indent as isize);
            let broke = render(&probe, width)
                .lines()
                .skip(1)
                .any(|l| l.trim_start().starts_with("||"));
            if broke {
                broken.clone()
            } else {
                inline.clone()
            }
        })
    })
}

/// The chain of `||` that is the only thing in a PAREN_EXPR.
fn paren_or_chain(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut inner = node.children();
    let (Some(only), None) = (inner.next(), inner.next()) else {
        return None;
    };
    (only.kind() == BIN_EXPR && bin_op(&only).is_some_and(|t| t.kind() == PIPE2)).then_some(only)
}

/// Arms with `->` aligned. `indent` is the arms' indentation.
///
/// Only arms that fit on one line are aligned. If alignment pushes any arm past the width, the
/// arm with the longest head is dropped from the alignment and it is retried.
fn aligned_arms(arms: &[ArmDoc], indent: usize, width: usize) -> Doc {
    let mut aligned: Vec<bool> = arms.iter().map(|a| a.widths.is_some()).collect();
    let column = loop {
        let column = arms
            .iter()
            .zip(&aligned)
            .filter(|(_, on)| **on)
            .filter_map(|(a, _)| a.widths.map(|(h, _)| h))
            .max()
            .unwrap_or(0);
        let overflow = arms.iter().zip(&aligned).any(|(a, on)| {
            *on && a
                .widths
                .is_some_and(|(_, b)| indent + column + 4 + b > width)
        });
        if !overflow {
            break column;
        }
        // Drop the arm with the longest head (the later one on ties).
        let widest = arms
            .iter()
            .enumerate()
            .filter(|(i, _)| aligned[*i])
            .max_by_key(|(i, a)| (a.widths.map_or(0, |(h, _)| h), *i))
            .map(|(i, _)| i);
        match widest {
            Some(i) => aligned[i] = false,
            None => break 0,
        }
    };
    let mut out = RcDoc::nil();
    for (arm, on) in arms.iter().zip(aligned) {
        out = out.append(arm.pre.clone()).append(arm.head.clone());
        out = match (on, arm.widths) {
            (true, Some((h, _))) => out.append(" ".repeat(column - h + 1)),
            _ => out.append(arm.gap.clone()),
        };
        out = out
            .append(arm.arrow.clone())
            .append(arm.after_arrow.clone())
            .append(arm.body.clone());
    }
    out
}

/// Freezes the shape rendered with no width limit. Forced newlines remain, and following lines
/// are indented relative to the current indentation.
pub(crate) fn unbreakable(doc: &Doc) -> Doc {
    let rendered = render(doc, HUGE);
    let mut out = RcDoc::nil();
    for (i, line) in rendered.split('\n').enumerate() {
        if i > 0 {
            out = out.append(RcDoc::hardline());
        }
        out = out.append(text(line));
    }
    out
}

fn bin_op(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| op_class(t.kind()) != 0)
}

/// Whether there is a newline in the trivia right before the token.
impl Printer {
    /// A uses clause in its canonical form (`[fmt.signature]`): `uses` and the set in braces.
    /// The old keyword `use` (E0147) is printed as `uses`, and a bare element gets braces
    /// (`uses Db` becomes `uses { Db }`). The formatter does not resolve names, so an effect and
    /// a set type parameter are treated alike.
    fn uses_clause(&mut self, node: &SyntaxNode) -> Doc {
        let mut out = RcDoc::nil();
        for child in significant_children(node) {
            match child {
                rowan::NodeOrToken::Token(t) if matches!(t.kind(), USE_KW | LOWER_NAME) => {
                    out = out.append(self.tok_as(&t, text("uses")));
                }
                rowan::NodeOrToken::Node(set) if set.kind() == EFFECT_SET => {
                    out = out.append(self.space());
                    if child_token(&set, L_BRACE).is_some() {
                        out = out.append(self.node(&set));
                    } else {
                        // The same document as the braced set `{ X }` gives, so that the
                        // output reads back to itself at every width.
                        let brk = self.line();
                        let item = self.seq(&set, |_, _| true);
                        let end_break = self.line();
                        let parts = super::ListParts {
                            open: text("{"),
                            breaks: vec![brk],
                            elements: vec![item],
                            last_comma: text(",").flat_alt(RcDoc::nil()),
                            closing: RcDoc::nil(),
                            end_break,
                            close: text("}"),
                        };
                        out = out.append(parts.grouped());
                    }
                }
                other => out = out.append(self.element(&other)),
            }
        }
        out
    }
}

pub(crate) fn newline_before(token: &SyntaxToken) -> bool {
    let mut cur = token.prev_token();
    while let Some(t) = cur {
        match t.kind() {
            NEWLINE | NEWLINE_CONT => return true,
            k if k.is_trivia() => cur = t.prev_token(),
            _ => return false,
        }
    }
    false
}

/// A `|>` chain.
fn is_pipe(node: &SyntaxNode) -> bool {
    node.kind() == BIN_EXPR && bin_op(node).is_some_and(|t| t.kind() == PIPE_GT)
}

/// A lambda whose body is printed as a block, or a named argument whose value is one.
fn is_lambda_arg(node: &SyntaxNode) -> bool {
    let lambda = match node.kind() {
        LAMBDA_EXPR => Some(node.clone()),
        NAMED_ARG => node.children().find(|n| n.kind() == LAMBDA_EXPR),
        _ => None,
    };
    lambda.is_some_and(|l| {
        l.children()
            .find(|n| n.kind() != PARAM_LIST)
            .is_some_and(|body| strip_one_line_blocks(body).kind() == BLOCK_EXPR)
    })
}

/// The body of an anonymous function as printed: a block written on one line with one
/// expression is replaced by that expression, repeatedly (`|x| { { x } }` is `|x| x`), so that
/// formatting twice gives the same result.
pub(crate) fn strip_one_line_blocks(mut body: SyntaxNode) -> SyntaxNode {
    while body.kind() == BLOCK_EXPR
        && let Some(expr) = rewrite::single_line_expression(&body)
    {
        body = expr;
    }
    body
}

/// Whether a `|` of the type is outside any bracket inside it.
fn pipe_outside_brackets(ty: &SyntaxNode) -> bool {
    let mut depth = 0i32;
    for token in ty.descendants_with_tokens().filter_map(|e| e.into_token()) {
        match token.kind() {
            L_PAREN | L_BRACK | L_BRACE => depth += 1,
            R_PAREN | R_BRACK | R_BRACE => depth -= 1,
            PIPE if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Whether a line break here continues the line by rule 2 of `[lex.continuation]`: the
/// innermost bracket around the node is `(`, `[` or an interpolation, not a block or arm list.
fn inside_brackets(node: &SyntaxNode) -> bool {
    for ancestor in node.ancestors().skip(1) {
        match ancestor.kind() {
            ARG_LIST | PAREN_EXPR | TUPLE_EXPR | LIST_EXPR | INTERP => return true,
            BLOCK_EXPR | MATCH_ARM_LIST | ROOT => return false,
            _ => {}
        }
    }
    false
}
