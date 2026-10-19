//! Rewriting `[dependencies]` and `[dev-dependencies]` of a `Pome.toml` in place, keeping
//! comments, order and the form of each entry (a string or a table with `module`).

use toml_edit::{DocumentMut, Item, Table, value};

use crate::manifest::ManifestError;
use crate::source_path::SourcePath;
use crate::version::Version;

pub const DEPENDENCIES: &str = "dependencies";
pub const DEV_DEPENDENCIES: &str = "dev-dependencies";

/// Sets the requirement on `path` in the table `table` (`DEPENDENCIES` or `DEV_DEPENDENCIES`) to
/// `version`, adding the entry if there is none.
pub fn set_dependency(
    text: &str,
    table: &str,
    path: &SourcePath,
    version: &Version,
) -> Result<String, ManifestError> {
    let mut doc = parse(text)?;
    let deps = dependencies(&mut doc, table)?;
    match deps.get_mut(path.as_str()) {
        Some(Item::Value(toml_edit::Value::InlineTable(t))) => {
            t.insert("version", version.as_str().into());
        }
        Some(Item::Table(t)) => {
            t.insert("version", value(version.as_str()));
        }
        Some(item) => {
            let decor = item.as_value().map(|v| v.decor().clone());
            *item = value(version.as_str());
            if let (Some(decor), Some(v)) = (decor, item.as_value_mut()) {
                *v.decor_mut() = decor;
            }
        }
        None => {
            deps.insert(path.as_str(), value(version.as_str()));
        }
    }
    Ok(doc.to_string())
}

/// Removes the entry for `path` from the table `table`. `None` if there was none.
pub fn remove_dependency(
    text: &str,
    table: &str,
    path: &SourcePath,
) -> Result<Option<String>, ManifestError> {
    let mut doc = parse(text)?;
    let removed = dependencies(&mut doc, table)?
        .remove(path.as_str())
        .is_some();
    drop_if_empty(&mut doc, table);
    Ok(removed.then(|| doc.to_string()))
}

/// Moves the entry for `path`, as written, from the table `from` to the table `to`. `None` if
/// `from` has no entry for it.
pub fn move_dependency(
    text: &str,
    path: &SourcePath,
    from: &str,
    to: &str,
) -> Result<Option<String>, ManifestError> {
    let mut doc = parse(text)?;
    let Some((key, item)) = dependencies(&mut doc, from)?.remove_entry(path.as_str()) else {
        return Ok(None);
    };
    dependencies(&mut doc, to)?.insert_formatted(&key, item);
    drop_if_empty(&mut doc, from);
    Ok(Some(doc.to_string()))
}

// `[dev-dependencies]` goes away with its last entry; `[dependencies]` stays as written.
fn drop_if_empty(doc: &mut DocumentMut, table: &str) {
    if table == DEV_DEPENDENCIES
        && doc
            .get(table)
            .and_then(Item::as_table)
            .is_some_and(Table::is_empty)
    {
        doc.remove(table);
    }
}

/// Adds source paths to `[external] allow`, creating the table and the array if needed.
pub fn allow_external(text: &str, paths: &[SourcePath]) -> Result<String, ManifestError> {
    let mut doc = parse(text)?;
    let external = doc
        .entry("external")
        .or_insert_with(|| Item::Table(Table::new()));
    let table = external
        .as_table_like_mut()
        .ok_or_else(|| ManifestError("`external` must be a table".into()))?;
    let allow = table
        .entry("allow")
        .or_insert(value(toml_edit::Array::new()));
    let array = allow
        .as_array_mut()
        .ok_or_else(|| ManifestError("`external.allow` must be an array".into()))?;
    for path in paths {
        if !array.iter().any(|v| v.as_str() == Some(path.as_str())) {
            array.push(path.as_str());
        }
    }
    Ok(doc.to_string())
}

fn parse(text: &str) -> Result<DocumentMut, ManifestError> {
    text.parse()
        .map_err(|e: toml_edit::TomlError| ManifestError(e.to_string()))
}

