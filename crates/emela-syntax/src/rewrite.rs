//! Rewrites of the old forms the parser still reads (E0144, E0145, E0147, E0148). Validation shows them
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

/// The old form of an anonymous function (`fn(x) { x + 1 }`, E0148, `[fn.decl]`), and the text to
/// write instead (`|x| x + 1`).
///
/// The range covers the whole function. The braces are dropped when the block is on one line and
/// holds one expression, as the formatter does (`[fmt.closure]`); otherwise the block is kept as
/// written. A parameter type with `|` outside brackets is parenthesized, since there `|` would
/// close the parameter list. The result is parenthesized where the extent of the body requires
/// it, or where the formatter would write parentheses: as the callee, the receiver of a field, an
/// operand of a binary or unary operator, or the value of a postfix match or escape.
///
/// Also returns the parameter list written in the new form (`|x|`), which the help shows when the
/// rewrite spans lines.
///
/// `None` for an anonymous function in the new form, or one too broken to rewrite.
pub fn old_lambda(node: &SyntaxNode) -> Option<(TextRange, String, String)> {
    if node.kind() != LAMBDA_EXPR || !node.children_with_tokens().any(|e| e.kind() == FN_KW) {
        return None;
    }
    let list = node.children().find(|n| n.kind() == PARAM_LIST)?;
    let block = node.children().find(|n| n.kind() == BLOCK_EXPR)?;
    let params: Vec<String> = list
        .children()
        .filter(|n| n.kind() == PARAM)
        .map(|param| closure_param(&param))
        .collect();
    let head = if params.is_empty() {
        "||".to_owned()
    } else {
        format!("|{}|", params.join(", "))
    };
    let body = match single_line_expression(&block) {
        Some(expr) => expr.text().to_string(),
        None => block.text().to_string(),
    };
    let mut text = format!("{head} {body}");
    if needs_parens(node) {
        text = format!("({text})");
    }
    Some((node.text_range(), text, head))
}

/// A parameter written for a closure's parameter list.
fn closure_param(param: &SyntaxNode) -> String {
    let whole = compact(&param.text().to_string());
    let Some(ty) = param.children().filter(|n| is_type(n.kind())).last() else {
        return whole;
    };
    if !pipe_outside_brackets(&ty) {
        return whole;
    }
    let start = param.text_range().start();
    let text = param.text().to_string();
    let before = &text[..usize::from(ty.text_range().start() - start)];
    format!("{} ({})", compact(before), compact(&ty.text().to_string()))
}

fn is_type(kind: crate::SyntaxKind) -> bool {
    matches!(
        kind,
        PATH_TYPE | TYPE_VAR | SELF_TYPE | PAREN_TYPE | UNIT_TYPE | TUPLE_TYPE | FN_TYPE
    )
}

/// Whether a `|` of the type is outside any bracket inside it.
fn pipe_outside_brackets(ty: &SyntaxNode) -> bool {
    let mut depth = 0i32;
    for token in ty
        .descendants_with_tokens()
        .filter_map(NodeOrToken::into_token)
    {
        match token.kind() {
            L_PAREN | L_BRACK | L_BRACE => depth += 1,
            R_PAREN | R_BRACK | R_BRACE => depth -= 1,
            PIPE if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// The expression of a block written on one line with one expression (`[fmt.closure]`). As for
/// the body of an if, "on one line" is about the block's own line breaks: a line break inside
/// brackets within the expression does not count.
pub fn single_line_expression(block: &SyntaxNode) -> Option<SyntaxNode> {
    let mut stmts = block.children();
    let only = stmts.next()?;
    if stmts.next().is_some() || only.kind() == BINDING || only.kind() == ERROR {
        return None;
    }
    let broken = block
        .children_with_tokens()
        .any(|e| matches!(e.kind(), NEWLINE | NEWLINE_CONT | COMMENT | DOC_COMMENT));
    (!broken).then_some(only)
}

/// Whether an anonymous function at `node` is written in parentheses: where its body would take
/// what follows, and as an operand of an operator (`[fmt.closure]`).
pub fn needs_parens(node: &SyntaxNode) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let first = parent.children().next().as_ref() == Some(node);
    match parent.kind() {
        BIN_EXPR | PREFIX_EXPR => true,
        CALL_EXPR | FIELD_EXPR | MATCH_EXPR | ESCAPE_EXPR => first,
        _ => false,
    }
}

/// The help for a line that starts with `||` (E0149, and E0301 in the driver).
pub const OR_AT_LINE_START_HELP: &str = "a line that starts with `||` starts an anonymous function; to continue the previous line with `||`, enclose the expression in parentheses";

/// Whether the anonymous function starts with `||` at the start of a line.
pub fn starts_line_with_or(lambda: &SyntaxNode) -> bool {
    let Some(first) = lambda
        .descendants_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| !t.kind().is_trivia())
    else {
        return false;
    };
    if first.kind() != PIPE2 {
        return false;
    }
    let mut cur = first.prev_token();
    while let Some(t) = cur {
        match t.kind() {
            NEWLINE | NEWLINE_CONT => return true,
            k if k.is_trivia() => cur = t.prev_token(),
            _ => return false,
        }
    }
    true
}

/// The old effect clause (`use Db`, `[grammar.decl]`): the `use` token and the uses clause to
/// write instead, in its canonical form (`uses { Db }`). `None` for a clause written with
/// `uses`.
pub fn old_uses_clause(node: &SyntaxNode) -> Option<(SyntaxToken, String)> {
    if node.kind() != USES_CLAUSE {
        return None;
    }
    let keyword = node
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == USE_KW)?;
    Some((keyword, canonical_uses_clause(node)?))
}

/// The canonical form of a uses clause (`[fmt.signature]`): `uses` and its set, always in
/// braces (`uses { Db }`, `uses { Db, Log }`, `uses {}`). `None` if the clause has no set.
pub fn canonical_uses_clause(node: &SyntaxNode) -> Option<String> {
    let set = node.children().find(|n| n.kind() == EFFECT_SET)?;
    let items: Vec<String> = set
        .children()
        .map(|item| compact(&without_comments(&item)))
        .collect();
    Some(if items.is_empty() {
        "uses {}".to_owned()
    } else {
        format!("uses {{ {} }}", items.join(", "))
    })
}

/// The old clause as written, for the message of E0147: its text without comments, compacted.
pub(crate) fn old_uses_text(node: &SyntaxNode) -> String {
    compact(&without_comments(node))
}

fn without_comments(node: &SyntaxNode) -> String {
    node.descendants_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .filter(|t| !matches!(t.kind(), COMMENT | DOC_COMMENT))
        .map(|t| t.text().to_owned())
        .collect()
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
