//! `Pome.lock` (spec 4.7): every Pome of the build list with its version, commit and hashes,
//! including those only `[dev-dependencies]` reach (marked `dev`).
//! `emela pome` writes it; nothing edits it by hand.
//!
//! ```toml
//! version = 1
//!
//! [[pome]]
//! path = "github.com/emela-lang/x"
//! version = "v0.2.0"
//! commit = "0123456789abcdef0123456789abcdef01234567"
//! hash = "h1:..."
//! manifest = "h1:..."
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hash::H1;
use crate::source_path::SourcePath;
use crate::version::Version;

pub const LOCK_FILE: &str = "Pome.lock";
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// Sorted by source path; one entry per Pome.
    pub pomes: Vec<Locked>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locked {
    pub path: SourcePath,
    pub version: Version,
    pub commit: String,
    pub hash: H1,
    pub manifest: H1,
    /// The `.emel` files with `@external` (spec 4.7); empty for a Pome without them. Shown in
    /// the lock so that a review sees when a new version reaches the host in more places.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external: Vec<String>,
    /// Reached only through `[dev-dependencies]` (spec 4.7): `build` and `run` do not get it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dev: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The `.emel` files of a tree with an `@external` annotation, sorted. An annotation is a line
/// starting with `@external` (comments start with `#`, so they do not count). Files under `test/`
/// are left out: a dependency's tests are never read (spec 13.1).
pub fn external_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Vec<String> {
    let mut found: Vec<String> = files
        .into_iter()
        .filter(|(path, _)| path.ends_with(".emel") && !path.starts_with("test/"))
        .filter(|(_, content)| {
            String::from_utf8_lossy(content)
                .lines()
                .any(|l| l.trim_start().starts_with("@external"))
        })
        .map(|(path, _)| path.to_string())
        .collect();
    found.sort();
    found
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockError(pub String);

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {LOCK_FILE}: {}", self.0)
    }
}

impl std::error::Error for LockError {}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default, rename = "pome", skip_serializing_if = "Vec::is_empty")]
    pomes: Vec<Locked>,
}

impl Lock {
    pub fn new(mut pomes: Vec<Locked>) -> Lock {
        pomes.sort_by(|a, b| a.path.cmp(&b.path));
        Lock { pomes }
    }

    pub fn parse(text: &str) -> Result<Lock, LockError> {
        let file: File = toml::from_str(text).map_err(|e| LockError(e.to_string()))?;
        if file.version != FORMAT_VERSION {
            return Err(LockError(format!(
                "unknown format version {}",
                file.version
            )));
        }
        for p in &file.pomes {
            if !(p.commit.len() == 40
                && p.commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            {
                return Err(LockError(format!(
                    "`{}`: commit must be 40 hex digits",
                    p.path
                )));
            }
        }
        let lock = Lock::new(file.pomes);
        if let Some(w) = lock.pomes.windows(2).find(|w| w[0].path == w[1].path) {
            return Err(LockError(format!("`{}` appears twice", w[0].path)));
        }
        Ok(lock)
    }

    pub fn to_toml(&self) -> String {
        let file = File {
            version: FORMAT_VERSION,
            pomes: self.pomes.clone(),
        };
        let body = toml::to_string(&file).expect("a lock serializes");
        format!("# Written by `emela pome`. Do not edit.\n{body}")
    }

    pub fn get(&self, path: &SourcePath) -> Option<&Locked> {
        self.pomes.iter().find(|p| &p.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locked(path: &str, version: &str) -> Locked {
        let h = H1::parse("h1:+Thj6XlK+eABClH7yw779GrFFl/umwEs9PO0k3JjISY=").unwrap();
        Locked {
            path: SourcePath::parse(path).unwrap(),
            version: Version::parse(version).unwrap(),
            commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            hash: h.clone(),
            manifest: h,
            external: Vec::new(),
            dev: false,
        }
    }

    #[test]
    fn round_trips_sorted_by_path() {
        let lock = Lock::new(vec![
            locked("github.com/b/y", "v1.0.0"),
            locked("github.com/a/x", "v0.2.0"),
        ]);
        let text = lock.to_toml();
        assert!(text.find("github.com/a/x").unwrap() < text.find("github.com/b/y").unwrap());
        assert_eq!(Lock::parse(&text).unwrap(), lock);
    }

    #[test]
    fn rejects_duplicates_and_bad_commits() {
        let twice = Lock {
            pomes: vec![
                locked("github.com/a/x", "v0.2.0"),
                locked("github.com/a/x", "v0.3.0"),
            ],
        };
        assert!(Lock::parse(&twice.to_toml()).is_err());
        let mut bad = locked("github.com/a/x", "v0.2.0");
        bad.commit = "abc".to_string();
        assert!(Lock::parse(&Lock::new(vec![bad]).to_toml()).is_err());
        assert!(Lock::parse("version = 2\n").is_err());
    }

    #[test]
    fn lists_files_with_external() {
        let files: [(&str, &[u8]); 5] = [
            (
                "src/kv.emel",
                b"pub effect Kv {}\n@external(js, \"./kv.mjs\", \"get\")\nfn get() -> Int\n",
            ),
            (
                "src/a.emel",
                b"# @external in a comment\nfn a() -> Int { 1 }\n",
            ),
            ("src/kv.mjs", b"@external\n"),
            ("src/b.emel", b"  @external\n  type T\n"),
            (
                "test/fake.emel",
                b"@external(js, \"./f.mjs\", \"f\")\nfn f() -> Int\n",
            ),
        ];
        assert_eq!(external_files(files), ["src/b.emel", "src/kv.emel"]);
    }

    #[test]
    fn writes_external_only_when_present() {
        let mut l = locked("github.com/a/x", "v0.2.0");
        assert!(!Lock::new(vec![l.clone()]).to_toml().contains("external"));
        l.external = vec!["src/kv.emel".into()];
        let text = Lock::new(vec![l.clone()]).to_toml();
        assert!(text.contains("external = [\"src/kv.emel\"]"), "{text}");
        assert_eq!(Lock::parse(&text).unwrap().pomes[0], l);
    }

    #[test]
    fn writes_dev_only_when_set() {
        let mut l = locked("github.com/a/x", "v0.2.0");
        assert!(!Lock::new(vec![l.clone()]).to_toml().contains("dev"));
        l.dev = true;
        let text = Lock::new(vec![l.clone()]).to_toml();
        assert!(text.contains("dev = true"), "{text}");
        assert_eq!(Lock::parse(&text).unwrap().pomes[0], l);
    }
}
