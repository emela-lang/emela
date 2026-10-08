//! 木ができた後の形の検査．パーサは寛容に読み，ここで文法の外の制約を見る．
//!
//! 束縛の左辺は式として読んでいるので，パターンとして読める形かをここで確かめる．
//! あわせて，パターンにしか書けない `_` と式のない `..` が左辺の外にないかを見る．

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
        UNDERSCORE_EXPR => error(
            out,
            node,
            DiagnosticCode::UnderscoreOutsidePattern,
            "`_` can only be used in patterns",
        ),
        REST_EXPR if node.first_child().is_none() => error(
            out,
            node,
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
            node,
            DiagnosticCode::InvalidBindingTarget,
            "invalid left-hand side of binding: expected a pattern",
        );
    }
}

fn has_token(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    node.children_with_tokens().any(|t| t.kind() == kind)
}

fn error(out: &mut Vec<Diagnostic>, node: &SyntaxNode, code: DiagnosticCode, message: &str) {
    out.push(Diagnostic {
        range: node.text_range(),
        code,
        message: message.to_owned(),
    });
}
