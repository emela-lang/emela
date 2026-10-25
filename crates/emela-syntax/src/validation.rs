//! Shape checks after the tree is built. The parser reads leniently; constraints outside the
//! grammar are checked here.
//!
//! - The left side of a binding is parsed as an expression, so check that it reads as a pattern.
//!   Also check that `_` and a bare `..`, which only patterns allow, do not appear outside it.
//! - Check the constraints beyond the spec 17.3 grammar that syntax alone decides (whether
//!   `@external` is needed, empty enums, the shape of handler and impl fns).
//! - Check the arguments of `@external` (spec 12.1, 12.3) and of `@export` (spec 12.7, 16.6.2).
//! - Report the old impl and handler heads with the head to write instead (E0144, E0145).

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
        ANNOTATION if is_named(node, "export") => export(node, out),
        TYPE_DECL => {
            if child_kind(node, FIELD_LIST).is_none() && !is_external(node) {
                error(
                    out,
                    name_or(node),
                    DiagnosticCode::MissingExternal,
                    "a type without fields must be `@external`",
                );
            }
            // The annotation on `@external type` takes no arguments (spec 12.3).
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
        IMPL_DECL => {
            if let Some((range, head)) = crate::rewrite::old_impl_head(node) {
                let text = node.text().to_string();
                let start = node.text_range().start();
                let old =
                    &text[usize::from(range.start() - start)..usize::from(range.end() - start)];
                let old = old.split_whitespace().collect::<Vec<_>>().join(" ");
                error(
                    out,
                    range,
                    DiagnosticCode::OldImplHead,
                    &format!("write `{head}` instead of `{old}`"),
                );
            }
        }
        HANDLER_DECL => {
            if let Some((implements, _, head)) = crate::rewrite::old_handler_head(node) {
                error(
                    out,
                    implements.text_range(),
                    DiagnosticCode::OldHandlerHead,
                    &format!("write `{head}` instead of `implements`"),
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

/// Accepts only the expression shapes that correspond to spec 17.6 patterns.
fn binding_target(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let ok = match node.kind() {
        UNDERSCORE_EXPR | PATH_EXPR | UNIT_EXPR | LITERAL => true,
        NAME_REF => !has_token(node, SELF_KW),
        // `-1` `-1.5`: a negative number is a pattern (spec 17.6).
        PREFIX_EXPR => {
            has_token(node, MINUS)
                && node.children().count() == 1
                && node.first_child().is_some_and(|c| {
                    c.kind() == LITERAL && (has_token(&c, INT) || has_token(&c, FLOAT))
                })
        }
        STRING => node.children().all(|c| c.kind() != INTERP),
        TUPLE_EXPR | LIST_EXPR => {
            node.children().for_each(|c| binding_target(&c, out));
            true
        }
        // Only a name may follow `..`.
        REST_EXPR => match node.first_child() {
            None => true,
            Some(c) => c.kind() == NAME_REF && has_token(&c, LOWER_NAME),
        },
        // `Circle(radius:)` `User(name: n, ..)`. The callee must be a type reference.
        CALL_EXPR => {
            let mut children = node.children();
            let callee_ok = children.next().is_some_and(|c| c.kind() == PATH_EXPR);
            for args in children {
                for arg in args.children() {
                    match arg.kind() {
                        NAMED_ARG => arg.children().for_each(|c| binding_target(&c, out)),
                        // Only a bare `..` is a pattern; `..expr` is the partial update syntax.
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

/// Constraints that depend on where the fn is.
fn fn_decl(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let has_body = child_kind(node, BLOCK_EXPR).is_some();
    let owner = node
        .parent()
        .filter(|p| p.kind() == ITEM_LIST)
        .and_then(|list| list.parent())
        .map(|decl| decl.kind());
    match owner {
        // Top level.
        None => {
            // Built-in functions in the bundled core omit the body with `@intrinsic`. Name
            // resolution checks where they may appear.
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
        // A trait fn may omit the body.
        _ => {}
    }
}

/// Known `@external` targets (spec 12.1).
const EXTERNAL_TARGETS: [&str; 3] = ["js", "wasm", "c"];

/// The shape of a fn's `@external(target, "a")` and `@external(target, "a", "b")` (spec 12.1):
/// one or two strings after the target name, and one annotation per target.
fn fn_externals(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let mut seen: Vec<String> = Vec::new();
    for annotation in annotations(node, "external") {
        let args: Vec<SyntaxNode> = child_kind(&annotation, ANNOT_ARG_LIST)
            .into_iter()
            .flat_map(|list| list.children())
            .filter(|arg| arg.kind() == ANNOT_ARG)
            .collect();
        // Arguments that failed to parse already have a syntax diagnostic; do not pile on.
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
                &format!("unknown target `{target}` in `@external`: expected `js`, `wasm` or `c`"),
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

/// The place and the arguments of `@export(target, "name")` and
/// `@export(target, "interface", "name")` (spec 12.7). Unlike `@external`, a fn may have several
/// for the same target. On `js` the middle argument of the three-argument form is `"default"`
/// (spec 16.6.2).
fn export(annotation: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let on_fn_with_body = annotation.parent().is_some_and(|decl| {
        decl.kind() == FN_DECL
            && decl.parent().is_none_or(|p| p.kind() != ITEM_LIST)
            && child_kind(&decl, BLOCK_EXPR).is_some()
    });
    if !on_fn_with_body {
        error(
            out,
            annotation.text_range(),
            DiagnosticCode::MalformedExport,
            "`@export` is written on a top-level fn with a body",
        );
        return;
    }
    let args: Vec<SyntaxNode> = child_kind(annotation, ANNOT_ARG_LIST)
        .into_iter()
        .flat_map(|list| list.children())
        .filter(|arg| arg.kind() == ANNOT_ARG)
        .collect();
    if args
        .iter()
        .any(|arg| arg.descendants().any(|n| n.kind() == ERROR))
    {
        return;
    }
    let malformed = |out: &mut Vec<Diagnostic>, range| {
        error(
            out,
            range,
            DiagnosticCode::MalformedExport,
            "expected `@export(target, \"name\")`",
        );
    };
    let Some((first, rest)) = args.split_first() else {
        malformed(out, annotation.text_range());
        return;
    };
    let Some(target) = annot_name(first) else {
        malformed(out, first.text_range());
        return;
    };
    if !EXTERNAL_TARGETS.contains(&target.as_str()) {
        error(
            out,
            first.text_range(),
            DiagnosticCode::UnknownExternalTarget,
            &format!("unknown target `{target}` in `@export`: expected `js`, `wasm` or `c`"),
        );
    }
    let mut strings = true;
    for arg in rest {
        match annot_string(arg) {
            Some(true) => {}
            Some(false) => {
                strings = false;
                error(
                    out,
                    arg.text_range(),
                    DiagnosticCode::MalformedExport,
                    "string interpolation is not allowed in `@export`",
                );
            }
            None => {
                strings = false;
                malformed(out, arg.text_range());
            }
        }
    }
    if !(1..=2).contains(&rest.len()) {
        malformed(out, annotation.text_range());
    } else if target == "c" && rest.len() == 2 {
        // `[target.c.export]` gives only `@export(c, "name")` a meaning.
        error(
            out,
            annotation.text_range(),
            DiagnosticCode::MalformedExport,
            "on target `c`, `@export` takes one name: `@export(c, \"name\")`",
        );
    } else if strings
        && target == "js"
        && let [name] = rest
        && string_value(name).as_deref() == Some("default")
    {
        // `(js, "default")` would be the default export itself, the object that the
        // three-argument form adds properties to.
        error(
            out,
            name.text_range(),
            DiagnosticCode::MalformedExport,
            "on target `js`, the name `\"default\"` is the default export: write `@export(js, \"default\", \"name\")`",
        );
    } else if strings
        && target == "js"
        && let [interface, _] = rest
        && string_value(interface).as_deref() != Some("default")
    {
        error(
            out,
            interface.text_range(),
            DiagnosticCode::MalformedExport,
            "on target `js`, the middle argument of `@export` must be `\"default\"`",
        );
    }
}

/// The text of a string argument without interpolation, as written (escapes are not decoded;
/// `"default"` has none).
fn string_value(arg: &SyntaxNode) -> Option<String> {
    let string = child_kind(arg, STRING)?;
    Some(
        string
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == STRING_TEXT)
            .map(|t| t.text().to_owned())
            .collect(),
    )
}

/// Whether the annotation's name (the token after `@`) is `name`.
fn is_named(annotation: &SyntaxNode, name: &str) -> bool {
    annotation
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == LOWER_NAME)
        .is_some_and(|t| t.text() == name)
}

/// The name of an annotation argument that is a bare name (`js`).
fn annot_name(arg: &SyntaxNode) -> Option<String> {
    if arg.first_child().is_some() || has_token(arg, COLON) {
        return None;
    }
    arg.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == LOWER_NAME)
        .map(|t| t.text().to_owned())
}

/// Whether the annotation argument is just a string: `Some(true)` without interpolation,
/// `Some(false)` with it, `None` if it is not a string.
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

fn is_external(node: &SyntaxNode) -> bool {
    has_annotation(node, "external")
}

fn has_annotation(node: &SyntaxNode, name: &str) -> bool {
    annotations(node, name).next().is_some()
}

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

/// The range to report on. A whole declaration is long, so use its name token if it has one.
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
