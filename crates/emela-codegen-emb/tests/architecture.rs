//! Keep the extracted data contract independent of upstream IR and implementations.
use std::{fs, path::Path};
use syn::visit::{self, Visit};

#[derive(Default)]
struct ContractDependencies {
    violations: Vec<String>,
}

impl ContractDependencies {
    fn path(&mut self, segments: &[String]) {
        let names: Vec<_> = segments.iter().map(String::as_str).collect();
        let forbidden = match names.as_slice() {
            ["crate", module, ..] => *module != "contract",
            ["super", ..] => true,
            ["std", module, ..] => !["collections", "error", "fmt", "ops"].contains(module),
            _ => names.iter().any(|name| {
                matches!(
                    *name,
                    "emela_core" | "emela_lir" | "compiler" | "input" | "writer"
                )
            }),
        };
        if forbidden {
            self.violations.push(segments.join("::"));
        }
    }

    fn imports(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.imports(prefix, &path.tree);
                prefix.pop();
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.imports(prefix, tree);
                }
            }
            syn::UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Glob(_) => self.violations.push("glob import".into()),
        }
    }
}

impl<'ast> Visit<'ast> for ContractDependencies {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.path(
            &path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>(),
        );
        visit::visit_path(self, path);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.imports(&mut Vec::new(), &item.tree);
    }

    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if invocation.path.segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "include" | "include_str" | "include_bytes"
            )
        }) {
            self.violations
                .push("external source or asset include".into());
        }
        visit::visit_macro(self, invocation);
    }

    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if attribute.path().is_ident("path") {
            self.violations.push("module path override".into());
        }
        visit::visit_attribute(self, attribute);
    }
}

#[test]
fn contracts_do_not_depend_on_upstream_ir_or_compiler_implementations() {
    let mut pending = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src/contract")];
    let mut checked = 0;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let mut dependencies = ContractDependencies::default();
                dependencies
                    .visit_file(&syn::parse_file(&fs::read_to_string(&path).unwrap()).unwrap());
                assert!(
                    dependencies.violations.is_empty(),
                    "{}: {:?}",
                    path.display(),
                    dependencies.violations
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "the contract source directory must be checked");
}

#[test]
fn contract_guard_rejects_aliases_grouped_imports_and_indirect_source_includes() {
    for source in [
        "use emela_lir::Module as Program;",
        "use std::{collections::HashMap, fs as storage};",
        "pub use crate::{compiler as implementation};",
        "type Layout = crate::compiler::SomeLayout;",
        "use super::implementation;",
        "use crate::contract::*;",
        "include!(\"../backend/implementation.rs\");",
        "#[path = \"../backend/implementation.rs\"] mod implementation;",
    ] {
        let mut dependencies = ContractDependencies::default();
        dependencies.visit_file(&syn::parse_file(source).unwrap());
        assert!(!dependencies.violations.is_empty(), "accepted {source}");
    }
}

