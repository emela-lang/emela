//! 木ができた後の形の検査．パーサは寛容に読み，ここで文法の外の制約を見る．
//!
//! - 束縛の左辺は式として読んでいるので，パターンとして読める形かを確かめる．あわせて，
//!   パターンにしか書けない `_` と式のない `..` が左辺の外にないかを見る
//! - 17.3 の文法の外の制約のうち，構文だけで判るもの（`@external` の要否，空の enum，
//!   handler と impl の fn の形）を見る
//! - `@external` の引数の形（12.1，12.3）を見る

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
        FN_DECL => {
            fn_decl(node, out);
            fn_externals(node, out);
        }
        TYPE_DECL => {
            if child_kind(node, FIELD_LIST).is_none() && !is_external(node) {
                error(
                    out,
                    name_or(node),
                    DiagnosticCode::MissingExternal,
                    "a type without fields must be `@external`",
                );
            }
            // `@external type` の注釈は引数を取らない（12.3）．
            for list in annotations(node, "external").filter_map(|a| child_kind(&a, ANNOT_ARG_LIST))
            {
                error(
                    out,
                    list.text_range(),
                    DiagnosticCode::ExternalTypeWithArgs,
                    "`@external` on a type takes no arguments",
                );
            }
        }
        OP_SIG => {
            if let Some(clause) = child_kind(node, USE_CLAUSE) {
                error(
                    out,
                    clause.text_range(),
                    DiagnosticCode::UseOnEffectOp,
                    "effect operations cannot have a `use` clause",
                );
            }
        }
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

/// `@external` のターゲットとして知っている名前（12.1）．
const EXTERNAL_TARGETS: [&str; 2] = ["js", "wasm"];

/// fn の `@external(ターゲット, "a")` と `@external(ターゲット, "a", "b")` の形（12.1）．
/// ターゲットの名前の後ろに文字列が1つか2つ．ターゲットごとに1つ．
fn fn_externals(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let mut seen: Vec<String> = Vec::new();
    for annotation in annotations(node, "external") {
        let args: Vec<SyntaxNode> = child_kind(&annotation, ANNOT_ARG_LIST)
            .into_iter()
            .flat_map(|list| list.children())
            .filter(|arg| arg.kind() == ANNOT_ARG)
            .collect();
        // 読めなかった引数は構文の診断が出ている．重ねない．
        if args
            .iter()
            .any(|arg| arg.descendants().any(|n| n.kind() == ERROR))
        {
            continue;
        }
        let Some((first, rest)) = args.split_first() else {
            error(
                out,
                annotation.text_range(),
                DiagnosticCode::MalformedExternal,
                "`@external` on a function needs a target and a name: `@external(js, \"name\")`",
            );
            continue;
        };
        let Some(target) = annot_name(first) else {
            error(
                out,
                first.text_range(),
                DiagnosticCode::MalformedExternal,
                "the first argument of `@external` must be a target name such as `js`",
            );
            continue;
        };
        if !EXTERNAL_TARGETS.contains(&target.as_str()) {
            error(
                out,
                first.text_range(),
                DiagnosticCode::UnknownExternalTarget,
                &format!("unknown target `{target}` in `@external`: expected `js` or `wasm`"),
            );
        } else if seen.contains(&target) {
            error(
                out,
                first.text_range(),
                DiagnosticCode::DuplicateExternalTarget,
                &format!("more than one `@external` for target `{target}`"),
            );
        } else {
            seen.push(target);
        }
        for arg in rest {
            let message = match annot_string(arg) {
                Some(true) => continue,
                Some(false) => "string interpolation is not allowed in `@external`",
                None => "arguments after the target of `@external` must be strings",
            };
            error(
                out,
                arg.text_range(),
                DiagnosticCode::MalformedExternal,
                message,
            );
        }
        if !(1..=2).contains(&rest.len()) {
            error(
                out,
                annotation.text_range(),
                DiagnosticCode::MalformedExternal,
                &format!(
                    "`@external` takes one or two strings after the target, found {}",
                    rest.len()
                ),
            );
        }
    }
}

/// 名前だけの注釈の引数（`js`）の名前．
fn annot_name(arg: &SyntaxNode) -> Option<String> {
    if arg.first_child().is_some() || has_token(arg, COLON) {
        return None;
    }
    arg.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == LOWER_NAME)
        .map(|t| t.text().to_owned())
}

/// 文字列だけの注釈の引数か．補間のない文字列なら `Some(true)`，補間のある文字列なら
/// `Some(false)`，文字列でなければ `None`．
fn annot_string(arg: &SyntaxNode) -> Option<bool> {
    if has_token(arg, LOWER_NAME) {
        return None;
    }
    let string = child_kind(arg, STRING)?;
    Some(child_kind(&string, INTERP).is_none())
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
    annotations(node, name).next().is_some()
}

/// 宣言に付いた `@name` の注釈．
fn annotations<'a>(node: &SyntaxNode, name: &'a str) -> impl Iterator<Item = SyntaxNode> + 'a {
    node.children()
        .filter(|c| c.kind() == ANNOTATION)
        .filter(move |a| {
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
