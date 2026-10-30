//! Source declarations only: no inference of stage legality or backend support.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use proc_macro2::Span;
use quote::ToTokens;
use serde::Serialize;
use sha2::{Digest, Sha256};
use syn::{Attribute, Expr, Fields, Item, Lit, Meta, Visibility, spanned::Spanned};

#[derive(Debug, Clone, Serialize)]
pub struct Scope {
    pub path: String,
    pub namespace: String,
    /// None selects every top-level public enum, struct and type alias in this file.
    pub include: Option<Vec<String>>,
}

impl Scope {
    pub fn new(path: &str, namespace: &str, include: Option<&[&str]>) -> Self {
        Self {
            path: path.into(),
            namespace: namespace.into(),
            include: include.map(|names| names.iter().map(|s| (*s).into()).collect()),
        }
    }
}

pub fn default_scope() -> Vec<Scope> {
    vec![
        Scope::new("crates/emela-lir/src/ir.rs", "emela_lir", None),
        Scope::new("crates/emela-lir/src/repr.rs", "emela_lir", None),
        Scope::new(
            "crates/emela-core/src/intrinsics.rs",
            "emela_core",
            Some(&["Builtin"]),
        ),
        Scope::new(
            "crates/emela-core/src/ir.rs",
            "emela_core",
            Some(&[
                "FiberOp",
                "Type",
                "FnType",
                "FnSets",
                "SetSummary",
                "EnumDef",
                "VariantDef",
                "VariantFields",
                "CtorRef",
                "EffectDef",
                "OpDef",
                "External",
                "ExternalBinding",
                "HostRef",
                "HostModule",
            ]),
        ),
    ]
}

#[derive(Debug, Serialize)]
pub struct Inventory {
    pub schema_version: u32,
    pub inventory_kind: &'static str,
    pub scope: Vec<Scope>,
    pub sources: Vec<SourceFile>,
    pub items: Vec<Entry>,
}

#[derive(Debug, Serialize)]
pub struct SourceFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct Location {
    pub path: String,
    pub line: usize,
    pub end_line: usize,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub id: String,
    pub kind: &'static str,
    pub parent: Option<String>,
    pub declaration: String,
    pub attributes: Vec<String>,
    pub docs: String,
    pub source: Location,
    /// Syntax fingerprint, excluding doc attributes and source position. Not a behavior hash.
    pub definition_sha256: String,
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn docs(attrs: &[Attribute]) -> Result<String, String> {
    let mut lines = Vec::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("doc")) {
        if let Meta::NameValue(meta) = &attr.meta
            && let Expr::Lit(value) = &meta.value
            && let Lit::Str(text) = &value.lit
        {
            lines.push(text.value().trim().to_owned());
        } else {
            return Err("non-literal #[doc = ...] requires explicit extractor support".into());
        }
    }
    Ok(lines.join("\n").trim().to_owned())
}

fn without_docs(attrs: &mut Vec<Attribute>) {
    attrs.retain(|a| !a.path().is_ident("doc"));
}

fn clean_fields(fields: &mut Fields) {
    for field in fields {
        without_docs(&mut field.attrs);
    }
}

fn entry(
    scope: &Scope,
    id: String,
    kind: &'static str,
    parent: Option<&str>,
    syntax: impl ToTokens,
    attrs: &[Attribute],
    span: Span,
) -> Result<Entry, String> {
    let attributes: Vec<_> = attrs
        .iter()
        .filter(|a| !a.path().is_ident("doc"))
        .map(|a| a.to_token_stream().to_string())
        .collect();
    let declaration = syntax.to_token_stream().to_string();
    let definition_sha256 =
        sha256(format!("{kind}\n{declaration}\n{}", attributes.join("\n")).as_bytes());
    Ok(Entry {
        id,
        kind,
        parent: parent.map(str::to_owned),
        declaration,
        attributes,
        docs: docs(attrs)?,
        source: Location {
            path: scope.path.clone(),
            line: span.start().line,
            end_line: span.end().line,
        },
        definition_sha256,
    })
}

fn fields(
    scope: &Scope,
    parent: &str,
    fields: &Fields,
    out: &mut Vec<Entry>,
) -> Result<(), String> {
    for (index, field) in fields.iter().enumerate() {
        let name = field
            .ident
            .as_ref()
            .map_or_else(|| index.to_string(), ToString::to_string);
        let mut clean = field.clone();
        without_docs(&mut clean.attrs);
        out.push(entry(
            scope,
            format!("{parent}.{name}"),
            "field",
            Some(parent),
            clean,
            &field.attrs,
            field.span(),
        )?);
    }
    Ok(())
}

