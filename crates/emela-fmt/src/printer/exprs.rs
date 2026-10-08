//! 式（17.5），型（17.4），パターン（17.6）．

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::{SyntaxElement, SyntaxNode, SyntaxToken};
use pretty::RcDoc;

use super::{
    Doc, HUGE, INDENT, Printer, child_token, first_significant_token, has_newline_child, render,
    significant_children, text,
};
use crate::comments::{has_comments, has_comments_outside_blocks};

/// ブロック `{ ... }` の部品．group で包むかどうかは呼ぶ側が決める．
pub(crate) struct BlockDoc {
    open: Doc,
    first_break: Doc,
    body: Doc,
    last_break: Doc,
    close: Doc,
    /// 文もコメントもない．`{}` と出す．
    empty: bool,
    /// 中身はないが，`{` に行末のコメントがある．`{ # c⏎}` と出す．
    comment_only: bool,
}

impl BlockDoc {
    /// `force` なら，元が1行でも必ず折る．
    pub(crate) fn assemble(&self, force: bool) -> Doc {
        if self.empty {
            return self.open.clone().append(self.close.clone());
        }
        if self.comment_only {
            // 改行を2つ並べると空行になるので，1つだけ出す．
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

/// match の腕の部品．`->` を揃えるために，頭と本体を分けて持つ．
#[derive(Clone)]
struct ArmDoc {
    /// 前の腕との区切りと，前置きのコメント．
    pre: Doc,
    head: Doc,
    /// 頭と `->` の間（揃えないとき）．
    gap: Doc,
    arrow: Doc,
    after_arrow: Doc,
    body: Doc,
    /// 揃えてよい腕なら，頭と本体の幅．
    widths: Option<(usize, usize)>,
}

/// 二項演算子の結合の強さ．同じ強さの演算子の並びを1つの鎖として整形する．
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
    /// ノードの種類で振り分ける．
    pub(crate) fn node(&mut self, node: &SyntaxNode) -> Doc {
        match node.kind() {
            // 間に何も置かずにつなぐもの．
            PATH | PATH_TYPE | TYPE_VAR | SELF_TYPE | NAME_REF | PATH_EXPR | UNDERSCORE_EXPR
            | PAREN_EXPR | FIELD_EXPR | PREFIX_EXPR | REST_EXPR | LITERAL | STRING
            | WILDCARD_PAT | IDENT_PAT | CONST_PAT | LITERAL_PAT | VARIANT_PAT | REST_PAT
            | ANNOTATION => self.seq(node, |_, _| false),
            // 括弧で囲んだ並び．
            TYPE_ARG_LIST | PARAM_TYPE_LIST | UNIT_TYPE | TUPLE_TYPE | TYPE_PARAM_LIST
            | PARAM_LIST | UNIT_EXPR | TUPLE_EXPR | LIST_EXPR | PAT_ARG_LIST | UNIT_PAT
            | TUPLE_PAT | LIST_PAT | ANNOT_ARG_LIST | FIELD_LIST | TUPLE_FIELD_LIST
            | IMPORT_LIST => self.list(node, |p, n| p.node(n)),
            // 宣言の部品．
            VARIANT => self.seq(node, |_, _| false),
            FIELD => self.seq(node, |_, cur| cur != COLON),
            DERIVE_CLAUSE => self.seq(node, |_, cur| cur != COMMA),
            IMPLEMENTS_CLAUSE => self.seq(node, |_, _| true),
            // 空白で区切るもの．
            RET_TYPE | FAILS_CLAUSE | USE_CLAUSE | USE_EXPR | FAIL_EXPR | ASSERT_EXPR
            | MATCH_GUARD => self.seq(node, |_, _| true),
            // `name: value`．`:` の前に空白を置かない．
            PARAM | TYPE_PARAM | NAMED_ARG | FIELD_PAT | BINDING | ANNOT_ARG => {
                self.seq(node, |_, cur| cur != COLON)
            }
            FN_TYPE => self.seq(node, |_, cur| cur != PARAM_TYPE_LIST),
            ERROR_SET => self.seq(node, |prev, cur| prev != L_BRACE && cur != R_BRACE),
            EFFECT_SET if child_token(node, L_BRACE).is_some() => self.list(node, |p, n| p.node(n)),
            EFFECT_SET => self.seq(node, |_, _| true),
            INTERP => self.interp(node),
            CALL_EXPR => self.call(node),
            BIN_EXPR => self.bin_chain(node),
            BLOCK_EXPR => self.block(node).assemble(false).group(),
            IF_EXPR => self.if_chain(node).group(),
            MATCH_EXPR => self.seq(node, |_, _| true),
            MATCH_ARM_LIST => self.arm_list(node),
            WITH_EXPR => self.with_expr(node),
            ESCAPE_EXPR => self.seq(node, |_, _| true),
            LAMBDA_EXPR => {
                let (head, block) = self.lambda_parts(node);
                head.append(block.assemble(false).group())
            }
            _ => self.verbatim(node),
        }
    }

    /// トークンかノード．
    pub(crate) fn element(&mut self, element: &SyntaxElement) -> Doc {
        match element {
            rowan::NodeOrToken::Token(t) => self.tok(t),
            rowan::NodeOrToken::Node(n) => self.node(n),
        }
    }

    /// 子を順に出す．`space(前, 今)` が真なら間に空白を置く．
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

    /// 想定していないノード．元のテキストをそのまま出し，中のコメントは出したことにする．
    pub(crate) fn verbatim(&mut self, node: &SyntaxNode) -> Doc {
        // 前置きの `##` と改行は呼ぶ側が出しているので，最初の意味のあるトークンから出す．
        let Some(first) = first_significant_token(node) else {
            return RcDoc::nil();
        };
        let last = node.last_token();
        for token in node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            self.take_leading(&token);
            // 最後のトークンの行末のコメントはノードの外にあるので，下で出す．
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

    // ---- ブロックと腕 ----

    /// `{ stmt NL ... }`．元が1行（文が1つ以下）なら，収まるかぎり1行のままにする．
    pub(crate) fn block(&mut self, node: &SyntaxNode) -> BlockDoc {
        let multi = has_newline_child(node);
        let open = match child_token(node, L_BRACE) {
            Some(t) => self.tok(&t),
            None => RcDoc::nil(),
        };
        // `{` の行末のコメントは，ここで改行を強制する．
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

    /// match と escape の腕の並び．1行の腕は `->` を揃える．
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
            // `{` に行末のコメントがあるときも，改行は1つだけにする．
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

    // ---- 制御構文 ----

    /// `if c { a } else if d { b } else { c }`．ブロックの改行を1つの group で共有するので，
    /// どれかが折れれば全部折れる．
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
                        // 1つ目は条件．
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
                // `,` の前には空白を置かない．
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

    /// `fn(params) ` と本体のブロック．
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

    // ---- 呼び出し ----

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

    /// 引数の並び．最後の引数がブロックを持つ無名関数なら，括弧を開いたまま本体を
    /// 抱え込む形（`f(xs, fn(x) {` ... `})`）を試す．
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

    /// 最後の引数の無名関数．（そのままの形，抱え込む形）を返す．
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

    /// 文字列の補間．中身は幅で折らない．コメントや複数行のブロックによる強制の改行だけを残す．
    fn interp(&mut self, node: &SyntaxNode) -> Doc {
        // 抱え込みや腕の揃えの判定は組むときの幅を覚えるので，中身は幅の制限なしで組む．
        let width = std::mem::replace(&mut self.width, HUGE);
        let doc = self.seq(node, |_, _| false);
        self.width = width;
        unbreakable(&doc)
    }

    // ---- 二項演算子 ----

    /// 同じ強さの演算子の鎖．折るときは演算子を行頭に置いて1段下げる．
    ///
    /// `-` の前では折らない（行頭の `-` は単項マイナスとして新しい文になる．2.5）．
    /// パイプは，元のソースで1か所でも `|>` の前に改行があれば全部折る．
    fn bin_chain(&mut self, node: &SyntaxNode) -> Doc {
        let class = bin_op(node).map_or(0, |t| op_class(t.kind()));
        // 左結合なので，左の子をたどって鎖を集める．
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
            // `-` の前で折れるのは括弧の中だけ．行だけのコメントがあれば，それを行頭に保つ．
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
        // `xs |> List.map(fn(x) {` のように，1つのパイプが抱え込んだ本体で折れるとき，
        // 1行目が収まるならパイプは折らない．
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

/// `->` を揃えた腕の並び．`indent` は腕の字下げ．
///
/// 揃えるのは1行に収まる腕だけ．揃えた結果どれかが幅を超えるなら，頭が最も長い腕を
/// 揃えから外してやり直す．
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
        // 頭が最も長い腕（同じなら後ろのもの）を外す．
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

/// 幅を無視して描いた形に固定する．強制の改行はそのまま残し，続く行の字下げは
/// 今の字下げからの相対にする．
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

/// トークンの直前（トリビアの中）に改行があるか．
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

/// ブロックを持つ無名関数か，それを値にした名前付き引数．
fn is_lambda_arg(node: &SyntaxNode) -> bool {
    let lambda = match node.kind() {
        LAMBDA_EXPR => Some(node.clone()),
        NAMED_ARG => node.children().find(|n| n.kind() == LAMBDA_EXPR),
        _ => None,
    };
    lambda.is_some_and(|l| l.children().any(|n| n.kind() == BLOCK_EXPR))
}
