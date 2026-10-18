//! The content hash `h1:` (spec 4.7).
//!
//! For each file, the line `<hex sha256 of the content>  <path>\n`; the lines sorted by the UTF-8
//! bytes of the path make the summary, and `h1:` + base64(sha256(summary)) is the hash. The files
//! are those of the git tree the tag's commit points to, so the hash does not depend on the
//! archive format or on a working tree.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

pub const MANIFEST_FILE: &str = "Pome.toml";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct H1(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashError {
    /// Not of the form `h1:` + 44 characters of base64.
    Malformed(String),
    /// A path that cannot be part of a tree.
    BadPath { path: String, reason: &'static str },
}

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HashError::Malformed(s) => write!(
                f,
                "invalid hash `{s}`: expected `h1:` and 44 characters of base64"
            ),
            HashError::BadPath { path, reason } => {
                write!(f, "invalid path `{path}` in tree: {reason}")
            }
        }
    }
}

impl std::error::Error for HashError {}

impl H1 {
    pub fn parse(s: &str) -> Result<H1, HashError> {
        let ok = s.strip_prefix("h1:").is_some_and(|b| {
            b.len() == 44 && b.ends_with('=') && STANDARD.decode(b).is_ok_and(|d| d.len() == 32)
        });
        if ok {
            Ok(H1(s.to_string()))
        } else {
            Err(HashError::Malformed(s.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for H1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for H1 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        H1::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for H1 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

/// The hash of a tree given as (path, content) pairs. Paths are `/`-separated and relative.
pub fn tree_hash<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<H1, HashError> {
    let mut entries: Vec<(&str, [u8; 32])> = Vec::new();
    for (path, content) in files {
        check_path(path)?;
        entries.push((path, Sha256::digest(content).into()));
    }
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    if let Some(w) = entries.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(HashError::BadPath {
            path: w[0].0.to_string(),
            reason: "duplicate path",
        });
    }
    let mut summary = String::new();
    for (path, digest) in &entries {
        for b in digest {
            summary.push_str(&format!("{b:02x}"));
        }
        summary.push_str("  ");
        summary.push_str(path);
        summary.push('\n');
    }
    Ok(H1(format!(
        "h1:{}",
        STANDARD.encode(Sha256::digest(summary.as_bytes()))
    )))
}

/// The hash of `Pome.toml` alone: a tree with that one file. Recorded separately because walking
/// the dependency graph reads only each version's `Pome.toml`.
pub fn manifest_hash(content: &[u8]) -> H1 {
    tree_hash([(MANIFEST_FILE, content)]).expect("Pome.toml is a valid path")
}

fn check_path(path: &str) -> Result<(), HashError> {
    let bad = |reason| {
        Err(HashError::BadPath {
            path: path.to_string(),
            reason,
        })
    };
    if path.is_empty() {
        return bad("empty");
    }
    if path.contains('\n') || path.contains('\r') {
        return bad("contains a newline");
    }
    if path.contains('\\') {
        return bad("contains a backslash");
    }
    if path
        .split('/')
        .any(|e| e.is_empty() || e == "." || e == "..")
    {
        return bad("must be a clean relative path");
    }
    if path == ".git" || path.starts_with(".git/") {
        return bad("`.git` is not part of the tree");
    }
    Ok(())
}
