//! An event-based parser.
//!
//! The parser does not build the tree directly; it only pushes `Event`s. Trivia is invisible to
//! the parser and is inserted by `sink` when the tree is built.

mod grammar;
mod sink;

use std::cell::Cell;

use crate::SyntaxKind::{self, *};
use crate::Token;
use crate::diagnostic::DiagnosticCode;

pub(crate) use grammar::root;
pub(crate) use sink::build;

#[derive(Debug)]
pub(crate) enum Event {
    /// `forward_parent` is the offset to a parent attached later by `precede`.
    Start {
        kind: SyntaxKind,
        forward_parent: Option<u32>,
    },
    Token,
    Finish,
    Error {
        code: DiagnosticCode,
        message: String,
    },
    /// The start of a node whose kind is not decided yet, or that was abandoned.
    Tombstone,
}

/// How many times the parser may look at the same position without advancing. Exceeding it is a
/// parser bug, so it panics.
const FUEL: u32 = 256;

pub(crate) struct Parser<'t> {
    /// Tokens without trivia. The text is used to recognize contextual words.
    tokens: Vec<ParserToken<'t>>,
    pos: usize,
    events: Vec<Event>,
    fuel: Cell<u32>,
}

struct ParserToken<'t> {
    kind: SyntaxKind,
    text: &'t str,
    /// There is a newline since the previous token, continuation newlines (trivia) included.
    line_start: bool,
    /// There is a doc comment `##` since the previous token.
    doc_before: bool,
}

/// Tokens that start a declaration. Error recovery stops at one at the start of a line instead of
/// skipping it.
const DECL_START: &[SyntaxKind] = &[
    FN_KW, PUB_KW, SUSPEND_KW, OPAQUE_KW, TYPE_KW, ENUM_KW, ERROR_KW, CONST_KW, EFFECT_KW,
    HANDLER_KW, LAYER_KW, TRAIT_KW, IMPL_KW, IMPORT_KW, AT,
];

impl<'t> Parser<'t> {
    pub(crate) fn new(src: &'t str, tokens: &[Token]) -> Self {
        let mut line_start = true;
        let mut doc_before = false;
        let mut parser_tokens = Vec::new();
        for t in tokens {
            match t.kind {
                NEWLINE_CONT => line_start = true,
                DOC_COMMENT => doc_before = true,
                _ => {}
            }
            if t.kind.is_trivia() {
                continue;
            }
            parser_tokens.push(ParserToken {
                kind: t.kind,
                text: &src[t.range],
                line_start,
                doc_before,
            });
            line_start = t.kind == NEWLINE;
            doc_before = false;
        }
        Parser {
            tokens: parser_tokens,
            pos: 0,
            events: Vec::new(),
            fuel: Cell::new(FUEL),
        }
    }

    /// The number of tokens read, used to check that a grammar function advanced.
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }

    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        let fuel = self.fuel.get();
        assert!(fuel > 0, "the parser is stuck at position {}", self.pos);
        self.fuel.set(fuel - 1);
        self.tokens.get(self.pos + n).map_or(EOF, |t| t.kind)
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    /// Contextual words (`fails` and so on) lex as LOWER_NAME, so they are told apart by text.
    pub(crate) fn at_contextual_kw(&self, kw: &str) -> bool {
        self.at(LOWER_NAME) && self.tokens[self.pos].text == kw
    }

    /// At the start of a declaration at the start of a line. `fn(` is excluded as a lambda.
    ///
    /// Inside an unclosed `(` newlines are continuations and no NEWLINE arrives, so this is how
    /// recovery from a broken declaration finds the next one.
    pub(crate) fn at_decl_start(&self) -> bool {
        let kind = self.current();
        let line_start = self.tokens.get(self.pos).is_some_and(|t| t.line_start);
        line_start && DECL_START.contains(&kind) && !(kind == FN_KW && self.nth(1) == L_PAREN)
    }

    pub(crate) fn at_doc_comment(&self) -> bool {
        self.tokens.get(self.pos).is_some_and(|t| t.doc_before)
    }

    pub(crate) fn nth_at_contextual_kw(&self, n: usize, kw: &str) -> bool {
        self.nth(n) == LOWER_NAME && self.tokens[self.pos + n].text == kw
    }

    /// At the start of a declaration other than `fn` and `suspend`, at the start of a line. Items
    /// in a declaration body start with `fn` at the start of a line, so inside a body this is the
    /// recovery sync point.
    pub(crate) fn at_top_decl_start(&self) -> bool {
        self.at_decl_start() && !matches!(self.current(), FN_KW | SUSPEND_KW)
    }

    pub(crate) fn at_line_start(&self) -> bool {
        self.tokens.get(self.pos).is_some_and(|t| t.line_start)
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.at(EOF)
    }

    pub(crate) fn bump(&mut self) {
        assert!(!self.at_eof(), "cannot advance past EOF");
        self.pos += 1;
        self.fuel.set(FUEL);
        self.events.push(Event::Token);
    }

    /// Being elsewhere is a grammar bug, so it panics.
    pub(crate) fn bump_kind(&mut self, kind: SyntaxKind) {
        assert!(
            self.at(kind),
            "expected to be at {kind:?}, found {:?}",
            self.current()
        );
        self.bump();
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if !self.at(kind) {
            return false;
        }
        self.bump();
        true
    }

    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(
            DiagnosticCode::ExpectedToken,
            format!("expected {}", kind.describe()),
        );
        false
    }

    pub(crate) fn error(&mut self, code: DiagnosticCode, message: impl Into<String>) {
        self.events.push(Event::Error {
            code,
            message: message.into(),
        });
    }

    /// Reports a diagnostic, and unless the next token is in `recovery`, wraps one token in ERROR
    /// and skips it.
    pub(crate) fn err_recover(
        &mut self,
        code: DiagnosticCode,
        message: impl Into<String>,
        recovery: &[SyntaxKind],
    ) {
        self.error(code, message);
        if self.at_eof() || recovery.contains(&self.current()) || self.at_decl_start() {
            return;
        }
        let m = self.start();
        self.bump();
        m.complete(self, SyntaxKind::ERROR);
    }

    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.events.push(Event::Tombstone);
        Marker { pos }
    }
}

/// An open node. Dropping it without `complete` leaves a Tombstone, which never reaches the tree.
pub(crate) struct Marker {
    pos: u32,
}

impl Marker {
    /// The start stays a Tombstone and never reaches the tree.
    pub(crate) fn abandon(self, _p: &mut Parser<'_>) {}

    pub(crate) fn complete(self, p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
        p.events[self.pos as usize] = Event::Start {
            kind,
            forward_parent: None,
        };
        p.events.push(Event::Finish);
        CompletedMarker { pos: self.pos }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CompletedMarker {
    pos: u32,
}

impl CompletedMarker {
    /// Wraps a completed node in a new parent. Used for bindings `pattern = expr` and binary
    /// operators.
    pub(crate) fn precede(self, p: &mut Parser<'_>) -> Marker {
        let parent = p.start();
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(parent.pos - self.pos),
            _ => unreachable!("a closed node must begin with Start"),
        }
        parent
    }
}
