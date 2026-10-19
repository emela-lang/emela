//! Rewrites of the old forms the parser still reads (E0144, E0145). Validation shows them
//! in the message, and the driver's quick fixes apply them, so both come from here.

use rowan::{NodeOrToken, TextRange};

use crate::SyntaxKind::*;
use crate::{SyntaxNode, SyntaxToken};

/// The old head of an impl (`impl Show for User`, `impl[A: Eq] Box[A]: Eq`, spec 10.2) and the
/// head to write instead (`impl User: Show`, `impl Box[A: Eq]: Eq`).
///
/// The range runs from `impl` to the end of the head, before the body. `None` for an impl
/// already in the new form, or one whose head is too broken to rewrite.
pub fn old_impl_head(node: &SyntaxNode) -> Option<(TextRange, String)> {
    if node.kind() != IMPL_DECL {
        return None;
    }
    let leading = node.children().find(|n| n.kind() == TYPE_PARAM_LIST);
    let has_for = node
        .children_with_tokens()
        .any(|e| e.kind() == LOWER_NAME && e.as_token().is_some_and(|t| t.text() == "for"));
    if leading.is_none() && !has_for {
        return None;
    }
    let keyword = node
        .children_with_tokens()
        .find(|e| e.kind() == IMPL_KW)?
        .text_range();
    let ty = node.children().find(|n| n.kind() == IMPL_TYPE)?;
    let trait_ref = node.children().find(|n| n.kind() == PATH)?;
    let end = ty.text_range().end().max(trait_ref.text_range().end());
    let ty_text = match leading {
        Some(leading) => with_bounds(&ty, &leading),
        None => compact(&ty.text().to_string()),
    };
    let head = format!("impl {ty_text}: {}", compact(&trait_ref.text().to_string()));
    Some((TextRange::new(keyword.start(), end), head))
}

/// Moves the bounds of the leading type parameters (`impl[A: Eq] Box[A]`) onto the type's own
/// parameters (`Box[A: Eq]`). Without a parameter list on the type, the leading list goes there.
fn with_bounds(ty: &SyntaxNode, leading: &SyntaxNode) -> String {
    let bounds: Vec<(String, String)> = leading
        .children()
        .filter(|n| n.kind() == TYPE_PARAM)
        .filter_map(|param| Some((param_name(&param)?, compact(&param.text().to_string()))))
        .collect();
    let Some(path) = ty.children().find(|n| n.kind() == PATH) else {
        return compact(&ty.text().to_string());
    };
    let path = compact(&path.text().to_string());
    let Some(list) = ty.children().find(|n| n.kind() == TYPE_PARAM_LIST) else {
        return format!("{path}{}", compact(&leading.text().to_string()));
    };
    let params: Vec<String> = list
        .children()
        .filter(|n| n.kind() == TYPE_PARAM)
        .map(|param| {
            let own = compact(&param.text().to_string());
            let bounded = param.children().any(|n| n.kind() == PATH);
            match param_name(&param) {
                Some(name) if !bounded => bounds
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or(own, |(_, text)| text.clone()),
                _ => own,
            }
        })
        .collect();
    format!("{path}[{}]", params.join(", "))
}

fn param_name(param: &SyntaxNode) -> Option<String> {
    param
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| matches!(t.kind(), UPPER_NAME | TYPE_NAME))
        .map(|t| t.text().to_owned())
}

/// The old `implements` of a handler (`handler H implements E`, spec 8.4): the `implements`
/// token, the range to replace with `:` (from the end of the name or fields to the end of
/// `implements`), and the head to write instead (`handler H: E`).
pub fn old_handler_head(node: &SyntaxNode) -> Option<(SyntaxToken, TextRange, String)> {
    if node.kind() != HANDLER_DECL {
        return None;
    }
    let implements = node
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == LOWER_NAME && t.text() == "implements")?;
    // The name, or the field list after it.
    let before = node
        .children_with_tokens()
        .take_while(|e| e.text_range().end() <= implements.text_range().start())
        .filter(|e| matches!(e.kind(), TYPE_NAME | FIELD_LIST))
        .last()?;
    let name = node
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == TYPE_NAME)?;
    let fields = node
        .children()
        .find(|n| n.kind() == FIELD_LIST)
        .map(|f| compact(&f.text().to_string()))
        .unwrap_or_default();
    let effect = node.children().find(|n| n.kind() == HANDLER_EFFECT)?;
    let head = format!(
        "handler {}{fields}: {}",
        name.text(),
        compact(&effect.text().to_string())
    );
    let range = TextRange::new(before.text_range().end(), implements.text_range().end());
    Some((implements, range, head))
}

/// The text with each run of whitespace (newlines included) made one space, and none just
/// inside brackets.
fn compact(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("[ ", "[")
        .replace(" ]", "]")
        .replace("( ", "(")
        .replace(" )", ")")
}
