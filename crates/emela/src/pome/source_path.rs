//! Source-path handling for Pomes (spec 0032 S1-S3).
//!
//! A Pome's canonical identity is its source path `host/path` — the location it
//! is fetched from, with the scheme and any trailing `.git` removed (S1). Users
//! may type a host shorthand such as `github:acme/util` (S2); [`normalize`]
//! folds every accepted form to the same canonical path, which is the only form
//! written to `Pome.toml`/`Pome.lock` (S3).

use std::env;

use crate::error::{Error, Result};

/// Host shorthands (spec 0032 S2). `github:` is required; the others are the
/// examples the spec lists. `<alias>:<path>` expands to `<host>/<path>`.
const HOST_ALIASES: &[(&str, &str)] = &[
    ("github", "github.com"),
    ("gitlab", "gitlab.com"),
    ("codeberg", "codeberg.org"),
    ("sourcehut", "git.sr.ht"),
    ("bitbucket", "bitbucket.org"),
];

/// Folds any accepted spelling of a Pome reference to its canonical source path
/// `host/path` (spec 0032 S1). Accepts:
///
/// - a bare canonical path: `github.com/emela-lang/stdlib`
/// - a full URL: `https://github.com/emela-lang/stdlib(.git)`
/// - a host shorthand: `github:emela-lang/stdlib` (spec 0032 S2)
///
/// A `scp`-like Git address (`git@github.com:acme/util`) is also accepted for
/// convenience. The result never carries a scheme, a trailing `.git`, a
/// trailing slash, or a `#`/`?` fragment.
pub(crate) fn normalize(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::new("empty source path"));
    }

    // `git@host:path` — the SSH scp form, before the generic alias split so the
    // `git@` user is not mistaken for a host alias.
    if let Some(rest) = trimmed.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
    {
        return finish(host, path);
    }

    // A URL scheme (`https://`, `http://`, `ssh://`, `git://`).
    if let Some((scheme, rest)) = split_scheme(trimmed) {
        if !matches!(scheme, "http" | "https" | "ssh" | "git") {
            return Err(Error::new(format!(
                "unsupported URL scheme `{scheme}:` in `{input}`"
            )));
        }
        // Strip an optional `user@` in `ssh://user@host/path`.
        let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
        let (host, path) = rest.split_once('/').ok_or_else(|| {
            Error::new(format!(
                "source path `{input}` is missing a path after the host"
            ))
        })?;
        return finish(host, path);
    }

    // A host shorthand `alias:path` (spec 0032 S2). The alias must be known; an
    // unknown one is more likely a typo than a raw host, so reject it.
    if let Some((alias, path)) = trimmed.split_once(':') {
        // Guard against a `host/path:with:colon` — only treat as a shorthand
        // when the part before `:` has no slash.
        if !alias.contains('/') {
            let host = HOST_ALIASES
                .iter()
                .find(|(name, _)| *name == alias)
                .map(|(_, host)| *host)
                .ok_or_else(|| {
                    Error::new(format!(
                        "unknown host shorthand `{alias}:` in `{input}` (known: {})",
                        known_aliases()
                    ))
                })?;
            return finish(host, path);
        }
    }

    // A bare canonical path `host/path`.
    let (host, path) = trimmed.split_once('/').ok_or_else(|| {
        Error::new(format!(
            "source path `{input}` must be `host/path` (e.g. github.com/acme/util)"
        ))
    })?;
    finish(host, path)
}

/// Joins a host and path into a canonical source path, stripping `.git`, query
/// and fragment, and trailing slashes.
fn finish(host: &str, path: &str) -> Result<String> {
    let host = host.trim();
    if host.is_empty() || !host.contains('.') {
        return Err(Error::new(format!(
            "`{host}` is not a valid host (expected a domain like github.com)"
        )));
    }
    let path = path
        .split(['#', '?'])
        .next()
        .unwrap_or(path)
        .trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() {
        return Err(Error::new(format!(
            "source path for host `{host}` is empty"
        )));
    }
    Ok(format!("{host}/{path}"))
}

fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = input.split_once("://")?;
    if scheme.chars().all(|c| c.is_ascii_alphabetic()) && !scheme.is_empty() {
        Some((scheme, rest))
    } else {
        None
    }
}

fn known_aliases() -> String {
    HOST_ALIASES
        .iter()
        .map(|(name, _)| format!("{name}:"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Git URL a canonical source path is fetched from.
///
/// By default this is `https://<source-path>.git`, which keeps fetching
/// decentralized: the source path *is* the location (spec 0032 R4). For local
/// development and offline testing, `EMELA_POME_REPLACE` may map source paths to
/// alternate Git URLs or local repository paths — the same role Go modules'
/// `replace` and `GOPROXY` play. The format is a `;`-separated list of
/// `source-path=url` entries; the first exact match wins.
pub(crate) fn git_url(source_path: &str) -> String {
    if let Some(url) = replacement(source_path) {
        return url;
    }
    format!("https://{source_path}.git")
}

/// Looks up a `EMELA_POME_REPLACE` override for `source_path`, if any.
fn replacement(source_path: &str) -> Option<String> {
    let raw = env::var("EMELA_POME_REPLACE").ok()?;
    for entry in raw.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some((from, to)) = entry.split_once('=')
            && from.trim() == source_path
        {
            return Some(to.trim().to_string());
        }
    }
    None
}

/// The leaf module namespace a source path contributes — its last path segment
/// (spec 0032 M1/M2, pending full integration with 0010/0018). For
/// `github.com/emela-lang/stdlib` this is `stdlib`.
pub(crate) fn leaf(source_path: &str) -> &str {
    source_path.rsplit('/').next().unwrap_or(source_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_path_is_idempotent() {
        assert_eq!(
            normalize("github.com/emela-lang/stdlib").unwrap(),
            "github.com/emela-lang/stdlib"
        );
    }

    #[test]
    fn url_and_bare_path_agree() {
        // Spec 0032 S1: the URL form and the bare form name the same Pome.
        let url = normalize("https://github.com/emela-lang/stdlib").unwrap();
        let bare = normalize("github.com/emela-lang/stdlib").unwrap();
        assert_eq!(url, bare);
    }

    #[test]
    fn strips_dot_git_and_trailing_slash() {
        assert_eq!(
            normalize("https://github.com/emela-lang/stdlib.git/").unwrap(),
            "github.com/emela-lang/stdlib"
        );
    }

    #[test]
    fn expands_host_shorthands() {
        // Spec 0032 S2 examples.
        assert_eq!(
            normalize("github:emela-lang/stdlib").unwrap(),
            "github.com/emela-lang/stdlib"
        );
        assert_eq!(
            normalize("gitlab:acme/util").unwrap(),
            "gitlab.com/acme/util"
        );
        assert_eq!(
            normalize("codeberg:acme/util").unwrap(),
            "codeberg.org/acme/util"
        );
    }

    #[test]
    fn scp_form_is_accepted() {
        assert_eq!(
            normalize("git@github.com:acme/util.git").unwrap(),
            "github.com/acme/util"
        );
    }

    #[test]
    fn unknown_shorthand_is_rejected() {
        assert!(normalize("nope:acme/util").is_err());
    }

    #[test]
    fn host_without_dot_is_rejected() {
        assert!(normalize("localhost/acme").is_err());
    }

    #[test]
    fn default_git_url_is_https_dot_git() {
        // Only asserted when no replacement is configured for this path.
        if super::replacement("github.com/acme/util").is_none() {
            assert_eq!(
                git_url("github.com/acme/util"),
                "https://github.com/acme/util.git"
            );
        }
    }

    #[test]
    fn leaf_is_last_segment() {
        assert_eq!(leaf("github.com/emela-lang/stdlib"), "stdlib");
    }
}
