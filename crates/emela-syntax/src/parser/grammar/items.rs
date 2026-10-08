//! 宣言（17.3）．この段は `fn` だけ．

use super::expressions::block;
use super::param_list;
use super::types::{effects_and_errors, path, ret_type, type_};
use crate::SyntaxKind::*;
use crate::diagnostic::DiagnosticCode;
use crate::parser::{Marker, Parser};

/// `{ import NL } { decl NL }`．宣言の後ろに改行がなければ診断を出し，次の宣言まで読み飛ばす．
pub(crate) fn source_file(p: &mut Parser<'_>) {
    let root = p.start();
    let mut seen_decl = false;
    loop {
        let doc = newlines_with_doc(p);
        if p.at_eof() {
            if let Some(doc) = doc {
                doc.abandon(p);
            }
            break;
        }
        let m = doc.unwrap_or_else(|| p.start());
        if p.at(IMPORT_KW) {
            if seen_decl {
                p.error(
                    DiagnosticCode::ImportAfterDeclaration,
                    "imports must come before declarations",
                );
            }
            import(p, m);
        } else {
            seen_decl = true;
            decl(p, m);
        }
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

/// 区切りの改行を読む．ドキュメントコメントが付いていれば，その位置から開いたマーカーを返す．
/// 宣言はこのマーカーで開くので，sink がコメントを宣言のノードの中に入れる．
pub(crate) fn newlines_with_doc(p: &mut Parser<'_>) -> Option<Marker> {
    let mut doc = None;
    while p.at(NEWLINE) {
        if doc.is_none() && p.at_doc_comment() {
            doc = Some(p.start());
        }
        p.bump();
    }
    doc
}

/// `"import" module_path [ "." "{" import_item { "," import_item } "}" ]`
fn import(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(IMPORT_KW);
    if p.at(TYPE_NAME) {
        path(p);
    } else {
        p.err_recover(
            DiagnosticCode::ExpectedToken,
            "expected module path",
            &[NEWLINE],
        );
    }
    if p.at(DOT) && p.nth(1) == L_BRACE {
        let list = p.start();
        p.bump();
        super::delimited(p, L_BRACE, R_BRACE, |p| {
            if matches!(p.current(), LOWER_NAME | TYPE_NAME | UPPER_NAME) {
                p.bump();
            } else {
                p.err_recover(
                    DiagnosticCode::ExpectedToken,
                    "expected name to import",
                    &[COMMA, R_BRACE, NEWLINE],
                );
            }
        });
        list.complete(p, IMPORT_LIST);
    }
    m.complete(p, IMPORT);
}

/// `{ annotation NL } [ "pub" [ "opaque" ] ] item`
fn decl(p: &mut Parser<'_>, m: Marker) {
    while p.at(AT) {
        let named = annotation(p);
        if named && !p.at_eof() && !p.eat(NEWLINE) && !p.at_line_start() {
            p.error(
                DiagnosticCode::ExpectedToken,
                "expected newline after annotation",
            );
        }
        while p.eat(NEWLINE) {}
    }
    p.eat(PUB_KW);
    p.eat(OPAQUE_KW);
    match p.current() {
        FN_KW | SUSPEND_KW => fn_decl(p, m),
        TYPE_KW => type_decl(p, m),
        ERROR_KW => error_decl(p, m),
        ENUM_KW => enum_decl(p, m),
        CONST_KW | EFFECT_KW | HANDLER_KW | LAYER_KW | TRAIT_KW | IMPL_KW => {
            // 次のステップで読む．それまでは診断を出して宣言ごと読み飛ばす．
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
            if !p.at_eof() {
                skip_to_decl(p);
            }
            m.complete(p, ERROR);
        }
    }
}

/// `"@" lower_name [ "(" annot_arg { "," annot_arg } ")" ]`
///
/// `annot_arg = lower_name | literal | lower_name ":" literal`
///
/// 名前があれば `true` を返す．名前がないときは呼ぶ側で診断を重ねない．
pub(crate) fn annotation(p: &mut Parser<'_>) -> bool {
    let m = p.start();
    p.bump_kind(AT);
    let named = p.expect(LOWER_NAME);
    if p.at(L_PAREN) {
        let args = p.start();
        super::delimited(p, L_PAREN, R_PAREN, |p| {
            let arg = p.start();
            if p.at(LOWER_NAME) {
                p.bump();
                if p.eat(COLON) {
                    annot_literal(p);
                }
            } else {
                annot_literal(p);
            }
            arg.complete(p, ANNOT_ARG);
        });
        args.complete(p, ANNOT_ARG_LIST);
    }
    m.complete(p, ANNOTATION);
    named
}

fn annot_literal(p: &mut Parser<'_>) {
    if matches!(p.current(), INT | FLOAT | STRING_QUOTE) {
        super::literal(p, |p| {
            super::expressions::expr(p);
        });
    } else {
        p.err_recover(
            DiagnosticCode::ExpectedToken,
            "expected name or literal",
            &[COMMA, R_PAREN, NEWLINE],
        );
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

/// `"type" type_name [ type_params ] [ "(" [ fields ] ")" ] [ derive ]`
fn type_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(TYPE_KW);
    decl_name(p, TYPE_NAME);
    if p.at(L_BRACK) {
        type_param_list(p);
    }
    if p.at(L_PAREN) {
        field_list(p);
    }
    derive_clause(p);
    m.complete(p, TYPE_DECL);
}

/// `"error" type_name [ "(" [ fields ] ")" ]`
fn error_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(ERROR_KW);
    decl_name(p, TYPE_NAME);
    if p.at(L_PAREN) {
        field_list(p);
    }
    m.complete(p, ERROR_DECL);
}

/// `"enum" type_name [ type_params ] "{" variant { sep variant } "}" [ derive ]`．`sep = "," | NL`
fn enum_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(ENUM_KW);
    decl_name(p, TYPE_NAME);
    if p.at(L_BRACK) {
        type_param_list(p);
    }
    if p.at(L_BRACE) {
        variant_list(p);
    } else {
        p.expect(L_BRACE);
    }
    derive_clause(p);
    m.complete(p, ENUM_DECL);
}

fn variant_list(p: &mut Parser<'_>) {
    let list = p.start();
    p.bump_kind(L_BRACE);
    separated_items(p, "variants", |p, m| {
        if matches!(p.current(), LOWER_NAME | UPPER_NAME) {
            // 名前のクラスだけが違う．診断を出してバリアントとして読む．
            p.error(
                DiagnosticCode::ExpectedToken,
                "expected variant name (a type name)",
            );
            p.bump();
            if p.at(L_PAREN) {
                variant_fields(p);
            }
            m.complete(p, VARIANT);
            return;
        }
        if !p.at(TYPE_NAME) {
            m.abandon(p);
            p.err_recover(
                DiagnosticCode::ExpectedToken,
                "expected variant name",
                &[COMMA, R_BRACE, NEWLINE],
            );
            return;
        }
        p.bump();
        if p.at(L_PAREN) {
            variant_fields(p);
        }
        m.complete(p, VARIANT);
    });
    p.expect(R_BRACE);
    list.complete(p, VARIANT_LIST);
}

/// `{` の直後から `}` の手前まで，カンマか改行で区切られた要素を読む．
///
/// 要素は `item` が読む．渡すマーカーはドキュメントコメントの位置から開いてあるので，
/// 要素のノードはこれで閉じる．`}` は読まない．
pub(crate) fn separated_items(
    p: &mut Parser<'_>,
    what: &str,
    mut item: impl FnMut(&mut Parser<'_>, Marker),
) {
    loop {
        let doc = newlines_with_doc(p);
        if p.at(R_BRACE) || p.at_eof() || p.at_decl_start() {
            if let Some(doc) = doc {
                doc.abandon(p);
            }
            break;
        }
        let m = doc.unwrap_or_else(|| p.start());
        let before = p.position();
        item(p, m);
        if p.position() == before {
            // 1トークンも読めなかった．そのトークンを読み飛ばして進める．
            let e = p.start();
            p.bump();
            e.complete(p, ERROR);
            continue;
        }
        if p.eat(COMMA) || matches!(p.current(), NEWLINE | R_BRACE | EOF) || p.at_line_start() {
            continue;
        }
        p.error(
            DiagnosticCode::ExpectedToken,
            format!("expected `,` or newline between {what}"),
        );
        let e = p.start();
        while !matches!(p.current(), NEWLINE | R_BRACE | EOF) && !p.at_decl_start() {
            p.bump();
        }
        e.complete(p, ERROR);
    }
}

/// バリアントの中身．名前付きなら FIELD_LIST，名前なしなら TUPLE_FIELD_LIST．混ぜたら E0128．
fn variant_fields(p: &mut Parser<'_>) {
    let m = p.start();
    let mut named = None;
    super::delimited(p, L_PAREN, R_PAREN, |p| {
        let is_named = p.at(AT) || (p.at(LOWER_NAME) && p.nth(1) == COLON);
        if *named.get_or_insert(is_named) != is_named {
            p.error(
                DiagnosticCode::MixedVariantFields,
                "a variant cannot mix named and positional fields",
            );
        }
        if is_named {
            field(p);
        } else {
            type_(p);
        }
    });
    m.complete(
        p,
        if named == Some(true) {
            FIELD_LIST
        } else {
            TUPLE_FIELD_LIST
        },
    );
}

/// `"(" [ field { "," field } ] ")"`
fn field_list(p: &mut Parser<'_>) {
    let m = p.start();
    super::delimited(p, L_PAREN, R_PAREN, field);
    m.complete(p, FIELD_LIST);
}

/// `{ annotation } lower_name ":" type`
fn field(p: &mut Parser<'_>) {
    let m = p.start();
    while p.at(AT) {
        annotation(p);
    }
    if !p.at(LOWER_NAME) {
        p.err_recover(
            DiagnosticCode::ExpectedToken,
            "expected field name",
            &[COMMA, R_PAREN, NEWLINE],
        );
        m.complete(p, FIELD);
        return;
    }
    p.bump();
    // `:` がなくても型が続いていれば型として読む．
    if p.expect(COLON) || at_type_start(p) {
        type_(p);
    }
    m.complete(p, FIELD);
}

/// `"derive" type_ref { "," type_ref }`．次の行に書いてもよい．
fn derive_clause(p: &mut Parser<'_>) {
    if p.at(NEWLINE) && p.nth_at_contextual_kw(1, "derive") {
        p.bump();
    }
    if !p.at_contextual_kw("derive") {
        return;
    }
    let m = p.start();
    p.bump();
    loop {
        if p.at(TYPE_NAME) {
            path(p);
        } else {
            p.expect(TYPE_NAME);
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    m.complete(p, DERIVE_CLAUSE);
}

/// 宣言の名前．違うトークンなら診断を出し，宣言の続きでなければ ERROR に包んで読み飛ばす．
fn decl_name(p: &mut Parser<'_>, kind: crate::SyntaxKind) {
    if p.eat(kind) {
        return;
    }
    p.err_recover(
        DiagnosticCode::ExpectedToken,
        format!("expected {}", kind.describe()),
        &[L_PAREN, L_BRACK, L_BRACE, NEWLINE],
    );
}

fn at_type_start(p: &Parser<'_>) -> bool {
    matches!(
        p.current(),
        TYPE_NAME | UPPER_NAME | SELF_TYPE_KW | L_PAREN | FN_KW
    )
}
