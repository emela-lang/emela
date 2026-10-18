//! The test vectors shared with pome-registry (the proxy and checksum database): `h1:` hashes, tree
//! roots and tiles, and a signed checkpoint. The same files pass the TypeScript implementation.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use emela_pome::sumdb::note::{self, Checkpoint, Verifier};
use emela_pome::sumdb::tlog::{
    self, Hash, ProofError, TILE_HEIGHT, TILE_WIDTH, Tile, TileSource, TreeReader, node_hash,
    record_hash,
};
use emela_pome::{manifest_hash, tree_hash};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn data(name: &str) -> Value {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn h1_vectors() {
    for v in data("hash-vectors.json")["vectors"].as_array().unwrap() {
        let files: Vec<(String, Vec<u8>)> = v["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["path"].as_str().unwrap().to_string(),
                    STANDARD.decode(f["base64"].as_str().unwrap()).unwrap(),
                )
            })
            .collect();
        let h = tree_hash(files.iter().map(|(p, c)| (p.as_str(), c.as_slice()))).unwrap();
        assert_eq!(h.as_str(), v["h1"].as_str().unwrap(), "{}", v["name"]);
        if let [(p, c)] = files.as_slice()
            && p == "Pome.toml"
        {
            assert_eq!(manifest_hash(c), h);
        }
    }
}

#[test]
fn h1_rejects_bad_paths() {
    for p in ["", "a/../b", "/abs", "a\\b", ".git/config", "a\nb"] {
        assert!(tree_hash([(p, b"x".as_slice())]).is_err(), "{p:?}");
    }
    assert!(tree_hash([("a", b"x".as_slice()), ("a", b"y".as_slice())]).is_err());
}

// Record i of the vectors: the same as `rec(i)` that made them.
fn rec(i: u64) -> Vec<u8> {
    let b64 = |s: String| STANDARD.encode(Sha256::digest(s.as_bytes()));
    format!(
        "example.com/p{i} v1.0.{i} h1:{}\nexample.com/p{i} v1.0.{i}/Pome.toml h1:{}\n",
        b64(format!("tree{i}")),
        b64(format!("toml{i}"))
    )
    .into_bytes()
}

/// A tree held in memory that serves tiles, as the checksum database does.
#[derive(Default)]
struct MemoryTree {
    size: u64,
    nodes: HashMap<(u32, u64), Hash>,
    requests: usize,
}

impl MemoryTree {
    fn append(&mut self, record: &[u8]) {
        let (mut level, mut index, mut hash) = (0, self.size, record_hash(record));
        self.nodes.insert((0, index), hash);
        while index % 2 == 1 {
            hash = node_hash(&self.nodes[&(level, index - 1)], &hash);
            level += 1;
            index /= 2;
            self.nodes.insert((level, index), hash);
        }
        self.size += 1;
    }
}

impl TileSource for MemoryTree {
    type Error = String;
    fn tile(&mut self, t: Tile) -> Result<Vec<u8>, String> {
        self.requests += 1;
        let mut out = Vec::new();
        for i in 0..t.width {
            let key = (t.level * TILE_HEIGHT, t.index * TILE_WIDTH + i);
            out.extend_from_slice(
                self.nodes
                    .get(&key)
                    .ok_or_else(|| format!("no node {key:?}"))?,
            );
        }
        Ok(out)
    }
}

#[test]
fn tree_roots_and_tiles_match_the_vectors() {
    let vectors = data("tlog-vectors.json");
    let mut tree = MemoryTree::default();
    for v in vectors["sizes"].as_array().unwrap() {
        let size = v["size"].as_u64().unwrap();
        while tree.size < size {
            tree.append(&rec(tree.size));
        }
        let root = TreeReader::new(size, &mut tree).root(size).unwrap();
        assert_eq!(
            STANDARD.encode(root),
            v["root"].as_str().unwrap(),
            "root of {size}"
        );
        for t in v["tiles"].as_array().unwrap() {
            let path = t["path"].as_str().unwrap();
            let tile = parse_tile(path);
            assert_eq!(tile.path(), path);
            let bytes = tree.tile(tile).unwrap();
            let digest: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(digest, t["sha256"].as_str().unwrap(), "{path}");
        }
    }
}

