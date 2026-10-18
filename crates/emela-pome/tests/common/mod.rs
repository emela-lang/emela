//! A fake proxy and checksum database held in memory, and local git repositories for `direct`.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use emela_pome::fetch::Fetcher;
use emela_pome::fetch::config::{Config, Source, SumdbConfig};
use emela_pome::fetch::direct::Direct;
use emela_pome::fetch::transport::{Response, Transport};
use emela_pome::sumdb::note::Verifier;
use emela_pome::sumdb::record::Record;
use emela_pome::sumdb::tlog::{Hash, TILE_HEIGHT, TILE_WIDTH, node_hash, record_hash};
use emela_pome::{SourcePath, Version, manifest_hash, tree_hash};

pub const PROXY: &str = "https://proxy.test";
pub const SUMDB: &str = "https://sum.example.test";

pub fn vectors() -> serde_json::Value {
    let path = format!(
        "{}/tests/data/tlog-vectors.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A proxy and a checksum database in memory, signing with the key of the test vectors.
pub struct Registry {
    pub proxy: RefCell<HashMap<String, Vec<u8>>>,
    nodes: RefCell<HashMap<(u32, u64), Hash>>,
    records: RefCell<Vec<(String, String)>>,
    pub requests: RefCell<Vec<String>>,
    key: SigningKey,
    key_hash: [u8; 4],
}

impl Registry {
    pub fn new() -> Registry {
        let v = vectors();
        let signer = v["signerKey"].as_str().unwrap();
        let raw = STANDARD.decode(signer.rsplit('+').next().unwrap()).unwrap();
        let hash = u32::from_str_radix(signer.split('+').nth(3).unwrap(), 16).unwrap();
        Registry {
            proxy: RefCell::default(),
            nodes: RefCell::default(),
            records: RefCell::default(),
            requests: RefCell::default(),
            key: SigningKey::from_bytes(&raw[1..].try_into().unwrap()),
            key_hash: hash.to_be_bytes(),
        }
    }

    /// Records a version in the checksum database and, if `proxy`, serves it from the proxy.
    pub fn add(&self, path: &str, version: &str, files: &[(&str, &str)], proxy: bool) {
        let tree = tree_hash(files.iter().map(|(p, c)| (*p, c.as_bytes()))).unwrap();
        let manifest = manifest_hash(
            files
                .iter()
                .find(|(p, _)| *p == "Pome.toml")
                .unwrap()
                .1
                .as_bytes(),
        );
        let record = Record {
            path: SourcePath::parse(path).unwrap(),
            version: Version::parse(version).unwrap(),
            tree,
            manifest,
        };
        self.append(
            &format!("{}@{version}", record.path.escaped()),
            record.to_text(),
        );
        if proxy {
            self.serve(path, version, files);
        }
    }

    pub fn serve(&self, path: &str, version: &str, files: &[(&str, &str)]) {
        let escaped = SourcePath::parse(path).unwrap().escaped();
        let mut p = self.proxy.borrow_mut();
        let base = format!("{PROXY}/{escaped}/@v");
        p.insert(
            format!("{base}/{version}.info"),
            format!(
                r#"{{"version":"{version}","commit":"{}","time":"2026-10-18T00:00:00Z"}}"#,
                "c".repeat(40)
            )
            .into_bytes(),
        );
        p.insert(
            format!("{base}/{version}.toml"),
            files
                .iter()
                .find(|(f, _)| *f == "Pome.toml")
                .unwrap()
                .1
                .as_bytes()
                .to_vec(),
        );
        p.insert(format!("{base}/{version}.tar.gz"), tar_gz(files));
        let list = p.entry(format!("{base}/list")).or_default();
        list.extend_from_slice(format!("{version}\n").as_bytes());
    }

    fn append(&self, key: &str, data: String) {
        let mut nodes = self.nodes.borrow_mut();
        let mut records = self.records.borrow_mut();
        let (mut level, mut index, mut hash) =
            (0, records.len() as u64, record_hash(data.as_bytes()));
        nodes.insert((0, index), hash);
        while index % 2 == 1 {
            hash = node_hash(&nodes[&(level, index - 1)], &hash);
            level += 1;
            index /= 2;
            nodes.insert((level, index), hash);
        }
        records.push((key.to_string(), data));
    }

    fn checkpoint(&self) -> String {
        let n = self.records.borrow().len() as u64;
        let nodes = self.nodes.borrow();
        // The root of n leaves from the stored complete subtrees, folded from the right.
        let mut subtrees = Vec::new();
        let mut start = 0;
        for level in (0..64).rev() {
            if n - start >= 1 << level {
                subtrees.push(nodes[&(level, start >> level)]);
                start += 1 << level;
            }
        }
        let mut root = subtrees.pop().unwrap();
        while let Some(left) = subtrees.pop() {
            root = node_hash(&left, &root);
        }
        let text = format!("sum.example.test\n{n}\n{}\n", STANDARD.encode(root));
        let sig = self.key.sign(text.as_bytes());
        let mut blob = self.key_hash.to_vec();
        blob.extend_from_slice(&sig.to_bytes());
        format!(
            "{text}\n\u{2014} sum.example.test {}\n",
            STANDARD.encode(blob)
        )
    }

    pub fn requested(&self, prefix: &str) -> usize {
        self.requests
            .borrow()
            .iter()
            .filter(|u| u.starts_with(prefix))
            .count()
    }
}

impl Transport for Registry {
    fn get(&self, url: &str) -> Result<Response, String> {
        self.requests.borrow_mut().push(url.to_string());
        let not_found = || {
            Ok(Response {
                status: 404,
                body: b"not found".to_vec(),
            })
        };
        if let Some(body) = self.proxy.borrow().get(url) {
            return Ok(Response {
                status: 200,
                body: body.clone(),
            });
        }
        let Some(rest) = url.strip_prefix(&format!("{SUMDB}/")) else {
            return not_found();
        };
        if let Some(key) = rest.strip_prefix("lookup/") {
            let records = self.records.borrow();
            let Some(i) = records.iter().position(|(k, _)| k == key) else {
                return not_found();
            };
            let body = format!("{i}\n{}\n{}", records[i].1, self.checkpoint());
            return Ok(Response {
                status: 200,
                body: body.into_bytes(),
            });
        }
        if let Some(tile) = rest.strip_prefix("tile/") {
            let (tile, width) = tile
                .split_once(".p/")
                .map_or((tile, TILE_WIDTH), |(t, w)| (t, w.parse().unwrap()));
            let (level, index) = tile.split_once('/').unwrap();
            let level: u32 = level.parse().unwrap();
            let index: u64 = index
                .split('/')
                .map(|g| g.trim_start_matches('x'))
                .collect::<String>()
                .parse()
                .unwrap();
            let nodes = self.nodes.borrow();
            let mut body = Vec::new();
            for i in 0..width {
                match nodes.get(&(level * TILE_HEIGHT, index * TILE_WIDTH + i)) {
                    Some(h) => body.extend_from_slice(h),
                    None => return not_found(),
                }
            }
            return Ok(Response { status: 200, body });
        }
        not_found()
    }
}

pub fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, content.as_bytes())
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

pub fn verifier() -> Verifier {
    Verifier::parse(vectors()["verifierKey"].as_str().unwrap()).unwrap()
}

pub fn fetcher<'a>(registry: &'a Registry, cache: &Path, repos: &Path) -> Fetcher<&'a Registry> {
    let config = Config {
        sources: vec![Source::Proxy(PROXY.into()), Source::Direct],
        sumdb: Some(SumdbConfig {
            verifier: verifier(),
            url: SUMDB.into(),
        }),
        private: vec!["github.com/private".into()],
        cache_dir: cache.to_path_buf(),
    };
    let repos = repos.to_path_buf();
    Fetcher::new(config, registry).with_direct(Direct {
        url: Box::new(move |r| format!("file://{}", repos.join(r).display())),
    })
}

