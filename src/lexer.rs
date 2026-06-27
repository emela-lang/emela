use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Fn,
    Match,
    True,
    False,
    Ident(String),
    Int(i32),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Hash,
    Bang,
    Eq,
    EqEq,
    Arrow,
    Lt,
    Plus,
    Minus,
    Star,
    Newline,
    Eof,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) pos: usize,
}

pub(crate) fn lex(source: &str) -> Result<Vec<Token>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let pos = i;
        match bytes[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' => {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    pos,
                });
                i += 1;
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                tokens.push(Token {
                    kind: TokenKind::Arrow,
                    pos,
                });
                i += 2;
            }
            b'(' => push_one(&mut tokens, TokenKind::LParen, pos, &mut i),
            b')' => push_one(&mut tokens, TokenKind::RParen, pos, &mut i),
            b'{' => push_one(&mut tokens, TokenKind::LBrace, pos, &mut i),
            b'}' => push_one(&mut tokens, TokenKind::RBrace, pos, &mut i),
            b'[' => push_one(&mut tokens, TokenKind::LBracket, pos, &mut i),
            b']' => push_one(&mut tokens, TokenKind::RBracket, pos, &mut i),
            b',' => push_one(&mut tokens, TokenKind::Comma, pos, &mut i),
            b'.' => push_one(&mut tokens, TokenKind::Dot, pos, &mut i),
            b'#' => push_one(&mut tokens, TokenKind::Hash, pos, &mut i),
            b'!' => push_one(&mut tokens, TokenKind::Bang, pos, &mut i),
            b'<' => push_one(&mut tokens, TokenKind::Lt, pos, &mut i),
            b'+' => push_one(&mut tokens, TokenKind::Plus, pos, &mut i),
            b'*' => push_one(&mut tokens, TokenKind::Star, pos, &mut i),
            b'-' => push_one(&mut tokens, TokenKind::Minus, pos, &mut i),
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::EqEq,
                    pos,
                });
                i += 2;
            }
            b'=' => push_one(&mut tokens, TokenKind::Eq, pos, &mut i),
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let text = &source[start..i];
                let value = text.parse::<i32>().map_err(|_| {
                    Error::new(format!("integer literal out of I32 range at byte {start}"))
                })?;
                tokens.push(Token {
                    kind: TokenKind::Int(value),
                    pos: start,
                });
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let text = &source[start..i];
                let kind = match text {
                    "fn" => TokenKind::Fn,
                    "match" => TokenKind::Match,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    _ => TokenKind::Ident(text.to_string()),
                };
                tokens.push(Token { kind, pos: start });
            }
            other => {
                return Err(Error::new(format!(
                    "unexpected byte {:?} at byte {pos}",
                    other as char
                )));
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        pos: source.len(),
    });
    Ok(tokens)
}

fn push_one(tokens: &mut Vec<Token>, kind: TokenKind, pos: usize, i: &mut usize) {
    tokens.push(Token { kind, pos });
    *i += 1;
}
