//! Bridges the compiler frontend to the language server (spec 0033): runs the
//! whole pipeline over an open document, converts collected errors to LSP
//! diagnostics grouped by file, and extracts the scope snapshot completions
//! draw from.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::ast::{EffectRow, Function, Program, Type};
use crate::driver;
use crate::error::Error;
use crate::lsp::documents::{Document, DocumentStore, path_to_uri};
use crate::lsp::position::offset_to_position;
use crate::lsp::protocol::{Diagnostic, Position, Range, SEVERITY_ERROR};
use crate::parser::parse_program;

pub(crate) struct CheckOutcome {
    /// Diagnostics grouped by target URI. The entry document's URI is always
    /// present — with an empty list when it is clean — so stale squiggles clear.
    pub(crate) diagnostics: Vec<(String, Vec<Diagnostic>)>,
    pub(crate) snapshot: Snapshot,
}

/// Runs the frontend over `doc` with every open buffer as the import overlay,
/// and maps the collected errors (spec 0033) to per-file diagnostics.
pub(crate) fn check_document(
    doc: &Document,
    store: &DocumentStore,
    package_paths: &[PathBuf],
) -> CheckOutcome {
    let path = doc
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from("untitled.emel"));
    let mut extra_errors = Vec::new();
    let packages = match driver::load_import_roots(&path, package_paths) {
        Ok(packages) => packages,
        Err(error) => {
            extra_errors.push(error);
            Vec::new()
        }
    };
    let overlay = store.overlay();
    let label = path.display().to_string();
    // The entrypoint checks (spec 0003) run only when this file declares its
    // own `main` (spec 0033); a library module gets `check --library` behavior.
    let (own, _) = parse_program(&label, &doc.text);
    let require_main = own.functions.iter().any(|function| function.name == "main");
    let (program, _, mut errors) = driver::compile_frontend_source_all(
        &path,
        &doc.text,
        &packages,
        require_main,
        &overlay,
        &emela_codegen::platform_interface(),
    );
    errors.extend(extra_errors);
    CheckOutcome {
        diagnostics: group_diagnostics(doc, &label, &errors),
        snapshot: Snapshot::build(own, &program),
    }
}

/// Converts errors to LSP diagnostics, routed to the file their span lives in.
/// An error in an imported module is published at that module, plus one
/// summary line in the entry document so the failure is visible there.
fn group_diagnostics(
    doc: &Document,
    entry_label: &str,
    errors: &[Error],
) -> Vec<(String, Vec<Diagnostic>)> {
    let mut order: Vec<String> = vec![doc.uri.clone()];
    let mut by_uri: HashMap<String, Vec<Diagnostic>> = HashMap::new();
    by_uri.insert(doc.uri.clone(), Vec::new());
    let mut summarized: BTreeSet<String> = BTreeSet::new();
    for error in errors {
        let mut push = |uri: String, diagnostic: Diagnostic| {
            if !by_uri.contains_key(&uri) {
                order.push(uri.clone());
            }
            by_uri.entry(uri).or_default().push(diagnostic);
        };
        match error.diagnostic_ref().and_then(|d| d.primary_label()) {
            Some(label) => {
                let span = label.span();
                let file = &span.file;
                let diagnostic = Diagnostic {
                    range: Range {
                        start: offset_to_position(&file.source, span.start),
                        end: offset_to_position(&file.source, span.end),
                    },
                    severity: SEVERITY_ERROR,
                    source: "emela",
                    message: render_message(error),
                };
                if file.label == entry_label {
                    push(doc.uri.clone(), diagnostic);
                } else if file.label.starts_with('<') {
                    // A virtual module (`<core-prelude>`, `<std.io>`; spec
                    // 0038) has no file to publish at, so the error surfaces
                    // only as an entry-file summary.
                    if summarized.insert(file.label.clone()) {
                        push(
                            doc.uri.clone(),
                            top_of_file_diagnostic(format!(
                                "imported module `{}` has errors: {}",
                                file.label,
                                error.message()
                            )),
                        );
                    }
                } else {
                    push(path_to_uri(Path::new(&file.label)), diagnostic);
                    if summarized.insert(file.label.clone()) {
                        push(
                            doc.uri.clone(),
                            top_of_file_diagnostic(format!(
                                "imported module `{}` has errors: {}",
                                file.label,
                                error.message()
                            )),
                        );
                    }
                }
            }
            None => push(doc.uri.clone(), top_of_file_diagnostic(error.render())),
        }
    }
    order
        .into_iter()
        .map(|uri| {
            let diagnostics = by_uri.remove(&uri).unwrap_or_default();
            (uri, diagnostics)
        })
        .collect()
}

