//! イベント列から rowan の木を組む．トリビアはここで差し込む．

use std::mem;

use rowan::{GreenNode, GreenNodeBuilder, Language, TextRange, TextSize};

use super::Event;
use crate::{Diagnostic, EmelaLanguage, SyntaxKind, Token};

struct Sink<'a> {
    src: &'a str,
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
    builder: GreenNodeBuilder<'static>,
    diagnostics: Vec<Diagnostic>,
}

impl Sink<'_> {
    fn start_node(&mut self, kind: SyntaxKind) {
        self.builder.start_node(EmelaLanguage::kind_to_raw(kind));
        self.depth += 1;
    }

    /// tokens[pos] がトリビアである間，木に入れて進める．
    fn eat_trivia(&mut self) {
        while let Some(token) = self.tokens.get(self.pos) {
            if !token.kind.is_trivia() {
                break;
            }
            self.push_token();
        }
    }

    /// tokens[pos] を1つ木に入れて進める．
    fn push_token(&mut self) {
        let token = self.tokens[self.pos];
        let text = &self.src[token.range];
        self.builder
            .token(EmelaLanguage::kind_to_raw(token.kind), text);
        self.pos += 1;
    }

    /// Error イベントの位置．pos から先の最初の非トリビアの範囲で，なければ末尾の長さ0の範囲．
    fn error_range(&self) -> TextRange {
        self.tokens[self.pos..]
            .iter()
            .find(|t| !t.kind.is_trivia())
            .map_or(TextRange::empty(TextSize::of(self.src)), |t| t.range)
    }
}

/// `tokens` はトリビア込みの全トークン．`events` はトリビアを除いた列に対するもの．
///
/// 返す診断はパースの `Error` イベントの分だけ．位置は次の意味のあるトークンの範囲で，
/// 入力の終わりなら末尾の長さ0の範囲にする．字句解析の診断は `parse` が足す．
///
/// トリビアの置き場所:
/// - ノードの直前のトリビアはそのノードの外（親の側）に置く
/// - 先頭と末尾のトリビアは ROOT の中に置く
pub(crate) fn build(
    src: &str,
    tokens: &[Token],
    mut events: Vec<Event>,
) -> (GreenNode, Vec<Diagnostic>) {
    let mut sink = Sink {
        src,
        tokens,
        pos: 0,
        depth: 0,
        builder: GreenNodeBuilder::new(),
        diagnostics: Vec::new(),
    };

    let mut kinds = Vec::new();

    for i in 0..events.len() {
        match mem::replace(&mut events[i], Event::Tombstone) {
            Event::Start {
                kind,
                forward_parent,
            } => {
                kinds.push(kind);
                let (mut at, mut fp) = (i, forward_parent);
                while let Some(d) = fp {
                    at += d as usize;
                    fp = match mem::replace(&mut events[at], Event::Tombstone) {
                        Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            kinds.push(kind);
                            forward_parent
                        }
                        _ => None,
                    };
                }
                if sink.depth > 0 {
                    sink.eat_trivia();
                }

                for kind in kinds.drain(..).rev() {
                    sink.start_node(kind);
                }
            }
            Event::Token => {
                sink.eat_trivia();
                sink.push_token();
            }
            Event::Finish => {
                if sink.depth == 1 {
                    while sink.pos < tokens.len() {
                        sink.push_token();
                    }
                }
                sink.builder.finish_node();
                sink.depth -= 1;
            }
            Event::Error(message) => {
                let range = sink.error_range();
                sink.diagnostics.push(Diagnostic { range, message });
            }
            Event::Tombstone => {}
        }
    }

    debug_assert_eq!(
        sink.pos,
        tokens.len(),
        "木に入っていないトークンが残っている"
    );
    debug_assert_eq!(sink.depth, 0);

    (sink.builder.finish(), sink.diagnostics)
}

#[cfg(test)]
mod tests {
    use rowan::{TextRange, TextSize};

    use super::build;
    use crate::SyntaxKind::{ERROR, ROOT};
    use crate::parser::Parser;
    use crate::{Diagnostic, SyntaxNode, debug_tree, lex};

    /// `src` を字句解析し，`f` でパーサを手で動かして木を組む．
    fn run(src: &str, f: impl FnOnce(&mut Parser)) -> (String, Vec<Diagnostic>) {
        let lexed = lex(src);
        let mut p = Parser::new(lexed.tokens.iter().map(|t| t.kind));
        f(&mut p);
        let (green, diagnostics) = build(src, &lexed.tokens, p.finish());
        (debug_tree(&SyntaxNode::new_root(green)), diagnostics)
    }

    #[test]
    fn 後から包んだ親が外側に開く() {
        let (tree, _) = run("a + b", |p| {
            let root = p.start();
            let lhs = p.start();
            p.bump();
            let lhs = lhs.complete(p, ERROR);
            let bin = lhs.precede(p);
            p.bump();
            let rhs = p.start();
            p.bump();
            rhs.complete(p, ERROR);
            bin.complete(p, ERROR);
            root.complete(p, ROOT);
        });
        let expected = r##"ROOT@0..5
  ERROR@0..5
    ERROR@0..1
      LOWER_NAME@0..1 "a"
    WHITESPACE@1..2 " "
    PLUS@2..3 "+"
    WHITESPACE@3..4 " "
    ERROR@4..5
      LOWER_NAME@4..5 "b"
"##;
        assert_eq!(tree, expected);
    }

    #[test]
    fn ノードの直前と末尾のトリビアはノードの外() {
        let (tree, _) = run("  a # c", |p| {
            let root = p.start();
            let e = p.start();
            p.bump();
            e.complete(p, ERROR);
            root.complete(p, ROOT);
        });
        let expected = r##"ROOT@0..7
  WHITESPACE@0..2 "  "
  ERROR@2..3
    LOWER_NAME@2..3 "a"
  WHITESPACE@3..4 " "
  COMMENT@4..7 "# c"
"##;
        assert_eq!(tree, expected);
    }

    #[test]
    fn エラーの位置は次のトークンか末尾() {
        let (_, diagnostics) = run("a  b ", |p| {
            let root = p.start();
            p.bump();
            p.error("途中");
            p.bump();
            p.error("終わり");
            root.complete(p, ROOT);
        });
        let range = |s: u32, e: u32| TextRange::new(TextSize::from(s), TextSize::from(e));
        let got: Vec<_> = diagnostics
            .iter()
            .map(|d| (d.range, d.message.as_str()))
            .collect();
        assert_eq!(got, [(range(3, 4), "途中"), (range(5, 5), "終わり")]);
    }
}
