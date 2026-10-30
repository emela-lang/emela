//! Explicit test associations and source discovery; this does not assess test semantics.
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::{ext::IdentExt, spanned::Spanned};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct TestRef {
    pub target: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestEntry {
    #[serde(flatten)]
    pub reference: TestRef,
    pub path: String,
    pub symbol: String,
    pub line: usize,
    pub ignored: bool,
}

pub type Catalog = BTreeMap<TestRef, TestEntry>;
pub type Associations = BTreeMap<String, Vec<TestRef>>;

fn error(path: &Path, message: impl std::fmt::Display) -> String {
    format!("{}: {message}", path.display())
}

#[allow(clippy::too_many_arguments)]
fn items(
    root: &Path,
    file: &Path,
    directory: &Path,
    prefix: &str,
    target: &str,
    source: Vec<syn::Item>,
    catalog: &mut Catalog,
    active: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    for item in source {
        match item {
            syn::Item::Fn(function) if function.attrs.iter().any(|a| a.path().is_ident("test")) => {
                let symbol = function.sig.ident.unraw().to_string();
                let name = if prefix.is_empty() {
                    symbol.clone()
                } else {
                    format!("{prefix}::{symbol}")
                };
                let reference = TestRef {
                    target: target.into(),
                    name,
                };
                let ignored = function.attrs.iter().any(|a| {
                    a.path().is_ident("ignore")
                        || (a.path().is_ident("cfg_attr")
                            && a.to_token_stream()
                                .to_string()
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .any(|w| w == "ignore"))
                });
                let entry = TestEntry {
                    reference: reference.clone(),
                    path: file
                        .strip_prefix(root)
                        .map_err(|_| error(file, "test source is outside crate"))?
                        .to_string_lossy()
                        .into(),
                    symbol,
                    line: function.sig.span().start().line,
                    ignored,
                };
                if catalog.insert(reference.clone(), entry).is_some() {
                    return Err(format!(
                        "ambiguous test: {}::{}",
                        reference.target, reference.name
                    ));
                }
            }
            syn::Item::Mod(module) => {
                let name = module.ident.unraw().to_string();
                let child_prefix = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}::{name}")
                };
                if let Some((_, children)) = module.content {
                    items(
                        root,
                        file,
                        &directory.join(&name),
                        &child_prefix,
                        target,
                        children,
                        catalog,
                        active,
                    )?;
                } else {
                    let explicit = module.attrs.iter().find_map(|attr| {
                        if !attr.path().is_ident("path") {
                            return None;
                        }
                        if let syn::Meta::NameValue(value) = &attr.meta
                            && let syn::Expr::Lit(lit) = &value.value
                            && let syn::Lit::Str(path) = &lit.lit
                        {
                            return Some(path.value());
                        }
                        None
                    });
                    let path = if let Some(path) = explicit {
                        file.parent().unwrap().join(path)
                    } else {
                        let flat = directory.join(format!("{name}.rs"));
                        if flat.is_file() {
                            flat
                        } else {
                            directory.join(&name).join("mod.rs")
                        }
                    };
                    let child_dir = if path.file_name().is_some_and(|n| n == "mod.rs") {
                        path.parent().unwrap().to_owned()
                    } else {
                        path.parent().unwrap().join(path.file_stem().unwrap())
                    };
                    walk(
                        root,
                        &path,
                        &child_dir,
                        &child_prefix,
                        target,
                        catalog,
                        active,
                    )?;
                }
            }
            syn::Item::Macro(item) if item.mac.path.is_ident("include") => {
                let include: syn::LitStr =
                    syn::parse2(item.mac.tokens).map_err(|e| error(file, e))?;
                let path = file.parent().unwrap().join(include.value());
                walk(root, &path, directory, prefix, target, catalog, active)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn walk(
    root: &Path,
    file: &Path,
    directory: &Path,
    prefix: &str,
    target: &str,
    catalog: &mut Catalog,
    active: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let file = file.canonicalize().map_err(|e| error(file, e))?;
    if !active.insert(file.clone()) {
        return Err(error(&file, "recursive module/include"));
    }
    let source = fs::read_to_string(&file).map_err(|e| error(&file, e))?;
    let parsed = syn::parse_file(&source).map_err(|e| error(&file, e))?;
    let result = items(
        root,
        &file,
        directory,
        prefix,
        target,
        parsed.items,
        catalog,
        active,
    );
    active.remove(&file);
    result
}

pub fn discover(root: &Path) -> Result<Catalog, String> {
    let root = root.canonicalize().map_err(|e| error(root, e))?;
    let mut catalog = BTreeMap::new();
    walk(
        &root,
        &root.join("src/lib.rs"),
        &root.join("src"),
        "",
        "lib",
        &mut catalog,
        &mut BTreeSet::new(),
    )?;
    let mut files = fs::read_dir(root.join("tests"))
        .map_err(|e| error(&root, e))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    files.sort();
    for file in files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
    {
        let target = file.file_stem().unwrap().to_string_lossy();
        walk(
            &root,
            &file,
            &root.join("tests"),
            "",
            &target,
            &mut catalog,
            &mut BTreeSet::new(),
        )?;
    }
    Ok(catalog)
}

pub fn validate(
    required: &BTreeSet<String>,
    associations: &Associations,
    catalog: &Catalog,
) -> Result<(), String> {
    let actual: BTreeSet<_> = associations.keys().cloned().collect();
    if required != &actual {
        return Err(format!(
            "test associations differ: missing={:?}, orphan={:?}",
            required.difference(&actual).collect::<Vec<_>>(),
            actual.difference(required).collect::<Vec<_>>()
        ));
    }
    for (id, tests) in associations {
        if tests.is_empty() {
            return Err(format!("no tests registered: {id}"));
        }
        let mut unique = BTreeSet::new();
        let mut enabled = false;
        for reference in tests {
            if !unique.insert(reference) {
                return Err(format!("duplicate test association: {id}: {reference:?}"));
            }
            let entry = catalog.get(reference).ok_or_else(|| {
                format!(
                    "missing test: {id}: {}::{}",
                    reference.target, reference.name
                )
            })?;
            enabled |= !entry.ignored;
        }
        if !enabled {
            return Err(format!("all associated tests are ignored: {id}"));
        }
    }
    Ok(())
}

pub fn load(root: &Path, required: &BTreeSet<String>) -> Result<(Associations, Catalog), String> {
    let associations: Associations = serde_json::from_slice(
        &fs::read(root.join("inventory/tests.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let catalog = discover(root)?;
    validate(required, &associations, &catalog)?;
    Ok((associations, catalog))
}