/// A spanless error (IO failure, cyclic import, …) lands at the top of the
/// entry file.
fn top_of_file_diagnostic(message: String) -> Diagnostic {
    let zero = Position {
        line: 0,
        character: 0,
    };
    Diagnostic {
        range: Range {
            start: zero,
            end: zero,
        },
        severity: SEVERITY_ERROR,
        source: "emela",
        message,
    }
}

/// One readable message from a diagnostic: title, label detail, and hint.
fn render_message(error: &Error) -> String {
    let Some(diagnostic) = error.diagnostic_ref() else {
        return error.render();
    };
    let mut message = diagnostic.title().to_string();
    if let Some(label) = diagnostic.primary_label()
        && !label.message().is_empty()
    {
        message.push_str(": ");
        message.push_str(label.message());
    }
    if let Some(help) = diagnostic.help_text() {
        message.push_str("\nHint: ");
        message.push_str(help);
    }
    message
}

/// What completions know about the scope of a document (spec 0033): extracted
/// from the merged program — the entry file plus its imports and the Core
/// Prelude — after the last check. Parser recovery keeps this usable while the
/// file has errors.
#[derive(Default)]
pub(crate) struct Snapshot {
    pub(crate) enums: Vec<EnumSym>,
    pub(crate) functions: Vec<FnSym>,
    pub(crate) trait_methods: Vec<FnSym>,
    /// Every effect name mentioned by a `uses` row in scope.
    pub(crate) effects: BTreeSet<String>,
    /// Enum names appearing in a `throws` clause in scope — the `catch`
    /// completion's first candidates.
    pub(crate) throws_enums: BTreeSet<String>,
    /// The entry file's own functions and impl methods, with bodies and spans,
    /// for locating the enclosing function and its locals at a cursor offset.
    pub(crate) entry_functions: Vec<Function>,
}

pub(crate) struct EnumSym {
    pub(crate) name: String,
    pub(crate) variants: Vec<VariantSym>,
}

pub(crate) struct VariantSym {
    pub(crate) name: String,
    pub(crate) arity: usize,
}

pub(crate) struct FnSym {
    pub(crate) name: String,
    pub(crate) detail: String,
}

