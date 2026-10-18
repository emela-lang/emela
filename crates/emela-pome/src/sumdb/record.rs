//! Records of the checksum database: two lines per version.
//!
//! ```text
//! github.com/emela-lang/quince v0.3.2 h1:...
//! github.com/emela-lang/quince v0.3.2/Pome.toml h1:...
//! ```

use crate::hash::{H1, MANIFEST_FILE};
use crate::source_path::SourcePath;
use crate::version::Version;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub path: SourcePath,
    pub version: Version,
    pub tree: H1,
    pub manifest: H1,
}

impl Record {
    pub fn to_text(&self) -> String {
        format!(
            "{p} {v} {t}\n{p} {v}/{MANIFEST_FILE} {m}\n",
            p = self.path,
            v = self.version,
            t = self.tree,
            m = self.manifest
        )
    }

    /// A record in canonical form.
    pub fn parse(text: &str) -> Option<Record> {
        let mut lines = text.strip_suffix('\n')?.split('\n');
        let (tree, manifest) = (lines.next()?, lines.next()?);
        if lines.next().is_some() {
            return None;
        }
        let [path, version, tree] = fields(tree)?;
        let [path2, version2, manifest] = fields(manifest)?;
        if path != path2 || version2 != format!("{version}/{MANIFEST_FILE}") {
            return None;
        }
        let record = Record {
            path: SourcePath::parse(path).ok()?,
            version: Version::parse(version).ok()?,
            tree: H1::parse(tree).ok()?,
            manifest: H1::parse(manifest).ok()?,
        };
        (record.to_text() == text).then_some(record)
    }
}

/// The body of `lookup/<path>@<version>`: the record's index, the record, and a signed checkpoint
/// of a tree that includes it.
pub struct Lookup {
    pub index: u64,
    pub record: Record,
    /// The raw record, the leaf's data.
    pub data: String,
    pub signed_checkpoint: String,
}

impl Lookup {
    pub fn parse(body: &str) -> Option<Lookup> {
        let (index, rest) = body.split_once('\n')?;
        if index.is_empty() || (index.len() > 1 && index.starts_with('0')) {
            return None;
        }
        let index = index.parse().ok()?;
        // The record is two lines; a blank line separates it from the note.
        let mut ends = rest.match_indices('\n').map(|(i, _)| i);
        let (_, second) = (ends.next()?, ends.next()?);
        let data = &rest[..second + 1];
        let note = rest[second + 1..].strip_prefix('\n')?;
        Some(Lookup {
            index,
            record: Record::parse(data)?,
            data: data.to_string(),
            signed_checkpoint: note.to_string(),
        })
    }
}

fn fields(line: &str) -> Option<[&str; 3]> {
    let v: Vec<&str> = line.split(' ').collect();
    v.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_record_and_a_lookup() {
        let h = "h1:+Thj6XlK+eABClH7yw779GrFFl/umwEs9PO0k3JjISY=";
        let text =
            format!("github.com/acme/x v1.0.0 {h}\ngithub.com/acme/x v1.0.0/Pome.toml {h}\n");
        let record = Record::parse(&text).unwrap();
        assert_eq!(record.to_text(), text);
        let lookup = Lookup::parse(&format!(
            "7\n{text}\nsum.example.test\n1\nAAAA\n\n\u{2014} k sig\n"
        ))
        .unwrap();
        assert_eq!(
            (lookup.index, lookup.record, lookup.data),
            (7, record, text.clone())
        );
        assert!(lookup.signed_checkpoint.starts_with("sum.example.test\n"));
        assert!(Record::parse(&text.replace("v1.0.0/Pome.toml", "v1.0.1/Pome.toml")).is_none());
    }
}
