//! The open-document store (spec 0033): the text the editor sees, keyed by
//! URI, plus `file://` URI ↔ path conversion. The store doubles as the import
//! overlay — open buffers shadow the filesystem during import resolution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) struct Document {
    pub(crate) uri: String,
    /// The decoded filesystem path; `None` for non-`file://` URIs (e.g. an
    /// untitled buffer), which then can't resolve relative imports.
    pub(crate) path: Option<PathBuf>,
    pub(crate) text: String,
    pub(crate) version: i64,
}

#[derive(Default)]
pub(crate) struct DocumentStore {
    documents: HashMap<String, Document>,
}

impl DocumentStore {
    pub(crate) fn open(&mut self, uri: String, version: i64, text: String) {
        let path = uri_to_path(&uri);
        self.documents.insert(
            uri.clone(),
            Document {
                uri,
                path,
                text,
                version,
            },
        );
    }

    pub(crate) fn change(&mut self, uri: &str, version: i64, text: String) {
        if let Some(document) = self.documents.get_mut(uri) {
            document.text = text;
            document.version = version;
        }
    }

    pub(crate) fn close(&mut self, uri: &str) {
        self.documents.remove(uri);
    }

    pub(crate) fn get(&self, uri: &str) -> Option<&Document> {
        self.documents.get(uri)
    }

    pub(crate) fn uris(&self) -> Vec<String> {
        self.documents.keys().cloned().collect()
    }

    /// The import overlay (spec 0033): canonicalized path → buffer text, so
    /// import resolution sees unsaved edits. Keys are canonicalized to match
    /// `imports.rs`, which looks modules up by canonical path.
    pub(crate) fn overlay(&self) -> HashMap<PathBuf, String> {
        self.documents
            .values()
            .filter_map(|document| {
                let canonical = document.path.as_ref()?.canonicalize().ok()?;
                Some((canonical, document.text.clone()))
            })
            .collect()
    }
}

/// Decodes a `file://` URI to a path. Anything else is `None`.
pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // An authority (host) is not supported; local files have an empty one.
    let encoded = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut iter = encoded.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let high = iter.next()?;
            let low = iter.next()?;
            let hex = [high, low];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// Encodes a path as a `file://` URI, percent-encoding everything outside the
/// URI path charset (matching what editors send for spaces and non-ASCII).
pub(crate) fn path_to_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_path_round_trip() {
        let path = PathBuf::from("/tmp/emela test/メイン.emel");
        let uri = path_to_uri(&path);
        assert!(uri.starts_with("file:///tmp/emela%20test/"), "{uri}");
        assert_eq!(uri_to_path(&uri), Some(path));
    }

    #[test]
    fn rejects_non_file_uris() {
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }
}
