//! 診断を端末向けの文字列にする（ariadne）．

use std::ops::Range;

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind};

use crate::diagnostic::{Diagnostic, HELP_PREFIX, Location, Severity, Span};
use crate::source::SourceDb;

/// 診断を全部並べ，最後に件数をまとめる．診断の間は空行で区切る．
pub fn render(diagnostics: &[Diagnostic], sources: &SourceDb, color: bool) -> String {
    let mut out = String::new();
    for (i, diagnostic) in diagnostics.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&render_one(diagnostic, sources, color));
    }
    if let Some(summary) = summary(diagnostics) {
        if !diagnostics.is_empty() {
            out.push('\n');
        }
        out.push_str(&summary);
        out.push('\n');
    }
    out
}

/// `2 errors, 1 warning`．1件なら単数形．どちらもなければ `None`．
pub fn summary(diagnostics: &[Diagnostic]) -> Option<String> {
    let errors = diagnostics.iter().filter(|d| d.is_error()).count();
    let warnings = diagnostics.len() - errors;
    let count = |n: usize, word: &str| {
        if n == 1 {
            format!("1 {word}")
        } else {
            format!("{n} {word}s")
        }
    };
    match (errors, warnings) {
        (0, 0) => None,
        (e, 0) => Some(count(e, "error")),
        (0, w) => Some(count(w, "warning")),
        (e, w) => Some(format!("{}, {}", count(e, "error"), count(w, "warning"))),
    }
}

/// 1件の診断．
pub fn render_one(diagnostic: &Diagnostic, sources: &SourceDb, color: bool) -> String {
    // 見出しは rustc と同じ `error[E0204]`．コードのない診断は `error` だけ．
    let (word, kind_color) = match diagnostic.severity {
        Severity::Error => ("error", Color::Red),
        Severity::Warning => ("warning", Color::Yellow),
    };
    let heading = match diagnostic.code {
        Some(code) => format!("{word}[{code}]"),
        None => word.to_owned(),
    };
    let kind = ReportKind::Custom(&heading, kind_color);
    let config = Config::default()
        .with_color(color)
        .with_index_type(IndexType::Byte);

    let span = |span: Span| -> (String, Range<usize>) {
        let path = sources
            .display_path(sources[span.file].path())
            .display()
            .to_string();
        (path, span.range.start().into()..span.range.end().into())
    };
    // ソースの範囲を持たない診断は，ariadne の見出しの後にパスだけを添える．
    let (report_span, path_only) = match &diagnostic.location {
        Some(Location::Span(s)) => (span(*s), None),
        Some(Location::File(file)) => (
            span(Span::new(*file, Default::default())),
            Some(
                sources
                    .display_path(sources[*file].path())
                    .display()
                    .to_string(),
            ),
        ),
        Some(Location::Path(path)) => (
            (String::new(), 0..0),
            Some(sources.display_path(path).display().to_string()),
        ),
        None => ((String::new(), 0..0), None),
    };

    let mut builder = Report::build(kind, report_span.clone())
        .with_config(config)
        .with_message(&diagnostic.message);
    // ariadne は渡した順に並べ，位置が戻ると枠を分けるので，ファイルと位置の順に並べて渡す．
    let mut labels: Vec<(Span, Option<&str>)> = Vec::new();
    if let Some(Location::Span(primary)) = diagnostic.location {
        labels.push((primary, None));
    }
    labels.extend(
        diagnostic
            .labels
            .iter()
            .map(|label| (label.span, Some(label.message.as_str()))),
    );
    labels.sort_by_key(|(span, _)| (span.file, span.range.start(), span.range.end()));
    for (label_span, message) in labels {
        let label = Label::new(span(label_span));
        builder.add_label(match message {
            Some(message) => label.with_message(message).with_color(Color::Blue),
            // 文言のないラベルは色なしでは見えないので，空の文言を付けて下線を引かせる．
            None => label
                .with_message("")
                .with_color(match diagnostic.severity {
                    Severity::Error => Color::Red,
                    Severity::Warning => Color::Yellow,
                }),
        });
    }

    let cache = ariadne::sources(sources.iter().map(|(_, file)| {
        let path = sources.display_path(file.path()).display().to_string();
        (path, file.text().to_owned())
    }));
    let mut buf = Vec::new();
    builder
        .finish()
        .write(cache, &mut buf)
        .expect("Vec への書き込みは失敗しない");
    let mut out = String::from_utf8(buf).expect("ariadne は UTF-8 を出す");
    if !color {
        // ariadne 0.6 は `ReportKind::Custom` の色を `with_color(false)` でも付けるので取り除く．
        out = strip_ansi(&out);
    }
    if let Some(path) = path_only {
        out = insert_after_first_line(&out, &format!("   ─[ {path} ]\n"));
    }
    // ariadne は注記をソースの枠の中にしか出さないので，枠の後に自分で並べる．
    for note in &diagnostic.notes {
        match note.strip_prefix(HELP_PREFIX) {
            Some(help) => out.push_str(&format!("   = help: {help}\n")),
            None => out.push_str(&format!("   = note: {note}\n")),
        }
    }
    out
}

fn insert_after_first_line(text: &str, insert: &str) -> String {
    match text.find('\n') {
        Some(i) => format!("{}{insert}{}", &text[..=i], &text[i + 1..]),
        None => format!("{text}\n{insert}"),
    }
}

/// `ESC [ ... m` の形の色指定を取り除く．
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
