//! 字句解析．入力を `SyntaxKind` と範囲の列に切る．空白，改行，コメントも捨てずに残す．

use logos::Logos;
use rowan::{TextRange, TextSize};

use crate::SyntaxKind;
use crate::diagnostic::{Diagnostic, DiagnosticCode};

mod continuation;
mod name;
mod string;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub range: TextRange,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn lex(src: &str) -> Lexed {
    let mut lexer = Lexer {
        src,
        lexed: Lexed::default(),
        modes: vec![Mode::Code],
    };
    let mut pos = 0;
    while pos < src.len() {
        pos = match lexer.modes.last().copied().unwrap_or(Mode::Code) {
            Mode::Str { open } => lexer.string_step(pos, open),
            Mode::Code | Mode::Interp { .. } => lexer.code_step(pos),
        };
    }
    // 入力の終わりで閉じていないものを，内側から順に報告する．
    while lexer.modes.len() > 1 {
        match lexer.modes.pop() {
            Some(Mode::Str { open }) => lexer.error(
                open,
                src.len(),
                DiagnosticCode::UnterminatedString,
                "unterminated string literal",
            ),
            Some(Mode::Interp { open, .. }) => lexer.error(
                open,
                src.len(),
                DiagnosticCode::UnterminatedInterpolation,
                "unterminated interpolation",
            ),
            _ => {}
        }
    }
    continuation::mark_continuations(&mut lexer.lexed.tokens);
    // 出した順ではなく位置の順に並べる．同じ位置なら出した順のまま．
    lexer
        .lexed
        .diagnostics
        .sort_by_key(|d| (d.range.start(), d.range.end()));
    lexer.lexed
}

/// いま読んでいる場所．底は必ず `Code` で，文字列と補間に入るたびに積む．
#[derive(Debug, Clone, Copy)]
enum Mode {
    Code,
    /// `open` は開いた `"` の位置．
    Str {
        open: usize,
    },
    /// `open` は `#{` の位置．`depth` は補間の中で開いている `{` の数．
    Interp {
        open: usize,
        depth: u32,
    },
}

struct Lexer<'a> {
    src: &'a str,
    lexed: Lexed,
    modes: Vec<Mode>,
}

impl Lexer<'_> {
    /// コード（文字列の外か補間の中）で1トークン読み，次の位置を返す．
    fn code_step(&mut self, pos: usize) -> usize {
        let mut raw_lexer = RawToken::lexer(&self.src[pos..]);
        let Some(raw) = raw_lexer.next() else {
            return self.src.len();
        };
        let end = pos + raw_lexer.span().end;
        let (mut kind, error) = classify(raw, &self.src[pos..end]);
        match (raw, self.modes.last_mut()) {
            (Ok(RawToken::Quote), _) => self.modes.push(Mode::Str { open: pos }),
            (Ok(RawToken::LBrace), Some(Mode::Interp { depth, .. })) => *depth += 1,
            (Ok(RawToken::RBrace), Some(Mode::Interp { depth: 0, .. })) => {
                kind = SyntaxKind::INTERP_END;
                self.modes.pop();
            }
            (Ok(RawToken::RBrace), Some(Mode::Interp { depth, .. })) => *depth -= 1,
            _ => {}
        }
        self.token(kind, pos, end);
        if let Some((code, message)) = error {
            self.error(pos, end, code, message);
        }
        end
    }

    /// 文字列の中で1トークン読み，次の位置を返す．
    fn string_step(&mut self, pos: usize, open: usize) -> usize {
        let rest = &self.src[pos..];
        if rest.starts_with('"') {
            self.token(SyntaxKind::STRING_QUOTE, pos, pos + 1);
            self.modes.pop();
            pos + 1
        } else if rest.starts_with("#{") {
            self.token(SyntaxKind::INTERP_START, pos, pos + 2);
            self.modes.push(Mode::Interp {
                open: pos,
                depth: 0,
            });
            pos + 2
        } else if rest.starts_with(['\n', '\r']) {
            // 改行はコードとして読み直す．
            self.error(
                open,
                pos,
                DiagnosticCode::UnterminatedString,
                "unterminated string literal",
            );
            self.modes.pop();
            pos
        } else {
            let (end, diagnostics) = string::scan_string_text(self.src, pos);
            self.token(SyntaxKind::STRING_TEXT, pos, end);
            self.lexed.diagnostics.extend(diagnostics);
            end
        }
    }

    fn token(&mut self, kind: SyntaxKind, start: usize, end: usize) {
        self.lexed.tokens.push(Token {
            kind,
            range: range(start, end),
        });
    }

    fn error(
        &mut self,
        start: usize,
        end: usize,
        code: DiagnosticCode,
        message: impl Into<String>,
    ) {
        self.lexed.diagnostics.push(Diagnostic {
            range: range(start, end),
            code,
            message: message.into(),
        });
    }
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(TextSize::new(start as u32), TextSize::new(end as u32))
}

