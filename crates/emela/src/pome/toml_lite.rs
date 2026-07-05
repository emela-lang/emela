//! A deliberately small TOML reader for the packaging manifests defined in
//! spec 0032 (`Pome.toml`, `Pome.lock`, `Bushel.toml`).
//!
//! Emela keeps its dependency surface tiny (see `Cargo.toml`), so rather than
//! pull in a full TOML crate we hand-roll a reader that covers exactly the
//! subset these files use: top-level tables (`[pome]`), arrays of tables
//! (`[[package]]`), bare or quoted keys, string values, and arrays of strings.
//! Writing is done by the individual file modules, which build their output
//! explicitly for deterministic encoding (spec 0032 F7); this module only
//! parses, plus [`escape`]/[`quote`] helpers the writers share.
//!
//! This is not a conforming TOML parser and intentionally rejects constructs
//! the manifests never contain (integers, booleans, inline tables, dotted
//! keys). Anything unexpected is a hard error so a malformed manifest is caught
//! rather than silently misread.

use std::fmt;

/// A parsed value. The manifests only ever hold strings and arrays of strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    String(String),
    Array(Vec<String>),
}

impl Value {
    fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(value) => Some(value),
            Value::Array(_) => None,
        }
    }

    fn as_array(&self) -> Option<&[String]> {
        match self {
            Value::Array(items) => Some(items),
            Value::String(_) => None,
        }
    }
}

/// A TOML table: an ordered list of `key = value` pairs. Order is preserved so
/// callers that iterate (e.g. `[dependencies]`) see a stable sequence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Table {
    entries: Vec<(String, Value)>,
}

impl Table {
    fn insert(&mut self, key: String, value: Value) {
        self.entries.push((key, value));
    }

    pub(crate) fn get_string(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, value)| value.as_string())
    }

    pub(crate) fn get_array(&self, key: &str) -> Option<&[String]> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, value)| value.as_array())
    }

    /// All `key = "string"` entries, in file order. Used to read the
    /// `[dependencies]` table where keys are canonical source paths.
    pub(crate) fn string_entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .filter_map(|(key, value)| value.as_string().map(|value| (key.as_str(), value)))
    }
}

/// A parsed document: the root table (keys before any header, e.g. the lock's
/// format `version`) plus named sub-tables and arrays-of-tables. The manifests
/// never nest sub-tables, so a flat map of header name to contents is enough.
#[derive(Debug, Clone, Default)]
pub(crate) struct Document {
    root: Table,
    tables: Vec<(String, Table)>,
    array_tables: Vec<(String, Vec<Table>)>,
}

impl Document {
    pub(crate) fn table(&self, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, table)| table)
    }

    pub(crate) fn array_of_tables(&self, name: &str) -> &[Table] {
        self.array_tables
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tables)| tables.as_slice())
            .unwrap_or(&[])
    }

    /// Keys written before any `[table]` header, such as the lock's top-level
    /// format `version`.
    pub(crate) fn root(&self) -> &Table {
        &self.root
    }
}

/// A parse failure, carrying the 1-based line number for a useful diagnostic.
#[derive(Debug, Clone)]
pub(crate) struct ParseError {
    pub(crate) line: usize,
    pub(crate) message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Where the parser is currently appending `key = value` pairs.
enum Cursor {
    /// The implicit root table (keys before any header).
    Root,
    /// A `[name]` table.
    Table(usize),
    /// The most recent element of a `[[name]]` array of tables.
    ArrayTable(usize),
}

/// Parses a manifest into a [`Document`]. Comments (`#`) and blank lines are
/// ignored. Keys written before any header land in the root table.
pub(crate) fn parse(source: &str) -> Result<Document, ParseError> {
    let mut doc = Document::default();
    let mut cursor = Cursor::Root;

    let mut lines = source.lines().enumerate().peekable();
    while let Some((index, raw)) = lines.next() {
        let line_no = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(name) = line.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
            let name = table_name(name.trim(), line_no)?;
            let position = match doc.array_tables.iter().position(|(key, _)| key == &name) {
                Some(position) => position,
                None => {
                    doc.array_tables.push((name, Vec::new()));
                    doc.array_tables.len() - 1
                }
            };
            doc.array_tables[position].1.push(Table::default());
            cursor = Cursor::ArrayTable(position);
            continue;
        }

        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let name = table_name(name.trim(), line_no)?;
            if doc.tables.iter().any(|(key, _)| key == &name) {
                return Err(ParseError {
                    line: line_no,
                    message: format!("duplicate table `[{name}]`"),
                });
            }
            doc.tables.push((name, Table::default()));
            cursor = Cursor::Table(doc.tables.len() - 1);
            continue;
        }

