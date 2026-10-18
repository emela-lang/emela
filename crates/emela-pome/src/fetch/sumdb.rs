//! The checksum database client (spec 4.7).
//!
//! A lookup is trusted only after:
//! 1. the checkpoint in it is signed by the configured key and names that key as its origin,
//! 2. the record is in that checkpoint's tree (rebuilt from tiles),
//! 3. that tree and the largest tree seen before are one a prefix of the other.
//!
//! The largest checkpoint seen is kept in the cache, so a database that shows different trees to
//! different people, or drops records, is caught on a later lookup.

use crate::fetch::FetchError;
use crate::fetch::cache::Cache;
use crate::fetch::config::SumdbConfig;
use crate::fetch::transport::Transport;
use crate::source_path::SourcePath;
use crate::sumdb::note::{self, Checkpoint};
use crate::sumdb::record::{Lookup, Record};
use crate::sumdb::tlog::{self, ProofError, Tile, TileSource};
use crate::version::Version;

pub struct SumdbClient<'a, T: Transport> {
    config: &'a SumdbConfig,
    transport: &'a T,
    cache: &'a Cache,
}

impl<'a, T: Transport> SumdbClient<'a, T> {
    pub fn new(config: &'a SumdbConfig, transport: &'a T, cache: &'a Cache) -> Self {
        SumdbClient {
            config,
            transport,
            cache,
        }
    }

    /// The record of a version. Asking for a version not recorded yet makes the database ingest it.
    pub fn lookup(&self, path: &SourcePath, version: &Version) -> Result<Record, FetchError> {
        let url = format!("{}/lookup/{}@{}", self.config.url, path.escaped(), version);
        let res = self.transport.get(&url).map_err(FetchError::Network)?;
        match res.status {
            200 => {}
            404 | 410 => {
                return Err(FetchError::NotFound(format!(
                    "checksum database: {}",
                    res.text()
                )));
            }
            s => {
                return Err(FetchError::Network(format!(
                    "GET {url}: {s} {}",
                    res.text()
                )));
            }
        }
        let body = String::from_utf8(res.body).map_err(|_| self.bad("the lookup is not UTF-8"))?;
        let lookup = Lookup::parse(&body).ok_or_else(|| self.bad("malformed lookup"))?;
        if &lookup.record.path != path || &lookup.record.version != version {
            return Err(self.bad(&format!(
                "asked for {path}@{version}, got {}@{}",
                lookup.record.path, lookup.record.version
            )));
        }
        let checkpoint = self.open(&lookup.signed_checkpoint)?;
        tlog::verify_inclusion(
            &mut self.tiles(),
            checkpoint.size,
            &checkpoint.root,
            lookup.index,
            lookup.data.as_bytes(),
        )
        .map_err(|e| self.proof(e, "the record is not in the signed tree"))?;
        self.advance(&checkpoint, &lookup.signed_checkpoint)?;
        Ok(lookup.record)
    }

    fn open(&self, signed: &str) -> Result<Checkpoint, FetchError> {
        let text =
            note::open(signed, &self.config.verifier).map_err(|e| self.bad(&e.to_string()))?;
        let checkpoint = Checkpoint::parse(&text).map_err(|e| self.bad(&e.to_string()))?;
        if checkpoint.origin != self.config.verifier.name {
            return Err(self.bad(&format!("checkpoint of `{}`", checkpoint.origin)));
        }
        Ok(checkpoint)
    }

    // Checks the new checkpoint against the largest one seen, and keeps the larger.
    fn advance(&self, new: &Checkpoint, signed: &str) -> Result<(), FetchError> {
        let file = self.cache.checkpoint_file(&self.config.verifier.name);
        let seen = self
            .cache
            .read_file(&file)
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| self.open(&s))
            .transpose()?;
        let Some(seen) = seen else {
            return self
                .cache
                .put_file(&file, signed.as_bytes())
                .map_err(FetchError::Cache);
        };
        let (old, newer) = if seen.size <= new.size {
            (&seen, new)
        } else {
            (new, &seen)
        };
        tlog::verify_consistency(
            &mut self.tiles(),
            old.size,
            &old.root,
            newer.size,
            &newer.root,
        )
        .map_err(|e| {
            self.proof(
                e,
                "the checksum database's tree is not consistent with the one seen before",
            )
        })?;
        if new.size > seen.size {
            self.cache
                .put_file(&file, signed.as_bytes())
                .map_err(FetchError::Cache)?;
        }
        Ok(())
    }

    fn tiles(&self) -> Tiles<'_, T> {
        Tiles {
            url: &self.config.url,
            transport: self.transport,
        }
    }

    fn bad(&self, message: &str) -> FetchError {
        FetchError::Verify(format!(
            "checksum database {}: {message}",
            self.config.verifier.name
        ))
    }

    fn proof(&self, e: ProofError<FetchError>, mismatch: &str) -> FetchError {
        match e {
            ProofError::Source(e) => e,
            ProofError::BadTile(t) => self.bad(&format!("{} has the wrong size", t.path())),
            ProofError::Mismatch => self.bad(mismatch),
        }
    }
}

struct Tiles<'a, T: Transport> {
    url: &'a str,
    transport: &'a T,
}

impl<T: Transport> TileSource for Tiles<'_, T> {
    type Error = FetchError;
    fn tile(&mut self, tile: Tile) -> Result<Vec<u8>, FetchError> {
        let url = format!("{}/{}", self.url, tile.path());
        let res = self.transport.get(&url).map_err(FetchError::Network)?;
        if res.status != 200 {
            return Err(FetchError::Network(format!("GET {url}: {}", res.status)));
        }
        Ok(res.body)
    }
}