/// logos のトークンを `SyntaxKind` に写す．エラーならコードと文面も返す．
fn classify(
    raw: Result<RawToken, ()>,
    text: &str,
) -> (SyntaxKind, Option<(DiagnosticCode, String)>) {
    let shown = text.escape_debug();
    match raw {
        Ok(RawToken::Word) => match name::classify_name(text) {
            Some(kind) => (kind, None),
            None => (
                SyntaxKind::ERROR_TOKEN,
                Some((
                    DiagnosticCode::InvalidName,
                    format!("invalid name `{shown}`: not a lower, type, or upper name"),
                )),
            ),
        },
        Ok(RawToken::BadNumber) => (
            SyntaxKind::ERROR_TOKEN,
            Some((
                DiagnosticCode::InvalidNumber,
                format!("invalid number literal `{shown}`"),
            )),
        ),
        Ok(raw) => (raw.into(), None),
        Err(()) => (
            SyntaxKind::ERROR_TOKEN,
            Some((
                DiagnosticCode::UnrecognizedCharacter,
                format!("unrecognized character `{shown}`"),
            )),
        ),
    }
}

/// logos が直接切り出すトークン．`SyntaxKind` にはノードの種類も入っているので分けておく．
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
enum RawToken {
    #[regex(r"[ \t]+")]
    Whitespace,
    #[regex(r"\r?\n")]
    Newline,
    #[regex(r"#[^\r\n]*", allow_greedy = true)]
    Comment,
    #[regex(r"##[^\r\n]*", allow_greedy = true)]
    DocComment,
    /// 名前と予約語．分類は `name::classify_name` で行う．
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Word,

    #[token("\"")]
    Quote,

    #[regex(r"[0-9][0-9_]*")]
    Int,
    /// 16進と2進の整数（`0xFF` `0b1010`）．接頭辞は小文字だけ．`0xFF` は BadNumber と同じ長さで
    /// 当たるので，優先度を上げてある．
    #[regex(r"0x[0-9a-fA-F][0-9a-fA-F_]*|0b[01][01_]*", priority = 10)]
    RadixInt,
    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?", priority = 10)]
    Float,
    /// 数値の直後に英字が続いたもの（`1e9` `1.0e` `3px` `0o17` `0XFF`）と，16進と2進の
    /// 小数（`0x1.5`）．かたまりごとエラーにする．
    /// `1.0e9` は Float と同じ長さで当たるので，Float の優先度を上げてある．
    #[regex(r"[0-9][0-9_]*(\.[0-9][0-9_]*)?[A-Za-z][A-Za-z0-9_]*|0[xb][0-9A-Za-z_]*\.[0-9][0-9A-Za-z_]*")]
    BadNumber,

    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBrack,
    #[token("]")]
    RBrack,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token("..")]
    Dot2,
    #[token(":")]
    Colon,
    #[token("=")]
    Eq,
    #[token("->")]
    ThinArrow,
    #[token("|")]
    Pipe,
    #[token("|>")]
    PipeGt,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("!")]
    Bang,
    #[token("==")]
    Eq2,
    #[token("!=")]
    Neq,
    #[token("<")]
    Lt,
    #[token("<=")]
    LtEq,
    #[token(">")]
    Gt,
    #[token(">=")]
    GtEq,
    #[token("&&")]
    Amp2,
    #[token("||")]
    Pipe2,
    #[token("@")]
    At,
}

