//! textDocument/hover (spec 0033): type information at a cursor position,
//! looked up in the span→type index recorded by the last check. Falls back to
//! the scope snapshot's rendered signatures for function and enum names, which
//! the checker does not index.

use crate::ast::Type;
use crate::lsp::analysis::{Snapshot, render_fn_sig, render_type};
use crate::lsp::documents::Document;
use crate::lsp::position::{offset_to_position, position_to_offset};
use crate::lsp::protocol::{Hover, MarkupContent, Position, Range};
use crate::typecheck::{EntryKind, TypeEntry};

pub(crate) fn hover(doc: &Document, position: &Position, snapshot: &Snapshot) -> Option<Hover> {
    let offset = position_to_offset(&doc.text, position);
    if let Some((start, end)) = word_at(&doc.text, offset) {
        // 1. A binding or expression spanning exactly the hovered word: a
        //    parameter or `let` name (`name: Type`), a variable reference, a
        //    literal.
        let word = &doc.text[start..end];
        if let Some(entry) = exact_entry(&snapshot.type_index, start, end) {
            // A function-typed reference — a call's callee, a function passed
            // by name — reads better as a named signature, with generic
            // parameters already instantiated by the checker.
            if let (EntryKind::Expr, Type::Function(function)) = (&entry.kind, &entry.ty) {
                let line = render_fn_sig(
                    word,
                    &function.params,
                    &function.ret,
                    &function.throws.clone().map(|ty| *ty),
                    &function.effects,
                );
                return Some(text_hover(doc, line, start, end));
            }
            return Some(entry_hover(doc, entry));
        }
        // 2. A function or trait-method name at its definition, or a callee
        //    the checker resolved outside expression checking: the snapshot's
        //    rendered signature stands in.
        if let Some(sym) = snapshot
            .functions
            .iter()
            .chain(&snapshot.trait_methods)
            .find(|sym| sym.name == word)
        {
            return Some(text_hover(doc, sym.detail.clone(), start, end));
        }
        // 3. An enum name.
        if snapshot.enums.iter().any(|sym| sym.name == word) {
            return Some(text_hover(doc, format!("enum {word}"), start, end));
        }
    }
    // 4. The smallest recorded expression containing the cursor — hovering an
    //    operator or delimiter shows the enclosing expression's type.
    let entry = containing_entry(&snapshot.type_index, offset)?;
    Some(entry_hover(doc, entry))
}

/// The identifier-shaped word around `offset`: the maximal ASCII
/// `[A-Za-z0-9_]` run, as byte offsets. A cursor just past the last character
/// still counts (hover at a word's right edge). Digit runs are included so
/// numeric literals resolve through their exact-span index entry.
fn word_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut anchor = offset.min(bytes.len());
    if anchor >= bytes.len() || !is_word(bytes[anchor]) {
        if anchor == 0 || !is_word(bytes[anchor - 1]) {
            return None;
        }
        anchor -= 1;
    }
    let mut start = anchor;
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    Some((start, end))
}

/// The entry spanning exactly `start..end`, preferring a `Binding` — a `let`
/// whose statement value is the block tail records a same-span `Unit`
/// expression entry that must not shadow the binding's type.
fn exact_entry(index: &[TypeEntry], start: usize, end: usize) -> Option<&TypeEntry> {
    let mut fallback = None;
    for entry in index {
        if entry.span.start != start || entry.span.end != end {
            continue;
        }
        if matches!(entry.kind, EntryKind::Binding(_)) {
            return Some(entry);
        }
        fallback.get_or_insert(entry);
    }
    fallback
}

/// The smallest entry containing `offset`, `Binding` winning ties.
fn containing_entry(index: &[TypeEntry], offset: usize) -> Option<&TypeEntry> {
    index
        .iter()
        .filter(|entry| entry.span.start <= offset && offset < entry.span.end)
        .min_by_key(|entry| {
            let kind_rank = match entry.kind {
                EntryKind::Binding(_) => 0,
                EntryKind::Expr => 1,
            };
            (entry.span.end - entry.span.start, kind_rank)
        })
}

fn entry_hover(doc: &Document, entry: &TypeEntry) -> Hover {
    let line = match &entry.kind {
        EntryKind::Binding(name) => format!("{name}: {}", render_type(&entry.ty)),
        EntryKind::Expr => render_type(&entry.ty),
    };
    text_hover(doc, line, entry.span.start, entry.span.end)
}

fn text_hover(doc: &Document, line: String, start: usize, end: usize) -> Hover {
    Hover {
        contents: MarkupContent {
            kind: "markdown",
            value: format!("```emela\n{line}\n```"),
        },
        range: Some(Range {
            start: offset_to_position(&doc.text, start),
            end: offset_to_position(&doc.text, end),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Type;
    use crate::error::{SourceFile, Span};

    fn entry(source: &str, start: usize, end: usize, ty: Type, kind: EntryKind) -> TypeEntry {
        TypeEntry {
            span: Span::new(SourceFile::new("test.emel", source), start, end),
            ty,
            kind,
        }
    }

    #[test]
    fn word_at_finds_identifier_boundaries() {
        let text = "let value = other";
        assert_eq!(word_at(text, 4), Some((4, 9))); // on `v`
        assert_eq!(word_at(text, 8), Some((4, 9))); // on `e`
        assert_eq!(word_at(text, 9), Some((4, 9))); // right edge
        assert_eq!(word_at(text, 10), None); // on `=`
        assert_eq!(word_at(text, 17), Some((12, 17))); // end of text
    }

    #[test]
    fn word_at_ignores_multibyte_neighbours() {
        let text = "\"こんにちは\" n";
        // Inside the string literal: not an ASCII word.
        assert_eq!(word_at(text, 4), None);
        let n = text.rfind('n').unwrap();
        assert_eq!(word_at(text, n), Some((n, n + 1)));
    }

    #[test]
    fn exact_entry_prefers_binding_over_unit_expr() {
        let source = "let x = 1";
        let index = vec![
            entry(source, 4, 5, Type::Unit, EntryKind::Expr),
            entry(source, 4, 5, Type::Int, EntryKind::Binding("x".into())),
        ];
        let found = exact_entry(&index, 4, 5).unwrap();
        assert!(matches!(found.kind, EntryKind::Binding(_)));
        assert!(matches!(found.ty, Type::Int));
    }

    #[test]
    fn containing_entry_picks_smallest_span() {
        let source = "1 + 2";
        let index = vec![
            entry(source, 0, 5, Type::Int, EntryKind::Expr),
            entry(source, 4, 5, Type::Int, EntryKind::Expr),
        ];
        // On `+`: only the whole binary expression contains it.
        let on_op = containing_entry(&index, 2).unwrap();
        assert_eq!((on_op.span.start, on_op.span.end), (0, 5));
        // On `2`: the literal is smaller than the binary expression.
        let on_lit = containing_entry(&index, 4).unwrap();
        assert_eq!((on_lit.span.start, on_lit.span.end), (4, 5));
    }
}
