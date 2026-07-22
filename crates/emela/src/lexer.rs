use std::sync::Arc;

use crate::error::{Diagnostic, Error, Result, SourceFile, Span};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Fn,
    Extern,
    Intrinsic,
    Trait,
    Impl,
    Effect,
    For,
    Import,
    Let,
    Module,
    Pub,
    Uses,
    Enum,
    /// `record` (spec 0006).
    Record,
    Match,
    If,
    Else,
    Throws,
    Throw,
    Try,
    Catch,
    Panic,
    True,
    False,
    Ident(String),
    Int(i32),
    Float(f64),
    String(String),
    Char(char),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    /// `::` — the type-path separator for enum variants (`Enum::Variant`; specs
    /// 0005/0018).
    ColonColon,
    Dot,
    Eq,
    EqEq,
    Ne,
    Arrow,
    Lt,
    Gt,
    Le,
    Ge,
    Plus,
    PlusPlus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    AmpAmp,
    PipePipe,
    /// `|>` — the pipeline operator (spec 0019). Desugared in the parser to a
    /// first-argument-insertion `Call`, so no later stage sees this token.
    PipeGt,
    Question,
    /// `@name` — an attribute (spec 0039). The `@` and the name are one token,
    /// which is what makes them inseparable (R1: no whitespace between them).
    At(String),
    Newline,
    Eof,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
}

/// A `--` comment, discarded by `lex` but collected by `lex_with_comments`
/// for the formatter. The span covers `--` through the end of the comment
/// text (excluding the newline); the text itself is sliced from the source.
#[derive(Debug, Clone)]
pub(crate) struct Comment {
    pub(crate) span: Span,
}

/// Lexes `source`, collecting every error instead of stopping at the first
/// (spec 0033). A malformed literal still produces a placeholder token and an
/// unknown character is skipped, so the parser always gets a full token stream.
pub(crate) fn lex(label: &str, source: &str) -> (Vec<Token>, Vec<Error>) {
    let file = SourceFile::new(label, source.to_string());
    lex_with_file(source, file, None)
}

/// Like `lex`, but additionally collects every comment (spec 0035 F7). The
/// token stream is identical to `lex`'s. Formatting requires a clean lex, so a
/// collected error is surfaced as a single `Err` (unlike `lex`, which keeps
/// going for the multi-error compile/LSP paths, spec 0033).
pub(crate) fn lex_with_comments(label: &str, source: &str) -> Result<(Vec<Token>, Vec<Comment>)> {
    let file = SourceFile::new(label, source.to_string());
    let mut comments = Vec::new();
    let (tokens, errors) = lex_with_file(source, file, Some(&mut comments));
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    Ok((tokens, comments))
}

