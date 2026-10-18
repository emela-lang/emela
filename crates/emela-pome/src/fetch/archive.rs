//! The proxy's archive (`.tar.gz`): regular files with paths relative to the Pome's root.

use std::io::Read;

use flate2::read::GzDecoder;
use tar::EntryType;

/// The limits a version must fit in (spec 4.7).
pub const MAX_FILES: usize = 10_000;
pub const MAX_TREE_BYTES: u64 = 32 * 1024 * 1024;

pub type Files = Vec<(String, Vec<u8>)>;

/// The files of an archive. Anything but regular files, and anything over the limits, is an error.
pub fn read_tar_gz(bytes: &[u8]) -> Result<Files, String> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in archive.entries().map_err(|e| format!("archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("archive: {e}"))?;
        match entry.header().entry_type() {
            EntryType::Regular | EntryType::Continuous => {}
            other => return Err(format!("archive: unexpected entry type {other:?}")),
        }
        let path = entry.path().map_err(|e| format!("archive: {e}"))?;
        let path = path
            .to_str()
            .ok_or("archive: a path is not UTF-8")?
            .to_string();
        total += entry.size();
        if files.len() >= MAX_FILES || total > MAX_TREE_BYTES {
            return Err(format!(
                "archive: more than {MAX_FILES} files or {MAX_TREE_BYTES} bytes"
            ));
        }
        let mut content = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut content)
            .map_err(|e| format!("archive: {e}"))?;
        files.push((path, content));
    }
    Ok(files)
}
