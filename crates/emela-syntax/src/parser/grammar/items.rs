//! Declarations (spec 17.3).

use super::expressions::{block, expr};
use super::param_list;
use super::types::{effect_type_args, effects_and_errors, path, ret_type, type_};
use crate::SyntaxKind::*;
use crate::diagnostic::DiagnosticCode;
use crate::parser::{Marker, Parser};

/// `{ import NL } { decl NL }`. A declaration not followed by a newline gets a diagnostic, and
/// the parser skips to the next declaration.
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

/// Reads separating newlines. If a doc comment is attached, returns a marker opened at its
/// position. The declaration opens with this marker, so `sink` puts the comment inside the
/// declaration node.
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
/// Returns `true` if there is a name. Without one, the caller should not add more diagnostics.
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

/// Reads at least one token and stops before the next declaration at the start of a line, or
/// before the end of the input.
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
        // A type parameter is an uppercase name or a type name (spec 2.3).
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

/// `"error" type_name [ "(" [ fields ] ")" ] [ derive ]`
fn error_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(ERROR_KW);
    decl_name(p, TYPE_NAME);
    if p.at(L_PAREN) {
        field_list(p);
    }
    derive_clause(p);
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
            // Only the name class differs. Report it and read it as a variant.
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

/// Reads elements separated by commas or newlines, from right after `{` up to before `}`.
///
/// `item` reads each element. The marker it gets is opened at the doc comment's position, so the
/// element node is completed with it. Does not consume `}`.
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
            // Not even one token was read. Skip that token to make progress.
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

/// FIELD_LIST if named, TUPLE_FIELD_LIST if unnamed; mixing them is E0128.
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
    // Read a type even without `:` if one follows.
    if p.expect(COLON) || at_type_start(p) {
        type_(p);
    }
    m.complete(p, FIELD);
}

/// `"derive" type_ref { "," type_ref }`. May be written on the next line.
///
/// Comment-only lines and blank lines in between are skipped (spec 2.5). A doc comment in between
/// opens the node at its position, as for declarations, so `sink` puts it inside DERIVE_CLAUSE.
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

/// On a different token, reports a diagnostic and, unless the declaration continues, wraps it in
/// ERROR and skips it.
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

/// `"effect" type_name [ type_params ] "{" { op_sig NL } "}"`, or a label
/// `"effect" type_name "of" type_ref [ "[" type { "," type } "]" ] [ "default" ]`.
fn effect_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(EFFECT_KW);
    decl_name(p, TYPE_NAME);
    if p.at_contextual_kw("of") {
        let c = p.start();
        p.bump();
        if p.at(TYPE_NAME) {
            path(p);
            effect_type_args(p);
        } else {
            p.expect(TYPE_NAME);
        }
        if p.at_contextual_kw("default") {
            p.bump();
        }
        c.complete(p, EFFECT_LABEL);
        m.complete(p, EFFECT_DECL);
        return;
    }
    if p.at(L_BRACK) {
        type_param_list(p);
    }
    item_list(p, |p, m| {
        if !matches!(p.current(), FN_KW | SUSPEND_KW) {
            item_error(p, m, "expected operation (`fn`)");
            return;
        }
        op_sig(p, m);
    });
    m.complete(p, EFFECT_DECL);
}

/// `[ "suspend" ] "fn" lower_name [ type_params ] "(" [ params ] ")" "->" type [ "fails" error_set ]`
///
/// Type parameters are only allowed on built-in effects (spec 8.1); the type checker reports E0426.
fn op_sig(p: &mut Parser<'_>, m: Marker) {
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
    } else {
        p.expect(THIN_ARROW);
    }
    effects_and_errors(p);
    m.complete(p, OP_SIG);
}

/// `"handler" type_name [ "(" [ fields ] ")" ] ":" type_ref [ "[" type { "," type } "]" ]
/// "{" { handler_item NL } "}"`
///
/// `handler_item = "init" block | "release" block
///               | [ "suspend" ] "fn" lower_name "(" [ lower_name { "," lower_name } ] ")" block`
fn handler_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(HANDLER_KW);
    decl_name(p, TYPE_NAME);
    if p.at(L_PAREN) {
        field_list(p);
    }
    // Without `:`, the old `implements` (E0145, reported by validation) or a following type name
    // is still read as the effect.
    if p.at(COLON) || p.at_contextual_kw("implements") || p.at(TYPE_NAME) {
        if p.at_contextual_kw("implements") {
            p.bump();
        } else if !p.eat(COLON) {
            p.error(DiagnosticCode::ExpectedToken, "expected `:`");
        }
        if p.at(TYPE_NAME) {
            let c = p.start();
            path(p);
            effect_type_args(p);
            c.complete(p, HANDLER_EFFECT);
        } else {
            p.expect(TYPE_NAME);
        }
    } else {
        p.error(DiagnosticCode::ExpectedToken, "expected `:`");
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
            // The syntax of derive rules is undecided (spec 18.1 #3). Report it and skip the
            // whole thing.
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

/// `"impl" type_ref [ type_params ] ":" type_ref "{" { impl_item NL } "}"`．
/// `impl_item = "fn" lower_name "(" [ params ] ")" block`
///
/// The type and its type parameters (`Map[K: Eq, V: Eq]`) become an IMPL_TYPE; the trait is the
/// PATH after `:`.
fn impl_decl(p: &mut Parser<'_>, m: Marker) {
    p.bump_kind(IMPL_KW);
    // The old `impl[A: Eq] Box[A]: Eq` puts the type parameters first (E0144, reported by
    // validation).
    if p.at(L_BRACK) {
        type_param_list(p);
    }
    let head = p.start();
    if p.at(TYPE_NAME) {
        path(p);
    } else {
        p.expect(TYPE_NAME);
    }
    if p.at_contextual_kw("for") {
        // The old `impl Show for User` (E0144, reported by validation): the first name is the
        // trait and the type follows `for`.
        head.abandon(p);
        p.bump();
        impl_type(p);
    } else {
        if p.at(L_BRACK) {
            type_param_list(p);
        }
        head.complete(p, IMPL_TYPE);
        // Even without `:`, a following type name is read as the trait.
        if p.expect(COLON) || p.at(TYPE_NAME) {
            if p.at(TYPE_NAME) {
                path(p);
            } else {
                p.expect(TYPE_NAME);
            }
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

/// The type of an impl with its type parameters: `type_ref [ type_params ]`.
fn impl_type(p: &mut Parser<'_>) {
    let m = p.start();
    if p.at(TYPE_NAME) {
        path(p);
    } else {
        p.expect(TYPE_NAME);
    }
    if p.at(L_BRACK) {
        type_param_list(p);
    }
    m.complete(p, IMPL_TYPE);
}

/// A declaration body `"{" { item NL } "}"`. `item` reads each item and completes it with the
/// marker it gets.
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

/// Reads up to a newline or the matching outer `}`. Inner `{ }` are read whole.
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

/// An unreadable item in a body. Reports a diagnostic and wraps everything up to the end of the
/// line or before `}` in ERROR.
fn item_error(p: &mut Parser<'_>, m: Marker, message: &str) {
    p.error(DiagnosticCode::ExpectedToken, message);
    while !matches!(p.current(), NEWLINE | R_BRACE | EOF) && !p.at_top_decl_start() {
        p.bump();
    }
    m.complete(p, ERROR);
}
