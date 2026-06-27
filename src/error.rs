use std::fmt;
use std::sync::Arc;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub(crate) struct Error {
    message: String,
    diagnostic: Option<Diagnostic>,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            diagnostic: None,
        }
    }

    pub(crate) fn diagnostic(diagnostic: Diagnostic) -> Self {
        Self {
            message: diagnostic.title.clone(),
            diagnostic: Some(diagnostic),
        }
    }

    pub(crate) fn render(&self) -> String {
        if let Some(diagnostic) = &self.diagnostic {
            diagnostic.render()
        } else {
            self.message.clone()
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SourceFile {
    pub(crate) label: String,
    pub(crate) source: Arc<str>,
}

impl SourceFile {
    pub(crate) fn new(label: impl Into<String>, source: impl Into<Arc<str>>) -> Arc<Self> {
        Arc::new(Self {
            label: label.into(),
            source: source.into(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Span {
    pub(crate) file: Arc<SourceFile>,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Span {
    pub(crate) fn new(file: Arc<SourceFile>, start: usize, end: usize) -> Self {
        Self { file, start, end }
    }

    pub(crate) fn point(file: Arc<SourceFile>, offset: usize) -> Self {
        Self {
            file,
            start: offset,
            end: offset.saturating_add(1),
        }
    }

    pub(crate) fn merge(&self, other: &Span) -> Span {
        if Arc::ptr_eq(&self.file, &other.file) {
            Span {
                file: self.file.clone(),
                start: self.start.min(other.start),
                end: self.end.max(other.end),
            }
        } else {
            self.clone()
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Diagnostic {
    title: String,
    message: Option<String>,
    primary: Option<Label>,
    help: Option<String>,
}

#[derive(Debug, Clone)]
struct Label {
    span: Span,
    message: String,
}

impl Diagnostic {
    pub(crate) fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: None,
            primary: None,
            help: None,
        }
    }

    pub(crate) fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub(crate) fn label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.primary = Some(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub(crate) fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("error: ");
        out.push_str(&self.title);
        out.push('\n');
        if let Some(message) = &self.message {
            out.push('\n');
            out.push_str(message);
            out.push('\n');
        }
        if let Some(label) = &self.primary {
            out.push('\n');
            out.push_str(&render_label(label));
        }
        if let Some(help) = &self.help {
            out.push('\n');
            out.push_str("Hint: ");
            out.push_str(help);
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

fn render_label(label: &Label) -> String {
    let location = line_location(&label.span);
    let line_number_width = location.line.to_string().len();
    let underline_len = label.span.end.saturating_sub(label.span.start).max(1).min(
        location
            .line_text
            .len()
            .saturating_sub(location.column0)
            .max(1),
    );
    let mut out = String::new();
    out.push_str(&format!(
        "  --> {}:{}:{}\n",
        label.span.file.label, location.line, location.column
    ));
    out.push_str(&format!("{:>width$} |\n", "", width = line_number_width));
    out.push_str(&format!(
        "{:>width$} | {}\n",
        location.line,
        location.line_text,
        width = line_number_width
    ));
    out.push_str(&format!(
        "{:>width$} | {}{} {}\n",
        "",
        " ".repeat(location.column0),
        "^".repeat(underline_len),
        label.message,
        width = line_number_width
    ));
    out
}

struct LineLocation {
    line: usize,
    column: usize,
    column0: usize,
    line_text: String,
}

fn line_location(span: &Span) -> LineLocation {
    let source = span.file.source.as_ref();
    let start = span.start.min(source.len());
    let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = source[start..]
        .find('\n')
        .map_or(source.len(), |index| start + index);
    let line = source[..line_start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let column0 = source[line_start..start].chars().count();
    LineLocation {
        line,
        column: column0 + 1,
        column0,
        line_text: source[line_start..line_end].to_string(),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render())
    }
}

impl std::error::Error for Error {}
