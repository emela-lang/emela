//! The checksum database's tree: an RFC 6962 Merkle tree published as tiles (C2SP tlog-tiles).
//!
//!   leaf = sha256(0x00 || record)
//!   node = sha256(0x01 || left || right)
//!
//! A tile of level L holds the hashes of the complete subtrees of height 8L, 256 to a tile. Any
//! complete subtree is folded from the hashes of one tile. Proofs are not sent: the client rebuilds
//! the root of a signed checkpoint from tiles, so tile contents are trusted only once the rebuilt
//! root matches the signed one.
//!
//! - Inclusion: the root of size n, with the leaf at the record's index recomputed from the record.
//! - Consistency: the roots of the old size and the new size, both from the tiles of the new tree.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

pub const TILE_HEIGHT: u32 = 8;
pub const TILE_WIDTH: u64 = 1 << TILE_HEIGHT;

pub fn record_hash(record: &[u8]) -> Hash {
    let mut h = Sha256::new();
    h.update([0x00]);
    h.update(record);
    h.finalize().into()
}

pub fn node_hash(left: &Hash, right: &Hash) -> Hash {
    let mut h = Sha256::new();
    h.update([0x01]);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

/// A tile, as named in URLs: `tile/<level>/<index>[.p/<width>]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tile {
    pub level: u32,
    pub index: u64,
    pub width: u64,
}

impl Tile {
    /// The path under the checksum database: the index in groups of three digits, all but the last
    /// prefixed with `x` (1234067 → x001/x234/067).
    pub fn path(&self) -> String {
        let mut groups = Vec::new();
        let mut n = self.index;
        loop {
            groups.push(format!("{:03}", n % 1000));
            n /= 1000;
            if n == 0 {
                break;
            }
        }
        groups.reverse();
        let last = groups.len() - 1;
        let index: Vec<String> = groups
            .into_iter()
            .enumerate()
            .map(|(i, g)| if i < last { format!("x{g}") } else { g })
            .collect();
        let partial = if self.width < TILE_WIDTH {
            format!(".p/{}", self.width)
        } else {
            String::new()
        };
        format!("tile/{}/{}{partial}", self.level, index.join("/"))
    }
}

/// Where tiles come from (the network, a cache, or memory in tests).
pub trait TileSource {
    type Error;
    fn tile(&mut self, tile: Tile) -> Result<Vec<u8>, Self::Error>;
}

#[derive(Debug)]
pub enum ProofError<E> {
    /// The tile source failed.
    Source(E),
    /// A tile had the wrong length.
    BadTile(Tile),
    /// The rebuilt root differs from the signed one.
    Mismatch,
}

/// Hashes of the tree of a given size, read from tiles.
pub struct TreeReader<'a, S: TileSource> {
    size: u64,
    source: &'a mut S,
    tiles: HashMap<Tile, Vec<u8>>,
}

impl<'a, S: TileSource> TreeReader<'a, S> {
    pub fn new(size: u64, source: &'a mut S) -> Self {
        TreeReader {
            size,
            source,
            tiles: HashMap::new(),
        }
    }

    /// The root of the first `n` leaves (n ≤ size).
    pub fn root(&mut self, n: u64) -> Result<Hash, ProofError<S::Error>> {
        if n == 0 {
            return Ok(Sha256::digest([]).into());
        }
        self.range(0, n, None)
    }

    /// The root of the first `n` leaves with leaf `index` replaced by `leaf`.
    pub fn root_with_leaf(
        &mut self,
        n: u64,
        index: u64,
        leaf: Hash,
    ) -> Result<Hash, ProofError<S::Error>> {
        self.range(0, n, Some((index, leaf)))
    }

    // RFC 6962 MTH over leaves [lo, hi): split at the largest power of two below the width. An
    // aligned power-of-two range not containing the replaced leaf is one stored subtree.
    fn range(
        &mut self,
        lo: u64,
        hi: u64,
        leaf: Option<(u64, Hash)>,
    ) -> Result<Hash, ProofError<S::Error>> {
        let width = hi - lo;
        let leaf = leaf.filter(|(i, _)| (lo..hi).contains(i));
        if let Some((_, h)) = leaf.filter(|_| width == 1) {
            return Ok(h);
        }
        if leaf.is_none() && width.is_power_of_two() && lo.is_multiple_of(width) {
            return self.subtree(width.trailing_zeros(), lo / width);
        }
        let k = 1u64 << (63 - (width - 1).leading_zeros());
        let left = self.range(lo, lo + k, leaf)?;
        let right = self.range(lo + k, hi, leaf)?;
        Ok(node_hash(&left, &right))
    }

    // The complete subtree of height `height` at `index`: 2^(height mod 8) hashes of one tile of
    // level height / 8, folded.
    fn subtree(&mut self, height: u32, index: u64) -> Result<Hash, ProofError<S::Error>> {
        let level = height / TILE_HEIGHT;
        let span = 1u64 << (height % TILE_HEIGHT);
        let first = index * span;
        let tile_index = first / TILE_WIDTH;
        let count = self.size >> (level * TILE_HEIGHT);
        let tile = Tile {
            level,
            index: tile_index,
            width: (count - tile_index * TILE_WIDTH).min(TILE_WIDTH),
        };
        let data = match self.tiles.get(&tile) {
            Some(d) => d,
            None => {
                let d = self.source.tile(tile).map_err(ProofError::Source)?;
                if d.len() as u64 != tile.width * 32 {
                    return Err(ProofError::BadTile(tile));
                }
                self.tiles.entry(tile).or_insert(d)
            }
        };
        let start = (first - tile_index * TILE_WIDTH) as usize;
        let mut hashes: Vec<Hash> = (start..start + span as usize)
            .map(|i| data[i * 32..i * 32 + 32].try_into().expect("32 bytes"))
            .collect();
        while hashes.len() > 1 {
            hashes = hashes.chunks(2).map(|p| node_hash(&p[0], &p[1])).collect();
        }
        Ok(hashes[0])
    }
}

/// Whether `record` is leaf `index` of the tree whose signed root is `root`.
pub fn verify_inclusion<S: TileSource>(
    source: &mut S,
    size: u64,
    root: &Hash,
    index: u64,
    record: &[u8],
) -> Result<(), ProofError<S::Error>> {
    if index >= size {
        return Err(ProofError::Mismatch);
    }
    let rebuilt = TreeReader::new(size, source).root_with_leaf(size, index, record_hash(record))?;
    if &rebuilt == root {
        Ok(())
    } else {
        Err(ProofError::Mismatch)
    }
}

/// Whether the tree of `old_size` is a prefix of the tree of `new_size`.
pub fn verify_consistency<S: TileSource>(
    source: &mut S,
    old_size: u64,
    old_root: &Hash,
    new_size: u64,
    new_root: &Hash,
) -> Result<(), ProofError<S::Error>> {
    if old_size > new_size {
        return Err(ProofError::Mismatch);
    }
    let mut tree = TreeReader::new(new_size, source);
    if &tree.root(new_size)? != new_root || &tree.root(old_size)? != old_root {
        return Err(ProofError::Mismatch);
    }
    Ok(())
}