// Resolve imports relative to their owning stage, including grouped/renamed imports.
// Shared libraries are explicit; an orchestration re-export cannot bypass the boundary.
struct StageDependencies {
    stage: String,
    module: Vec<String>,
    violations: Vec<String>,
}
impl StageDependencies {
    fn path(&mut self, names: &[String]) {
        let parts: Vec<_> = names.iter().map(String::as_str).collect();
        let bad = match parts.as_slice() {
            ["std", module, ..] => [
                "fs", "env", "process", "net", "io", "time", "thread", "sync",
            ]
            .contains(module),
            ["emela_core" | "emela_lir", ..] => self.stage != "lower",
            ["crate", "compiler", stage, ..] => *stage != self.stage,
            ["crate", module, ..] => match *module {
                "contract" | "semantics" => false,
                "input" => self.stage != "lower" || parts.get(2) != Some(&"upstream"),
                "artifact" => self.stage != "emit" && self.stage != "connection",
                "writer" => self.stage != "emit",
                "verify_flow" => true,
                "Error" | "ErrorKind" | "Options" | "ExternalResult" | "ExternalAdapter"
                | "DerivedOperation" | "Output" | "GeneratedFile" | "ExportedFunction" => false,
                _ => true,
            },
            ["super", ..] => {
                let parents = parts.iter().take_while(|part| **part == "super").count();
                if self.stage == "connection" {
                    parents > 1
                        || parts
                            .get(parents)
                            .is_none_or(|name| !["contract", "fingerprint"].contains(name))
                } else {
                    parents > self.module.len()
                }
            }
            _ => false,
        };
        if bad {
            self.violations.push(names.join("::"));
        }
    }
    fn imports(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.imports(prefix, &path.tree);
                prefix.pop();
            }
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.imports(prefix, item);
                }
            }
            syn::UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Glob(_) => self.path(prefix),
        }
    }
}
impl<'ast> Visit<'ast> for StageDependencies {
    fn visit_visibility(&mut self, _: &'ast syn::Visibility) {}
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.path(
            &path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>(),
        );
        visit::visit_path(self, path);
    }
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.imports(&mut Vec::new(), &item.tree);
    }
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if item.attrs.iter().any(|attr| {
            attr.path().is_ident("cfg") && quote::quote!(#attr).to_string().contains("test")
        }) {
            return;
        }
        self.module.push(item.ident.to_string());
        visit::visit_item_mod(self, item);
        self.module.pop();
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !item.attrs.iter().any(|attr| attr.path().is_ident("test")) {
            visit::visit_item_fn(self, item);
        }
    }
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if attr.path().is_ident("path") {
            self.violations.push("module path override".into());
        }
        visit::visit_attribute(self, attr);
    }
    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if invocation.path.is_ident("include") {
            self.violations.push("source include".into());
        }
        if invocation.path.is_ident("include_str") || invocation.path.is_ident("include_bytes") {
            let allowed = syn::parse2::<syn::LitStr>(invocation.tokens.clone()).is_ok_and(|path| {
                !path.value().split('/').any(|part| part == "..")
                    && !Path::new(&path.value()).is_absolute()
            });
            if !allowed {
                self.violations.push("asset outside stage".into());
            }
        }
        visit::visit_macro(self, invocation);
    }
}
#[test]
fn stages_use_only_contracts_and_explicit_pure_shared_libraries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for stage in ["lower", "plan", "emit", "connection"] {
        let directory = if stage == "connection" {
            root.join("src/platform")
        } else {
            root.join("src/compiler").join(stage)
        };
        let mut pending = vec![directory.clone()];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    fs::read_dir(path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            if stage == "connection" && path.file_name().unwrap() != "plan.rs" {
                continue;
            }
            let relative = path.strip_prefix(&directory).unwrap();
            let mut module: Vec<_> = relative
                .parent()
                .unwrap()
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            if path.file_name().unwrap() != "mod.rs" {
                module.push(path.file_stem().unwrap().to_string_lossy().into_owned());
            }
            let mut guard = StageDependencies {
                stage: stage.into(),
                module,
                violations: vec![],
            };
            guard.visit_file(&syn::parse_file(&fs::read_to_string(&path).unwrap()).unwrap());
            assert!(
                guard.violations.is_empty(),
                "{}: {:?}",
                path.display(),
                guard.violations
            );
        }
    }
}
#[test]
fn stage_guard_rejects_reexports_effect_aliases_and_cross_stage_relative_imports() {
    for source in [
        "use std::{env as settings};",
        "pub use crate::compiler::*;",
        "use super::super::lower as previous;",
        "use emela_lir::Module as Program;",
        "type Plan=crate::compiler::plan::Hidden;",
        "include!(\"hidden.rs\");",
    ] {
        let mut guard = StageDependencies {
            stage: "emit".into(),
            module: vec!["functions".into()],
            violations: vec![],
        };
        guard.visit_file(&syn::parse_file(source).unwrap());
        assert!(!guard.violations.is_empty(), "accepted {source}");
    }
}

#[test]
fn shared_semantics_cannot_hide_stage_implementations_or_external_effects() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/semantics");
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let mut guard = StageDependencies {
            stage: "semantics".into(),
            module: vec![path.file_stem().unwrap().to_string_lossy().into_owned()],
            violations: vec![],
        };
        guard.visit_file(&syn::parse_file(&fs::read_to_string(&path).unwrap()).unwrap());
        assert!(
            guard.violations.is_empty(),
            "{}: {:?}",
            path.display(),
            guard.violations
        );
    }
}
