//! Pome (spec 4.7): the unit of distribution and dependency.
//!
//! This crate holds what does not touch the network: source paths and versions, `Pome.toml` and
//! `Pome.lock`, minimal version selection, the content hash `h1:`, and verification of what the
//! checksum database says.

pub mod hash;
pub mod lock;
pub mod manifest;
pub mod mvs;
pub mod source_path;
pub mod sumdb;
pub mod version;

pub use hash::{H1, manifest_hash, tree_hash};
pub use lock::{Lock, Locked};
pub use manifest::{Manifest, Requirement};
pub use source_path::SourcePath;
pub use version::Version;
