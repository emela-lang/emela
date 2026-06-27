use std::sync::Arc;

use crate::error::{Diagnostic, Error, Result, SourceFile, Span};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Fn,
    Import,
    Struct,
    Enum,
    Match,
    True,
    False,
    Ident(String),
    Int(i32),
    String(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Colon,
    Hash,
    Bang,
    Eq,
    EqEq,
    Arrow,
    Pipe,
    Lt,
    Gt,
    Plus,
    Minus,
    Star,
    Newline,
    Eof,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
}

pub(crate) fn lex_with_file(source: &str, file: Arc<SourceFile>) -> Result<Vec<Token>> {
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
                    span: Span::new(file.clone(), pos, pos + 1),
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
                    span: Span::new(file.clone(), pos, pos + 2),
                });
                i += 2;
            }
            b'|' if i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                tokens.push(Token {
                    kind: TokenKind::Pipe,
                    span: Span::new(file.clone(), pos, pos + 2),
                });
                i += 2;
            }
            b'(' => push_one(&mut tokens, TokenKind::LParen, file.clone(), pos, &mut i),
            b')' => push_one(&mut tokens, TokenKind::RParen, file.clone(), pos, &mut i),
            b'{' => push_one(&mut tokens, TokenKind::LBrace, file.clone(), pos, &mut i),
            b'}' => push_one(&mut tokens, TokenKind::RBrace, file.clone(), pos, &mut i),
            b'[' => push_one(&mut tokens, TokenKind::LBracket, file.clone(), pos, &mut i),
            b']' => push_one(&mut tokens, TokenKind::RBracket, file.clone(), pos, &mut i),
            b',' => push_one(&mut tokens, TokenKind::Comma, file.clone(), pos, &mut i),
            b'.' => push_one(&mut tokens, TokenKind::Dot, file.clone(), pos, &mut i),
            b':' => push_one(&mut tokens, TokenKind::Colon, file.clone(), pos, &mut i),
            b'#' => push_one(&mut tokens, TokenKind::Hash, file.clone(), pos, &mut i),
            b'!' => push_one(&mut tokens, TokenKind::Bang, file.clone(), pos, &mut i),
            b'<' => push_one(&mut tokens, TokenKind::Lt, file.clone(), pos, &mut i),
            b'>' => push_one(&mut tokens, TokenKind::Gt, file.clone(), pos, &mut i),
            b'+' => push_one(&mut tokens, TokenKind::Plus, file.clone(), pos, &mut i),
            b'*' => push_one(&mut tokens, TokenKind::Star, file.clone(), pos, &mut i),
            b'-' => push_one(&mut tokens, TokenKind::Minus, file.clone(), pos, &mut i),
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::EqEq,
                    span: Span::new(file.clone(), pos, pos + 2),
                });
                i += 2;
            }
            b'=' => push_one(&mut tokens, TokenKind::Eq, file.clone(), pos, &mut i),
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let text = &source[start..i];
                let value = text.parse::<i32>().map_err(|_| {
                    Error::diagnostic(
                        Diagnostic::new("Integer literal is too large")
                            .label(
                                Span::new(file.clone(), start, i),
                                "This number does not fit in I32.",
                            )
                            .help("Use a value between -2147483648 and 2147483647."),
                    )
                })?;
                tokens.push(Token {
                    kind: TokenKind::Int(value),
                    span: Span::new(file.clone(), start, i),
                });
            }
            b'"' => {
                i += 1;
                let mut value = String::new();
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\\' => {
                            i += 1;
                            if i >= bytes.len() {
                                return Err(unterminated_string(file.clone(), pos));
                            }
                            let escaped = match bytes[i] {
                                b'"' => '"',
                                b'\\' => '\\',
                                b'n' => '\n',
                                b't' => '\t',
                                other => {
                                    return Err(Error::diagnostic(
                                        Diagnostic::new("Unsupported string escape")
                                            .label(
                                                Span::new(file.clone(), i.saturating_sub(1), i + 1),
                                                format!(
                                                    "`\\{}` is not a supported escape sequence.",
                                                    other as char
                                                ),
                                            )
                                            .help(
                                                "Supported escapes are \\n, \\t, \\\", and \\\\.",
                                            ),
                                    ));
                                }
                            };
                            value.push(escaped);
                            i += 1;
                        }
                        b'\n' => {
                            return Err(unterminated_string(file.clone(), pos));
                        }
                        _ => {
                            let ch = source[i..].chars().next().expect("valid char boundary");
                            value.push(ch);
                            i += ch.len_utf8();
                        }
                    }
                }
                if i > bytes.len() || bytes.get(i.saturating_sub(1)) != Some(&b'"') {
                    return Err(unterminated_string(file.clone(), pos));
                }
                tokens.push(Token {
                    kind: TokenKind::String(value),
                    span: Span::new(file.clone(), pos, i),
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
                    "import" => TokenKind::Import,
                    "struct" => TokenKind::Struct,
                    "enum" => TokenKind::Enum,
                    "match" => TokenKind::Match,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    _ => TokenKind::Ident(text.to_string()),
                };
                tokens.push(Token {
                    kind,
                    span: Span::new(file.clone(), start, i),
                });
            }
            other => {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unexpected character")
                        .label(
                            Span::new(file.clone(), pos, pos + 1),
                            format!("I do not know how to read `{}` here.", other as char),
                        )
                        .help("Remove this character, or replace it with valid Emela syntax."),
                ));
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::point(file.clone(), source.len()),
    });
    Ok(tokens)
}

fn push_one(
    tokens: &mut Vec<Token>,
    kind: TokenKind,
    file: Arc<SourceFile>,
    pos: usize,
    i: &mut usize,
) {
    tokens.push(Token {
        kind,
        span: Span::new(file, pos, pos + 1),
    });
    *i += 1;
}

fn unterminated_string(file: Arc<SourceFile>, pos: usize) -> Error {
    Error::diagnostic(
        Diagnostic::new("Unterminated string")
            .label(
                Span::new(file, pos, pos + 1),
                "This string starts here but never closes.",
            )
            .help("Add a closing double quote before the end of the line."),
    )
}
