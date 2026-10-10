//! driver の診断と LSP の診断の変換，パスと URI の変換．

use std::path::{Path, PathBuf};
use std::str::FromStr;

use emela_driver::{Diagnostic, Location, Severity, SourceDb, Span};
use line_index::{LineIndex, TextSize, WideEncoding};
use lsp_types as lsp;

/// 診断に付ける `source`．
pub const SOURCE: &str = "emela";

/// 主な位置のファイルのパスと，LSP の診断の組．位置のない診断は `anchor` に付ける．
pub fn diagnostic(
    diagnostic: &Diagnostic,
    sources: &SourceDb,
    anchor: &Path,
    uri_of: &dyn Fn(&Path) -> Option<lsp::Uri>,
) -> (PathBuf, lsp::Diagnostic) {
    let (path, range) = match &diagnostic.location {
        Some(Location::Span(span)) => (
            sources[span.file].path().to_owned(),
            span_range(sources, span),
        ),
        Some(Location::File(file)) => (sources[*file].path().to_owned(), lsp::Range::default()),
        Some(Location::Path(path)) => (path.clone(), lsp::Range::default()),
        None => (anchor.to_owned(), lsp::Range::default()),
    };
    let mut message = diagnostic.message.clone();
    for note in &diagnostic.notes {
        message.push_str("\nnote: ");
        message.push_str(note);
    }
    let related: Vec<_> = diagnostic
        .labels
        .iter()
        .filter_map(|label| {
            Some(lsp::DiagnosticRelatedInformation {
                location: lsp::Location {
                    uri: uri_of(sources[label.span.file].path())?,
                    range: span_range(sources, &label.span),
                },
                message: label.message.clone(),
            })
        })
        .collect();
    let lsp = lsp::Diagnostic {
        range,
        severity: Some(match diagnostic.severity {
            Severity::Error => lsp::DiagnosticSeverity::ERROR,
            Severity::Warning => lsp::DiagnosticSeverity::WARNING,
        }),
        code: diagnostic
            .code
            .map(|code| lsp::NumberOrString::String(code.to_owned())),
        source: Some(SOURCE.to_owned()),
        message,
        related_information: (!related.is_empty()).then_some(related),
        ..lsp::Diagnostic::default()
    };
    (path, lsp)
}

fn span_range(sources: &SourceDb, span: &Span) -> lsp::Range {
    let index = sources[span.file].line_index();
    lsp::Range {
        start: position(index, span.range.start()),
        end: position(index, span.range.end()),
    }
}

/// バイト位置を LSP の既定の位置（行と UTF-16 の列）にする．
pub fn position(index: &LineIndex, offset: TextSize) -> lsp::Position {
    let offset = offset.min(index.len());
    let line_col = index.line_col(offset);
    let col = index
        .to_wide(WideEncoding::Utf16, line_col)
        .map_or(line_col.col, |wide| wide.col);
    lsp::Position {
        line: line_col.line,
        character: col,
    }
}

/// `file:` の URI をパスにする．ほかの scheme なら `None`．
pub fn uri_to_path(uri: &lsp::Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // `file://localhost/…` と `file:///…` を受ける．
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let bytes = percent_decode(path)?;
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// 絶対パスを `file:` の URI にする．英数字と `-._~/` 以外は `%` で符号化する（VS Code と同じ）．
pub fn path_to_uri(path: &Path) -> Option<lsp::Uri> {
    let path = path.to_str()?;
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    lsp::Uri::from_str(&uri).ok()
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_utf16() {
        // `あ` は UTF-8 で3バイト，UTF-16 で1単位．`😀` は4バイト，2単位．
        let text = "let あ😀 = $\n";
        let index = LineIndex::new(text);
        let dollar = text.find('$').unwrap() as u32;
        assert_eq!(
            position(&index, dollar.into()),
            lsp::Position {
                line: 0,
                character: 10
            }
        );
        assert_eq!(
            position(&index, (text.len() as u32).into()),
            lsp::Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn labels_notes_and_locations() {
        use line_index::TextRange;

        let mut sources = SourceDb::new();
        let a = sources.add("/p/src/a.emel", "let 日 = 1\n");
        let b = sources.add("/p/src/b.emel", "x\ny\n");
        let span =
            |file, start: u32, end: u32| Span::new(file, TextRange::new(start.into(), end.into()));
        let uri_of = |path: &Path| path_to_uri(path);
        let d = Diagnostic::warning("w")
            .with_code("W0301")
            .with_span(span(a, 4, 7))
            .with_label(span(b, 2, 3), "here")
            .with_note("n1")
            .with_note("n2");
        let (path, lsp) = diagnostic(&d, &sources, Path::new("/anchor"), &uri_of);
        assert_eq!(path, Path::new("/p/src/a.emel"));
        assert_eq!(lsp.range.start, lsp::Position::new(0, 4));
        assert_eq!(lsp.range.end, lsp::Position::new(0, 5));
        assert_eq!(lsp.severity, Some(lsp::DiagnosticSeverity::WARNING));
        assert_eq!(lsp.code, Some(lsp::NumberOrString::String("W0301".into())));
        assert_eq!(lsp.message, "w\nnote: n1\nnote: n2");
        assert_eq!(
            lsp.related_information,
            Some(vec![lsp::DiagnosticRelatedInformation {
                location: lsp::Location {
                    uri: lsp::Uri::from_str("file:///p/src/b.emel").unwrap(),
                    range: lsp::Range::new(lsp::Position::new(1, 0), lsp::Position::new(1, 1)),
                },
                message: "here".into(),
            }])
        );

        let whole = Diagnostic::error("e").at(Location::File(b));
        let (path, lsp) = diagnostic(&whole, &sources, Path::new("/anchor"), &uri_of);
        assert_eq!(
            (path.as_path(), lsp.range),
            (Path::new("/p/src/b.emel"), lsp::Range::default())
        );
        assert_eq!((lsp.code, lsp.related_information), (None, None));
        let dir = Diagnostic::error("e").at(Location::Path("/p/src/Bad".into()));
        assert_eq!(
            diagnostic(&dir, &sources, Path::new("/anchor"), &uri_of).0,
            Path::new("/p/src/Bad")
        );
        let nowhere = Diagnostic::error("e");
        assert_eq!(
            diagnostic(&nowhere, &sources, Path::new("/anchor"), &uri_of).0,
            Path::new("/anchor")
        );
    }

    #[test]
    fn uri_round_trip() {
        let path = Path::new("/tmp/my project (1)/日本.emel");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(
            uri.as_str(),
            "file:///tmp/my%20project%20%281%29/%E6%97%A5%E6%9C%AC.emel"
        );
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        let localhost = lsp::Uri::from_str("file://localhost/a/b.emel").unwrap();
        assert_eq!(uri_to_path(&localhost).unwrap(), Path::new("/a/b.emel"));
        let untitled = lsp::Uri::from_str("untitled:Untitled-1").unwrap();
        assert_eq!(uri_to_path(&untitled), None);
    }
}