fn lex_with_file(
    source: &str,
    file: Arc<SourceFile>,
    mut comments: Option<&mut Vec<Comment>>,
) -> (Vec<Token>, Vec<Error>) {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    // Open-bracket stack for newline significance (spec 0034): inside `(...)`
    // and `[...]` a newline is whitespace; a `{` frame restores significance,
    // so statements inside `foo(match x { ... })` are still newline-separated.
    let mut brackets: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        match bytes[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' if matches!(brackets.last(), Some(b'(' | b'[')) => i += 1,
            b'\n' => push(&mut tokens, TokenKind::Newline, file.clone(), start, &mut i),
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                if let Some(comments) = comments.as_deref_mut() {
                    comments.push(Comment {
                        span: Span::new(file.clone(), start, i),
                    });
                }
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                tokens.push(Token {
                    kind: TokenKind::Arrow,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::EqEq,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            // Two-character comparison operators (spec 0027); matched before the
            // single-character `<` / `>` arms below.
            b'!' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::Ne,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'<' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::Le,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'>' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::Ge,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'(' => {
                brackets.push(b'(');
                push(&mut tokens, TokenKind::LParen, file.clone(), start, &mut i)
            }
            b')' => {
                brackets.pop();
                push(&mut tokens, TokenKind::RParen, file.clone(), start, &mut i)
            }
            b'{' => {
                brackets.push(b'{');
                push(&mut tokens, TokenKind::LBrace, file.clone(), start, &mut i)
            }
            b'}' => {
                brackets.pop();
                push(&mut tokens, TokenKind::RBrace, file.clone(), start, &mut i)
            }
            b'[' => {
                brackets.push(b'[');
                push(
                    &mut tokens,
                    TokenKind::LBracket,
                    file.clone(),
                    start,
                    &mut i,
                )
            }
            b']' => {
                brackets.pop();
                push(
                    &mut tokens,
                    TokenKind::RBracket,
                    file.clone(),
                    start,
                    &mut i,
                )
            }
            b',' => push(&mut tokens, TokenKind::Comma, file.clone(), start, &mut i),
            b':' if i + 1 < bytes.len() && bytes[i + 1] == b':' => {
                tokens.push(Token {
                    kind: TokenKind::ColonColon,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b':' => push(&mut tokens, TokenKind::Colon, file.clone(), start, &mut i),
            b'.' => push(&mut tokens, TokenKind::Dot, file.clone(), start, &mut i),
            b'=' => push(&mut tokens, TokenKind::Eq, file.clone(), start, &mut i),
            b'<' => push(&mut tokens, TokenKind::Lt, file.clone(), start, &mut i),
            b'>' => push(&mut tokens, TokenKind::Gt, file.clone(), start, &mut i),
            b'+' if i + 1 < bytes.len() && bytes[i + 1] == b'+' => {
                tokens.push(Token {
                    kind: TokenKind::PlusPlus,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            // Short-circuiting logical operators (spec 0027). Only the doubled
            // forms are tokens; a lone `&` / `|` is not used by the language yet.
            b'&' if i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                tokens.push(Token {
                    kind: TokenKind::AmpAmp,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'|' if i + 1 < bytes.len() && bytes[i + 1] == b'|' => {
                tokens.push(Token {
                    kind: TokenKind::PipePipe,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            // The pipeline operator `|>` (spec 0019). A lone `|` remains unused.
            b'|' if i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                tokens.push(Token {
                    kind: TokenKind::PipeGt,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'+' => push(&mut tokens, TokenKind::Plus, file.clone(), start, &mut i),
            b'-' => push(&mut tokens, TokenKind::Minus, file.clone(), start, &mut i),
            b'*' => push(&mut tokens, TokenKind::Star, file.clone(), start, &mut i),
            b'/' => push(&mut tokens, TokenKind::Slash, file.clone(), start, &mut i),
            b'%' => push(&mut tokens, TokenKind::Percent, file.clone(), start, &mut i),
            // A lone `!`; `!=` is matched earlier (spec 0027).
            b'!' => push(&mut tokens, TokenKind::Bang, file.clone(), start, &mut i),
            b'?' => push(
                &mut tokens,
                TokenKind::Question,
                file.clone(),
                start,
                &mut i,
            ),
            b'@' => {
                // An attribute (spec 0039 R1): the name must immediately follow
                // the `@`. Lexed as a single token so no whitespace can slip in.
                i += 1;
                let name_start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if i == name_start {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Attribute name is missing").label(
                            Span::new(file.clone(), start, start + 1),
                            "`@` must be immediately followed by an attribute name, e.g. `@test`",
                        ),
                    ));
                    continue;
                }
                tokens.push(Token {
                    kind: TokenKind::At(source[name_start..i].to_string()),
                    span: Span::new(file.clone(), start, i),
                });
            }
            b'0'..=b'9' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let text = &source[start..i];
                if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let text = &source[start..i];
                    // A malformed literal is reported but still yields a
                    // placeholder token, so lexing and parsing continue.
                    let value = text.parse::<f64>().unwrap_or_else(|_| {
                        errors.push(Error::diagnostic(
                            Diagnostic::new("Float literal is invalid")
                                .label(Span::new(file.clone(), start, i), "invalid Float literal"),
                        ));
                        0.0
                    });
                    tokens.push(Token {
                        kind: TokenKind::Float(value),
                        span: Span::new(file.clone(), start, i),
                    });
                    continue;
                }
                let value = text.parse::<i32>().unwrap_or_else(|_| {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Integer literal is too large")
                            .label(Span::new(file.clone(), start, i), "does not fit in Int"),
                    ));
                    0
                });
                tokens.push(Token {
                    kind: TokenKind::Int(value),
                    span: Span::new(file.clone(), start, i),
                });
            }
            b'"' => {
                i += 1;
                let mut value = String::new();
                let mut terminated = false;
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' => {
                            i += 1;
                            terminated = true;
                            break;
                        }
                        b'\\' => {
                            i += 1;
                            if i >= bytes.len() {
                                break;
                            }
                            match bytes[i] {
                                b'n' => value.push('\n'),
                                b'r' => value.push('\r'),
                                b't' => value.push('\t'),
                                b'"' => value.push('"'),
                                b'\\' => value.push('\\'),
                                other => {
                                    errors.push(Error::diagnostic(
                                        Diagnostic::new("Unsupported string escape").label(
                                            Span::new(file.clone(), i - 1, i + 1),
                                            format!("unsupported escape `\\{}`", other as char),
                                        ),
                                    ));
                                }
                            }
                            i += 1;
                        }
                        b'\n' => break,
                        _ => {
                            let ch = source[i..].chars().next().expect("char boundary");
                            value.push(ch);
                            i += ch.len_utf8();
                        }
                    }
                }
                if !terminated {
                    errors.push(unterminated_string(file.clone(), start));
                }
                tokens.push(Token {
                    kind: TokenKind::String(value),
                    span: Span::new(file.clone(), start, i),
                });
            }
            b'\'' => {
                // On any malformed literal, report it and still emit a
                // placeholder `Char` token so the parser continues past it.
                i += 1;
                let mut value = None;
                let mut reported = false;
                if i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 1;
                        if i < bytes.len() {
                            match bytes[i] {
                                b'n' => value = Some('\n'),
                                b'r' => value = Some('\r'),
                                b't' => value = Some('\t'),
                                b'\'' => value = Some('\''),
                                b'"' => value = Some('"'),
                                b'\\' => value = Some('\\'),
                                other => {
                                    errors.push(Error::diagnostic(
                                        Diagnostic::new("Unsupported character escape").label(
                                            Span::new(file.clone(), i - 1, i + 1),
                                            format!("unsupported escape `\\{}`", other as char),
                                        ),
                                    ));
                                    reported = true;
                                }
                            }
                            i += 1;
                        }
                    } else {
                        let ch = source[i..].chars().next().expect("char boundary");
                        value = Some(ch);
                        i += ch.len_utf8();
                    }
                }
                let closed = i < bytes.len() && bytes[i] == b'\'';
                if closed {
                    i += 1;
                }
                if (value.is_none() || !closed) && !reported {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Invalid character literal").label(
                            Span::new(file.clone(), start, i.max(start + 1)),
                            "a character literal holds exactly one character, e.g. `'a'`",
                        ),
                    ));
                }
                tokens.push(Token {
                    kind: TokenKind::Char(value.unwrap_or('\0')),
                    span: Span::new(file.clone(), start, i),
                });
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let text = &source[start..i];
                let kind = match text {
                    "fn" => TokenKind::Fn,
                    "extern" => TokenKind::Extern,
                    "intrinsic" => TokenKind::Intrinsic,
                    "trait" => TokenKind::Trait,
                    "impl" => TokenKind::Impl,
                    "effect" => TokenKind::Effect,
                    "for" => TokenKind::For,
                    "import" => TokenKind::Import,
                    "let" => TokenKind::Let,
                    "module" => TokenKind::Module,
                    "pub" => TokenKind::Pub,
                    "uses" => TokenKind::Uses,
                    "enum" => TokenKind::Enum,
                    "record" => TokenKind::Record,
                    "match" => TokenKind::Match,
                    "if" => TokenKind::If,
                    "else" => TokenKind::Else,
                    "throws" => TokenKind::Throws,
                    "throw" => TokenKind::Throw,
                    "try" => TokenKind::Try,
                    "catch" => TokenKind::Catch,
                    "panic" => TokenKind::Panic,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    _ => TokenKind::Ident(text.to_string()),
                };
                tokens.push(Token {
                    kind,
                    span: Span::new(file.clone(), start, i),
                });
            }
            _ => {
                // Report and skip the whole character (which may be multi-byte)
                // so lexing resumes at the next boundary.
                let ch = source[i..].chars().next().expect("char boundary");
                let end = start + ch.len_utf8();
                errors.push(Error::diagnostic(
                    Diagnostic::new("Unexpected character").label(
                        Span::new(file.clone(), start, end),
                        format!("unexpected character `{ch}`"),
                    ),
                ));
                i = end;
            }
        }
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::point(file, source.len()),
    });
    (tokens, errors)
}

fn push(
    tokens: &mut Vec<Token>,
    kind: TokenKind,
    file: Arc<SourceFile>,
    start: usize,
    i: &mut usize,
) {
    tokens.push(Token {
        kind,
        span: Span::new(file, start, start + 1),
    });
    *i += 1;
}

fn unterminated_string(file: Arc<SourceFile>, start: usize) -> Error {
    Error::diagnostic(
        Diagnostic::new("Unterminated string")
            .label(Span::new(file, start, start + 1), "string starts here")
            .help("Add a closing double quote before the end of the line."),
    )
}
