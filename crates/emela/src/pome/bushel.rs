//! The workspace file, `Bushel.toml` (spec 0032 F9-F10).
//!
//! A Bushel bundles several Pomes developed together. It lists its member Pome
//! directories and shares a single `Pome.lock` so member dependency versions
//! stay consistent (F10). Discovery of the enclosing Bushel lets commands run
//! from any member resolve against the shared lock.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::toml_lite;

/// The file name a Bushel always uses (spec 0032 F9).
pub(crate) const FILE_NAME: &str = "Bushel.toml";

/// A parsed `Bushel.toml`.
#[derive(Debug, Clone)]
pub(crate) struct Bushel {
    /// The directory containing `Bushel.toml`.
    pub(crate) root: PathBuf,
    /// Member Pome directories, relative to `root` (spec 0032 F9).
    pub(crate) members: Vec<String>,
}

impl Bushel {
    /// Loads the `Bushel.toml` located directly in `dir`, if any.
    pub(crate) fn load(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(FILE_NAME);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(Error::new(format!(
                    "failed to read `{}`: {err}",
                    path.display()
                )));
            }
        };
        Ok(Some(Self::parse(&source, dir)?))
    }

    fn parse(source: &str, root: &Path) -> Result<Self> {
        let doc = toml_lite::parse(source).map_err(|err| {
            Error::new(format!(
                "failed to parse `{}`: {err}",
                root.join(FILE_NAME).display()
            ))
        })?;
        // Accept `members` either at the top level via a `[bushel]` table.
        let members = doc
            .table("bushel")
            .and_then(|table| table.get_array("members"))
            .map(|members| members.to_vec())
            .ok_or_else(|| {
                Error::new(format!(
                    "`{}` is missing `[bushel].members`",
                    root.join(FILE_NAME).display()
                ))
            })?;
        Ok(Bushel {
            root: root.to_path_buf(),
            members,
        })
    }

    /// The absolute directory of each member Pome.
    pub(crate) fn member_dirs(&self) -> Vec<PathBuf> {
        self.members.iter().map(|m| self.root.join(m)).collect()
    }
}

/// Walks up from `start` looking for the nearest enclosing `Bushel.toml`. A
/// command run inside a member resolves against the workspace's shared lock
/// (spec 0032 F10). Returns `None` when the Pome is standalone.
pub(crate) fn discover(start: &Path) -> Result<Option<Bushel>> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        if let Some(bushel) = Bushel::load(current)? {
            return Ok(Some(bushel));
        }
        dir = current.parent();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_members() {
        let bushel = Bushel::parse(
            "[bushel]\nmembers = [\"core\", \"cli\"]\n",
            Path::new("/ws"),
        )
        .unwrap();
        assert_eq!(bushel.members, ["core", "cli"]);
        assert_eq!(
            bushel.member_dirs(),
            vec![PathBuf::from("/ws/core"), PathBuf::from("/ws/cli")]
        );
    }

    #[test]
    fn missing_members_is_rejected() {
        assert!(Bushel::parse("[bushel]\n", Path::new("/ws")).is_err());
    }
}
