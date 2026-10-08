//! イベント方式のパーサ．
//!
//! パーサは木を直接組まず，`Event` を積むだけにする．トリビアはパーサから見えず，
//! 木を組むときに `sink` が差し込む．

mod sink;

use std::cell::Cell;

use crate::SyntaxKind::{self, EOF};

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
    Error(String),
    /// まだ種類が決まっていない，または捨てたノードの開始．
    Tombstone,
}

/// 進まないまま同じ位置を見てよい回数．超えたらパーサの不具合なので panic する．
const FUEL: u32 = 256;

pub(crate) struct Parser {
    kinds: Vec<SyntaxKind>,
    pos: usize,
    events: Vec<Event>,
    fuel: Cell<u32>,
}

impl Parser {
    pub(crate) fn new(kinds: impl IntoIterator<Item = SyntaxKind>) -> Self {
        let kinds = kinds.into_iter().filter(|k| !k.is_trivia()).collect();
        Parser {
            kinds,
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
        self.kinds.get(self.pos + n).copied().unwrap_or(EOF)
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
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
        self.error(format!("{kind:?} がありません"));
        false
    }

    pub(crate) fn error(&mut self, message: impl Into<String>) {
        self.events.push(Event::Error(message.into()));
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
    pub(crate) fn complete(self, p: &mut Parser, kind: SyntaxKind) -> CompletedMarker {
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
    pub(crate) fn precede(self, p: &mut Parser) -> Marker {
        let parent = p.start();
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(parent.pos - self.pos),
            _ => unreachable!("閉じたノードの開始は Start のはず"),
        }
        parent
    }
}
