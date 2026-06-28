pub(crate) mod cache;
pub(crate) mod fetch;
pub(crate) mod imports;
pub(crate) mod manifest;
pub(crate) mod resolve;

use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct PackageSource {
    pub(crate) name: String,
    pub(crate) source_root: PathBuf,
}