/// Parse source syntax with syn, including all cfg branches. Do not compile or expand macros.
pub fn extract(scope: &Scope, source: &str) -> Result<Vec<Entry>, String> {
    let file = syn::parse_file(source).map_err(|e| format!("{}: {e}", scope.path))?;
    let mut out = Vec::new();
    let mut found = BTreeSet::new();
    for item in file.items {
        let (name, visibility) = match &item {
            Item::Enum(i) => (&i.ident, &i.vis),
            Item::Struct(i) => (&i.ident, &i.vis),
            Item::Type(i) => (&i.ident, &i.vis),
            Item::Union(i) => (&i.ident, &i.vis),
            Item::Mod(_) | Item::Macro(_) if scope.include.is_none() => {
                return Err(format!(
                    "{}: module/macro in a whole-file scope; extend the extraction scope explicitly",
                    scope.path
                ));
            }
            _ => continue,
        };
        let name = name.to_string();
        if !matches!(visibility, Visibility::Public(_))
            || scope
                .include
                .as_ref()
                .is_some_and(|names| !names.contains(&name))
        {
            continue;
        }
        if !found.insert(name.clone()) {
            return Err(format!(
                "{}: duplicate declaration {name}; cfg alternatives need distinct scope handling",
                scope.path
            ));
        }
        let id = format!("{}::{name}", scope.namespace);
        match item {
            Item::Enum(i) => {
                let mut clean = i.clone();
                without_docs(&mut clean.attrs);
                for variant in &mut clean.variants {
                    without_docs(&mut variant.attrs);
                    clean_fields(&mut variant.fields);
                }
                out.push(entry(
                    scope,
                    id.clone(),
                    "enum",
                    None,
                    clean,
                    &i.attrs,
                    i.span(),
                )?);
                for variant in i.variants {
                    let child_id = format!("{id}::{}", variant.ident);
                    let mut clean = variant.clone();
                    without_docs(&mut clean.attrs);
                    clean_fields(&mut clean.fields);
                    out.push(entry(
                        scope,
                        child_id.clone(),
                        "variant",
                        Some(&id),
                        clean,
                        &variant.attrs,
                        variant.span(),
                    )?);
                    fields(scope, &child_id, &variant.fields, &mut out)?;
                }
            }
            Item::Struct(i) => {
                let mut clean = i.clone();
                without_docs(&mut clean.attrs);
                clean_fields(&mut clean.fields);
                out.push(entry(
                    scope,
                    id.clone(),
                    "struct",
                    None,
                    clean,
                    &i.attrs,
                    i.span(),
                )?);
                fields(scope, &id, &i.fields, &mut out)?;
            }
            Item::Type(i) => {
                let mut clean = i.clone();
                without_docs(&mut clean.attrs);
                out.push(entry(
                    scope,
                    id,
                    "type_alias",
                    None,
                    clean,
                    &i.attrs,
                    i.span(),
                )?);
            }
            _ => {
                return Err(format!(
                    "{}: unsupported data declaration {name}",
                    scope.path
                ));
            }
        }
    }
    if let Some(names) = &scope.include {
        for name in names {
            if !found.contains(name) {
                return Err(format!(
                    "{}: selected public declaration {name} is missing",
                    scope.path
                ));
            }
        }
    }
    Ok(out)
}

pub fn collect(root: &Path, scope: Vec<Scope>) -> Result<Inventory, String> {
    let mut sources = Vec::new();
    let mut items = Vec::new();
    let mut ids = BTreeSet::new();
    for selection in &scope {
        let path = root.join(&selection.path);
        let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        sources.push(SourceFile {
            path: selection.path.clone(),
            sha256: sha256(source.as_bytes()),
        });
        for item in extract(selection, &source)? {
            if !ids.insert(item.id.clone()) {
                return Err(format!("duplicate inventory ID {}", item.id));
            }
            items.push(item);
        }
    }
    Ok(Inventory {
        schema_version: 1,
        inventory_kind: "definitions",
        scope,
        sources,
        items,
    })
}

pub(crate) fn cell(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace('\r', "")
        .replace('\n', "<br>")
}

pub(crate) fn code(text: &str) -> String {
    // HTML code avoids Markdown fence collisions and table pipes in Rust strings/doc attributes.
    format!("<code>{}</code>", cell(text))
}

pub(crate) fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in &from[common..] {
        path.push("..");
    }
    for component in &to[common..] {
        path.push(component.as_os_str());
    }
    path.to_string_lossy()
        .replace('\\', "/")
        .replace(' ', "%20")
}

