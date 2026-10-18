//! Source paths (spec 4.7): `host/path`, the canonical name of a Pome. A Pome of major v2 or
//! above ends in `/vN`; the path without it is where the Pome is. On github.com that is
//! `github.com/<owner>/<repo>` and a directory in the repository after it; on other hosts the
//! whole path is the repository.

use std::fmt;

use crate::version::Version;

const MAX_LENGTH: usize = 255;

/// The host whose repositories are `<host>/<owner>/<repo>`, with Pomes in their directories.
const GITHUB: &str = "github.com";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourcePath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePathError {
    pub input: String,
    pub reason: &'static str,
}

impl fmt::Display for SourcePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid source path `{}`: {}", self.input, self.reason)
    }
}

impl std::error::Error for SourcePathError {}

impl SourcePath {
    /// The canonical form, as written in `Pome.toml` and `Pome.lock`.
    pub fn parse(input: &str) -> Result<SourcePath, SourcePathError> {
        let err = |reason| SourcePathError {
            input: input.to_string(),
            reason,
        };
        if input.is_empty() || input.len() > MAX_LENGTH {
            return Err(err("length must be 1..255"));
        }
        let mut parts = input.split('/');
        let host = parts.next().unwrap_or_default();
        if !is_host(host) {
            return Err(err("host must be a lowercase domain name"));
        }
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            return Err(err("path is missing"));
        }
        if !rest.iter().all(|e| is_element(e)) {
            return Err(err(
                "a path element is empty or has characters other than letters, digits, `.`, `_` and `-`",
            ));
        }
        if input.ends_with(".git") {
            return Err(err("must not end with `.git`"));
        }
        if let Some(digits) = major_digits(input) {
            if digits.starts_with('0') || digits.parse::<u64>().map_or(true, |n| n < 2) {
                return Err(err(
                    "a major suffix must be `/v2` or above, without leading zeros",
                ));
            }
            if rest.len() < 2 {
                return Err(err("repository path is missing before the major suffix"));
            }
        }
        let location = rest.len() - usize::from(major_digits(input).is_some());
        if host == GITHUB && location < 2 {
            return Err(err("a github.com path needs the owner and the repository"));
        }
        Ok(SourcePath(input.to_string()))
    }

    /// What a command accepts: a source path, optionally with `https://` and `.git`, or the
    /// shorthand `github:owner/repo`. Only commands take these forms (spec 4.7).
    pub fn from_input(input: &str) -> Result<SourcePath, SourcePathError> {
        let s = match input.strip_prefix("github:") {
            Some(rest) => format!("github.com/{rest}"),
            None => input.strip_prefix("https://").unwrap_or(input).to_string(),
        };
        SourcePath::parse(s.strip_suffix(".git").unwrap_or(&s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn host(&self) -> &str {
        self.0.split('/').next().unwrap_or_default()
    }

    /// N of the trailing `/vN`; `None` for a Pome of v0 or v1.
    pub fn major(&self) -> Option<u64> {
        major_digits(&self.0).and_then(|d| d.parse().ok())
    }

    /// Where the Pome is: the path without `/vN`.
    pub fn location(&self) -> &str {
        match major_digits(&self.0) {
            Some(d) => &self.0[..self.0.len() - d.len() - 2],
            None => &self.0,
        }
    }

    /// The repository to fetch: `github.com/<owner>/<repo>` on github.com, else the location.
    pub fn repository(&self) -> &str {
        let location = self.location();
        if self.host() != GITHUB {
            return location;
        }
        match location.match_indices('/').nth(2) {
            Some((i, _)) => &location[..i],
            None => location,
        }
    }

    /// The directory of the Pome in its repository (`platform-cloudflare`); `None` at the root.
    pub fn directory(&self) -> Option<&str> {
        self.location()
            .strip_prefix(self.repository())?
            .strip_prefix('/')
    }

    /// The git tag of a version: the directory, then `/`, before the version for a Pome in a
    /// directory (`platform-cloudflare/v0.1.0`).
    pub fn tag(&self, version: &Version) -> String {
        match self.directory() {
            Some(dir) => format!("{dir}/{version}"),
            None => version.to_string(),
        }
    }

    /// The version a tag of the repository names for this Pome, if it is one of its tags.
    pub fn version_of_tag(&self, tag: &str) -> Option<Version> {
        let version = match self.directory() {
            Some(dir) => tag.strip_prefix(dir)?.strip_prefix('/')?,
            None => tag,
        };
        Version::parse(version)
            .ok()
            .filter(|v| self.is_version_of(v))
    }

    /// The last element that names the Pome, ignoring `/vN`. The default root name comes from
    /// it (spec 4.7).
    pub fn last_element(&self) -> &str {
        self.location().rsplit('/').next().unwrap_or_default()
    }

    /// Whether a tag is a version of this Pome: major N with `/vN`, else v0 or v1.
    pub fn is_version_of(&self, version: &Version) -> bool {
        match self.major() {
            Some(n) => version.major == n,
            None => version.major <= 1,
        }
    }

    /// The form used in proxy URLs and cache directories: an uppercase letter becomes `!` and
    /// the lowercase letter, so that case-insensitive storage keeps `Foo` and `foo` apart.
    pub fn escaped(&self) -> String {
        let mut out = String::with_capacity(self.0.len());
        for c in self.0.chars() {
            if c.is_ascii_uppercase() {
                out.push('!');
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }
}

impl fmt::Display for SourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for SourcePath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        SourcePath::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for SourcePath {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

/// The digits of a trailing `/v<digits>`.
fn major_digits(path: &str) -> Option<&str> {
    let (_, last) = path.rsplit_once('/')?;
    let digits = last.strip_prefix('v')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

// A lowercase domain name with at least one dot: labels of [a-z0-9-], not starting or ending
// with `-`.
fn is_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

// Letters, digits, `.`, `_` and `-`; starts with a letter or digit and does not end with `.`.
fn is_element(e: &str) -> bool {
    let ok = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-');
    e.as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphanumeric())
        && !e.ends_with('.')
        && e.bytes().all(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_source_paths() {
        for p in [
            "github.com/emela-lang/stdlib",
            "github.com/Acme/Json-Codec",
            "git.example.co.jp/a/b/c",
            "github.com/acme/http/v2",
        ] {
            assert_eq!(SourcePath::parse(p).unwrap().as_str(), p);
        }
    }

    #[test]
    fn rejects_malformed_paths() {
        for p in [
            "",
            "github.com",
            "GitHub.com/a/b",
            "localhost/a",
            "https://github.com/a/b",
            "github.com/a/b.git",
            "github.com/a/../b",
            "github.com/a//b",
            "github.com/.hidden/b",
            "github.com/a/b.",
            "github.com/a/v0",
            "github.com/a/b/v1",
            "github.com/a/b/v02",
            "github.com/v2",
            "github.com/acme",
            "github.com/acme/v2",
        ] {
            assert!(SourcePath::parse(p).is_err(), "{p}");
        }
    }

    #[test]
    fn reads_command_input() {
        for input in [
            "github:acme/x",
            "https://github.com/acme/x.git",
            "github.com/acme/x",
        ] {
            assert_eq!(
                SourcePath::from_input(input).unwrap().as_str(),
                "github.com/acme/x"
            );
        }
    }

    #[test]
    fn splits_the_major_suffix() {
        let v2 = SourcePath::parse("github.com/acme/http/v2").unwrap();
        assert_eq!(
            (v2.major(), v2.repository(), v2.last_element()),
            (Some(2), "github.com/acme/http", "http")
        );
        let v1 = SourcePath::parse("github.com/acme/http").unwrap();
        assert_eq!(
            (v1.major(), v1.repository()),
            (None, "github.com/acme/http")
        );
        assert_eq!(
            SourcePath::parse("github.com/acme/v2/x").unwrap().major(),
            None
        );
    }

    #[test]
    fn finds_a_pome_in_a_directory_of_a_github_repository() {
        let p = SourcePath::parse("github.com/emela-lang/x/platform-cloudflare").unwrap();
        assert_eq!(
            (p.repository(), p.directory(), p.last_element()),
            (
                "github.com/emela-lang/x",
                Some("platform-cloudflare"),
                "platform-cloudflare"
            )
        );
        assert_eq!(
            p.tag(&Version::parse("v0.1.0").unwrap()),
            "platform-cloudflare/v0.1.0"
        );
        assert_eq!(
            p.version_of_tag("platform-cloudflare/v0.1.0")
                .unwrap()
                .as_str(),
            "v0.1.0"
        );
        assert_eq!(p.version_of_tag("v0.1.0"), None);
        assert_eq!(p.version_of_tag("other/v0.1.0"), None);

        let v2 = SourcePath::parse("github.com/emela-lang/x/a/b/v2").unwrap();
        assert_eq!(
            (v2.repository(), v2.directory(), v2.last_element()),
            ("github.com/emela-lang/x", Some("a/b"), "b")
        );
        assert_eq!(v2.version_of_tag("a/b/v2.1.0").unwrap().as_str(), "v2.1.0");
        assert_eq!(v2.version_of_tag("a/b/v1.0.0"), None);

        let root = SourcePath::parse("github.com/emela-lang/x").unwrap();
        assert_eq!(
            (root.repository(), root.directory()),
            ("github.com/emela-lang/x", None)
        );
        assert_eq!(root.version_of_tag("platform-cloudflare/v0.1.0"), None);
        // Other hosts: the whole path is the repository.
        let other = SourcePath::parse("git.example.com/a/b/c").unwrap();
        assert_eq!(
            (other.repository(), other.directory()),
            ("git.example.com/a/b/c", None)
        );
    }

    #[test]
    fn matches_versions_to_the_major() {
        let v = |s| Version::parse(s).unwrap();
        let v1 = SourcePath::parse("github.com/acme/http").unwrap();
        let v2 = SourcePath::parse("github.com/acme/http/v2").unwrap();
        assert!(
            v1.is_version_of(&v("v0.3.0"))
                && v1.is_version_of(&v("v1.4.0"))
                && !v1.is_version_of(&v("v2.0.0"))
        );
        assert!(
            v2.is_version_of(&v("v2.1.0-rc.1"))
                && !v2.is_version_of(&v("v1.4.0"))
                && !v2.is_version_of(&v("v3.0.0"))
        );
    }

    #[test]
    fn escapes_uppercase() {
        assert_eq!(
            SourcePath::parse("github.com/Acme/JSON").unwrap().escaped(),
            "github.com/!acme/!j!s!o!n"
        );
    }
}
