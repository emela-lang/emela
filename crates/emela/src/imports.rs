use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ast::{
    EffectDecl, EnumDecl, Extern, Function, ImplDecl, Import, Program, RecordDecl, TraitDecl,
};
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

/// Rejects `intrinsic fn` declarations in user-authored sources: every
/// intrinsic is declared exactly once, in the compiler's embedded std (spec
/// 0038), where its name is what backends key their instruction tables on.
/// Offending declarations are reported and dropped (spec 0033's report-and-
/// skip recovery), so the rest of the program still checks.
pub(crate) fn reject_user_intrinsics(program: &mut Program, errors: &mut Vec<Error>) {
    program.externs.retain(|declaration| {
        if !declaration.is_intrinsic {
            return true;
        }
        errors.push(Error::diagnostic(
            Diagnostic::new("Intrinsic outside the embedded std")
                .label(
                    declaration.name_span.clone(),
                    format!(
                        "`intrinsic fn {}` may only be declared by the compiler's embedded std",
                        declaration.name
                    ),
                )
                .help("Intrinsics are declared once, in the embedded std (spec 0038); call the `std` wrapper that provides the operation instead."),
        ));
        false
    });
}

/// Rejects a package addressed as `std` that provides a module whose name is
/// reserved by the embedded core (spec 0038). The check is eager — it fires
/// whether or not the module is imported — because embedded modules always
/// win resolution, so a conflicting file could only ever be silently
/// shadowed, never an override. Non-`std` packages may use the names freely.
pub(crate) fn check_reserved_std_modules(packages: &[PackageSource]) -> Result<()> {
    let mut conflicts = Vec::new();
    for package in packages.iter().filter(|package| package.name == "std") {
        for name in prelude::reserved_std_modules() {
            let module_file = join_module_path(&package.source_root, &[name.to_string()]);
            if module_file.exists() {
                conflicts.push(format!(
                    "package `std` at `{}` provides `{name}.emel`, but module `std.{name}` \
                     is embedded in the compiler (spec 0038); remove the module or rename \
                     the package",
                    package.source_root.display()
                ));
            }
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(Error::new(conflicts.join("\n")))
    }
}

/// The declarations pulled in from an imported module (spec 0037): its
/// functions (qualified by the import path), its type declarations (enums, spec
/// 0028; effects, spec 0037), their impls (spec 0020), and its externs (needed
/// for platform lowering; bare visibility is gated to the declaring module by
/// the type checker). Emitted once per module (see `emitted`).
#[derive(Default)]
struct Imported {
    functions: Vec<Function>,
    externs: Vec<Extern>,
    enums: Vec<EnumDecl>,
    records: Vec<RecordDecl>,
    traits: Vec<TraitDecl>,
    impls: Vec<ImplDecl>,
    effects: Vec<EffectDecl>,
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
            acc.records.extend(items.records);
            acc.traits.extend(items.traits);
            acc.impls.extend(items.impls);
            acc.effects.extend(items.effects);
        }
        // Imported declarations come first so this file's own definitions can
        // shadow / extend them, matching the existing function ordering.
        acc.functions.extend(program.functions);
        program.functions = acc.functions;
        acc.externs.extend(program.externs);
        program.externs = acc.externs;
        acc.enums.extend(program.enums);
        program.enums = acc.enums;
        acc.records.extend(program.records);
        program.records = acc.records;
        acc.traits.extend(program.traits);
        program.traits = acc.traits;
        acc.impls.extend(program.impls);
        program.impls = acc.impls;
        acc.effects.extend(program.effects);
        program.effects = acc.effects;
        program
    }

    fn resolve_import(&mut self, source_path: &Path, import: &Import) -> Result<Imported> {
        let (module_file, module_name) = self.resolve_module_file(source_path, &import.path);
        // A missing module file may be an old per-item import (`import
        // std.list.map`, dropped by spec 0037): if the path minus its last
        // segment resolves to a module, guide to the module-unit form.
        if embedded_source_for(&module_file).is_none() && !module_file.exists() {
            if import.path.len() >= 2 {
                let parent = &import.path[..import.path.len() - 1];
                let (parent_file, _) = self.resolve_module_file(source_path, parent);
                let parent_exists =
                    embedded_source_for(&parent_file).is_some() || parent_file.exists();
                if parent_exists && let Ok(parent_module) = self.load_module(&parent_file) {
                    return Err(module_unit_guidance(
                        &parent_module,
                        parent,
                        &import.path[import.path.len() - 1],
                        &import.span,
                    ));
                }
            }
            return Err(Error::diagnostic(Diagnostic::new("Unknown module").label(
                import.span.clone(),
                format!(
                    "cannot find module `{}` (`{}` does not exist)",
                    import.path.join("."),
                    module_file.display()
                ),
            )));
        }
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
            // Stamp every one of this module's own functions with the import
            // path the user wrote (spec 0037). Public functions become callable
            // by any suffix of `path + [name]` (`list.map`, `std.list.map`);
            // stamping private helpers too moves them out of the compilation
            // root's bare-name scope, so they resolve only from their own
            // module (spec 0037 R5). Effect operations (stamped `[EffectName]`
            // at parse time) and transitively imported functions keep their
            // original qualifier.
            let qualifier = import.path.clone();
            let mut functions = module.functions.clone();
            for function in &mut functions {
                if function.module_path.is_empty() {
                    function.module_path = qualifier.clone();
                }
            }
            // Impl method bodies resolve bare names within their own module
            // too (spec 0037), so they carry the same qualifier as their
            // module's functions — e.g. a `list` impl calling `append` bare.
            let mut impls = module.impls.clone();
            for decl in &mut impls {
                for method in &mut decl.methods {
                    if method.module_path.is_empty() {
                        method.module_path = qualifier.clone();
                    }
                }
            }
            // The module's type declarations (spec 0028), effects (spec 0037),
            // and impls (spec 0020) travel with its functions; the imported
            // functions' signatures need them. A loaded module is not merged
            // with the prelude, so these are only its own.
            Ok(Imported {
                functions,
                externs: module.externs.clone(),
                enums: module.enums.clone(),
                records: module.records.clone(),
                traits: module.traits.clone(),
                impls,
                effects: module.effects.clone(),
            })
        } else {
            Ok(Imported::default())
        }
    }

    /// Locates the module file an import path refers to, and its expected
    /// declared module name. An import always names a whole module (spec 0037):
    /// the last segment is the module, resolved against the package whose name
    /// matches the first segment (spec 0032), the embedded std (spec 0038), or
    /// the importing file's directory.
    fn resolve_module_file(&self, source_path: &Path, path: &[String]) -> (PathBuf, String) {
        // Embedded std modules (spec 0038) resolve first: their `std.<name>`
        // paths are reserved, needing no `--package` (and shadowing any
        // relative `std/` directory or `std` package — a `std` package may not
        // provide these module names, see `check_reserved_std_modules`).
        if path.len() == 2 && path[0] == "std" && prelude::embedded_std_source(&path[1]).is_some() {
            return (embedded_module_path(&path[1]), path[1].clone());
        }

        if path.len() >= 2
            && let Some(package) = self.packages.iter().find(|package| package.name == path[0])
        {
            let module_parts = &path[1..];
            return (
                join_module_path(&package.source_root, module_parts),
                module_parts.join("."),
            );
        }

        let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
        (join_module_path(base_dir, path), path.join("."))
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
        let (mut program, errors) = parse_program(&label, &source);
        self.errors.extend(errors);
        // A filesystem module is user-authored: its intrinsics are rejected
        // (spec 0038). The embedded branch above is exempt.
        reject_user_intrinsics(&mut program, &mut self.errors);
        let program = self.expand_program(&canonical, program);
        self.resolving.remove(&canonical);
        self.loaded.insert(canonical.clone(), program.clone());
        Ok(program)
    }
}

