//! Formats a syntax tree.
//!
//! The input is the lossless tree from `emela_syntax`. Comments and blank lines stay in the
//! tree, so the printer walks it instead of rebuilding a token stream. Line breaking is left to
//! `pretty` (Wadler-style).
//!
//! Files with syntax errors are not formatted: the contents of an ERROR node do not follow the
//! grammar, so reprinting them could break the file.

mod comments;
mod printer;

use std::fmt;

use emela_syntax::{Diagnostic, SyntaxKind, SyntaxNode, SyntaxToken};

pub const DEFAULT_WIDTH: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The input has syntax errors. The diagnostics are those of `emela_syntax::parse`.
    Syntax(Vec<Diagnostic>),
    /// The output did not reproduce the input's tokens, or a comment was dropped. This is a
    /// formatter bug, so we stop rather than rewrite the input.
    Internal(String),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Syntax(diagnostics) => {
                let n = diagnostics.len();
                let plural = if n == 1 { "" } else { "s" };
                write!(
                    f,
                    "cannot format a file with syntax errors ({n} error{plural})"
                )
            }
            FormatError::Internal(message) => write!(f, "internal formatter error: {message}"),
        }
    }
}

impl std::error::Error for FormatError {}

pub fn format(src: &str) -> Result<String, FormatError> {
    format_with_width(src, DEFAULT_WIDTH)
}

/// `width` is a target for the display width of a line. Tokens that cannot fit (such as long
/// strings) exceed it.
pub fn format_with_width(src: &str, width: usize) -> Result<String, FormatError> {
    let parse = emela_syntax::parse(src);
    if !parse.diagnostics().is_empty() {
        return Err(FormatError::Syntax(parse.diagnostics().to_vec()));
    }
    let root = parse.syntax();
    let mut printer = printer::Printer::new(&root, width);
    let doc = printer.root(&root);
    if printer.remaining_comments() != 0 {
        return Err(FormatError::Internal(format!(
            "{} comment(s) were not printed",
            printer.remaining_comments()
        )));
    }
    let out = finish(&printer::render(&doc, width));
    verify(&root, &out)?;
    Ok(out)
}

/// Strips trailing whitespace and ends the output with exactly one newline.
///
/// Whitespace before a newline inside a multiline string `"""` is part of the value, so it is
/// kept.
fn finish(rendered: &str) -> String {
    let mut in_string = std::collections::HashSet::new();
    for token in emela_syntax::lex(rendered).tokens {
        if matches!(
            token.kind,
            SyntaxKind::STRING_TEXT | SyntaxKind::TRIPLE_QUOTE
        ) {
            let start = usize::from(token.range.start());
            for (i, _) in rendered[token.range].match_indices('\n') {
                in_string.insert(start + i);
            }
        }
    }
    let mut out = String::with_capacity(rendered.len());
    let mut offset = 0;
    for line in rendered.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let newline_at = offset + body.len();
        offset += line.len();
        let body = body.strip_suffix('\r').unwrap_or(body);
        if in_string.contains(&newline_at) {
            out.push_str(body);
        } else {
            out.push_str(body.trim_end());
        }
        out.push('\n');
    }
    let trimmed = out.trim_end_matches('\n').len();
    out.truncate(trimmed);
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Checks that formatting did not change the meaning: the output must parse without errors, and
/// its tokens (`normalized_tokens`) and comments must match the input's.
fn verify(before: &SyntaxNode, out: &str) -> Result<(), FormatError> {
    let parse = emela_syntax::parse(out);
    if let Some(d) = parse.diagnostics().first() {
        return Err(FormatError::Internal(format!(
            "the output has a syntax error: {} at {:?}",
            d.message, d.range
        )));
    }
    let after = parse.syntax();
    if normalized_tokens(before) != normalized_tokens(&after) {
        return Err(FormatError::Internal(
            "the output changed the tokens".into(),
        ));
    }
    if comment_texts(before) != comment_texts(&after) {
        return Err(FormatError::Internal(
            "the output changed the comments".into(),
        ));
    }
    Ok(())
}

/// The tokens that carry meaning, minus what formatting is allowed to change:
///
/// - trivia (whitespace, continuation newlines, comments)
/// - trailing commas in lists (a comma right before a closing bracket)
/// - statement-separating newlines right after a block's `{`, right before `}`, around commas,
///   and at the start and end of the input; consecutive newlines are collapsed into one
/// - newlines that do not separate anything (after an annotation, before `derive`, ...)
/// - the remaining newlines become `,` (enum variants and the like accept either)
#[doc(hidden)]
pub fn normalized_tokens(root: &SyntaxNode) -> Vec<(SyntaxKind, String)> {
    use SyntaxKind::*;
    let tokens: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .filter(|t| t.kind() != NEWLINE || separates(t))
        // A `\r\n` inside a multiline string compares equal to `\n`.
        .map(|t| (t.kind(), t.text().replace("\r\n", "\n")))
        .collect();
    let mut out: Vec<(SyntaxKind, String)> = Vec::new();
    for (i, (kind, text)) in tokens.iter().enumerate() {
        let next = tokens[i + 1..]
            .iter()
            .map(|(k, _)| *k)
            .find(|k| *k != NEWLINE);
        match kind {
            NEWLINE => {
                let prev = out.last().map(|(k, _)| *k);
                let drop = matches!(prev, None | Some(NEWLINE | L_BRACE | COMMA))
                    || matches!(next, None | Some(R_BRACE | COMMA));
                // A remaining newline counts as a `,` separator: enum variants and the like
                // accept either, and the formatter swaps one for the other.
                if !drop {
                    out.push((COMMA, ",".to_owned()));
                }
            }
            COMMA if matches!(next, Some(R_PAREN | R_BRACK | R_BRACE)) => {}
            _ => out.push((*kind, text.clone())),
        }
    }
    out
}

/// Only newlines in a file, block, arm list or declaration body separate anything. The parser
/// skips newlines after an annotation, before `derive` and inside an effect set.
fn separates(newline: &SyntaxToken) -> bool {
    use SyntaxKind::*;
    newline.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            ROOT | BLOCK_EXPR | MATCH_ARM_LIST | ITEM_LIST | VARIANT_LIST
        )
    })
}

/// Comment texts, with trailing whitespace removed.
#[doc(hidden)]
pub fn comment_texts(root: &SyntaxNode) -> Vec<String> {
    root.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::COMMENT | SyntaxKind::DOC_COMMENT))
        .map(|t| t.text().trim_end().to_owned())
        .collect()
}
