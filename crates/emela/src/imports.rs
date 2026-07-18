use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ast::{EnumDecl, Extern, Function, ImplDecl, Import, Program, TraitDecl};
use crate::error::{Diagnostic, Error, Result};
use crate::parser::parse_program;
use crate::prelude;

/// The virtual path an embedded std module (spec 0038) resolves to. Angle
/// brackets cannot appear in a canonicalized filesystem path, so these keys
/// never collide with real modules in the resolver's caches; the same
/// convention names the merged prelude's label (`<core-prelude>`).
fn embedded_module_path(name: &str) -> PathBuf {
    PathBuf::from(format!("<std.{name}>"))
}

/// The embedded source behind a virtual module path, or `None` for a real
/// filesystem path.
fn embedded_source_for(path: &Path) -> Option<&'static str> {
    let label = path.to_str()?;
    let name = label.strip_prefix("<std.")?.strip_suffix('>')?;
    prelude::embedded_std_source(name)
}

/// The declarations pulled in from an imported module. A module's public
/// functions are what an `import` names, but its type declarations (enums, spec
/// 0028) and their impls (spec 0020) come along too, since the imported
/// functions' signatures refer to them. Emitted once per module (see `emitted`).
#[derive(Default)]
struct Imported {
    functions: Vec<Function>,
    externs: Vec<Extern>,
    enums: Vec<EnumDecl>,
    traits: Vec<TraitDecl>,
    impls: Vec<ImplDecl>,
}

#[derive(Debug, Clone)]
pub(crate) struct PackageSource {
    name: String,
    source_root: PathBuf,
}

impl PackageSource {
    /// Builds a package source directly from a resolved name and source root.
    /// Used to expose a dependency Pome's modules under its import-root name,
    /// without an `emela-package.json` (spec 0032 M1).
    pub(crate) fn new(name: String, source_root: PathBuf) -> Self {
        PackageSource { name, source_root }
    }

    /// The import-root name this package is addressed by.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The directory the package's modules live under.
    pub(crate) fn source_root(&self) -> &Path {
        &self.source_root
    }
}

#[derive(Debug, Deserialize)]
struct PackageManifest {
    name: String,
    source: String,
}