impl From<RawToken> for SyntaxKind {
    fn from(raw: RawToken) -> Self {
        use RawToken as R;
        use SyntaxKind as K;
        match raw {
            R::Whitespace => K::WHITESPACE,
            R::Newline => K::NEWLINE,
            R::Comment => K::COMMENT,
            R::DocComment => K::DOC_COMMENT,
            // `lex` が先に診断つきで扱うので，ここには来ない
            R::Word | R::BadNumber => K::ERROR_TOKEN,
            R::Quote => K::STRING_QUOTE,
            R::Int | R::RadixInt => K::INT,
            R::Float => K::FLOAT,
            R::LParen => K::L_PAREN,
            R::RParen => K::R_PAREN,
            R::LBrack => K::L_BRACK,
            R::RBrack => K::R_BRACK,
            R::LBrace => K::L_BRACE,
            R::RBrace => K::R_BRACE,
            R::Comma => K::COMMA,
            R::Dot => K::DOT,
            R::Dot2 => K::DOT2,
            R::Colon => K::COLON,
            R::Eq => K::EQ,
            R::ThinArrow => K::THIN_ARROW,
            R::Pipe => K::PIPE,
            R::PipeGt => K::PIPE_GT,
            R::Plus => K::PLUS,
            R::Minus => K::MINUS,
            R::Star => K::STAR,
            R::Slash => K::SLASH,
            R::Percent => K::PERCENT,
            R::Bang => K::BANG,
            R::Eq2 => K::EQ2,
            R::Neq => K::NEQ,
            R::Lt => K::LT,
            R::LtEq => K::LTEQ,
            R::Gt => K::GT,
            R::GtEq => K::GTEQ,
            R::Amp2 => K::AMP2,
            R::Pipe2 => K::PIPE2,
            R::At => K::AT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SyntaxKind::*;

    fn kinds(src: &str) -> Vec<SyntaxKind> {
        lex(src).tokens.iter().map(|t| t.kind).collect()
    }

    #[test]
    fn 記号は最長一致で切る() {
        assert_eq!(
            kinds("|> || | -> - .. . == = <= >= != && @"),
            [
                PIPE_GT, WHITESPACE, PIPE2, WHITESPACE, PIPE, WHITESPACE, THIN_ARROW, WHITESPACE,
                MINUS, WHITESPACE, DOT2, WHITESPACE, DOT, WHITESPACE, EQ2, WHITESPACE, EQ,
                WHITESPACE, LTEQ, WHITESPACE, GTEQ, WHITESPACE, NEQ, WHITESPACE, AMP2, WHITESPACE,
                AT,
            ]
        );
    }

    #[test]
    fn コメントは行末までで改行を含まない() {
        assert_eq!(
            kinds("# a\n## b\r\n"),
            [COMMENT, NEWLINE, DOC_COMMENT, NEWLINE]
        );
    }

    #[test]
    fn 認識できない文字は診断を出して続ける() {
        let lexed = lex("( & ü )");
        assert_eq!(lexed.tokens.last().unwrap().kind, R_PAREN);
        assert_eq!(lexed.diagnostics.len(), 2);
    }

    #[test]
    fn 数値() {
        assert_eq!(
            kinds("42 1_000_000 3.14 1.0e9 2.5E-3"),
            [
                INT, WHITESPACE, INT, WHITESPACE, FLOAT, WHITESPACE, FLOAT, WHITESPACE, FLOAT
            ]
        );
    }

    #[test]
    fn 範囲の点2つは浮動小数点にしない() {
        assert_eq!(kinds("1..2"), [INT, DOT2, INT]);
        assert_eq!(kinds("1.x"), [INT, DOT, LOWER_NAME]);
    }

    #[test]
    fn 数値の直後の英字はかたまりごとエラー() {
        for src in ["1e9", "1.0e", "3px", "1.0e9x"] {
            let lexed = lex(src);
            assert_eq!(kinds(src), [ERROR_TOKEN], "{src}");
            assert_eq!(lexed.diagnostics.len(), 1, "{src}");
        }
        // 指数の数字が欠けたら，符号の手前までがエラー
        assert_eq!(kinds("1.0e+"), [ERROR_TOKEN, PLUS]);
    }

    #[test]
    fn 十六進と二進の整数() {
        assert_eq!(
            kinds("0xFF 0xdead_BEEF 0b1010 0b1111_0000 0x0"),
            [
                INT, WHITESPACE, INT, WHITESPACE, INT, WHITESPACE, INT, WHITESPACE, INT
            ]
        );
        // 2進の後ろの範囲とフィールドは今までどおり
        assert_eq!(kinds("0b1..0x2"), [INT, DOT2, INT]);
    }

    #[test]
    fn 接頭辞の誤りと十六進の小数はかたまりごとエラー() {
        for src in [
            "0XFF", "0B1", "0o17", "0x", "0b", "0xFG", "0b102", "0x_1", "0x1.5", "0b1.0",
        ] {
            let lexed = lex(src);
            assert_eq!(kinds(src), [ERROR_TOKEN], "{src}");
            assert_eq!(lexed.diagnostics.len(), 1, "{src}");
            assert_eq!(
                lexed.diagnostics[0].code,
                DiagnosticCode::InvalidNumber,
                "{src}"
            );
        }
    }

    #[test]
    fn 補間() {
        assert_eq!(kinds(r##""""##), [STRING_QUOTE, STRING_QUOTE]);
        assert_eq!(
            kinds(r##""#{x}""##),
            [
                STRING_QUOTE,
                INTERP_START,
                LOWER_NAME,
                INTERP_END,
                STRING_QUOTE
            ]
        );
        assert_eq!(
            kinds(r##""hello #{user.name}!""##),
            [
                STRING_QUOTE,
                STRING_TEXT,
                INTERP_START,
                LOWER_NAME,
                DOT,
                LOWER_NAME,
                INTERP_END,
                STRING_TEXT,
                STRING_QUOTE,
            ]
        );
    }

    #[test]
    fn 補間の中の括弧と文字列() {
        assert_eq!(
            kinds(r##""#{f({})}""##),
            [
                STRING_QUOTE,
                INTERP_START,
                LOWER_NAME,
                L_PAREN,
                L_BRACE,
                R_BRACE,
                R_PAREN,
                INTERP_END,
                STRING_QUOTE,
            ]
        );
        assert_eq!(
            kinds(r##""#{"#{x}"}""##),
            [
                STRING_QUOTE,
                INTERP_START,
                STRING_QUOTE,
                INTERP_START,
                LOWER_NAME,
                INTERP_END,
                STRING_QUOTE,
                INTERP_END,
                STRING_QUOTE,
            ]
        );
        assert_eq!(kinds("}"), [R_BRACE]);
    }

    #[test]
    fn 閉じていない文字列() {
        let lexed = lex("\"\nx");
        assert_eq!(kinds("\"\nx"), [STRING_QUOTE, NEWLINE, LOWER_NAME]);
        assert_eq!(lexed.diagnostics.len(), 1);
        // 文字列と補間の両方が閉じていない
        let lexed = lex(r##""#{x"##);
        assert_eq!(kinds(r##""#{x"##), [STRING_QUOTE, INTERP_START, LOWER_NAME]);
        assert_eq!(lexed.diagnostics.len(), 2);
    }

    proptest::proptest! {
        #[test]
        fn 連結すると入力に戻る(src in "\\PC*") {
            let lexed = lex(&src);
            let text: String = lexed.tokens.iter().map(|t| &src[t.range]).collect();
            proptest::prop_assert_eq!(text, src);
        }
    }
}