        // A key/value line. Arrays may span multiple physical lines, so join
        // continuation lines until the brackets balance before parsing.
        let mut buffer = line.to_string();
        while needs_continuation(&buffer) {
            let Some((_, next)) = lines.next() else {
                return Err(ParseError {
                    line: line_no,
                    message: "unterminated array".to_string(),
                });
            };
            buffer.push(' ');
            buffer.push_str(strip_comment(next).trim());
        }

        let (key, value) = parse_entry(&buffer, line_no)?;
        let table = match &cursor {
            Cursor::Root => &mut doc.root,
            Cursor::Table(position) => &mut doc.tables[*position].1,
            Cursor::ArrayTable(position) => doc.array_tables[*position]
                .1
                .last_mut()
                .expect("array table pushed before entries"),
        };
        table.insert(key, value);
    }

    Ok(doc)
}

/// True while an accumulated key/value buffer has an open `[` without its `]`,
/// meaning the array value continues on the next line.
fn needs_continuation(buffer: &str) -> bool {
    let opens = buffer.matches('[').count();
    let closes = buffer.matches(']').count();
    opens > closes
}

fn table_name(name: &str, line_no: usize) -> Result<String, ParseError> {
    if name.is_empty() {
        return Err(ParseError {
            line: line_no,
            message: "empty table name".to_string(),
        });
    }
    // Table headers in these manifests are always simple bare names.
    Ok(name.to_string())
}

/// Splits a comment off a physical line, honoring `#` only outside a string so a
/// `#` inside a quoted source path or value is preserved.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' if !escaped => in_string = !in_string,
            '\\' if in_string => {
                escaped = !escaped;
                continue;
            }
            '#' if !in_string => return &line[..index],
            _ => {}
        }
        escaped = false;
    }
    line
}

fn parse_entry(line: &str, line_no: usize) -> Result<(String, Value), ParseError> {
    let Some((key_part, value_part)) = split_key_value(line) else {
        return Err(ParseError {
            line: line_no,
            message: "expected `key = value`".to_string(),
        });
    };
    let key = parse_key(key_part.trim(), line_no)?;
    let value = parse_value(value_part.trim(), line_no)?;
    Ok((key, value))
}

/// Finds the `=` separating key from value, skipping any `=` inside the quoted
/// key.
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' if !escaped => in_string = !in_string,
            '\\' if in_string => {
                escaped = !escaped;
                continue;
            }
            '=' if !in_string => return Some((&line[..index], &line[index + 1..])),
            _ => {}
        }
        escaped = false;
    }
    None
}

fn parse_key(key: &str, line_no: usize) -> Result<String, ParseError> {
    if let Some(inner) = key.strip_prefix('"') {
        let inner = inner.strip_suffix('"').ok_or_else(|| ParseError {
            line: line_no,
            message: "unterminated quoted key".to_string(),
        })?;
        return unescape(inner, line_no);
    }
    if key.is_empty() {
        return Err(ParseError {
            line: line_no,
            message: "empty key".to_string(),
        });
    }
    Ok(key.to_string())
}

fn parse_value(value: &str, line_no: usize) -> Result<Value, ParseError> {
    if value.starts_with('[') {
        let inner = value
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .ok_or_else(|| ParseError {
                line: line_no,
                message: "unterminated array".to_string(),
            })?;
        let mut items = Vec::new();
        for element in split_array_elements(inner, line_no)? {
            let element = element.trim();
            if element.is_empty() {
                continue;
            }
            items.push(parse_string(element, line_no)?);
        }
        return Ok(Value::Array(items));
    }
    if value.starts_with('"') {
        return Ok(Value::String(parse_string(value, line_no)?));
    }
    // A bare scalar (integer, boolean, or bare word) — kept as its literal text.
    // The manifests only use this for the lock's top-level format `version`; a
    // quoted string is still preferred for everything else.
    if value.is_empty() {
        return Err(ParseError {
            line: line_no,
            message: "missing value".to_string(),
        });
    }
    Ok(Value::String(value.to_string()))
}