/// The diagnostic for an old-style per-item import (`import std.list.map`,
/// spec 0037): the parent path names a module, so guide to the module-unit
/// form, tailored to what the trailing segment actually is.
fn module_unit_guidance(
    module: &Program,
    module_path: &[String],
    item: &str,
    span: &crate::error::Span,
) -> Error {
    let import_path = module_path.join(".");
    let qualifier = module_path.last().map(String::as_str).unwrap_or_default();
    let label = if let Some(function) = module.functions.iter().find(|f| f.name == item) {
        match (&function.effect_name, function.is_public) {
            (Some(effect), true) => format!(
                "`{item}` is an operation of effect `{effect}`; write `import {import_path}` and call it as `{effect}.{item}(...)` inside a `uses {{ {effect} }}` function"
            ),
            (Some(effect), false) => {
                format!("`{item}` is a private operation of effect `{effect}`")
            }
            (None, true) => format!(
                "imports name whole modules (spec 0037); write `import {import_path}` and call it as `{qualifier}.{item}(...)`"
            ),
            (None, false) => format!("`{item}` is not public in module `{import_path}`"),
        }
    } else if let Some(declaration) = module.externs.iter().find(|e| e.name == item) {
        match &declaration.effect_name {
            Some(effect) => format!("`{item}` is a private operation of effect `{effect}`"),
            None => format!("`{item}` is not importable; it is internal to `{import_path}`"),
        }
    } else if module.effects.iter().any(|e| e.name == item) {
        format!(
            "effects are imported with their module (spec 0037); write `import {import_path}` and use `uses {{ {item} }}`"
        )
    } else {
        format!(
            "imports name whole modules (spec 0037), and module `{import_path}` has no `{item}`; write `import {import_path}`"
        )
    };
    Error::diagnostic(Diagnostic::new("Module-unit import").label(span.clone(), label))
}

fn join_module_path(root: &Path, parts: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path.set_extension("emel");
    path
}
