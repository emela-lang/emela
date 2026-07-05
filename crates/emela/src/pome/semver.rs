//! Semantic versions and version requirements (spec 0032 V1-V3).
//!
//! Pome versions are Git tags of the form `v<major>.<minor>.<patch>` with an
//! optional `-pre` release suffix (V1). A dependency states a *requirement*
//! (V2); the resolver picks the greatest tag satisfying it (V3). The spec's
//! minimum requirement grammar is an exact version or a caret range
//! (`^1.2` = `>=1.2.0, <2.0.0`); the fuller grammar is an Open Question, so this
//! module implements exactly that minimum plus a bare version treated as caret
//! (the common `emela pome add` default).

use std::cmp::Ordering;
use std::fmt;

use crate::error::{Error, Result};

/// A parsed semantic version. Pre-release identifiers are kept as raw strings;
/// they order below the same core version (enough for tag selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Version {
    pub(crate) major: u64,
    pub(crate) minor: u64,
    pub(crate) patch: u64,
    /// Dot-separated pre-release identifiers (`1.0.0-rc.1` -> `["rc", "1"]`).
    pub(crate) pre: Vec<String>,
}

impl Version {
    /// Parses `v1.2.0` or `1.2.0`, with an optional `-pre` suffix. A missing
    /// minor or patch is not allowed for a concrete version (that shorthand is
    /// only meaningful in a requirement).
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let core = text.strip_prefix('v').unwrap_or(text);
        let (core, pre) = match core.split_once('-') {
            Some((core, pre)) => (core, parse_pre(pre)?),
            None => (core, Vec::new()),
        };
        let mut parts = core.split('.');
        let major = parse_number(parts.next(), text)?;
        let minor = parse_number(parts.next(), text)?;
        let patch = parse_number(parts.next(), text)?;
        if parts.next().is_some() {
            return Err(Error::new(format!("`{text}` has too many version parts")));
        }
        Ok(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

fn parse_number(part: Option<&str>, text: &str) -> Result<u64> {
    let part = part.ok_or_else(|| {
        Error::new(format!(
            "`{text}` is not a full `major.minor.patch` version"
        ))
    })?;
    part.parse::<u64>()
        .map_err(|_| Error::new(format!("`{part}` in `{text}` is not a number")))
}

fn parse_pre(pre: &str) -> Result<Vec<String>> {
    if pre.is_empty() {
        return Err(Error::new("empty pre-release identifier"));
    }
    Ok(pre.split('.').map(|id| id.to_string()).collect())
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| compare_pre(&self.pre, &other.pre))
    }
}

/// Pre-release ordering per semver: a version with a pre-release sorts *below*
/// the same core version without one; identifiers compare numerically when both
/// are numeric, else lexically.
fn compare_pre(a: &[String], b: &[String]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater, // no pre-release > has pre-release
        (false, true) => Ordering::Less,
        (false, false) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.len().cmp(&b.len())
        }
    }
}

/// A version requirement (spec 0032 V2). The minimum grammar: an exact version
/// or a caret range. A bare version like `1.2` is read as caret, matching the
/// default `emela pome add` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Requirement {
    /// `=1.2.0` — only that exact version.
    Exact(Version),
    /// `^1.2` — `>=1.2.0, <2.0.0` (or `<0.(minor+1).0` when major is 0, per the
    /// usual caret semantics for pre-1.0 versions).
    Caret(PartialVersion),
}

/// A possibly-incomplete version used as the base of a caret range: `^1` and
/// `^1.2` both parse, filling missing components with 0 for the lower bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PartialVersion {
    major: u64,
    minor: Option<u64>,
    patch: Option<u64>,
}

impl Requirement {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        if let Some(rest) = text.strip_prefix('=') {
            return Ok(Requirement::Exact(Version::parse(rest)?));
        }
        let body = text.strip_prefix('^').unwrap_or(text);
        Ok(Requirement::Caret(PartialVersion::parse(body)?))
    }

    /// The caret requirement that admits `version` and future compatible
    /// releases — what `emela pome add` records when the user gives no explicit
    /// requirement. `^1.4.0` for `v1.4.0`, `^0.3.1` for `v0.3.1`.
    pub(crate) fn caret_for(version: &Version) -> Self {
        Requirement::Caret(PartialVersion {
            major: version.major,
            minor: Some(version.minor),
            patch: Some(version.patch),
        })
    }

    /// Whether `version` satisfies this requirement. A pre-release only
    /// satisfies an exact match to itself, never a range (standard behavior:
    /// pre-releases are opt-in).
    pub(crate) fn matches(&self, version: &Version) -> bool {
        match self {
            Requirement::Exact(expected) => version == expected,
            Requirement::Caret(base) => {
                if version.is_prerelease() {
                    return false;
                }
                let lower = base.lower_bound();
                if *version < lower {
                    return false;
                }
                *version < base.upper_bound()
            }
        }
    }

    /// The canonical text written to `Pome.toml` (spec 0032 S3/F3). Requirements
    /// are written without the `v` tag prefix, matching the spec's examples
    /// (`^2.0`, `^0.3`).
    pub(crate) fn to_toml(&self) -> String {
        match self {
            Requirement::Exact(version) => {
                format!("={}", version.to_string().trim_start_matches('v'))
            }
            Requirement::Caret(base) => format!("^{base}"),
        }
    }
}

