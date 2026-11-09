//! Syntax-only links from a source to the host files its `@external` annotations name
//! (spec `[lsp.document-link]`).

use std::path::{Path, PathBuf};

use emela_syntax::ast::{self, AstNode};
use emela_syntax::{SyntaxKind, SyntaxNode};
use line_index::{TextRange, TextSize};

use crate::source::FileSystem;

/// The contents of a string literal (quotes excluded) that names a host file, and that file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLink {
    pub range: TextRange,
    /// Absolute, as `FileSystem::absolute` gives it.
    pub target: PathBuf,
}

/// The file arguments of `@external(target, "./file", "name")` in `text`, the contents of the
/// source at `path`, in source order.
///
/// Built from the syntax tree alone, so a file with syntax or type errors still has links. Only
/// the three-argument form names a file, and only when the string starts with `./` or `../`
/// (spec `[ext.decl]`); `@export` and `@entry` take no file. A path that does not lead to an
/// existing file gives no link, so that the editor never offers to open something the compiler
/// would not find. A string with an escape or an interpolation gives no link either: its value
/// is not the text as written, so no range of the source holds exactly the path.
pub fn document_links(fs: &dyn FileSystem, path: &Path, text: &str) -> Vec<DocumentLink> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let root = emela_syntax::parse(text).syntax();
    root.descendants()
        .filter_map(ast::Annotation::cast)
        .filter(|annotation| annotation.name().is_some_and(|n| n.text() == "external"))
        .filter_map(|annotation| {
            let args: Vec<SyntaxNode> = annotation.args().collect();
            let [_, file, _] = args.as_slice() else {
                return None;
            };
            let (range, value) = string_contents(file)?;
            if !(value.starts_with("./") || value.starts_with("../")) {
                return None;
            }
            let target = fs.absolute(&dir.join(&value)).ok()?;
            fs.is_file(&target)
                .then_some(DocumentLink { range, target })
        })
        .collect()
}

/// The range and value of the string that is the whole of an annotation argument, when the
/// value is the text as written: no escapes and no interpolation.
///
/// In a multiline string the value is one line: the newline after the opening `"""` and the one
/// before the closing `"""` are dropped, and the indentation of the closing `"""` is removed
/// from the start of the line, as for any multiline string (spec 2.4). A value of several lines
/// cannot be a path, so it gives `None`.
fn string_contents(arg: &SyntaxNode) -> Option<(TextRange, String)> {
    use SyntaxKind::*;
    let mut nodes = arg.children();
    let string = nodes.next().filter(|n| n.kind() == STRING)?;
    if nodes.next().is_some() {
        return None;
    }
    let tokens: Vec<_> = string
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    let (open, text, close) = match tokens.as_slice() {
        [open, text, close] if text.kind() == STRING_TEXT => (open, text, close),
        _ => return None,
    };
    let raw = text.text();
    let start = text.text_range().start();
    let (offset, value) = match (open.kind(), close.kind()) {
        (STRING_QUOTE, STRING_QUOTE) => (0, raw),
        (TRIPLE_QUOTE, TRIPLE_QUOTE) => {
            let line = raw
                .strip_prefix("\r\n")
                .or_else(|| raw.strip_prefix('\n'))
                .unwrap_or(raw);
            let skipped = raw.len() - line.len();
            let line = line
                .strip_suffix("\r\n")
                .or_else(|| line.strip_suffix('\n'))
                .unwrap_or(line);
            // The closing `"""` holds the whitespace before it on its line.
            let indent = close.text().strip_suffix("\"\"\"").unwrap_or("");
            match line.strip_prefix(indent) {
                Some(rest) => (skipped + indent.len(), rest),
                None => (skipped, line),
            }
        }
        _ => return None,
    };
    if value.contains(['\\', '\n', '\r']) {
        return None;
    }
    let start = start + TextSize::try_from(offset).ok()?;
    let len = TextSize::try_from(value.len()).ok()?;
    Some((TextRange::at(start, len), value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryFiles;

    fn links(text: &str) -> Vec<(String, PathBuf)> {
        let fs = MemoryFiles::new([
            ("/p/src/main.emel", ""),
            ("/p/src/ffi.mjs", ""),
            ("/p/host/x.c", ""),
        ]);
        document_links(&fs, Path::new("/p/src/main.emel"), text)
            .into_iter()
            .map(|link| (text[link.range].to_owned(), link.target))
            .collect()
    }

    #[test]
    fn file_arguments_of_external() {
        assert_eq!(
            links(
                "@external(js, \"./ffi.mjs\", \"f\")\n@external(c, \"../host/x.c\", \"f\")\nfn f() -> Int\n"
            ),
            [
                ("./ffi.mjs".to_owned(), PathBuf::from("/p/src/ffi.mjs")),
                ("../host/x.c".to_owned(), PathBuf::from("/p/host/x.c")),
            ]
        );
    }

    #[test]
    fn multiline_string_with_crlf() {
        assert_eq!(
            links(
                "@external(js, \"\"\"\r\n    ./ffi.mjs\r\n    \"\"\", \"f\")\r\nfn f() -> Int\r\n"
            ),
            [("./ffi.mjs".to_owned(), PathBuf::from("/p/src/ffi.mjs"))]
        );
    }

    #[test]
    fn no_link_without_a_file() {
        for text in [
            "@external(js, \"./none.mjs\", \"f\")\nfn f() -> Int\n",
            "@external(js, \"ffi.mjs\", \"f\")\nfn f() -> Int\n",
            "@external(js, \"./ffi.mjs\")\nfn f() -> Int\n",
            "@external(js, \"./ff\\u{69}.mjs\", \"f\")\nfn f() -> Int\n",
            "@export(js, \"./ffi.mjs\", \"f\")\nfn f() -> Int { 1 }\n",
            "@external(js, \"\"\"\n  ./ffi.mjs\n  ./ffi.mjs\n  \"\"\", \"f\")\nfn f() -> Int\n",
        ] {
            assert_eq!(links(text), [], "{text}");
        }
    }
}