/// Splits array elements on commas that sit outside a quoted string.
fn split_array_elements(inner: &str, line_no: usize) -> Result<Vec<String>, ParseError> {
    let mut elements = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in inner.chars() {
        match ch {
            '"' if !escaped => {
                in_string = !in_string;
                current.push(ch);
            }
            '\\' if in_string => {
                current.push(ch);
                escaped = !escaped;
                continue;
            }
            ',' if !in_string => {
                elements.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
        escaped = false;
    }
    if in_string {
        return Err(ParseError {
            line: line_no,
            message: "unterminated string in array".to_string(),
        });
    }
    elements.push(current);
    Ok(elements)
}

fn parse_string(value: &str, line_no: usize) -> Result<String, ParseError> {
    let inner = value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| ParseError {
            line: line_no,
            message: format!("expected a quoted string, found `{value}`"),
        })?;
    unescape(inner, line_no)
}

fn unescape(value: &str, line_no: usize) -> Result<String, ParseError> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => {
                return Err(ParseError {
                    line: line_no,
                    message: format!("unsupported escape `\\{other}`"),
                });
            }
            None => {
                return Err(ParseError {
                    line: line_no,
                    message: "trailing backslash".to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// Escapes a string for emission inside double quotes. Mirrors [`unescape`].
pub(crate) fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Wraps a string in double quotes with the interior escaped.
pub(crate) fn quote(value: &str) -> String {
    format!("\"{}\"", escape(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pome_manifest() {
        let doc = parse(
            r#"
[pome]
name = "github.com/emela-lang/json"
version = "1.2.0"
emela = "0.1"

[dependencies]
"github.com/emela-lang/parser" = "^2.0"
"gitlab.com/acme/util"         = "^0.3"  # inline comment
"#,
        )
        .unwrap();

        let pome = doc.table("pome").unwrap();
        assert_eq!(pome.get_string("name"), Some("github.com/emela-lang/json"));
        assert_eq!(pome.get_string("version"), Some("1.2.0"));

        let deps = doc.table("dependencies").unwrap();
        let entries: Vec<_> = deps.string_entries().collect();
        assert_eq!(
            entries,
            vec![
                ("github.com/emela-lang/parser", "^2.0"),
                ("gitlab.com/acme/util", "^0.3"),
            ]
        );
    }

    #[test]
    fn parses_array_of_tables() {
        let doc = parse(
            r#"
[[package]]
source = "github.com/emela-lang/stdlib"
version = "v1.4.0"

[[package]]
source = "gitlab.com/acme/util"
version = "v0.3.1"
"#,
        )
        .unwrap();

        let packages = doc.array_of_tables("package");
        assert_eq!(packages.len(), 2);
        assert_eq!(
            packages[0].get_string("source"),
            Some("github.com/emela-lang/stdlib")
        );
        assert_eq!(packages[1].get_string("version"), Some("v0.3.1"));
    }

    #[test]
    fn parses_string_array_across_lines() {
        let doc = parse(
            r#"
[bushel]
members = [
  "core",
  "cli",
]
"#,
        )
        .unwrap();
        let members = doc.table("bushel").unwrap().get_array("members").unwrap();
        assert_eq!(members, ["core", "cli"]);
    }

    #[test]
    fn hash_inside_string_is_not_a_comment() {
        let doc = parse("[pome]\nname = \"a#b\"\n").unwrap();
        assert_eq!(doc.table("pome").unwrap().get_string("name"), Some("a#b"));
    }

    #[test]
    fn root_level_keys_land_in_root() {
        // The lock writes a top-level `version = 1` before any `[[package]]`.
        let doc = parse("version = 1\nname = \"x\"\n").unwrap();
        assert_eq!(doc.root().get_string("version"), Some("1"));
        assert_eq!(doc.root().get_string("name"), Some("x"));
    }

    #[test]
    fn escape_round_trips() {
        let original = "a\"b\\c";
        let quoted = quote(original);
        let doc = parse(&format!("[t]\nk = {quoted}\n")).unwrap();
        assert_eq!(doc.table("t").unwrap().get_string("k"), Some(original));
    }
}
