//! 型（17.4）．

use super::delimited;
use crate::SyntaxKind::*;
use crate::diagnostic::DiagnosticCode;
use crate::parser::{CompletedMarker, Parser};

/// 型が読めなかったとき，次のトークンが続きの一部なら消費せずに止まる．
const RECOVERY: &[crate::SyntaxKind] = &[
    R_PAREN, R_BRACK, L_BRACE, R_BRACE, COMMA, EQ, THIN_ARROW, PIPE, NEWLINE,
];

pub(crate) fn type_(p: &mut Parser<'_>) {
    match p.current() {
        TYPE_NAME => path_type(p),
        UPPER_NAME => {
            type_var(p);
        }
        SELF_TYPE_KW => {
            let m = p.start();
            p.bump();
            m.complete(p, SELF_TYPE);
        }
        L_PAREN => paren_type(p),
        FN_KW => fn_type(p),
        _ => p.err_recover(DiagnosticCode::ExpectedType, "expected type", RECOVERY),
    }
}

/// `type_ref [ "[" type { "," type } "]" ]`
fn path_type(p: &mut Parser<'_>) {
    let m = p.start();
    path(p);
    if p.at(L_BRACK) {
        let args = p.start();
        delimited(p, L_BRACK, R_BRACK, type_);
        args.complete(p, TYPE_ARG_LIST);
    }
    m.complete(p, PATH_TYPE);
}

/// `type_ref = [ module_path "." ] type_name`．呼ぶ側が TYPE_NAME にいることを確かめる．
pub(crate) fn path(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump_kind(TYPE_NAME);
    while p.at(DOT) && p.nth(1) == TYPE_NAME {
        p.bump();
        p.bump();
    }
    m.complete(p, PATH)
}

fn type_var(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump_kind(UPPER_NAME);
    m.complete(p, TYPE_VAR)
}

/// `()` か，要素が2つ以上のタプル．要素が1つなら閉じ括弧に診断を出してタプルとして読む．
fn paren_type(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(L_PAREN);
    let mut count = 0;
    while !p.at(R_PAREN) && !p.at_eof() {
        count += 1;
        type_(p);
        if !p.eat(COMMA) {
            break;
        }
    }
    if count == 1 {
        p.error(
            DiagnosticCode::SingleElementTuple,
            "a tuple type needs at least two elements",
        );
    }
    p.expect(R_PAREN);
    m.complete(p, if count == 0 { UNIT_TYPE } else { TUPLE_TYPE });
}

/// `"fn" "(" [ type { "," type } ] ")" "->" type [ "fails" error_set ] [ "use" effect_set ]`
fn fn_type(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(FN_KW);
    let has_params = p.at(L_PAREN);
    if has_params {
        let params = p.start();
        delimited(p, L_PAREN, R_PAREN, type_);
        params.complete(p, PARAM_TYPE_LIST);
    } else {
        p.expect(L_PAREN);
    }
    // 引数の並びがないときは，`->` の診断を重ねて出さない．
    if p.at(THIN_ARROW) {
        ret_type(p);
    } else if has_params {
        p.expect(THIN_ARROW);
    }
    effects_and_errors(p);
    m.complete(p, FN_TYPE);
}

/// `"->" type`
pub(crate) fn ret_type(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(THIN_ARROW);
    type_(p);
    m.complete(p, RET_TYPE);
}

/// `[ "fails" error_set ] [ "use" effect_set ]`
pub(crate) fn effects_and_errors(p: &mut Parser<'_>) {
    if p.at_contextual_kw("fails") {
        let m = p.start();
        p.bump();
        error_set(p);
        m.complete(p, FAILS_CLAUSE);
    }
    if p.at(USE_KW) {
        let m = p.start();
        p.bump();
        effect_set(p);
        m.complete(p, USE_CLAUSE);
    }
}

/// `"{" "}" | type_ref { "|" type_ref } | upper_name`
fn error_set(p: &mut Parser<'_>) {
    let m = p.start();
    match p.current() {
        L_BRACE => {
            p.bump();
            p.expect(R_BRACE);
        }
        TYPE_NAME => {
            path(p);
            while p.eat(PIPE) {
                set_element(p);
            }
        }
        _ => set_element(p),
    }
    m.complete(p, ERROR_SET);
}

/// `"{" [ type_ref { "," type_ref } ] "}" | type_ref | upper_name`
///
/// 波括弧の中の改行は区切りとして字句に残るので，ここで読み飛ばす．
fn effect_set(p: &mut Parser<'_>) {
    let m = p.start();
    if p.at(L_BRACE) {
        p.bump();
        while p.eat(NEWLINE) {}
        while !p.at(R_BRACE) && !p.at_eof() {
            set_element(p);
            while p.eat(NEWLINE) {}
            if !p.eat(COMMA) {
                break;
            }
            while p.eat(NEWLINE) {}
        }
        p.expect(R_BRACE);
    } else {
        set_element(p);
    }
    m.complete(p, EFFECT_SET);
}

/// 集合の要素．type_ref か大文字名．
fn set_element(p: &mut Parser<'_>) {
    match p.current() {
        TYPE_NAME => {
            path(p);
        }
        UPPER_NAME => {
            type_var(p);
        }
        _ => p.err_recover(DiagnosticCode::ExpectedType, "expected type", RECOVERY),
    }
}