pub(crate) fn load_packages(paths: &[PathBuf]) -> Result<Vec<PackageSource>> {
    let mut packages = Vec::new();
    let mut names = HashSet::new();
    for path in paths {
        let manifest_path = path.join("emela-package.json");
        let manifest_source = fs::read_to_string(&manifest_path).map_err(|err| {
            Error::new(format!(
                "failed to read package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
        let manifest: PackageManifest = serde_json::from_str(&manifest_source).map_err(|err| {
            Error::new(format!(
                "failed to parse package manifest `{}`: {err}",
                manifest_path.display()
            ))
        })?;
        if !names.insert(manifest.name.clone()) {
            return Err(Error::new(format!(
                "duplicate package `{}` in --package arguments",
                manifest.name
            )));
        }
        packages.push(PackageSource {
            name: manifest.name,
            source_root: path.join(manifest.source),
        });
    }
    Ok(packages)
}

/// Expands every `import` in `program`, collecting errors per import statement
/// (spec 0033) so one broken import doesn't hide the others. An empty error
/// list means every import resolved. `overlay` (canonicalized path → source
/// text) is consulted before the filesystem, so an LSP client's unsaved
/// buffers (spec 0033) take precedence over what is on disk; pass an empty map
/// otherwise.
pub(crate) fn resolve_imports_with_overlay(
    input: &Path,
    program: Program,
    packages: &[PackageSource],
    overlay: &HashMap<PathBuf, String>,
) -> (Program, Vec<Error>) {
    let mut resolver = ImportResolver {
        packages,
        overlay,
        loaded: HashMap::new(),
        resolving: HashSet::new(),
        emitted: HashSet::new(),
        errors: Vec::new(),
    };
    let program = resolver.expand_program(input, program);
    (program, resolver.errors)
}

struct ImportResolver<'a> {
    packages: &'a [PackageSource],
    overlay: &'a HashMap<PathBuf, String>,
    loaded: HashMap<PathBuf, Program>,
    resolving: HashSet<PathBuf>,
    emitted: HashSet<PathBuf>,
    errors: Vec<Error>,
}

impl ImportResolver<'_> {
    fn expand_program(&mut self, source_path: &Path, mut program: Program) -> Program {
        let imports = std::mem::take(&mut program.imports);
        let mut acc = Imported::default();
        for import in imports {
            let items = match self.resolve_import(source_path, &import) {
                Ok(items) => items,
                Err(error) => {
                    self.errors.push(error);
                    continue;
                }
            };
            acc.functions.extend(items.functions);
            acc.externs.extend(items.externs);
            acc.enums.extend(items.enums);
            acc.traits.extend(items.traits);
            acc.impls.extend(items.impls);
        }
        // Imported declarations come first so this file's own definitions can
        // shadow / extend them, matching the existing function ordering.
        acc.functions.extend(program.functions);
        program.functions = acc.functions;
        acc.externs.extend(program.externs);
        program.externs = acc.externs;
        acc.enums.extend(program.enums);
        program.enums = acc.enums;
        acc.traits.extend(program.traits);
        program.traits = acc.traits;
        acc.impls.extend(program.impls);
        program.impls = acc.impls;
        program
    }

    fn resolve_import(&mut self, source_path: &Path, import: &Import) -> Result<Imported> {
        let Some((module_file, module_name, item_name)) =
            self.resolve_module_file(source_path, import)?
        else {
            return Err(Error::diagnostic(Diagnostic::new("Unknown package").label(
                import.span.clone(),
                format!("cannot resolve `{}`", import.path[0]),
            )));
        };
        let module = self.load_module(&module_file)?;
        if module.module.as_deref() != Some(module_name.as_str()) {
            return Err(Error::diagnostic(Diagnostic::new("Module mismatch").label(
                import.span.clone(),
                format!(
                    "expected module `{module_name}` in `{}`",
                    module_file.display()
                ),
            )));
        }
        // A per-item import must name a public item; a whole-module import
        // (`item_name == None`, e.g. an effect) brings in every public item.
        if let Some(item_name) = &item_name {
            match module.functions.iter().find(|f| &f.name == item_name) {
                // Effect operations are imported as a whole and called qualified
                // (spec 0036), never pulled in one at a time.
                Some(function) if function.is_effect_op => {
                    let effect_import = import.path[..import.path.len() - 1].join(".");
                    return Err(Error::diagnostic(
                        Diagnostic::new("Effect operation import").label(
                            import.span.clone(),
                            format!(
                                "`{item_name}` is an operation of effect `{module_name}`; import the effect with `import {effect_import}` and call it as `{module_name}.{item_name}`"
                            ),
                        ),
                    ));
                }
                Some(function) if function.is_public => {}
                Some(function) => {
                    return Err(Error::diagnostic(Diagnostic::new("Private import").label(
                        function.name_span.clone(),
                        format!("`{item_name}` is not public"),
                    )));
                }
                None => {
                    return Err(Error::diagnostic(Diagnostic::new("Unknown import").label(
                        import.span.clone(),
                        format!("`{item_name}` is not defined"),
                    )));
                }
            }
        }
        // A virtual embedded-module path (spec 0038) has no file behind it to
        // canonicalize; it is already its own canonical key.
        let canonical = if embedded_source_for(&module_file).is_some() {
            module_file.clone()
        } else {
            module_file.canonicalize().map_err(|err| {
                Error::new(format!(
                    "failed to resolve module `{}`: {err}",
                    module_file.display()
                ))
            })?
        };
        if self.emitted.insert(canonical) {
            // Stamp each of this module's own public functions with the qualifier
            // the user wrote: everything before the item name for a per-item
            // import (`["std", "int"]` for `import std.int.to_string`), or the
            // whole path for a whole-module import (`["std", "io"]` for
            // `import std.io`). They then become callable as `io.print` /
            // `std.io.print` (spec 0018); effect operations are qualified-only
            // (spec 0036). Private helpers and already-stamped transitively
            // imported functions are left unqualified.
            let qualifier = if item_name.is_some() {
                import.path[..import.path.len() - 1].to_vec()
            } else {
                import.path.clone()
            };
            let mut functions = module.functions.clone();
            for function in &mut functions {
                if function.is_public && function.module_path.is_empty() {
                    function.module_path = qualifier.clone();
                }
            }
            // The module's type declarations (spec 0028) and their impls
            // (spec 0020) travel with its functions; the imported functions'
            // signatures need them. A loaded module is not merged with the
            // prelude, so these are only its own.
            Ok(Imported {
                functions,
                externs: module.externs.clone(),
                enums: module.enums.clone(),
                traits: module.traits.clone(),
                impls: module.impls.clone(),
            })
        } else {
            Ok(Imported::default())
        }
    }

    /// Locates the module file an import refers to, its declared module name, and
    /// the imported item — `None` for a whole-module import. `import pkg.module`
    /// (two segments into a package) imports the whole module: this is how an
    /// `effect` is imported (spec 0036), and its operations are then callable only
    /// as `module.op`. `import pkg.module.item` (or deeper) imports a single item.
    /// Relative imports (not into a package) always name a single item, as before.
    fn resolve_module_file(
        &self,
        source_path: &Path,
        import: &Import,
    ) -> Result<Option<(PathBuf, String, Option<String>)>> {
        // Embedded std modules (spec 0038) resolve first: their `std.<name>`
        // paths are reserved, needing no `--package` (and shadowing any
        // relative `std/` directory). The whole-module / per-item split
        // mirrors the package branch below: `import std.io` is the whole
        // module (e.g. an effect), `import std.string.length` a single item.
        // A deeper path names a nested module, which is never embedded.
        if import.path[0] == "std" && (import.path.len() == 2 || import.path.len() == 3) {
            let module_name = &import.path[1];
            if prelude::embedded_std_source(module_name).is_some() {
                let item_name = (import.path.len() == 3).then(|| import.path[2].clone());
                return Ok(Some((
                    embedded_module_path(module_name),
                    module_name.clone(),
                    item_name,
                )));
            }
        }

        if let Some(package) = self
            .packages
            .iter()
            .find(|package| package.name == import.path[0])
        {
            let remaining = &import.path[1..];
            let (module_parts, item_name): (&[String], Option<String>) = if remaining.len() == 1 {
                // `import std.io` — the whole module (e.g. an effect).
                (remaining, None)
            } else {
                // `import std.list.map` — a single item from a module.
                (
                    &remaining[..remaining.len() - 1],
                    Some(import.path[import.path.len() - 1].clone()),
                )
            };
            let module_path = join_module_path(&package.source_root, module_parts);
            return Ok(Some((module_path, module_parts.join("."), item_name)));
        }

        let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
        let module_parts = &import.path[..import.path.len() - 1];
        let module_path = join_module_path(base_dir, module_parts);
        Ok(Some((
            module_path,
            module_parts.join("."),
            Some(import.item_name().to_string()),
        )))
    }

    fn load_module(&mut self, path: &Path) -> Result<Program> {
        // An embedded std module (spec 0038) is parsed from its compiled-in
        // source under its virtual label (`<std.io>`), skipping the
        // canonicalization, overlay, and filesystem reads below — which also
        // keeps it available where there is no filesystem (the playground).
        if let Some(source) = embedded_source_for(path) {
            if let Some(program) = self.loaded.get(path) {
                return Ok(program.clone());
            }
            if !self.resolving.insert(path.to_path_buf()) {
                return Err(Error::new(format!(
                    "cyclic import involving `{}`",
                    path.display()
                )));
            }
            let label = path.display().to_string();
            // Parse errors here mean the compiler shipped a broken module;
            // they are still reported through the normal channel (spec 0033).
            let (program, errors) = parse_program(&label, source);
            self.errors.extend(errors);
            let program = self.expand_program(path, program);
            self.resolving.remove(path);
            self.loaded.insert(path.to_path_buf(), program.clone());
            return Ok(program);
        }
        let canonical = path.canonicalize().map_err(|err| {
            Error::new(format!(
                "failed to resolve module `{}`: {err}",
                path.display()
            ))
        })?;
        if let Some(program) = self.loaded.get(&canonical) {
            return Ok(program.clone());
        }
        if !self.resolving.insert(canonical.clone()) {
            return Err(Error::new(format!(
                "cyclic import involving `{}`",
                canonical.display()
            )));
        }
        // An open editor buffer (spec 0033) takes precedence over the file on
        // disk, so unsaved edits are seen by whoever imports the module.
        let source = match self.overlay.get(&canonical) {
            Some(text) => text.clone(),
            None => fs::read_to_string(&canonical).map_err(|err| {
                Error::new(format!(
                    "failed to read module `{}`: {err}",
                    canonical.display()
                ))
            })?,
        };
        let label = canonical.display().to_string();
        // Parse errors in the module are collected, and its declarations that
        // did parse still flow to the importer, keeping diagnostics complete.
        let (program, errors) = parse_program(&label, &source);
        self.errors.extend(errors);
        let program = self.expand_program(&canonical, program);
        self.resolving.remove(&canonical);
        self.loaded.insert(canonical.clone(), program.clone());
        Ok(program)
    }
}

fn join_module_path(root: &Path, parts: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path.set_extension("emel");
    path
}