impl Transport for &Registry {
    fn get(&self, url: &str) -> Result<Response, String> {
        (*self).get(url)
    }
}

pub fn pome(name: &str, version: &str, deps: &str) -> String {
    format!(
        "[pome]\nname = \"{name}\"\nversion = \"{}\"\nemela = \"0.20.1\"\n\n[dependencies]\n{deps}",
        &version[1..]
    )
}

pub fn p(s: &str) -> SourcePath {
    SourcePath::parse(s).unwrap()
}

pub fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

/// A git repository with one commit per (version, files) and a tag for each.
pub fn git_repo(root: &Path, repo: &str, versions: &[(&str, &[(&str, &str)])]) {
    let dir = root.join(repo);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    for (version, files) in versions {
        for (path, content) in *files {
            let f = dir.join(path);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, content).unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", version]);
        git(&["tag", version]);
    }
}

pub struct Dirs {
    _tmp: tempfile::TempDir,
    pub cache: PathBuf,
    pub repos: PathBuf,
}

// The cache is read-only; make it writable again so that the temporary directory can be removed.
impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = Command::new("chmod")
            .args(["-R", "u+w"])
            .arg(self._tmp.path())
            .status();
    }
}

pub fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let (cache, repos) = (tmp.path().join("cache"), tmp.path().join("repos"));
    Dirs {
        _tmp: tmp,
        cache,
        repos,
    }
}
