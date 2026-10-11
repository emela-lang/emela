//! 木ができた後の形の検査．パーサは寛容に読み，ここで文法の外の制約を見る．
//!
//! - 束縛の左辺は式として読んでいるので，パターンとして読める形かを確かめる．あわせて，
//!   パターンにしか書けない `_` と式のない `..` が左辺の外にないかを見る
//! - 17.3 の文法の外の制約のうち，構文だけで判るもの（`@external` の要否，空の enum，
//!   handler と impl の fn の形）を見る

use rowan::TextRange;

use crate::SyntaxKind::*;
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::{SyntaxKind, SyntaxNode};

pub(crate) fn validate(root: &SyntaxNode) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    walk(root, &mut diagnostics);
    diagnostics
}

fn walk(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    if node.kind() == BINDING {
        let mut children = node.children();
        if let Some(lhs) = children.next() {
            binding_target(&lhs, out);
        }
        for child in children {
            walk(&child, out);
        }
        return;
    }
    match node.kind() {
        FN_DECL => fn_decl(node, out),
        TYPE_DECL if child_kind(node, FIELD_LIST).is_none() && !is_external(node) => error(
            out,
            name_or(node),
            DiagnosticCode::MissingExternal,
            "a type without fields must be `@external`",
        ),
        ENUM_DECL => {
            if let Some(list) = child_kind(node, VARIANT_LIST)
                && child_kind(&list, VARIANT).is_none()
            {
                error(
                    out,
                    name_or(node),
                    DiagnosticCode::EmptyEnum,
                    "an enum needs at least one variant",
                );
            }
        }
        UNDERSCORE_EXPR => error(
            out,
            node.text_range(),
            DiagnosticCode::UnderscoreOutsidePattern,
            "`_` can only be used in patterns",
        ),
        REST_EXPR | SPREAD_ARG if node.first_child().is_none() => error(
            out,
            node.text_range(),
            DiagnosticCode::BareRestOutsidePattern,
            "`..` without an expression can only be used in patterns",
        ),
        _ => {}
    }
    for child in node.children() {
        walk(&child, out);
    }
}

/// 束縛の左辺．17.6 のパターンに対応する式の形だけを通す．
fn binding_target(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let ok = match node.kind() {
        UNDERSCORE_EXPR | PATH_EXPR | UNIT_EXPR | LITERAL => true,
        NAME_REF => !has_token(node, SELF_KW),
        STRING => node.children().all(|c| c.kind() != INTERP),
        TUPLE_EXPR | LIST_EXPR => {
            node.children().for_each(|c| binding_target(&c, out));
            true
        }
        // `..` の後ろは名前だけ．
        REST_EXPR => match node.first_child() {
            None => true,
            Some(c) => c.kind() == NAME_REF && has_token(&c, LOWER_NAME),
        },
        // `Circle(radius:)` `User(name: n, ..)`．呼ぶ側は型の参照に限る．
        CALL_EXPR => {
            let mut children = node.children();
            let callee_ok = children.next().is_some_and(|c| c.kind() == PATH_EXPR);
            for args in children {
                for arg in args.children() {
                    match arg.kind() {
                        NAMED_ARG => arg.children().for_each(|c| binding_target(&c, out)),
                        // パターンに書けるのは式のない `..` だけ．`..式` は部分更新の構文．
                        SPREAD_ARG if arg.first_child().is_none() => {}
                        _ => binding_target(&arg, out),
                    }
                }
            }
            callee_ok
        }
        _ => false,
    };
    if !ok {
        error(
            out,
            node.text_range(),
            DiagnosticCode::InvalidBindingTarget,
            "invalid left-hand side of binding: expected a pattern",
        );
    }
}

/// fn の置き場所ごとの制約．
fn fn_decl(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let has_body = child_kind(node, BLOCK_EXPR).is_some();
    let owner = node
        .parent()
        .filter(|p| p.kind() == ITEM_LIST)
        .and_then(|list| list.parent())
        .map(|decl| decl.kind());
    match owner {
        // トップレベル．
        None => {
            // 同梱の core のソースの組み込み関数は `@intrinsic` で本体を省く．置き場所の検査は名前解決が行う．
            if !is_external(node) && !has_annotation(node, "intrinsic") {
                if !has_body {
                    error(
                        out,
                        name_or(node),
                        DiagnosticCode::MissingExternal,
                        "a function without a body must be `@external`",
                    );
                } else if has_token(node, SUSPEND_KW) {
                    error(
                        out,
                        name_or(node),
                        DiagnosticCode::MissingExternal,
                        "a top-level `suspend fn` must be `@external`",
                    );
                }
            }
        }
        Some(HANDLER_DECL) => {
            let typed = child_kind(node, PARAM_LIST)
                .into_iter()
                .flat_map(|list| list.children())
                .filter(|param| param.first_child().is_some());
            for param in typed {
                error(
                    out,
                    param.text_range(),
                    DiagnosticCode::TypedHandlerParam,
                    "handler operation parameters take their types from the effect",
                );
            }
            if !has_body {
                missing_body(node, out);
            }
        }
        Some(IMPL_DECL) => {
            for clause in node
                .children()
                .filter(|c| matches!(c.kind(), RET_TYPE | FAILS_CLAUSE | USE_CLAUSE))
            {
                error(
                    out,
                    clause.text_range(),
                    DiagnosticCode::SignatureInImpl,
                    "impl methods take their signature from the trait",
                );
            }
            if !has_body {
                missing_body(node, out);
            }
        }
        // trait の fn は本体を省ける．
        _ => {}
    }
}

fn missing_body(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    error(
        out,
        name_or(node),
        DiagnosticCode::MissingBody,
        "this function needs a body",
    );
}

/// `@external` が付いているか．
fn is_external(node: &SyntaxNode) -> bool {
    has_annotation(node, "external")
}

/// `@name` の注釈が付いているか．
fn has_annotation(node: &SyntaxNode, name: &str) -> bool {
    node.children().filter(|c| c.kind() == ANNOTATION).any(|a| {
        a.children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == LOWER_NAME && t.text() == name)
    })
}

fn child_kind(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

/// 診断を出す範囲．宣言全体は長いので，名前のトークンがあればそこにする．
fn name_or(node: &SyntaxNode) -> TextRange {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), LOWER_NAME | TYPE_NAME | UPPER_NAME))
        .map_or(node.text_range(), |t| t.text_range())
}

fn has_token(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    node.children_with_tokens().any(|t| t.kind() == kind)
}

fn error(out: &mut Vec<Diagnostic>, range: TextRange, code: DiagnosticCode, message: &str) {
    out.push(Diagnostic {
        range,
        code,
        message: message.to_owned(),
    });
}