fn dependencies<'a>(doc: &'a mut DocumentMut, table: &str) -> Result<&'a mut Table, ManifestError> {
    let item = doc
        .entry(table)
        .or_insert_with(|| Item::Table(Table::new()));
    item.as_table_mut()
        .ok_or_else(|| ManifestError(format!("`{table}` must be a table")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"# The app.
[pome]
name = "github.com/me/app"
version = "0.1.0"
emela = "0.20.1"

[dependencies]
# Routing.
"github.com/acme/router" = "v0.2.0" # keep this comment
"github.com/acme/http" = { version = "v1.4.0", module = "AcmeHttp" }

[build]
target = "c"
"#;

    fn p(s: &str) -> SourcePath {
        SourcePath::parse(s).unwrap()
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn updates_in_place() {
        let out = set_dependency(
            BASE,
            DEPENDENCIES,
            &p("github.com/acme/router"),
            &v("v0.3.0"),
        )
        .unwrap();
        assert_eq!(out, BASE.replace(r#"= "v0.2.0" #"#, r#"= "v0.3.0" #"#));
        let out =
            set_dependency(BASE, DEPENDENCIES, &p("github.com/acme/http"), &v("v1.5.0")).unwrap();
        assert!(
            out.contains(r#""github.com/acme/http" = { version = "v1.5.0", module = "AcmeHttp" }"#),
            "{out}"
        );
    }

    #[test]
    fn adds_and_removes() {
        let out =
            set_dependency(BASE, DEPENDENCIES, &p("github.com/acme/json"), &v("v1.0.0")).unwrap();
        assert!(
            out.contains("\"github.com/acme/json\" = \"v1.0.0\"\n\n[build]"),
            "{out}"
        );
        assert_eq!(
            remove_dependency(&out, DEPENDENCIES, &p("github.com/acme/json"))
                .unwrap()
                .unwrap(),
            BASE
        );
        assert_eq!(
            remove_dependency(BASE, DEPENDENCIES, &p("github.com/acme/none")).unwrap(),
            None
        );
    }

    #[test]
    fn creates_the_table() {
        let bare =
            "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.20.1\"\n";
        let out =
            set_dependency(bare, DEPENDENCIES, &p("github.com/acme/x"), &v("v1.0.0")).unwrap();
        assert_eq!(
            out,
            format!("{bare}\n[dependencies]\n\"github.com/acme/x\" = \"v1.0.0\"\n")
        );
    }

    #[test]
    fn allows_external() {
        let out = allow_external(BASE, &[p("github.com/acme/cf")]).unwrap();
        assert!(
            out.ends_with("[external]\nallow = [\"github.com/acme/cf\"]\n"),
            "{out}"
        );
        let again =
            allow_external(&out, &[p("github.com/acme/cf"), p("github.com/acme/d1")]).unwrap();
        assert!(
            again.ends_with("allow = [\"github.com/acme/cf\", \"github.com/acme/d1\"]\n"),
            "{again}"
        );
    }

    #[test]
    fn moves_between_tables() {
        let out = set_dependency(
            BASE,
            DEV_DEPENDENCIES,
            &p("github.com/emela-lang/brix"),
            &v("v0.2.0"),
        )
        .unwrap();
        assert!(
            out.ends_with("[dev-dependencies]\n\"github.com/emela-lang/brix\" = \"v0.2.0\"\n"),
            "{out}"
        );
        let moved = move_dependency(
            &out,
            &p("github.com/acme/http"),
            DEPENDENCIES,
            DEV_DEPENDENCIES,
        )
        .unwrap()
        .unwrap();
        assert!(
            moved.contains("[dev-dependencies]\n\"github.com/emela-lang/brix\" = \"v0.2.0\"\n\"github.com/acme/http\" = { version = \"v1.4.0\", module = \"AcmeHttp\" }"),
            "{moved}"
        );
        assert_eq!(
            move_dependency(
                BASE,
                &p("github.com/acme/none"),
                DEPENDENCIES,
                DEV_DEPENDENCIES
            )
            .unwrap(),
            None
        );
    }
}
