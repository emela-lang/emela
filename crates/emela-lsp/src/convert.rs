//! Conversions between driver and LSP diagnostics, and between paths and URIs.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use emela_driver::{Diagnostic, Location, Severity, SourceDb, Span, SymbolKind};
use line_index::{LineIndex, TextRange, TextSize, WideEncoding};
use lsp_types as lsp;

pub const SOURCE: &str = "emela";

pub fn code_action_kind(kind: emela_driver::CodeActionKind) -> lsp::CodeActionKind {
    match kind {
        emela_driver::CodeActionKind::QuickFix => lsp::CodeActionKind::QUICKFIX,
        emela_driver::CodeActionKind::RefactorRewrite => lsp::CodeActionKind::REFACTOR_REWRITE,
    }
}

/// Pairs the LSP diagnostic with the file of its primary location. Diagnostics without a location
/// attach to `anchor`.
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
    range(sources[span.file].line_index(), span.range)
}

#[allow(deprecated)] // `deprecated` is a required field.
pub fn document_symbol(
    index: &LineIndex,
    symbol: &emela_driver::DocumentSymbol,
) -> lsp::DocumentSymbol {
    lsp::DocumentSymbol {
        name: symbol.name.clone(),
        detail: None,
        kind: symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        range: range(index, symbol.range),
        selection_range: range(index, symbol.selection),
        children: (!symbol.children.is_empty()).then(|| {
            symbol
                .children
                .iter()
                .map(|child| document_symbol(index, child))
                .collect()
        }),
    }
}

pub(crate) fn symbol_kind(kind: SymbolKind) -> lsp::SymbolKind {
    match kind {
        SymbolKind::Fn => lsp::SymbolKind::FUNCTION,
        SymbolKind::Type | SymbolKind::Error => lsp::SymbolKind::STRUCT,
        SymbolKind::Enum => lsp::SymbolKind::ENUM,
        SymbolKind::Variant => lsp::SymbolKind::ENUM_MEMBER,
        SymbolKind::Const => lsp::SymbolKind::CONSTANT,
        SymbolKind::Effect | SymbolKind::Trait => lsp::SymbolKind::INTERFACE,
        SymbolKind::Op | SymbolKind::Method => lsp::SymbolKind::METHOD,
        SymbolKind::Handler => lsp::SymbolKind::CLASS,
        SymbolKind::Layer | SymbolKind::Impl => lsp::SymbolKind::NAMESPACE,
        SymbolKind::Field => lsp::SymbolKind::FIELD,
    }
}

/// `index` is the driver's ordering; it becomes `sortText` to preserve that order.
pub fn completion_item(index: usize, item: &emela_driver::Completion) -> lsp::CompletionItem {
    use emela_driver::CompletionKind as K;
    let kind = match item.kind {
        K::Local => lsp::CompletionItemKind::VARIABLE,
        K::Fn => lsp::CompletionItemKind::FUNCTION,
        K::Const => lsp::CompletionItemKind::CONSTANT,
        K::Ctor => lsp::CompletionItemKind::CONSTRUCTOR,
        K::Type | K::Error => lsp::CompletionItemKind::STRUCT,
        K::Enum => lsp::CompletionItemKind::ENUM,
        K::Effect | K::Trait => lsp::CompletionItemKind::INTERFACE,
        K::Handler | K::Layer => lsp::CompletionItemKind::CLASS,
        K::Method => lsp::CompletionItemKind::METHOD,
        K::Field => lsp::CompletionItemKind::FIELD,
        K::Module => lsp::CompletionItemKind::MODULE,
        K::Keyword => lsp::CompletionItemKind::KEYWORD,
        K::Label => lsp::CompletionItemKind::PROPERTY,
    };
    lsp::CompletionItem {
        label: item.label.clone(),
        kind: Some(kind),
        detail: item.detail.clone(),
        documentation: item.documentation.as_deref().map(documentation),
        sort_text: Some(format!("{index:05}")),
        insert_text: item.insert_text.clone(),
        ..lsp::CompletionItem::default()
    }
}

fn documentation(text: &str) -> lsp::Documentation {
    lsp::Documentation::MarkupContent(lsp::MarkupContent {
        kind: lsp::MarkupKind::Markdown,
        value: text.to_owned(),
    })
}

