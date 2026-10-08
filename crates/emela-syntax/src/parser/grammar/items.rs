//! 宣言（17.3）．この段は `fn` だけ．

use super::expressions::block;
use super::param_list;
use super::types::{effects_and_errors, path, ret_type};
use crate::SyntaxKind::*;
use crate::diagnostic::DiagnosticCode;
use crate::parser::{Marker, Parser};

/// `{ decl NL }`．宣言の後ろに改行がなければ診断を出し，次の宣言まで読み飛ばす．
pub(crate) fn source_file(p: &mut Parser<'_>) {
    let root = p.start();
    loop {
        while p.eat(NEWLINE) {}
        if p.at_eof() {
            break;
        }
        decl(p);
        if !p.at(NEWLINE) && !p.at_eof() && !p.at_line_start() {
            p.error(
                DiagnosticCode::ExpectedStatementEnd,
                "expected newline after declaration",
            );
            skip_to_decl(p);
        }
    }
    root.complete(p, ROOT);
}

/// `[ "pub" ] item`
fn decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.eat(PUB_KW);
    match p.current() {
        FN_KW | SUSPEND_KW => fn_decl(p, m),
        TYPE_KW | ENUM_KW | ERROR_KW | CONST_KW | EFFECT_KW | HANDLER_KW | LAYER_KW | TRAIT_KW
        | IMPL_KW | IMPORT_KW | OPAQUE_KW | AT => {
            // 次の段で読む．それまでは診断を出して宣言ごと読み飛ばす．
            let message = format!(
                "{} is not supported by the parser yet",
                p.current().describe()
            );
            p.error(DiagnosticCode::ExpectedDeclaration, message);
            skip_to_decl(p);
            m.complete(p, ERROR);
        }
        _ => {
            p.error(DiagnosticCode::ExpectedDeclaration, "expected declaration");
            skip_to_decl(p);
            m.complete(p, ERROR);
        }
    }
}

/// 少なくとも1トークン読み，次の行頭の宣言か入力の終わりの手前で止まる．
fn skip_to_decl(p: &mut Parser<'_>) {
    let m = p.start();
    while !p.at_eof() {
        p.bump();
        if p.at_decl_start() {
            break;
        }
    }
    m.complete(p, ERROR);
}

/// `[ "suspend" ] "fn" lower_name [ type_params ] "(" [ params ] ")" [ signature ] [ block ]`
///
/// `signature = [ "->" type [ "fails" error_set ] ] [ "use" effect_set ]`
fn fn_decl(p: &mut Parser<'_>, m: Marker) {
    p.eat(SUSPEND_KW);
    p.expect(FN_KW);
    p.expect(LOWER_NAME);
    if p.at(L_BRACK) {
        type_param_list(p);
    }
    if p.at(L_PAREN) {
        param_list(p);
    } else {
        p.expect(L_PAREN);
    }
    if p.at(THIN_ARROW) {
        ret_type(p);
    } else if p.at_contextual_kw("fails") {
        p.error(
            DiagnosticCode::ExpectedToken,
            "expected `->` before `fails`",
        );
    }
    effects_and_errors(p);
    if p.at(L_BRACE) {
        block(p);
    }
    m.complete(p, FN_DECL);
}

/// `"[" type_param { "," type_param } "]"`．`type_param = upper_name [ ":" bound { "+" bound } ]`
fn type_param_list(p: &mut Parser<'_>) {
    let m = p.start();
    super::delimited(p, L_BRACK, R_BRACK, |p| {
        let param = p.start();
        p.expect(UPPER_NAME);
        if p.eat(COLON) {
            loop {
                if p.at(TYPE_NAME) {
                    path(p);
                } else {
                    p.expect(TYPE_NAME);
                }
                if !p.eat(PLUS) {
                    break;
                }
            }
        }
        param.complete(p, TYPE_PARAM);
    });
    m.complete(p, TYPE_PARAM_LIST);
}
