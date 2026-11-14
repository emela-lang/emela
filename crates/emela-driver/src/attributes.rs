//! Checks of the field attributes `@key` and `@tag` (`[kw.annotation]`).
//!
//! `@key("...")` goes on a field of a type or of the named contents of a variant, and `@tag("...")`
//! on a variant, each with one string literal (E0338). The keys of the fields of one type or
//! variant, and the tags of the variants of one enum, are distinct once the attributes are applied
//! (E0337). Name resolution reads their values into the HIR (`emela_resolve::string_attr_value`)
//! for the derivations; these checks only need the syntax, so they run beside the checks of
//! `@test` and `@internal`.

use emela_resolve::ModuleId;
use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{Parse, SyntaxKind, SyntaxNode};
use smol_str::SmolStr;
use std::collections::HashMap;

use crate::code;
use crate::diagnostic::{Diagnostic, Span};
use crate::pipeline::Analysis;

const KEY: &str = "key";
const TAG: &str = "tag";

/// E0338 for each misplaced `@key` or `@tag` or invalid argument, and E0337 for each key or tag
/// that a field or variant before it in the same declaration already has.
pub(crate) fn check_key_tag<'p>(
    analysis: &Analysis,
    parse_of: impl Fn(ModuleId) -> Option<&'p Parse>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (module, _) in analysis.modules.iter() {
        let (Some(&file), Some(parse)) = (analysis.files.get(module), parse_of(module)) else {
            continue;
        };
        let tree = parse.tree();
        let error = |message: String, code: &'static str, node: &SyntaxNode| {
            Diagnostic::error(message)
                .with_code(code)
                .with_span(Span::new(file, node.text_range()))
        };
        for annotation in tree
            .syntax()
            .descendants()
            .filter_map(ast::Annotation::cast)
        {
            let Some(name) = annotation.name() else {
                continue;
            };
            let name = name.text();
            let placed = match name {
                KEY => on_field(&annotation),
                TAG => annotation
                    .syntax()
                    .parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::VARIANT),
                _ => continue,
            };
            if !placed {
                let target = if name == KEY { "a field" } else { "a variant" };
                diagnostics.push(error(
                    format!("`@{name}` can only be placed on {target}"),
                    code::MISPLACED_KEY_TAG,
                    annotation.syntax(),
                ));
            } else if emela_resolve::string_attr_value(&annotation).is_none() {
                diagnostics.push(error(
                    format!("`@{name}` takes one string literal"),
                    code::MISPLACED_KEY_TAG,
                    annotation.syntax(),
                ));
            }
        }
        for node in tree.syntax().descendants() {
            match node.kind() {
                SyntaxKind::FIELD_LIST
                    if node.parent().is_some_and(|p| {
                        matches!(p.kind(), SyntaxKind::TYPE_DECL | SyntaxKind::VARIANT)
                    }) =>
                {
                    let named = node
                        .children()
                        .filter_map(ast::Field::cast)
                        .filter_map(|f| {
                            let name = f.name()?;
                            let key = attr_value(&f, KEY).unwrap_or_else(|| name.text().into());
                            Some((key, SmolStr::from(name.text()), f.syntax().clone()))
                        });
                    for (key, first, second, node) in repeated(named) {
                        diagnostics.push(error(
                            format!("key `{key}` is used by both `{first}` and `{second}`"),
                            code::DUPLICATE_KEY,
                            &node,
                        ));
                    }
                }
                SyntaxKind::VARIANT_LIST => {
                    let named = node
                        .children()
                        .filter_map(ast::Variant::cast)
                        .filter_map(|v| {
                            let name = v.name()?;
                            let tag = attr_value(&v, TAG).unwrap_or_else(|| name.text().into());
                            Some((tag, SmolStr::from(name.text()), v.syntax().clone()))
                        });
                    for (tag, first, second, node) in repeated(named) {
                        diagnostics.push(error(
                            format!("tag `{tag}` is used by both `{first}` and `{second}`"),
                            code::DUPLICATE_KEY,
                            &node,
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    diagnostics
}

/// Whether `@key` is on a field of a type or of the named contents of a variant.
fn on_field(annotation: &ast::Annotation) -> bool {
    let owner = annotation
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::FIELD)
        .and_then(|f| f.parent())
        .filter(|l| l.kind() == SyntaxKind::FIELD_LIST)
        .and_then(|l| l.parent());
    owner.is_some_and(|o| matches!(o.kind(), SyntaxKind::TYPE_DECL | SyntaxKind::VARIANT))
}

/// The valid argument of the first attribute `name`.
fn attr_value(node: &impl HasAttrs, name: &str) -> Option<SmolStr> {
    node.annotations()
        .find(|a| a.name().is_some_and(|n| n.text() == name))
        .and_then(|a| emela_resolve::string_attr_value(&a))
}

/// For each element whose key an earlier one has: the key, the earlier name, its own name and its
/// node.
fn repeated(
    elements: impl Iterator<Item = (SmolStr, SmolStr, SyntaxNode)>,
) -> Vec<(SmolStr, SmolStr, SmolStr, SyntaxNode)> {
    let mut seen: HashMap<SmolStr, SmolStr> = HashMap::new();
    let mut out = Vec::new();
    for (key, name, node) in elements {
        match seen.get(&key) {
            // The same name twice is E0213 of name resolution.
            Some(first) if *first != name => out.push((key, first.clone(), name, node)),
            Some(_) => {}
            None => {
                seen.insert(key, name);
            }
        }
    }
    out
}