/// Parameter ranges are UTF-16 offsets into the label.
pub fn signature_help(info: &emela_driver::SignatureInfo) -> lsp::SignatureHelp {
    let utf16 = |byte: usize| info.label[..byte].encode_utf16().count() as u32;
    let parameters = info
        .params
        .iter()
        .map(|r| lsp::ParameterInformation {
            label: lsp::ParameterLabel::LabelOffsets([utf16(r.start), utf16(r.end)]),
            documentation: None,
        })
        .collect();
    let active = info.active.map(|i| i as u32);
    lsp::SignatureHelp {
        signatures: vec![lsp::SignatureInformation {
            label: info.label.clone(),
            documentation: info.documentation.as_deref().map(documentation),
            parameters: Some(parameters),
            active_parameter: active,
        }],
        active_signature: Some(0),
        active_parameter: active,
    }
}

/// Type hints start with `:`, so they get no left padding; other hints do.
pub fn inlay_hint(index: &LineIndex, hint: &emela_driver::InlayHint) -> lsp::InlayHint {
    use emela_driver::HintKind;
    let kind = match hint.kind {
        HintKind::Type | HintKind::Signature => lsp::InlayHintKind::TYPE,
        HintKind::Effect => lsp::InlayHintKind::PARAMETER,
    };
    lsp::InlayHint {
        position: position(index, hint.offset),
        label: lsp::InlayHintLabel::String(hint.label.clone()),
        kind: Some(kind),
        text_edits: None,
        tooltip: None,
        padding_left: Some(hint.kind != HintKind::Type),
        padding_right: None,
        data: None,
    }
}

/// The modifier on capability values. Not one of LSP's predefined modifiers.
pub const CAPABILITY_MODIFIER: &str = "capability";

/// Token types and modifiers, indexed by [`semantic_tokens`].
pub fn semantic_tokens_legend() -> lsp::SemanticTokensLegend {
    lsp::SemanticTokensLegend {
        token_types: vec![lsp::SemanticTokenType::VARIABLE],
        token_modifiers: vec![lsp::SemanticTokenModifier::new(CAPABILITY_MODIFIER)],
    }
}

/// Encodes tokens in position order as relative positions (UTF-16). A token does not span lines,
/// since each is a name.
pub fn semantic_tokens(
    index: &LineIndex,
    tokens: &[emela_driver::SemanticToken],
) -> lsp::SemanticTokens {
    use emela_driver::TokenKind;
    let mut data = Vec::with_capacity(tokens.len());
    let mut last = lsp::Position::new(0, 0);
    for token in tokens {
        let start = position(index, token.range.start());
        let end = position(index, token.range.end());
        if end.line != start.line {
            continue;
        }
        let (token_type, token_modifiers_bitset) = match token.kind {
            TokenKind::Capability => (0, 1),
        };
        data.push(lsp::SemanticToken {
            delta_line: start.line - last.line,
            delta_start: if start.line == last.line {
                start.character - last.character
            } else {
                start.character
            },
            length: end.character - start.character,
            token_type,
            token_modifiers_bitset,
        });
        last = start;
    }
    lsp::SemanticTokens {
        result_id: None,
        data,
    }
}

pub fn range(index: &LineIndex, range: TextRange) -> lsp::Range {
    lsp::Range {
        start: position(index, range.start()),
        end: position(index, range.end()),
    }
}

pub fn whole_range(text: &str) -> lsp::Range {
    let index = LineIndex::new(text);
    lsp::Range {
        start: lsp::Position::new(0, 0),
        end: position(&index, index.len()),
    }
}

/// Converts to LSP's default position encoding (line and UTF-16 column).
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

/// `None` if out of range.
pub fn offset(index: &LineIndex, position: lsp::Position) -> Option<TextSize> {
    let wide = line_index::WideLineCol {
        line: position.line,
        col: position.character,
    };
    let line_col = index.to_utf8(WideEncoding::Utf16, wide)?;
    index.offset(line_col)
}

/// `None` for schemes other than `file:`.
pub fn uri_to_path(uri: &lsp::Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // Accept both `file://localhost/…` and `file:///…`.
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let bytes = percent_decode(path)?;
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// Percent-encodes everything except alphanumerics and `-._~/`, as VS Code does.
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
        // `あ` is 3 bytes in UTF-8 and 1 unit in UTF-16; `😀` is 4 bytes and 2 units.
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