impl PartialVersion {
    fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let core = text.strip_prefix('v').unwrap_or(text);
        if core.contains('-') {
            return Err(Error::new(format!(
                "caret requirement `{text}` may not name a pre-release"
            )));
        }
        let mut parts = core.split('.');
        let major = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .ok_or_else(|| Error::new(format!("`{text}` is not a valid requirement")))?;
        let minor = parse_optional(parts.next(), text)?;
        let patch = parse_optional(parts.next(), text)?;
        if parts.next().is_some() {
            return Err(Error::new(format!("`{text}` has too many version parts")));
        }
        if patch.is_some() && minor.is_none() {
            return Err(Error::new(format!("`{text}` is malformed")));
        }
        Ok(PartialVersion {
            major,
            minor,
            patch,
        })
    }

    fn lower_bound(&self) -> Version {
        Version {
            major: self.major,
            minor: self.minor.unwrap_or(0),
            patch: self.patch.unwrap_or(0),
            pre: Vec::new(),
        }
    }

    /// The exclusive upper bound. Caret keeps the left-most non-zero component
    /// fixed: `^1.2` -> `<2.0.0`, `^0.3` -> `<0.4.0`, `^0.0.3` -> `<0.0.4`.
    fn upper_bound(&self) -> Version {
        let zero_major = self.major == 0;
        let minor = self.minor.unwrap_or(0);
        let zero_minor = minor == 0;
        if !zero_major {
            Version {
                major: self.major + 1,
                minor: 0,
                patch: 0,
                pre: Vec::new(),
            }
        } else if !zero_minor || self.minor.is_none() {
            // `^0.3(.x)` -> `<0.4.0`; `^0` -> `<1.0.0` (minor unspecified).
            if self.minor.is_none() {
                Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                    pre: Vec::new(),
                }
            } else {
                Version {
                    major: 0,
                    minor: minor + 1,
                    patch: 0,
                    pre: Vec::new(),
                }
            }
        } else {
            // `^0.0(.x)` -> only that patch line advances: `<0.0.(patch+1)` or
            // `<0.1.0` when patch is unspecified.
            match self.patch {
                Some(patch) => Version {
                    major: 0,
                    minor: 0,
                    patch: patch + 1,
                    pre: Vec::new(),
                },
                None => Version {
                    major: 0,
                    minor: 1,
                    patch: 0,
                    pre: Vec::new(),
                },
            }
        }
    }
}

fn parse_optional(part: Option<&str>, text: &str) -> Result<Option<u64>> {
    match part {
        None => Ok(None),
        Some(part) => part
            .parse::<u64>()
            .map(Some)
            .map_err(|_| Error::new(format!("`{part}` in `{text}` is not a number"))),
    }
}

impl fmt::Display for PartialVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.major)?;
        if let Some(minor) = self.minor {
            write!(f, ".{minor}")?;
        }
        if let Some(patch) = self.patch {
            write!(f, ".{patch}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn parses_and_orders_versions() {
        assert!(v("v1.2.0") < v("v1.10.0"));
        assert!(v("1.2.0") < v("2.0.0"));
        assert!(v("v1.0.0-rc.1") < v("v1.0.0"));
        assert!(v("v1.0.0-alpha") < v("v1.0.0-beta"));
    }

    #[test]
    fn caret_range_major() {
        // Spec 0032 V2: `^1.2` = `>=1.2.0, <2.0.0`.
        let req = Requirement::parse("^1.2").unwrap();
        assert!(req.matches(&v("v1.2.0")));
        assert!(req.matches(&v("v1.9.9")));
        assert!(!req.matches(&v("v1.1.0")));
        assert!(!req.matches(&v("v2.0.0")));
    }

    #[test]
    fn caret_range_zero_major() {
        // Pre-1.0 caret keeps the minor fixed.
        let req = Requirement::parse("^0.3").unwrap();
        assert!(req.matches(&v("v0.3.0")));
        assert!(req.matches(&v("v0.3.7")));
        assert!(!req.matches(&v("v0.4.0")));
    }

    #[test]
    fn exact_requirement() {
        let req = Requirement::parse("=1.2.0").unwrap();
        assert!(req.matches(&v("v1.2.0")));
        assert!(!req.matches(&v("v1.2.1")));
    }

    #[test]
    fn prerelease_excluded_from_caret() {
        let req = Requirement::parse("^1.0").unwrap();
        assert!(!req.matches(&v("v1.1.0-rc.1")));
    }

    #[test]
    fn caret_for_round_trips() {
        let req = Requirement::caret_for(&v("v1.4.0"));
        assert_eq!(req.to_toml(), "^1.4.0");
        assert!(req.matches(&v("v1.9.0")));
        assert!(!req.matches(&v("v2.0.0")));
    }
}
