//! 宣言（17.3）．この段は `fn` だけ．

use super::expressions::{block, expr};
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

/// `{ annotation NL } [ "pub" [ "opaque" ] ] item | impl_decl`
fn decl(p: &mut Parser<'_>, m: Marker) {
    let mut annotated = false;
    while p.at(AT) {
        annotated = true;
        let named = annotation(p);
        if named && !p.at_eof() && !p.eat(NEWLINE) && !p.at_line_start() {
            p.error(
                DiagnosticCode::ExpectedToken,
                "expected newline after annotation",
            );
        }
        while p.eat(NEWLINE) {}
    }
    let public = p.eat(PUB_KW);
    if p.at(OPAQUE_KW) {
        if !public {
            p.error(
                DiagnosticCode::MisplacedOpaque,
                "`opaque` must follow `pub`",
            );
        } else if !matches!(p.nth(1), TYPE_KW | ENUM_KW) {
            p.error(
                DiagnosticCode::MisplacedOpaque,
                "`opaque` can only be used on `type` and `enum`",
            );
        }
        p.bump();
    }
    if p.at(IMPL_KW) && (public || annotated) {
        p.error(
            DiagnosticCode::ModifierOnImpl,
            "`impl` cannot have `pub` or annotations",
        );
    }
    match p.current() {
        FN_KW | SUSPEND_KW => fn_decl(p, m),
        TYPE_KW => type_decl(p, m),
        ERROR_KW => error_decl(p, m),
        ENUM_KW => enum_decl(p, m),
        CONST_KW => const_decl(p, m),
        EFFECT_KW => effect_decl(p, m),
        HANDLER_KW => handler_decl(p, m),
        LAYER_KW => layer_decl(p, m),
        TRAIT_KW => trait_decl(p, m),
        IMPL_KW => impl_decl(p, m),
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
    if matches!(p.current(), INT | FLOAT | STRING_QUOTE | TRIPLE_QUOTE) {
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
    decl_name(p, LOWER_NAME);
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

/// `"[" type_param { "," type_param } "]"`．`type_param = ( upper_name | type_name ) [ ":" bound { "+" bound } ]`
fn type_param_list(p: &mut Parser<'_>) {
    let m = p.start();
    super::delimited(p, L_BRACK, R_BRACK, |p| {
        let param = p.start();
        // 型引数は大文字名か型名（2.3）．
        if !p.eat(UPPER_NAME) && !p.eat(TYPE_NAME) {
            p.err_recover(
                DiagnosticCode::ExpectedToken,
                "expected type parameter name",
                &[COLON, COMMA, R_BRACK, NEWLINE],
            );
        }
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
///
/// 間のコメントだけの行と空行は飛ばす（2.5）．間にドキュメントコメントがあれば，
/// 宣言と同じくその位置から開き，sink がコメントを DERIVE_CLAUSE の中に入れる．
fn derive_clause(p: &mut Parser<'_>) {
    let mut newlines = 0;
    while p.nth(newlines) == NEWLINE {
        newlines += 1;
    }
    if !p.nth_at_contextual_kw(newlines, "derive") {
        return;
    }
    let m = newlines_with_doc(p).unwrap_or_else(|| p.start());
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

/// `"const" upper_name [ ":" type ] "=" expr`
fn const_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(CONST_KW);
    decl_name(p, UPPER_NAME);
    if p.eat(COLON) {
        type_(p);
    }
    if p.expect(EQ) || !matches!(p.current(), NEWLINE | EOF) {
        expr(p);
    }
    m.complete(p, CONST_DECL);
}

/// `"effect" type_name "{" { op_sig NL } "}"`
fn effect_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(EFFECT_KW);
    decl_name(p, TYPE_NAME);
    item_list(p, |p, m| {
        if !matches!(p.current(), FN_KW | SUSPEND_KW) {
            item_error(p, m, "expected operation (`fn`)");
            return;
        }
        op_sig(p, m);
    });
    m.complete(p, EFFECT_DECL);
}

/// `[ "suspend" ] "fn" lower_name "(" [ params ] ")" "->" type [ "fails" error_set ]`
fn op_sig(p: &mut Parser<'_>, m: Marker) {
    p.eat(SUSPEND_KW);
    p.expect(FN_KW);
    decl_name(p, LOWER_NAME);
    if p.at(L_PAREN) {
        param_list(p);
    } else {
        p.expect(L_PAREN);
    }
    if p.at(THIN_ARROW) {
        ret_type(p);
    } else {
        p.expect(THIN_ARROW);
    }
    effects_and_errors(p);
    m.complete(p, OP_SIG);
}

/// `"handler" type_name [ "(" [ fields ] ")" ] "implements" type_ref "{" { handler_item NL } "}"`
///
/// `handler_item = "init" block | "release" block
///               | [ "suspend" ] "fn" lower_name "(" [ lower_name { "," lower_name } ] ")" block`
fn handler_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(HANDLER_KW);
    decl_name(p, TYPE_NAME);
    if p.at(L_PAREN) {
        field_list(p);
    }
    // `implements` がなくても型名が続いていれば，それを対象として読む．
    if p.at_contextual_kw("implements") || p.at(TYPE_NAME) {
        let c = p.start();
        if !p.at_contextual_kw("implements") {
            p.error(DiagnosticCode::ExpectedToken, "expected `implements`");
        } else {
            p.bump();
        }
        if p.at(TYPE_NAME) {
            path(p);
        } else {
            p.expect(TYPE_NAME);
        }
        c.complete(p, IMPLEMENTS_CLAUSE);
    } else {
        p.error(DiagnosticCode::ExpectedToken, "expected `implements`");
    }
    item_list(p, |p, m| {
        let kind = if p.at_contextual_kw("init") && p.nth(1) == L_BRACE {
            HANDLER_INIT
        } else if p.at_contextual_kw("release") && p.nth(1) == L_BRACE {
            HANDLER_RELEASE
        } else if matches!(p.current(), FN_KW | SUSPEND_KW) {
            fn_decl(p, m);
            return;
        } else {
            item_error(p, m, "expected `init`, `release`, or operation (`fn`)");
            return;
        };
        p.bump();
        block(p);
        m.complete(p, kind);
    });
    m.complete(p, HANDLER_DECL);
}

/// `"layer" type_name "{" type_ref { sep type_ref } "}"`
fn layer_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(LAYER_KW);
    decl_name(p, TYPE_NAME);
    let list = p.start();
    if p.expect(L_BRACE) {
        separated_items(p, "handlers", |p, m| {
            m.abandon(p);
            if p.at(TYPE_NAME) {
                path(p);
            } else {
                p.err_recover(
                    DiagnosticCode::ExpectedToken,
                    "expected handler name",
                    &[COMMA, R_BRACE, NEWLINE],
                );
            }
        });
        p.expect(R_BRACE);
    }
    list.complete(p, ITEM_LIST);
    m.complete(p, LAYER_DECL);
}

/// `"trait" type_name [ ":" bound { "+" bound } ] "{" { trait_item NL } "}"`
///
/// `trait_item = "fn" lower_name "(" [ params ] ")" [ signature ] [ block ] | derive_rule`
fn trait_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(TRAIT_KW);
    decl_name(p, TYPE_NAME);
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
    item_list(p, |p, m| {
        if p.at(FN_KW) {
            fn_decl(p, m);
        } else if p.at_contextual_kw("derive") {
            // 導出規則は構文が未確定（18.1 #3）．診断を出して丸ごと読み飛ばす．
            p.error(
                DiagnosticCode::DeriveRuleUnsupported,
                "derive rules in traits are not supported yet",
            );
            skip_braced_line(p);
            m.complete(p, ERROR);
        } else {
            item_error(p, m, "expected method (`fn`)");
        }
    });
    m.complete(p, TRAIT_DECL);
}

/// `"impl" type_ref "for" type "{" { impl_item NL } "}"`．`impl_item = "fn" lower_name "(" [ params ] ")" block`
fn impl_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(IMPL_KW);
    if p.at(TYPE_NAME) {
        path(p);
    } else {
        p.expect(TYPE_NAME);
    }
    if p.at_contextual_kw("for") {
        p.bump();
        type_(p);
    } else {
        p.error(DiagnosticCode::ExpectedToken, "expected `for`");
        // `for` がなくても型が続いていれば型として読む．
        if at_type_start(p) {
            type_(p);
        }
    }
    item_list(p, |p, m| {
        if p.at(FN_KW) {
            fn_decl(p, m);
        } else {
            item_error(p, m, "expected method (`fn`)");
        }
    });
    m.complete(p, IMPL_DECL);
}

/// 宣言の本体 `"{" { item NL } "}"`．項目は `item` が読み，渡したマーカーで閉じる．
fn item_list(p: &mut Parser<'_>, mut item: impl FnMut(&mut Parser<'_>, Marker)) {
    let list = p.start();
    if !p.expect(L_BRACE) {
        list.complete(p, ITEM_LIST);
        return;
    }
    loop {
        let doc = newlines_with_doc(p);
        if p.at(R_BRACE) || p.at_eof() || p.at_top_decl_start() {
            if let Some(doc) = doc {
                doc.abandon(p);
            }
            break;
        }
        let m = doc.unwrap_or_else(|| p.start());
        let before = p.position();
        item(p, m);
        if p.position() == before {
            let e = p.start();
            p.bump();
            e.complete(p, ERROR);
            continue;
        }
        if !matches!(p.current(), NEWLINE | R_BRACE | EOF) && !p.at_line_start() {
            p.error(
                DiagnosticCode::ExpectedStatementEnd,
                "expected newline or `}` after item",
            );
            let e = p.start();
            while !matches!(p.current(), NEWLINE | R_BRACE | EOF) && !p.at_decl_start() {
                p.bump();
            }
            e.complete(p, ERROR);
        }
    }
    p.expect(R_BRACE);
    list.complete(p, ITEM_LIST);
}

/// 改行か，対応の取れた外側の `}` の手前まで読む．中の `{ }` は丸ごと読む．
fn skip_braced_line(p: &mut Parser<'_>) {
    let mut depth = 0usize;
    while !p.at_eof() {
        match p.current() {
            NEWLINE | R_BRACE if depth == 0 => break,
            L_BRACE => depth += 1,
            R_BRACE => depth -= 1,
            _ => {}
        }
        p.bump();
    }
}

/// 本体の中の読めない項目．診断を出し，行末か `}` の手前までを ERROR に包む．
fn item_error(p: &mut Parser<'_>, m: Marker, message: &str) {
    p.error(DiagnosticCode::ExpectedToken, message);
    while !matches!(p.current(), NEWLINE | R_BRACE | EOF) && !p.at_top_decl_start() {
        p.bump();
    }
    m.complete(p, ERROR);
}
