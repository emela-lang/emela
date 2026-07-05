//! Byte-offset ↔ LSP position conversion (spec 0033). Compiler spans are byte
//! offsets into the source; LSP positions are zero-based lines and UTF-16
//! code-unit columns. Everything clamps rather than panics, since editors can
//! send positions for text the server hasn't seen yet.

use crate::lsp::protocol::Position;

/// The LSP position of `offset` in `source`. An offset past the end (or inside
/// a multi-byte character) is clamped to the nearest boundary before it.
pub(crate) fn offset_to_position(source: &str, offset: usize) -> Position {
    let mut offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &source[..offset];
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let character = before[line_start..]
        .chars()
        .map(char::len_utf16)
        .sum::<usize>();
    Position {
        line: line as u32,
        character: character as u32,
    }
}

/// The byte offset of `position` in `source`. A line past the end clamps to
/// the end of the text; a column past the end of its line clamps to the end
/// of that line.
pub(crate) fn position_to_offset(source: &str, position: &Position) -> usize {
    let mut offset = 0;
    for _ in 0..position.line {
        match source[offset..].find('\n') {
            Some(index) => offset += index + 1,
            None => return source.len(),
        }
    }
    let mut units = 0usize;
    for ch in source[offset..].chars() {
        if ch == '\n' || units >= position.character as usize {
            break;
        }
        units += ch.len_utf16();
        offset += ch.len_utf8();
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_round_trip() {
        let source = "fn main() -> Int {\n  42\n}\n";
        let offset = source.find("42").unwrap();
        let position = offset_to_position(source, offset);
        assert_eq!((position.line, position.character), (1, 2));
        assert_eq!(position_to_offset(source, &position), offset);
    }

    #[test]
    fn multibyte_counts_utf16_units() {
        // Japanese characters are 3 bytes but 1 UTF-16 unit each.
        let source = "let x = \"こんにちは\"\nlet y = 1\n";
        let offset = source.find('ち').unwrap();
        let position = offset_to_position(source, offset);
        assert_eq!((position.line, position.character), (0, 12));
        assert_eq!(position_to_offset(source, &position), offset);
    }

    #[test]
    fn emoji_counts_as_two_units() {
        // Astral-plane characters are 4 bytes and 2 UTF-16 units.
        let source = "\u{1F600}x";
        let position = offset_to_position(source, 4);
        assert_eq!((position.line, position.character), (0, 2));
        assert_eq!(position_to_offset(source, &position), 4);
    }

    #[test]
    fn clamps_out_of_range() {
        let source = "ab\ncd";
        assert_eq!(offset_to_position(source, 999).line, 1);
        let past_line = Position {
            line: 9,
            character: 0,
        };
        assert_eq!(position_to_offset(source, &past_line), source.len());
        let past_column = Position {
            line: 0,
            character: 99,
        };
        // Clamps to the end of line 0, before the newline.
        assert_eq!(position_to_offset(source, &past_column), 2);
    }
}