pub fn markdown(inventory: &Inventory, root: &Path, out_dir: &Path) -> String {
    let mut md = String::from(
        "# Flow IR関連定義の自動抽出一覧\n\n<!-- Generated by crates/emela-codegen-emb/tools/flow-ir-inventory.sh; schema_version=1. Do not edit. -->\n\n",
    );
    md.push_str("Rustの構文解析から抽出した宣言・variant・field・属性・原文コメントの一覧。Flow/Heapの共有定義と、下記で指定したCoreの共有型を含む。**Flowへの出現可否、backendの対応、動作の正しさは未判定。** コメントは定義元の記述であり、組み込みtarget固有の契約への読み替えは行わない。\n\n");
    md.push_str("機械処理用: [flow-ir-inventory.json](flow-ir-inventory.json)。安定IDと宣言のSHA-256で実装側の記録を照合できる。件数は機能数や試験数を表さない。\n\n");
    let variants = inventory
        .items
        .iter()
        .filter(|i| i.kind == "variant")
        .count();
    let fields = inventory.items.iter().filter(|i| i.kind == "field").count();
    md.push_str(&format!(
        "抽出結果: {}件（variant {variants}件、field {fields}件、型宣言・alias {}件）。\n\n",
        inventory.items.len(),
        inventory.items.len() - variants - fields
    ));
    md.push_str(
        "## 抽出範囲\n\n| 定義元 | namespace | 選択 | ソースSHA-256 |\n| --- | --- | --- | --- |\n",
    );
    for (scope, source) in inventory.scope.iter().zip(&inventory.sources) {
        let link = relative_path(out_dir, &root.join(&scope.path));
        let selection = scope.include.as_ref().map_or_else(
            || "全top-level public enum / struct / type alias".into(),
            |names| names.join(", "),
        );
        md.push_str(&format!(
            "| [{}]({link}) | {} | {} | {} |\n",
            scope.path,
            code(&scope.namespace),
            cell(&selection),
            code(&source.sha256)
        ));
    }
    md.push_str("\n`cfg`は評価せず属性を保存する。macro展開・関数本体・verifierの規則・型の全組み合わせは抽出対象に含めない。Coreの選択は明示リストであり、依存型全体の再帰的収集ではない。\n");
    for item in &inventory.items {
        if item.parent.is_none() {
            md.push_str(&format!("\n## {}\n\n| ID | 種別 | 宣言 | 原文コメント | 属性 | 定義 |\n| --- | --- | --- | --- | --- | --- |\n", item.id));
        }
        let path = relative_path(out_dir, &root.join(&item.source.path));
        // Parent shapes are available in JSON; show only their header to keep tables readable.
        let declaration = if matches!(item.kind, "enum" | "struct") {
            format!("{} {}", item.kind, item.id.rsplit("::").next().unwrap())
        } else {
            item.declaration.clone()
        };
        let doc = item.docs.replace("[`", "`").replace("`]", "`");
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | [L{}]({path}#L{}) |\n",
            code(&item.id),
            item.kind,
            code(&declaration),
            cell(&doc),
            code(&item.attributes.join(" ")),
            item.source.line,
            item.source.line
        ));
    }
    md
}

pub fn reports(
    inventory: &Inventory,
    root: &Path,
    out_dir: &Path,
) -> Result<Vec<(&'static str, String)>, String> {
    let json = serde_json::to_string_pretty(inventory).map_err(|e| e.to_string())? + "\n";
    Ok(vec![
        ("flow-ir-inventory.json", json),
        ("flow-ir-inventory.md", markdown(inventory, root, out_dir)),
    ])
}

pub fn save(reports: &[(&str, String)], out_dir: &Path, check: bool) -> Result<(), String> {
    if check {
        let stale: Vec<_> = reports
            .iter()
            .filter(|(name, expected)| {
                fs::read_to_string(out_dir.join(name)).ok().as_ref() != Some(expected)
            })
            .map(|(name, _)| *name)
            .collect();
        if !stale.is_empty() {
            return Err(format!(
                "missing or stale generated files: {}",
                stale.join(", ")
            ));
        }
    } else {
        fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
        for (name, contents) in reports {
            fs::write(out_dir.join(name), contents).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn absolute(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(part.as_os_str()),
        }
    }
    Ok(normalized)
}

/// Locate the audited crate for both the compiler examples and the standalone tool.
pub fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("inventory/owners.json").is_file())
        .expect("inventory tool must be located under emela-codegen-emb")
}

pub fn repository_root() -> &'static Path {
    crate_dir().parent().unwrap().parent().unwrap()
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let root = repository_root();
    let mut out_dir = root.join("crates/emela-codegen-emb/docs/generated");
    let mut check = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--out-dir" => out_dir = args.next().ok_or("--out-dir requires a path")?.into(),
            "--help" | "-h" => {
                println!(
                    "Usage: crates/emela-codegen-emb/tools/flow-ir-inventory.sh [--check] [--out-dir PATH]\n\nWrites Markdown and JSON from Rust source declarations.\nDefault: crates/emela-codegen-emb/docs/generated\n--check reports missing/stale output without writing.\n--out-dir is relative to the current working directory."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg}; use --help")),
        }
    }
    let out_dir = absolute(&out_dir)?;
    let inventory = collect(root, default_scope())?;
    let reports = reports(&inventory, root, &out_dir)?;
    save(&reports, &out_dir, check)?;
    println!(
        "{} {} records: {}",
        if check { "Checked" } else { "Generated" },
        inventory.items.len(),
        out_dir.display()
    );
    Ok(())
}
