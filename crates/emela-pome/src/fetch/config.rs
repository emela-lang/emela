//! Where Pomes come from (spec 4.7): `EMELA_POMEPROXY`, `EMELA_POMESUMDB`, `EMELA_POMEPRIVATE`
//! and `EMELA_POME_CACHE`.

use std::path::PathBuf;

use crate::source_path::SourcePath;
use crate::sumdb::note::Verifier;

pub const DEFAULT_PROXY: &str = "https://pomes.emela.dev,direct";

/// The verifier key of the default checksum database, built into the compiler, and its URL. The
/// key keeps its old name: renaming it would fail every compiler that has the old key built in.
pub const DEFAULT_SUMDB: &str = "sum.emela.ab2m.link+ad5364f4+AUu8x/imsHpAJe8Ee9U5GwuaVqG9hijmecEfo53D/FnS https://sum.emela.dev";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A proxy's base URL, without a trailing `/`.
    Proxy(String),
    /// The repository itself, with the machine's `git`.
    Direct,
}

#[derive(Debug, Clone)]
pub struct SumdbConfig {
    pub verifier: Verifier,
    /// The base URL, without a trailing `/`. By default `https://<key name>`.
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Tried in order; empty when `EMELA_POMEPROXY=off`.
    pub sources: Vec<Source>,
    /// `None` when `EMELA_POMESUMDB=off`.
    pub sumdb: Option<SumdbConfig>,
    /// Source path prefixes fetched directly and not checked against the checksum database.
    pub private: Vec<String>,
    pub cache_dir: PathBuf,
}

impl Config {
    /// Reads the environment. `home` is the user's home directory, for the default cache.
    pub fn from_env(
        var: impl Fn(&str) -> Option<String>,
        home: Option<PathBuf>,
    ) -> Result<Config, String> {
        let proxy = var("EMELA_POMEPROXY").unwrap_or_else(|| DEFAULT_PROXY.to_string());
        let sources = if proxy.trim() == "off" {
            Vec::new()
        } else {
            proxy
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| match s {
                    "direct" => Ok(Source::Direct),
                    url if url.starts_with("https://") || url.starts_with("http://") => {
                        Ok(Source::Proxy(url.trim_end_matches('/').to_string()))
                    }
                    other => Err(format!(
                        "EMELA_POMEPROXY: `{other}` is neither a URL nor `direct`"
                    )),
                })
                .collect::<Result<_, _>>()?
        };

        let sumdb = var("EMELA_POMESUMDB").unwrap_or_else(|| DEFAULT_SUMDB.to_string());
        let sumdb = match sumdb.trim() {
            "off" => None,
            value => {
                let (key, url) = match value.split_once(char::is_whitespace) {
                    Some((key, url)) => (key, Some(url.trim().trim_end_matches('/').to_string())),
                    None => (value, None),
                };
                let verifier = Verifier::parse(key).map_err(|e| format!("EMELA_POMESUMDB: {e}"))?;
                let url = url.unwrap_or_else(|| format!("https://{}", verifier.name));
                Some(SumdbConfig { verifier, url })
            }
        };

        let private = var("EMELA_POMEPRIVATE")
            .map(|v| {
                v.split(',')
                    .map(|p| p.trim().trim_end_matches('/').to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let cache_dir = match var("EMELA_POME_CACHE") {
            Some(dir) => PathBuf::from(dir),
            None => home
                .ok_or("cannot find the home directory; set EMELA_POME_CACHE")?
                .join(".emela")
                .join("pomes"),
        };
        Ok(Config {
            sources,
            sumdb,
            private,
            cache_dir,
        })
    }

    /// Whether `path` is under a prefix of `EMELA_POMEPRIVATE`, compared by path elements.
    pub fn is_private(&self, path: &SourcePath) -> bool {
        let p = path.as_str();
        self.private.iter().any(|prefix| {
            p == prefix
                || p.strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// The sources for `path`: only `direct` for a private Pome.
    pub fn sources_for(&self, path: &SourcePath) -> Vec<Source> {
        if self.is_private(path) {
            self.sources
                .iter()
                .filter(|s| **s == Source::Direct)
                .cloned()
                .collect()
        } else {
            self.sources.clone()
        }
    }

    /// The checksum database to check `path` against, unless it is private or the database is off.
    pub fn sumdb_for(&self, path: &SourcePath) -> Option<&SumdbConfig> {
        self.sumdb.as_ref().filter(|_| !self.is_private(path))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const KEY: &str = "sum.example.test+4ac087d0+AepKbGPinFIKvvVQexMuxfmVR3auvr57kkIe6mkURtIs";

    fn config(vars: &[(&str, &str)]) -> Result<Config, String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_env(|k| vars.get(k).cloned(), Some(PathBuf::from("/home/u")))
    }

    #[test]
    fn reads_the_environment() {
        let c = config(&[
            ("EMELA_POMEPROXY", "https://p.example/,direct"),
            ("EMELA_POMESUMDB", KEY),
            ("EMELA_POMEPRIVATE", "github.com/acme, git.corp.example"),
        ])
        .unwrap();
        assert_eq!(
            c.sources,
            [Source::Proxy("https://p.example".into()), Source::Direct]
        );
        assert_eq!(c.sumdb.as_ref().unwrap().url, "https://sum.example.test");
        assert_eq!(c.cache_dir, PathBuf::from("/home/u/.emela/pomes"));

        let p = |s| SourcePath::parse(s).unwrap();
        assert!(c.is_private(&p("github.com/acme/x")) && c.is_private(&p("git.corp.example/a/b")));
        assert!(!c.is_private(&p("github.com/acmex/y")));
        assert_eq!(c.sources_for(&p("github.com/acme/x")), [Source::Direct]);
        assert!(
            c.sumdb_for(&p("github.com/acme/x")).is_none()
                && c.sumdb_for(&p("github.com/other/x")).is_some()
        );
    }

    #[test]
    fn defaults_to_the_public_proxy_and_checksum_database() {
        let c = config(&[]).unwrap();
        assert_eq!(
            c.sources,
            [
                Source::Proxy("https://pomes.emela.dev".into()),
                Source::Direct
            ]
        );
        let sumdb = c.sumdb.unwrap();
        assert_eq!(
            (sumdb.verifier.name.as_str(), sumdb.url.as_str()),
            ("sum.emela.ab2m.link", "https://sum.emela.dev")
        );
    }

    #[test]
    fn turns_things_off() {
        let c = config(&[
            ("EMELA_POMEPROXY", "off"),
            ("EMELA_POMESUMDB", "off"),
            ("EMELA_POME_CACHE", "/c"),
        ])
        .unwrap();
        assert!(c.sources.is_empty() && c.sumdb.is_none());
        assert_eq!(c.cache_dir, PathBuf::from("/c"));
    }

    #[test]
    fn takes_a_sumdb_url() {
        let c = config(&[("EMELA_POMESUMDB", &format!("{KEY} http://127.0.0.1:8787/"))]).unwrap();
        assert_eq!(c.sumdb.unwrap().url, "http://127.0.0.1:8787");
        assert!(config(&[("EMELA_POMEPROXY", "ftp://x")]).is_err());
    }
}
