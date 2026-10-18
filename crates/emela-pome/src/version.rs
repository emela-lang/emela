//! Versions (spec 4.7): git tags of a semver with a leading `v`, without build metadata.

use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Vec<String>,
    raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionError(pub String);

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid version `{}`: expected a semver tag such as `v1.2.3`",
            self.0
        )
    }
}

impl std::error::Error for VersionError {}

impl Version {
    pub fn parse(input: &str) -> Result<Version, VersionError> {
        let err = || VersionError(input.to_string());
        let rest = input.strip_prefix('v').ok_or_else(err)?;
        let (core, prerelease) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        let nums: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = nums.as_slice() else {
            return Err(err());
        };
        let num = |s: &str| {
            (is_numeric(s))
                .then(|| s.parse::<u64>().ok())
                .flatten()
                .ok_or_else(err)
        };
        let prerelease = match prerelease {
            Some(pre) => {
                let ids: Vec<String> = pre.split('.').map(str::to_string).collect();
                if !ids
                    .iter()
                    .all(|id| is_numeric(id) || is_alphanumeric_id(id))
                {
                    return Err(err());
                }
                ids
            }
            None => Vec::new(),
        };
        Ok(Version {
            major: num(major)?,
            minor: num(minor)?,
            patch: num(patch)?,
            prerelease,
            raw: input.to_string(),
        })
    }

    /// The `v`-prefixed tag.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// The form of `[pome].version`: the tag without `v`.
    pub fn without_v(&self) -> &str {
        &self.raw[1..]
    }

    pub fn is_prerelease(&self) -> bool {
        !self.prerelease.is_empty()
    }
}

impl Ord for Version {
    /// Semver precedence.
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| compare_prerelease(&self.prerelease, &other.prerelease))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Version::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.raw)
    }
}

/// The order in which `@latest` tries versions (spec 4.7): releases from the largest, then
/// prereleases from the largest.
pub fn latest_candidates(versions: &[Version]) -> Vec<Version> {
    let mut desc = versions.to_vec();
    desc.sort_by(|a, b| b.cmp(a));
    let (pre, rel): (Vec<_>, Vec<_>) = desc.into_iter().partition(Version::is_prerelease);
    rel.into_iter().chain(pre).collect()
}

fn compare_prerelease(a: &[String], b: &[String]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }
    for (x, y) in a.iter().zip(b) {
        let ord = match (is_numeric(x), is_numeric(y)) {
            (true, true) => x.len().cmp(&y.len()).then_with(|| x.cmp(y)),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

// Digits without a leading zero (`0` itself is fine).
fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
}

// A prerelease identifier with at least one non-digit.
fn is_alphanumeric_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && s.bytes().any(|b| !b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_semver_tags() {
        let x = v("v1.2.3-rc.1");
        assert_eq!(
            (x.major, x.minor, x.patch, x.prerelease.clone()),
            (1, 2, 3, vec!["rc".to_string(), "1".to_string()])
        );
        assert_eq!(x.without_v(), "1.2.3-rc.1");
        for bad in [
            "1.2.3",
            "v1.2",
            "v01.2.3",
            "v1.2.3+build",
            "v1.2.3-",
            "v1.2.3-01",
            "v1.2.3-a..b",
            "latest",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn orders_by_semver_precedence() {
        let ordered = [
            "v1.0.0-alpha",
            "v1.0.0-alpha.1",
            "v1.0.0-alpha.beta",
            "v1.0.0-beta",
            "v1.0.0-beta.2",
            "v1.0.0-beta.11",
            "v1.0.0-rc.1",
            "v1.0.0",
            "v1.10.0",
        ];
        for w in ordered.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
    }

    #[test]
    fn tries_releases_before_prereleases() {
        let vs: Vec<Version> = ["v1.0.0", "v1.1.0-rc.1", "v0.9.0", "v1.0.1-rc.1"]
            .map(v)
            .to_vec();
        let order: Vec<String> = latest_candidates(&vs)
            .iter()
            .map(|x| x.to_string())
            .collect();
        assert_eq!(order, ["v1.0.0", "v0.9.0", "v1.1.0-rc.1", "v1.0.1-rc.1"]);
    }
}
