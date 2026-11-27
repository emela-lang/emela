//! Text edits shared by the quick fixes and the impl stubs.

use emela_syntax::{SyntaxKind, SyntaxNode};
use line_index::{TextRange, TextSize};

pub(super) type Edit = (TextRange, String);
/// A titled set of edits.
pub(super) type Fix = (String, Vec<Edit>);

/// Append after the last arm/method, preserving trailing comments and the closing brace.
pub(super) fn append(
    list: &SyntaxNode,
    kind: SyntaxKind,
    items: &[String],
    text: &str,
) -> Option<Edit> {
    let last = list.children().filter(|n| n.kind() == kind).last();
    let open = list
        .children_with_tokens()
        .find(|e| e.kind() == SyntaxKind::L_BRACE)?;
    let mut at = last
        .as_ref()
        .map_or(open.text_range().end(), |n| n.text_range().end());
    let tail = &text[usize::from(at)..usize::from(list.text_range().end())];
    let line = tail.split('\n').next()?.trim_end_matches('\r');
    if line.trim_start().starts_with('#') {
        at += TextSize::try_from(line.len()).ok()?;
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let base = indent(text, list.text_range().start());
    let inner = last
        .as_ref()
        .map(|n| indent(text, n.text_range().start()))
        .filter(|s| s.len() > base.len())
        .unwrap_or_else(|| format!("{base}  "));
    let mut inserted = items
        .iter()
        .map(|item| format!("{newline}{inner}{item}"))
        .collect::<String>();
    // An inline block needs a newline before its closing brace as well.
    let tail = &text[usize::from(at)..usize::from(list.text_range().end())];
    if !tail.trim_start_matches([' ', '\t']).starts_with('\n')
        && !tail.trim_start_matches([' ', '\t']).starts_with("\r\n")
    {
        inserted.push_str(&format!("{newline}{base}"));
    }
    Some((TextRange::empty(at), inserted))
}

pub(super) fn indent(text: &str, at: TextSize) -> String {
    text[..usize::from(at)]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect()
}
