//! Keep embedded source ownership and source-file dependencies inside this backend.

use std::{fs, path::Path};
use syn::visit::Visit;

#[test]
fn embedded_sources_are_supplied_by_the_backend_without_shadowing_core() {
    assert!(
        emela_core::core_sources()
            .iter()
            .all(|(name, _)| !name.starts_with("Std.Embedded.")),
        "embedded modules must not be bundled by core"
    );
    let mut names = Vec::new();
    for (name, text) in emela_codegen_emb::bundled_sources() {
        assert!(emela_core::core_source(name).is_none(), "duplicate {name}");
        let parsed = emela_syntax::parse(text);
        assert!(
            parsed.diagnostics().is_empty(),
            "{name}: {:?}",
            parsed.diagnostics()
        );
        names.push(name.strip_prefix("Std.Embedded.").unwrap());
    }
    names.sort();
    assert_eq!(names, ["Board", "Gpio", "I2c", "Peripheral", "Spi", "Uart"]);
}

#[derive(Default)]
struct FileReferences(Vec<String>);

impl<'ast> Visit<'ast> for FileReferences {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if attribute.path().is_ident("path")
            && let syn::Meta::NameValue(meta) = &attribute.meta
            && let syn::Expr::Lit(value) = &meta.value
            && let syn::Lit::Str(path) = &value.lit
        {
            self.0.push(path.value());
        }
        syn::visit::visit_attribute(self, attribute);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac.path.segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "include" | "include_str" | "include_bytes"
            )
        }) {
            let path: syn::LitStr = syn::parse2(mac.tokens.clone())
                .expect("source includes must use explicit paths for the ownership check");
            self.0.push(path.value());
        }
        syn::visit::visit_macro(self, mac);
    }
}

#[test]
fn source_helpers_and_fixtures_do_not_read_sibling_crates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let mut pending: Vec<_> = ["src", "tests", "examples", "tools"]
        .map(|name| root.join(name))
        .into();
    let mut checked = 0;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "target") {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let mut references = FileReferences::default();
                references
                    .visit_file(&syn::parse_file(&fs::read_to_string(&path).unwrap()).unwrap());
                for reference in references.0 {
                    let target = path.parent().unwrap().join(&reference);
                    let resolved = target
                        .canonicalize()
                        .unwrap_or_else(|error| panic!("{}: {reference}: {error}", path.display()));
                    assert!(
                        resolved.starts_with(&root),
                        "{} reads a file outside this crate: {reference}",
                        path.display()
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0, "no file references were checked");
}
