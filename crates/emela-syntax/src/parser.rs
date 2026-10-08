//! イベント方式のパーサ．
//!
//! パーサは木を直接組まず，`Event` を積むだけにする．トリビアはパーサから見えず，
//! 木を組むときに `sink` が差し込む．

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
    /// トリビアを除いたトークン．テキストは文脈依存の語の判定に使う．
    tokens: Vec<ParserToken<'t>>,
    pos: usize,
    events: Vec<Event>,
    fuel: Cell<u32>,
}

struct ParserToken<'t> {
    kind: SyntaxKind,
    text: &'t str,
    /// 直前のトークンとの間に改行がある．継続の改行（トリビア）も数える．
    line_start: bool,
    /// 直前のトークンとの間にドキュメントコメント `##` がある．
    doc_before: bool,
}

/// 宣言を始めるトークン．エラー回復で，行頭にあれば読み飛ばさずに止まる．
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

    /// 読んだトークンの数．文法の関数が進んだかを確かめるのに使う．
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }

    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        let fuel = self.fuel.get();
        assert!(fuel > 0, "パーサが位置 {} から進んでいない", self.pos);
        self.fuel.set(fuel - 1);
        self.tokens.get(self.pos + n).map_or(EOF, |t| t.kind)
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    /// 文脈依存の語（`fails` など）か．字句では LOWER_NAME なので，テキストで見分ける．
    pub(crate) fn at_contextual_kw(&self, kw: &str) -> bool {
        self.at(LOWER_NAME) && self.tokens[self.pos].text == kw
    }

    /// 行頭の宣言の始まりにいるか．`fn(` はラムダなので除く．
    ///
    /// 閉じていない `(` の中では改行が継続になり NEWLINE が来ないので，壊れた宣言から
    /// 立ち直るにはこれで次の宣言を見つける．
    pub(crate) fn at_decl_start(&self) -> bool {
        let kind = self.current();
        let line_start = self.tokens.get(self.pos).is_some_and(|t| t.line_start);
        line_start && DECL_START.contains(&kind) && !(kind == FN_KW && self.nth(1) == L_PAREN)
    }

    /// 直前にドキュメントコメントがあるか．
    pub(crate) fn at_doc_comment(&self) -> bool {
        self.tokens.get(self.pos).is_some_and(|t| t.doc_before)
    }

    /// `n` 個先が文脈依存の語 `kw` か．
    pub(crate) fn nth_at_contextual_kw(&self, n: usize, kw: &str) -> bool {
        self.nth(n) == LOWER_NAME && self.tokens[self.pos + n].text == kw
    }

    /// 行頭の，`fn` と `suspend` 以外の宣言の始まりにいるか．宣言の本体の中の項目は
    /// 行頭の `fn` で始まるので，本体の中ではこちらを回復の同期点にする．
    pub(crate) fn at_top_decl_start(&self) -> bool {
        self.at_decl_start() && !matches!(self.current(), FN_KW | SUSPEND_KW)
    }

    /// 行頭にいるか．
    pub(crate) fn at_line_start(&self) -> bool {
        self.tokens.get(self.pos).is_some_and(|t| t.line_start)
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