fn parse_tile(path: &str) -> Tile {
    let rest = path.strip_prefix("tile/").unwrap();
    let (rest, width) = match rest.split_once(".p/") {
        Some((r, w)) => (r, w.parse().unwrap()),
        None => (rest, TILE_WIDTH),
    };
    let (level, index) = rest.split_once('/').unwrap();
    let index = index
        .split('/')
        .map(|g| g.trim_start_matches('x'))
        .collect::<String>()
        .parse()
        .unwrap();
    Tile {
        level: level.parse().unwrap(),
        index,
        width,
    }
}

#[test]
fn proves_inclusion_and_consistency_from_tiles() {
    let mut tree = MemoryTree::default();
    for i in 0..70_000 {
        tree.append(&rec(i));
    }
    for (old, new) in [
        (1, 2),
        (5, 300),
        (256, 257),
        (300, 65_536),
        (65_535, 70_000),
        (70_000, 70_000),
    ] {
        let old_root = TreeReader::new(new, &mut tree).root(old).unwrap();
        let new_root = TreeReader::new(new, &mut tree).root(new).unwrap();
        tlog::verify_consistency(&mut tree, old, &old_root, new, &new_root).unwrap();
        let mut forked = old_root;
        forked[0] ^= 1;
        assert!(matches!(
            tlog::verify_consistency(&mut tree, old, &forked, new, &new_root),
            Err(ProofError::Mismatch)
        ));
    }
    let size = 70_000;
    let root = TreeReader::new(size, &mut tree).root(size).unwrap();
    for index in [0, 1, 255, 256, 65_535, 65_536, 69_999] {
        tlog::verify_inclusion(&mut tree, size, &root, index, &rec(index)).unwrap();
        assert!(matches!(
            tlog::verify_inclusion(&mut tree, size, &root, index, &rec(index + 1)),
            Err(ProofError::Mismatch)
        ));
    }
    assert!(tlog::verify_inclusion(&mut tree, size, &root, size, &rec(size)).is_err());

    // A proof reads one tile per level and a few partial ones, not the whole tree.
    tree.requests = 0;
    tlog::verify_inclusion(&mut tree, size, &root, 12_345, &rec(12_345)).unwrap();
    assert!(tree.requests <= 6, "{} tiles", tree.requests);
}

#[test]
fn opens_the_signed_checkpoint() {
    let vectors = data("tlog-vectors.json");
    let verifier = Verifier::parse(vectors["verifierKey"].as_str().unwrap()).unwrap();
    let signed = vectors["note"].as_str().unwrap();
    let checkpoint = Checkpoint::parse(&note::open(signed, &verifier).unwrap()).unwrap();
    let last = vectors["sizes"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(checkpoint.origin, "sum.example.test");
    assert_eq!(checkpoint.size, last["size"].as_u64().unwrap());
    assert_eq!(
        STANDARD.encode(checkpoint.root),
        last["root"].as_str().unwrap()
    );
    assert_eq!(checkpoint.to_text(), note::open(signed, &verifier).unwrap());

    assert!(note::open(&signed.replace("66000", "66001"), &verifier).is_err());
    let other =
        Verifier::parse("sum.example.test+c79654e5+ARIB6erYuoWRiulHCdLfU1QW+bA2sSHVe94B3ALbymq2")
            .unwrap();
    assert!(note::open(signed, &other).is_err());
    assert!(
        Verifier::parse(
            &vectors["verifierKey"]
                .as_str()
                .unwrap()
                .replace("+4ac087d0+", "+00000000+")
        )
        .is_err()
    );
}

/// An archive written by the proxy (pome-registry's `writeTar`): long and non-ASCII paths go in pax
/// headers.
#[test]
fn reads_the_proxys_archive() {
    let bytes = std::fs::read(format!(
        "{}/tests/data/proxy-archive.tar.gz",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let files = emela_pome::fetch::archive::read_tar_gz(&bytes).unwrap();
    let mut paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "Pome.toml",
            &format!("src/{}.emel", "long".repeat(40)),
            "src/日本語.emel"
        ]
    );
    let h = tree_hash(files.iter().map(|(p, c)| (p.as_str(), c.as_slice()))).unwrap();
    assert_eq!(
        h.as_str(),
        "h1:Z+09YW3KZFvDa5RF8Kz9g2PZ2mbWmrwt3vZcVIzgjUo="
    );
}