impl Snapshot {
    fn build(own: Program, merged: &Program) -> Snapshot {
        let mut snapshot = Snapshot::default();
        let mut seen_enums = BTreeSet::new();
        for decl in &merged.enums {
            if !seen_enums.insert(decl.name.clone()) {
                continue;
            }
            snapshot.enums.push(EnumSym {
                name: decl.name.clone(),
                variants: decl
                    .variants
                    .iter()
                    .map(|variant| VariantSym {
                        name: variant.name.clone(),
                        arity: variant.fields.len(),
                    })
                    .collect(),
            });
        }
        let mut seen_fns = BTreeSet::new();
        for function in &merged.functions {
            snapshot.collect_effects(&function.effects);
            snapshot.collect_throws(&function.throws);
            if !seen_fns.insert(function.name.clone()) {
                continue;
            }
            snapshot.functions.push(FnSym {
                name: function.name.clone(),
                detail: render_fn_sig(
                    &function.name,
                    &function
                        .params
                        .iter()
                        .map(|p| p.ty.clone())
                        .collect::<Vec<_>>(),
                    &function.ret,
                    &function.throws,
                    &function.effects,
                ),
            });
        }
        for declaration in &merged.externs {
            snapshot.collect_effects(&declaration.effects);
            snapshot.collect_throws(&declaration.throws);
        }
        for decl in &merged.traits {
            for method in &decl.methods {
                snapshot.collect_effects(&method.effects);
                snapshot.collect_throws(&method.throws);
                snapshot.trait_methods.push(FnSym {
                    name: method.name.clone(),
                    detail: format!(
                        "{} ({})",
                        render_fn_sig(
                            &method.name,
                            &method
                                .params
                                .iter()
                                .map(|p| p.ty.clone())
                                .collect::<Vec<_>>(),
                            &method.ret,
                            &method.throws,
                            &method.effects,
                        ),
                        decl.name
                    ),
                });
            }
        }
        for decl in &merged.impls {
            for method in &decl.methods {
                snapshot.collect_effects(&method.effects);
                snapshot.collect_throws(&method.throws);
            }
        }
        snapshot.entry_functions = own
            .functions
            .into_iter()
            .chain(own.impls.into_iter().flat_map(|decl| decl.methods))
            .collect();
        snapshot
    }

    /// A parse that lost everything (e.g. mid-edit garbage) would blank the
    /// completion scope; keep the previous snapshot instead (spec 0033).
    pub(crate) fn is_empty(&self) -> bool {
        self.enums.is_empty() && self.functions.is_empty() && self.entry_functions.is_empty()
    }

    fn collect_effects(&mut self, effects: &EffectRow) {
        self.effects.extend(effects.effects.iter().cloned());
    }

    fn collect_throws(&mut self, throws: &Option<Type>) {
        if let Some(Type::Enum(name, _)) = throws {
            self.throws_enums.insert(name.clone());
        }
    }
}

/// Renders a type the way source spells it, for completion details.
pub(crate) fn render_type(ty: &Type) -> String {
    match ty {
        Type::Unit => "Unit".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::Int => "Int".to_string(),
        Type::Float => "Float".to_string(),
        Type::String => "String".to_string(),
        Type::Char => "Char".to_string(),
        Type::Record => "Record".to_string(),
        Type::Never => "Never".to_string(),
        Type::Array(inner) => format!("Array<{}>", render_type(inner)),
        Type::Option(inner) => format!("Option<{}>", render_type(inner)),
        Type::Enum(name, args) => {
            if args.is_empty() {
                name.clone()
            } else {
                let args: Vec<String> = args.iter().map(render_type).collect();
                format!("{name}<{}>", args.join(", "))
            }
        }
        Type::Function(function) => {
            let params: Vec<String> = function.params.iter().map(render_type).collect();
            let mut out = format!("({}) -> {}", params.join(", "), render_type(&function.ret));
            if let Some(throws) = &function.throws {
                out.push_str(&format!(" throws {}", render_type(throws)));
            }
            if !function.effects.effects.is_empty() {
                out.push_str(&format!(
                    " uses {{ {} }}",
                    function.effects.effects.join(", ")
                ));
            }
            out
        }
        Type::OpaqueFunction => "Function".to_string(),
        Type::Var(name) => name.clone(),
    }
}

/// Renders a function signature for a completion item's detail line.
pub(crate) fn render_fn_sig(
    name: &str,
    params: &[Type],
    ret: &Type,
    throws: &Option<Type>,
    effects: &EffectRow,
) -> String {
    let params: Vec<String> = params.iter().map(render_type).collect();
    let mut out = format!("fn {name}({}) -> {}", params.join(", "), render_type(ret));
    if let Some(throws) = throws {
        out.push_str(&format!(" throws {}", render_type(throws)));
    }
    if !effects.effects.is_empty() {
        out.push_str(&format!(" uses {{ {} }}", effects.effects.join(", ")));
    }
    out
}
