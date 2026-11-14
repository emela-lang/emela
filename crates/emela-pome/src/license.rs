//! `[pome].license`: the syntax of an SPDX license expression (spec `[pome.manifest]`, SPDX
//! specification 2.3 annex D).
//!
//! Only the syntax is checked. Identifiers are not looked up in the SPDX License List, so a
//! license identifier and a license exception identifier are any idstring (letters, digits, `-`
//! and `.`). The grammar, with `WITH` binding tighter than `AND` and `AND` tighter than `OR`:
//!
//! ```text
//! expression = and { "OR" and }
//! and        = term { "AND" term }
//! term       = "(" expression ")" | simple [ "WITH" idstring ]
//! simple     = idstring [ "+" ] | [ "DocumentRef-" idstring ":" ] "LicenseRef-" idstring
//! ```
//!
//! Words are separated by whitespace or parentheses; `+` follows its identifier directly.

const LICENSE_REF: &str = "LicenseRef-";
const DOCUMENT_REF: &str = "DocumentRef-";

/// Whether `text` is an SPDX license expression.
pub fn is_license_expression(text: &str) -> bool {
    let tokens = tokenize(text);
    let mut parser = Parser {
        tokens: &tokens,
        next: 0,
    };
    parser.expression() && parser.next == tokens.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Open,
    Close,
    Word(&'a str),
}

fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() || c == '(' || c == ')' {
            if let Some(s) = start.take() {
                tokens.push(Token::Word(&text[s..i]));
            }
            match c {
                '(' => tokens.push(Token::Open),
                ')' => tokens.push(Token::Close),
                _ => {}
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        tokens.push(Token::Word(&text[s..]));
    }
    tokens
}

struct Parser<'t, 'a> {
    tokens: &'t [Token<'a>],
    next: usize,
}

impl Parser<'_, '_> {
    fn peek(&self) -> Option<Token<'_>> {
        self.tokens.get(self.next).copied()
    }

    /// Takes the next token if it is the operator `op`.
    fn operator(&mut self, op: &str) -> bool {
        let found = self.peek() == Some(Token::Word(op));
        if found {
            self.next += 1;
        }
        found
    }

    fn expression(&mut self) -> bool {
        if !self.and() {
            return false;
        }
        while self.operator("OR") {
            if !self.and() {
                return false;
            }
        }
        true
    }

    fn and(&mut self) -> bool {
        if !self.term() {
            return false;
        }
        while self.operator("AND") {
            if !self.term() {
                return false;
            }
        }
        true
    }

    fn term(&mut self) -> bool {
        match self.peek() {
            Some(Token::Open) => {
                self.next += 1;
                self.expression() && self.peek() == Some(Token::Close) && {
                    self.next += 1;
                    true
                }
            }
            Some(Token::Word(word)) if is_simple(word) => {
                self.next += 1;
                if self.operator("WITH") {
                    match self.peek() {
                        Some(Token::Word(exception)) if is_id(exception) => {
                            self.next += 1;
                            true
                        }
                        _ => false,
                    }
                } else {
                    true
                }
            }
            _ => false,
        }
    }
}

/// A simple expression: a license identifier, optionally followed by `+`, or a reference.
fn is_simple(word: &str) -> bool {
    if is_operator(word) {
        return false;
    }
    if word.starts_with(LICENSE_REF) || word.starts_with(DOCUMENT_REF) {
        return is_license_ref(word);
    }
    is_idstring(word.strip_suffix('+').unwrap_or(word))
}

/// `[DocumentRef-<idstring>:]LicenseRef-<idstring>`.
fn is_license_ref(word: &str) -> bool {
    let reference = match word.strip_prefix(DOCUMENT_REF) {
        Some(rest) => match rest.split_once(':') {
            Some((document, reference)) if is_idstring(document) => reference,
            _ => return false,
        },
        None => word,
    };
    reference.strip_prefix(LICENSE_REF).is_some_and(is_idstring)
}

/// A license exception identifier.
fn is_id(word: &str) -> bool {
    !is_operator(word) && is_idstring(word)
}

fn is_operator(word: &str) -> bool {
    matches!(word, "AND" | "OR" | "WITH")
}

fn is_idstring(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_license_expressions() {
        for text in [
            "MIT",
            "MIT OR Apache-2.0",
            "Apache-2.0 AND MIT",
            "Apache-2.0 WITH LLVM-exception",
            "LicenseRef-Acme-Internal",
            "DocumentRef-spdx-tool-1.2:LicenseRef-MIT-Style-2",
            "GPL-2.0+",
            "GPL-2.0-or-later WITH Classpath-exception-2.0 OR MIT",
            "(MIT OR Apache-2.0) AND BSD-3-Clause",
            "((MIT))",
            "(MIT)AND(Apache-2.0)",
            "  MIT  ",
            "MIT\tOR\nApache-2.0",
            // Identifiers are not checked against the SPDX License List.
            "No-Such-License-1.0",
            "0BSD",
            // Not in the list, but an identifier as far as the syntax goes.
            "MIT WITH AdditionRef-x",
        ] {
            assert!(is_license_expression(text), "{text:?}");
        }
    }

    #[test]
    fn rejects_what_is_not_a_license_expression() {
        for text in [
            "",
            "   ",
            "MIT/Apache-2.0",
            "MIT, Apache-2.0",
            "MIT OR",
            "OR MIT",
            "MIT Apache-2.0",
            "MIT AND OR Apache-2.0",
            // The operators are in upper case.
            "MIT or Apache-2.0",
            "MIT And Apache-2.0",
            // `+` follows a license identifier directly, and not a reference.
            "GPL-2.0 +",
            "LicenseRef-x+",
            "MIT++",
            // `WITH` takes a simple expression on the left and an exception identifier on the right.
            "(MIT OR Apache-2.0) WITH LLVM-exception",
            "MIT WITH (LLVM-exception)",
            "MIT WITH LLVM-exception WITH Other-exception",
            "MIT WITH",
            // References need an idstring.
            "LicenseRef-",
            "DocumentRef-x:MIT",
            "DocumentRef-:LicenseRef-x",
            "DocumentRef-x:LicenseRef-",
            "(MIT",
            "MIT)",
            "()",
            "MIT_2",
            "AND",
        ] {
            assert!(!is_license_expression(text), "{text:?}");
        }
    }
}
