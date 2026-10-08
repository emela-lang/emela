//! イベント方式のパーサ．
//!
//! パーサは木を直接組まず，`Event` を積むだけにする．トリビアはパーサから見えず，
//! 木を組むときに `sink` が差し込む．

mod grammar;
mod sink;

use std::cell::Cell;

use crate::SyntaxKind::{self, EOF, LOWER_NAME};
use crate::Token;
use crate::diagnostic::DiagnosticCode;

pub(crate) use grammar::root;
pub(crate) use sink::build;

#[derive(Debug)]
pub(crate) enum Event {
    /// `forward_parent` は `precede` で後から付いた親の位置（自分からの差分）．
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
    /// まだ種類が決まっていない，または捨てたノードの開始．
    Tombstone,
}

/// 進まないまま同じ位置を見てよい回数．超えたらパーサの不具合なので panic する．
const FUEL: u32 = 256;

pub(crate) struct Parser<'t> {
    /// トリビアを除いたトークンの種類とテキスト．テキストは文脈依存の語の判定に使う．
    tokens: Vec<(SyntaxKind, &'t str)>,
    pos: usize,
    events: Vec<Event>,
    fuel: Cell<u32>,
}

impl<'t> Parser<'t> {
    pub(crate) fn new(src: &'t str, tokens: &[Token]) -> Self {
        let tokens = tokens
            .iter()
            .filter(|t| !t.kind.is_trivia())
            .map(|t| (t.kind, &src[t.range]))
            .collect();
        Parser {
            tokens,
            pos: 0,
            events: Vec::new(),
            fuel: Cell::new(FUEL),
        }
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }

    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        let fuel = self.fuel.get();
        assert!(fuel > 0, "パーサが位置 {} から進んでいない", self.pos);
        self.fuel.set(fuel - 1);
        self.tokens.get(self.pos + n).map_or(EOF, |&(kind, _)| kind)
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    /// 文脈依存の語（`fails` など）か．字句では LOWER_NAME なので，テキストで見分ける．
    pub(crate) fn at_contextual_kw(&self, kw: &str) -> bool {
        self.at(LOWER_NAME) && self.tokens[self.pos].1 == kw
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.at(EOF)
    }

    pub(crate) fn bump(&mut self) {
        assert!(!self.at_eof(), "EOF は読み進められない");
        self.pos += 1;
        self.fuel.set(FUEL);
        self.events.push(Event::Token);
    }

    /// `kind` にいることを確かめてから読み進める．文法の誤りなので，違えば panic する．
    pub(crate) fn bump_kind(&mut self, kind: SyntaxKind) {
        assert!(self.at(kind), "{kind:?} にいるはずが {:?}", self.current());
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

    /// 診断を出し，次のトークンが `recovery` になければ1つ ERROR に包んで読み飛ばす．
    pub(crate) fn err_recover(
        &mut self,
        code: DiagnosticCode,
        message: impl Into<String>,
        recovery: &[SyntaxKind],
    ) {
        self.error(code, message);
        if self.at_eof() || recovery.contains(&self.current()) {
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

/// 開いたままのノード．`complete` を呼ばずに捨てると Tombstone のまま残り，木には出ない．
pub(crate) struct Marker {
    pos: u32,
}

impl Marker {
    /// ノードを作らずに捨てる．開始は Tombstone のまま残り，木には出ない．
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
    /// 閉じたノードを新しい親で包む．束縛 `パターン = 式` や二項演算で使う．
    pub(crate) fn precede(self, p: &mut Parser<'_>) -> Marker {
        let parent = p.start();
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(parent.pos - self.pos),
            _ => unreachable!("閉じたノードの開始は Start のはず"),
        }
        parent
    }
}
