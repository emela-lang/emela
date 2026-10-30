use super::contract::PackageFile;
use sha2::{Digest, Sha256};
pub(super) fn digest(files: &[PackageFile]) -> String {
    let mut hash = Sha256::new();
    for file in files {
        hash.update((file.path.len() as u64).to_le_bytes());
        hash.update(file.path.as_bytes());
        hash.update((file.contents.len() as u64).to_le_bytes());
        hash.update(&file.contents);
    }
    format!("{:x}", hash.finalize())
}
