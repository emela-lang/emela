//! Syntax-only folding and selection ranges, including incomplete programs.

use emela_syntax::{SyntaxKind, SyntaxNode};
use line_index::{LineIndex, TextRange, TextSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Region,
    Imports,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRange {
    pub range: TextRange,
    pub kind: FoldKind,
}

/// Multiline bodies and consecutive imports and comments. Blank lines break runs.
pub fn folding_ranges(text: &str) -> Vec<FoldRange> {
    use SyntaxKind::*;
    let root = emela_syntax::parse(text).syntax();
    let index = LineIndex::new(text);
    let mut ranges = Vec::new();
    for node in root.descendants() {
        if matches!(
            node.kind(),
            BLOCK_EXPR | MATCH_ARM_LIST | VARIANT_LIST | ITEM_LIST
        ) || (node.kind() == FIELD_LIST
            && node
                .parent()
                .is_some_and(|p| matches!(p.kind(), TYPE_DECL | ERROR_DECL | HANDLER_DECL)))
        {
            ranges.push(FoldRange {
                range: node.text_range(),
                kind: FoldKind::Region,
            });
        }
    }
    let imports: Vec<_> = root
        .children()
        .filter(|n| n.kind() == IMPORT)
        .filter_map(|n| {
            let keyword = n.children_with_tokens().find(|e| e.kind() == IMPORT_KW)?;
            Some(TextRange::new(
                keyword.text_range().start(),
                n.text_range().end(),
            ))
        })
        .collect();
    runs(text, &index, imports, FoldKind::Imports, &mut ranges);
    let comments: Vec<_> = root
        .descendants_with_tokens()
        .filter(|e| matches!(e.kind(), COMMENT | DOC_COMMENT))
        .map(|e| e.text_range())
        .collect();
    runs(text, &index, comments, FoldKind::Comment, &mut ranges);
    ranges.retain(|r| index.line_col(r.range.start()).line < index.line_col(r.range.end()).line);
    ranges.sort_by_key(|r| (r.range.start(), r.range.end()));
    ranges.dedup();
    ranges
}

fn runs(
    text: &str,
    index: &LineIndex,
    ranges: Vec<TextRange>,
    kind: FoldKind,
    out: &mut Vec<FoldRange>,
) {
    let mut run: Option<TextRange> = None;
    let mut count = 0;
    for range in ranges {
        if let Some(previous) = run {
            let gap = TextRange::new(previous.end(), range.start());
            if index.line_col(range.start()).line == index.line_col(previous.end()).line + 1
                && text[std::ops::Range::<usize>::from(gap)]
                    .chars()
                    .all(char::is_whitespace)
            {
                run = Some(TextRange::new(previous.start(), range.end()));
                count += 1;
                continue;
            }
            if count > 1 {
                out.push(FoldRange {
                    range: previous,
                    kind,
                });
            }
        }
        run = Some(range);
        count = 1;
    }
    if let Some(range) = run
        && count > 1
    {
        out.push(FoldRange { range, kind });
    }
}

/// Token then enclosing syntax nodes, innermost first, without duplicate ranges.
/// At token boundaries prefer the token to the right, except for trivia.
pub fn selection_ranges(text: &str, offsets: &[TextSize]) -> Vec<Vec<TextRange>> {
    let root = emela_syntax::parse(text).syntax();
    offsets
        .iter()
        .map(|&offset| selection(&root, offset))
        .collect()
}

fn selection(root: &SyntaxNode, offset: TextSize) -> Vec<TextRange> {
    if !root.text_range().contains_inclusive(offset) {
        return Vec::new();
    }
    let tokens: Vec<_> = root.token_at_offset(offset).collect();
    let token = tokens
        .iter()
        .rev()
        .find(|t| !t.kind().is_trivia())
        .or_else(|| tokens.last());
    let mut ranges = match token {
        Some(token) => std::iter::once(token.text_range())
            .chain(token.parent_ancestors().map(|n| n.text_range()))
            .collect(),
        None => vec![root.text_range()],
    };
    ranges.dedup();
    ranges
}
