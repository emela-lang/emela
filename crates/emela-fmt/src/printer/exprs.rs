//! Expressions (spec 17.5), types (spec 17.4) and patterns (spec 17.6).

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::{SyntaxElement, SyntaxNode, SyntaxToken};
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
            PATH | PATH_TYPE | TYPE_VAR | SELF_TYPE | NAME_REF | PATH_EXPR | UNDERSCORE_EXPR
            | PAREN_EXPR | FIELD_EXPR | PREFIX_EXPR | REST_EXPR | LITERAL | PAREN_TYPE
            | SPREAD_ARG | WILDCARD_PAT | IDENT_PAT | CONST_PAT | LITERAL_PAT | VARIANT_PAT
            | REST_PAT | ANNOTATION | ANNOT_PATH => self.seq(node, |_, _| false),
            TYPE_ARG_LIST | PARAM_TYPE_LIST | UNIT_TYPE | TUPLE_TYPE | TYPE_PARAM_LIST
            | PARAM_LIST | UNIT_EXPR | TUPLE_EXPR | LIST_EXPR | PAT_ARG_LIST | UNIT_PAT
            | TUPLE_PAT | LIST_PAT | ANNOT_ARG_LIST | FIELD_LIST | TUPLE_FIELD_LIST
            | IMPORT_LIST => self.list(node, |p, n| p.node(n)),
            VARIANT => self.seq(node, |_, _| false),
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
            RET_TYPE | FAILS_CLAUSE | USE_CLAUSE | FAIL_EXPR | ASSERT_EXPR | MATCH_GUARD => {
                self.seq(node, |_, _| true)
            }
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
            IF_EXPR => self.if_chain(node).group(),
            MATCH_EXPR | ESCAPE_EXPR => self.arms_expr(node),
            MATCH_ARM_LIST => self.arm_list(node),
            WITH_EXPR => self.with_expr(node),
            LAMBDA_EXPR => {
                let (head, block) = self.lambda_parts(node);
                head.append(block.assemble(false).group())
            }
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

    fn lambda_parts(&mut self, node: &SyntaxNode) -> (Doc, BlockDoc) {
        let mut head = RcDoc::nil();
        let mut block = None;
        for child in significant_children(node) {
            match child {
                rowan::NodeOrToken::Token(t) => head = head.append(self.tok(&t)),
                rowan::NodeOrToken::Node(n) if n.kind() == BLOCK_EXPR => {
                    head = head.append(self.space());
                    block = Some(self.block(&n));
                }
                rowan::NodeOrToken::Node(n) => head = head.append(self.node(&n)),
            }
        }
        let block = block.unwrap_or(BlockDoc {
            open: RcDoc::nil(),
            first_break: RcDoc::nil(),
            body: RcDoc::nil(),
            last_break: RcDoc::nil(),
            close: RcDoc::nil(),
            empty: true,
            comment_only: false,
        });
        (head, block)
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
    /// parenthesis open around its body (`f(xs, fn(x) {` ... `})`).
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
        let (head, block) = self.lambda_parts(&lambda);
        let head = prefix.append(head);
        (
            head.clone().append(block.assemble(false).group()),
            head.append(block.assemble(true)),
        )
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
        // When a single pipe breaks only inside a hugged body (`xs |> List.map(fn(x) {`), do
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

/// A lambda with a block, or a named argument whose value is one.
fn is_lambda_arg(node: &SyntaxNode) -> bool {
    let lambda = match node.kind() {
        LAMBDA_EXPR => Some(node.clone()),
        NAMED_ARG => node.children().find(|n| n.kind() == LAMBDA_EXPR),
        _ => None,
    };
    lambda.is_some_and(|l| l.children().any(|n| n.kind() == BLOCK_EXPR))
}
