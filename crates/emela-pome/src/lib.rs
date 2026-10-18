//! Pome (spec 4.7): the unit of distribution and dependency.
//!
//! - Source paths and versions, `Pome.toml` and `Pome.lock`, minimal version selection, the content
//!   hash `h1:`, and verification of what the checksum database says. None of these touch the
//!   network.
//! - [`fetch`]: getting versions from proxies and `direct`, checked against the checksum database
//!   or the lock, and the cache.

pub mod edit;
pub mod fetch;
pub mod hash;
pub mod lock;
pub mod manifest;
pub mod mvs;
pub mod project;
pub mod source_path;
pub mod sumdb;
pub mod version;

pub use hash::{H1, manifest_hash, tree_hash};
pub use lock::{Lock, Locked};
pub use manifest::{Manifest, Requirement};
pub use source_path::SourcePath;
pub use version::Version;
